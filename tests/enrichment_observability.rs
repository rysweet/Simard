//! TDD (Step 7) — failing tests for the enrichment-observability emit seam
//! (issue #2942: prove, with live evidence, that recalled memory reaches
//! Simard's OODA decisions).
//!
//! Contract under test (all not-yet-implemented — the compile/assert failures
//! are the intended TDD red state). Reference:
//! `docs/reference/enrichment-observability-api.md`.
//!
//! ```rust
//! // src/enrichment_observability/mod.rs  (pub mod in lib.rs)
//! pub struct EnrichmentObservation<'a> {
//!     pub objective: &'a str,
//!     pub attached: bool,
//!     pub expected: bool,
//!     pub facts_injected: usize,
//!     pub procedures_injected: usize,
//!     pub preamble_bytes: usize,
//! }
//! pub enum DegradeReason { MemoryIpc, KnowledgeLaunch }
//! impl DegradeReason { pub fn as_str(&self) -> &'static str; }
//! pub fn observe(obs: EnrichmentObservation<'_>);
//! pub fn observe_degrade(reason: DegradeReason, raw_error: &str);
//! ```
//!
//! The emit choke point is content-free: it carries counts and byte sizes,
//! never fact/procedure text; the `objective` is truncated (<=120 bytes) and
//! control-stripped and only ever a *log field*, never a metric attribute.

use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use serial_test::serial;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use simard::enrichment_observability::{self, DegradeReason, EnrichmentObservation};
use simard::telemetry::{self, names};

// ── tracing capture layer (thread-scoped, hermetic) ─────────────────────────

#[derive(Clone, Debug)]
struct CapturedEvent {
    level: String,
    target: String,
    /// `" k=v k2=v2 ..."` including the static `message` field.
    fields: String,
}

#[derive(Default)]
struct FieldVisitor {
    out: String,
}

impl FieldVisitor {
    fn push(&mut self, name: &str, value: &str) {
        use std::fmt::Write;
        let _ = write!(self.out, " {name}={value}");
    }
}

impl Visit for FieldVisitor {
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field.name(), &value.to_string());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field.name(), &value.to_string());
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field.name(), &value.to_string());
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field.name(), value);
    }
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.push(field.name(), &format!("{value:?}"));
    }
}

struct CollectLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: Subscriber> Layer<S> for CollectLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let meta = event.metadata();
        self.events.lock().unwrap().push(CapturedEvent {
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            fields: visitor.out,
        });
    }
}

/// Run `body` with a thread-local subscriber that captures every event, then
/// return only the events emitted under the `simard::enrichment` target.
fn capture_enrichment_events<F: FnOnce()>(body: F) -> Vec<CapturedEvent> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CollectLayer {
        events: Arc::clone(&events),
    };
    let subscriber = tracing_subscriber::registry::Registry::default().with(layer);
    tracing::subscriber::with_default(subscriber, body);
    let all = events.lock().unwrap().clone();
    all.into_iter()
        .filter(|e| e.target == "simard::enrichment")
        .collect()
}

// ── metric-name catalog is a public contract ────────────────────────────────

#[test]
fn enrichment_metric_names_are_dotted_and_stable() {
    assert_eq!(names::ENRICHMENT_DECISIONS, "simard.enrichment.decisions");
    assert_eq!(names::ENRICHMENT_DEGRADED, "simard.enrichment.degraded");
    assert_eq!(
        names::ENRICHMENT_PREAMBLE_BYTES,
        "simard.enrichment.preamble_bytes"
    );
    assert_eq!(
        names::ENRICHMENT_FACTS_INJECTED,
        "simard.enrichment.facts_injected"
    );
    assert_eq!(
        names::ENRICHMENT_PROCEDURES_INJECTED,
        "simard.enrichment.procedures_injected"
    );
    // Low-cardinality attribute keys.
    assert_eq!(names::ATTR_ATTACHED, "attached");
    assert_eq!(names::ATTR_REASON, "reason");
}

#[test]
fn degrade_reason_as_str_maps_to_bounded_enum() {
    assert_eq!(DegradeReason::MemoryIpc.as_str(), "memory_ipc");
    assert_eq!(DegradeReason::KnowledgeLaunch.as_str(), "knowledge_launch");
}

// ── observe: the attach path (INFO + counted) ───────────────────────────────

#[test]
#[serial(telemetry_registry)]
fn observe_attach_emits_info_and_counts_injected_payload() {
    telemetry::reset();

    let events = capture_enrichment_events(|| {
        enrichment_observability::observe(EnrichmentObservation {
            objective: "raise unit-test coverage on the goal-board store",
            attached: true,
            expected: true,
            facts_injected: 7,
            procedures_injected: 3,
            preamble_bytes: 812,
        });
    });

    // One INFO line under the enrichment target carrying the counts.
    let info = events
        .iter()
        .find(|e| e.level == "INFO")
        .expect("attach path must emit one INFO line under simard::enrichment");
    assert!(
        info.fields.contains("attached=true"),
        "INFO line must record attached=true, got: {}",
        info.fields
    );
    assert!(
        info.fields.contains("facts=7") || info.fields.contains("facts_injected=7"),
        "INFO line must record the injected fact count, got: {}",
        info.fields
    );
    assert!(
        info.fields.contains("procedures=3") || info.fields.contains("procedures_injected=3"),
        "INFO line must record the injected procedure count, got: {}",
        info.fields
    );
    assert!(
        info.fields.contains("preamble_bytes=812"),
        "INFO line must record the rendered preamble size, got: {}",
        info.fields
    );

    // Metrics: decisions{attached=true} + the three magnitude histograms.
    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DECISIONS,
            &[(names::ATTR_ATTACHED, "true")]
        ),
        Some(1),
        "one decision counted with attached=true"
    );
    let facts = snap
        .histogram(names::ENRICHMENT_FACTS_INJECTED, &[])
        .expect("facts_injected histogram must be recorded");
    assert_eq!(facts.count, 1);
    assert_eq!(facts.sum, 7.0);
    let procs = snap
        .histogram(names::ENRICHMENT_PROCEDURES_INJECTED, &[])
        .expect("procedures_injected histogram must be recorded");
    assert_eq!(procs.count, 1);
    assert_eq!(procs.sum, 3.0);
    let bytes = snap
        .histogram(names::ENRICHMENT_PREAMBLE_BYTES, &[])
        .expect("preamble_bytes histogram must be recorded");
    assert_eq!(bytes.count, 1);
    assert_eq!(bytes.sum, 812.0);
}

#[test]
#[serial(telemetry_registry)]
fn observe_attach_with_empty_store_is_still_counted_with_zeroes() {
    telemetry::reset();

    let events = capture_enrichment_events(|| {
        enrichment_observability::observe(EnrichmentObservation {
            objective: "triage stale pull requests",
            attached: true,
            expected: true,
            facts_injected: 0,
            procedures_injected: 0,
            preamble_bytes: 0,
        });
    });

    // attached=true with an empty store is a true signal, not a degrade: INFO,
    // not WARN.
    assert!(
        events.iter().any(|e| e.level == "INFO"),
        "empty-store attach must still emit INFO"
    );
    assert!(
        events.iter().all(|e| e.level != "WARN"),
        "attached=true must never be a WARN, even with an empty store"
    );

    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DECISIONS,
            &[(names::ATTR_ATTACHED, "true")]
        ),
        Some(1),
        "attached=true is counted regardless of injected count"
    );
    let facts = snap
        .histogram(names::ENRICHMENT_FACTS_INJECTED, &[])
        .expect("facts_injected histogram recorded even at zero");
    assert_eq!(facts.count, 1, "the decision is observed");
    assert_eq!(facts.sum, 0.0, "with zero facts injected");
}

// ── observe: population / expected rule ──────────────────────────────────────

#[test]
#[serial(telemetry_registry)]
fn observe_unconfigured_turn_logs_info_but_records_no_metrics() {
    telemetry::reset();

    let events = capture_enrichment_events(|| {
        enrichment_observability::observe(EnrichmentObservation {
            objective: "run local-harness smoke",
            attached: false,
            expected: false, // enrichment was never configured for this session
            facts_injected: 0,
            procedures_injected: 0,
            preamble_bytes: 0,
        });
    });

    // Benign unconfigured turn: an INFO for completeness, NOT a WARN.
    assert!(
        events.iter().any(|e| e.level == "INFO"),
        "unconfigured turn must still log an INFO line"
    );
    assert!(
        events.iter().all(|e| e.level != "WARN"),
        "an unconfigured (expected=false) turn is NOT a degrade — must not WARN"
    );
    let info = events.iter().find(|e| e.level == "INFO").unwrap();
    assert!(
        info.fields.contains("expected=false"),
        "the line must record expected=false, got: {}",
        info.fields
    );

    // Population rule: unconfigured turns are excluded from the attach-rate.
    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DECISIONS,
            &[(names::ATTR_ATTACHED, "false")]
        ),
        None,
        "an unconfigured turn must NOT be counted in simard.enrichment.decisions"
    );
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DECISIONS,
            &[(names::ATTR_ATTACHED, "true")]
        ),
        None
    );
    assert!(
        snap.histogram(names::ENRICHMENT_FACTS_INJECTED, &[])
            .is_none(),
        "no magnitude histogram is recorded for an unconfigured turn"
    );
}

#[test]
#[serial(telemetry_registry)]
fn observe_expected_but_degraded_turn_warns_and_is_counted() {
    telemetry::reset();

    // A `Native` source whose memory bridge fully degraded still resolves as
    // expected=true. This MUST surface as a WARN and stay in the attach-rate
    // denominator — never silently misread as "unconfigured".
    let events = capture_enrichment_events(|| {
        enrichment_observability::observe(EnrichmentObservation {
            objective: "triage stale pull requests",
            attached: false,
            expected: true,
            facts_injected: 0,
            procedures_injected: 0,
            preamble_bytes: 0,
        });
    });

    let warn = events
        .iter()
        .find(|e| e.level == "WARN")
        .expect("an expected-but-degraded memory bridge MUST emit a WARN, not a silent None");
    assert!(
        warn.fields.contains("attached=false") && warn.fields.contains("expected=true"),
        "the degrade WARN must carry attached=false expected=true, got: {}",
        warn.fields
    );

    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DECISIONS,
            &[(names::ATTR_ATTACHED, "false")]
        ),
        Some(1),
        "an expected-but-degraded decision IS counted (drags the attach-rate down honestly)"
    );
}

// ── observe: no content leak, objective sanitised, never a metric attr ───────

#[test]
#[serial(telemetry_registry)]
fn observe_truncates_and_control_strips_objective_and_never_uses_it_as_a_metric_attr() {
    telemetry::reset();

    // >120 bytes, with embedded control characters and a tail marker beyond the
    // truncation boundary.
    let head = "HEADMARKER";
    let tail = "TAILMARKER";
    let objective = format!("{head}\n\t\u{0007}{}{tail}", "A".repeat(300));
    assert!(objective.len() > 120);

    let events = capture_enrichment_events(|| {
        enrichment_observability::observe(EnrichmentObservation {
            objective: &objective,
            attached: true,
            expected: true,
            facts_injected: 1,
            procedures_injected: 0,
            preamble_bytes: 40,
        });
    });

    let line = events
        .iter()
        .find(|e| e.level == "INFO")
        .expect("attach path emits an INFO line");

    // Head survives (objective is present); tail beyond 120 bytes is truncated.
    assert!(
        line.fields.contains(head),
        "the (sanitised) objective head must appear on the line: {}",
        line.fields
    );
    assert!(
        !line.fields.contains(tail),
        "content beyond the 120-byte truncation boundary must NOT appear: {}",
        line.fields
    );

    // No raw control characters survive into the emitted line (only the
    // objective could contribute them; everything else is numbers/bools).
    for ch in ['\n', '\t', '\r', '\u{0007}'] {
        assert!(
            !line.fields.contains(ch),
            "raw control character {ch:?} must be stripped from the log line"
        );
    }

    // The objective must NEVER become a metric attribute (cardinality contract).
    let snap = telemetry::capture();
    for series in &snap.counters {
        for (k, v) in &series.attrs {
            assert!(
                !v.contains(head) && k != "objective",
                "objective must never be a counter attribute: {series:?}"
            );
        }
    }
    for series in &snap.histograms {
        for (k, v) in &series.attrs {
            assert!(
                !v.contains(head) && k != "objective",
                "objective must never be a histogram attribute: {series:?}"
            );
        }
    }
}

// ── observe_degrade: fail-LOUD bridge launch degrade ────────────────────────

#[test]
#[serial(telemetry_registry)]
fn observe_degrade_emits_warn_with_reason_and_hides_raw_error_from_warn() {
    telemetry::reset();

    let raw = "Broken pipe (os error 32)";
    let events = capture_enrichment_events(|| {
        enrichment_observability::observe_degrade(DegradeReason::MemoryIpc, raw);
    });

    let warn = events
        .iter()
        .find(|e| e.level == "WARN")
        .expect("a bridge-launch degrade MUST emit a WARN (never a silent eprintln!)");
    assert!(
        warn.fields.contains("reason=memory_ipc"),
        "the degrade WARN must carry the bounded reason enum, got: {}",
        warn.fields
    );
    // The raw error string must not ride on the WARN (log-forging / injection):
    // it goes to DEBUG only.
    assert!(
        !warn.fields.contains("Broken pipe"),
        "the raw error must NOT appear at WARN level (DEBUG only), got: {}",
        warn.fields
    );

    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DEGRADED,
            &[(names::ATTR_REASON, "memory_ipc")]
        ),
        Some(1),
        "a memory-ipc degrade increments simard.enrichment.degraded{{reason=memory_ipc}}"
    );
}

#[test]
#[serial(telemetry_registry)]
fn observe_degrade_knowledge_launch_is_tagged_distinctly() {
    telemetry::reset();

    let events = capture_enrichment_events(|| {
        enrichment_observability::observe_degrade(
            DegradeReason::KnowledgeLaunch,
            "spawn failed: ENOENT",
        );
    });

    assert!(
        events
            .iter()
            .any(|e| e.level == "WARN" && e.fields.contains("reason=knowledge_launch")),
        "a knowledge-launch degrade must WARN with reason=knowledge_launch"
    );
    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DEGRADED,
            &[(names::ATTR_REASON, "knowledge_launch")]
        ),
        Some(1)
    );
}

// ── launch_enrichment_bridges: the degrade is loud at the real seam ─────────

/// A regular file as `state_root` makes `<state_root>/cognitive` uncreatable,
/// so `connect_memory` fails and the memory bridge degrades to `None`. The
/// degrade must be a structured WARN under `simard::enrichment` PLUS a
/// `simard.enrichment.degraded{reason=memory_ipc}` increment — never the
/// silent `eprintln!` this replaces.
#[test]
#[serial(telemetry_registry)]
fn launch_enrichment_bridges_degrade_is_loud_and_metered() {
    use tempfile::NamedTempFile;

    telemetry::reset();
    let file = NamedTempFile::new().unwrap();

    let mut memory_present = true;
    let events = capture_enrichment_events(|| {
        let (memory, _knowledge) = simard::base_type_turn::launch_enrichment_bridges(file.path());
        memory_present = memory.is_some();
    });

    assert!(
        !memory_present,
        "memory bridge must degrade to None when the state_root cannot back a store"
    );
    assert!(
        events
            .iter()
            .any(|e| e.level == "WARN" && e.fields.contains("reason=memory_ipc")),
        "the memory degrade must be a loud WARN with reason=memory_ipc (fail-LOUD, never silent)"
    );

    let snap = telemetry::capture();
    assert_eq!(
        snap.counter(
            names::ENRICHMENT_DEGRADED,
            &[(names::ATTR_REASON, "memory_ipc")]
        ),
        Some(1),
        "the degrade must be metered so an operator can see a bridge is down"
    );
}

// ── EnrichmentSource provenance: `expected` survives a full degrade ──────────

/// The `expected` provenance bit is captured at resolve time so a fully
/// degraded `Native` source (memory unavailable) is still distinguishable from
/// an unconfigured `Disabled` session. Without this, a degrade would silently
/// drop out of the attach-rate.
#[test]
#[serial(telemetry_registry)]
fn native_source_carries_expected_true_even_when_memory_degrades() {
    use tempfile::NamedTempFile;

    // A regular file forces the memory bridge to degrade to None.
    let file = NamedTempFile::new().unwrap();
    let bridges = simard::base_type_turn::EnrichmentSource::Native {
        state_root: file.path().to_path_buf(),
    }
    .resolve();

    assert!(
        bridges.memory.is_none(),
        "precondition: the memory bridge degraded to None for a non-store state_root"
    );
    assert!(
        bridges.expected,
        "a Native source must carry expected=true even when its bridge fully degrades"
    );
}

#[test]
fn disabled_source_carries_expected_false() {
    let bridges = simard::base_type_turn::EnrichmentSource::Disabled.resolve();
    assert!(
        !bridges.expected,
        "a Disabled source must carry expected=false (a benign unconfigured turn)"
    );
}
