//! Enrichment observability (issue #2942) — prove, with live evidence, that
//! recalled memory is actually injected into Simard's OODA decisions.
//!
//! Recall is *computed* by [`crate::base_type_turn::enrich_turn_input`] and
//! rendered into each turn's prompt preamble, but whether the memory reader
//! **attached** (resolved to `Some`) or silently **degraded** (`None`) was
//! previously invisible: `EnrichmentSource::resolve` dropped a launch failure to
//! `eprintln!` and no per-turn signal recorded what actually reached the model.
//! This module closes that gap with a single emit choke point:
//!
//! * [`observe`] — one structured `simard::enrichment` tracing line per decision
//!   (INFO on attach, INFO for an unconfigured turn, **WARN** for an
//!   expected-but-degraded memory reader) plus the `simard.enrichment.*` metrics
//!   (`decisions{attached}`, and the `facts_injected` / `procedures_injected` /
//!   `preamble_bytes` histograms). The choke point is content-free: it carries
//!   counts and byte sizes, never fact/procedure text; the `objective` is
//!   truncated + control-stripped and only ever a *log field*, never a metric
//!   attribute.
//! * [`observe_degrade`] — the fail-LOUD replacement for the old silent
//!   `eprintln!` at `launch_enrichment_clients`: a WARN carrying the bounded
//!   `reason` enum (the raw error goes to DEBUG only) plus a
//!   `simard.enrichment.degraded{reason}` increment.
//! * An in-process rollup drained once per OODA cycle via [`snapshot_section`]
//!   into `metrics_snapshot.json`, the live source the dashboard reads.
//! * [`run_enrichment_ablation`] — the hard proof: a hermetic recall-on
//!   vs recall-off ablation that yields a reproducible delta, fed into the
//!   hybrid self-measurement (#2644) via [`record_ablation_feed`].

use std::sync::{LazyLock, Mutex};

use serde_json::{Value, json};

use crate::base_type_turn::{prepare_turn_context, render_enrichment_block};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::telemetry::snapshot::now_rfc3339;
use crate::telemetry::{self, names};

/// Max retained length of the `objective` log field (bytes). Beyond this the
/// value is truncated on a UTF-8 boundary so a long objective cannot bloat a log
/// line or (via a bug) a metric attribute.
const MAX_OBJECTIVE_BYTES: usize = 120;

/// Max retained length of the raw error string on the DEBUG degrade-detail line.
const MAX_RAW_ERROR_BYTES: usize = 256;

/// One observed enrichment decision, handed to [`observe`] at the
/// `enrich_turn_input` seam. Content-free: counts + byte sizes only.
pub struct EnrichmentObservation<'a> {
    /// The turn's objective/slug, for correlation. Sanitised (control-stripped,
    /// truncated) before it ever reaches a log line; never a metric attribute.
    pub objective: &'a str,
    /// Did the cognitive-memory reader resolve to `Some`? `true` means recalled
    /// *memory* reached the decision. This is `memory_client.is_some()`, NOT the
    /// combined memory-or-knowledge bundle.
    pub attached: bool,
    /// Was enrichment *configured* for this session (`EnrichmentSource::Native`)?
    /// `expected && !attached` is the degrade that raises a `WARN` and is
    /// counted; `!expected` is a benign unconfigured turn (`INFO`, uncounted).
    pub expected: bool,
    /// Facts actually rendered into the preamble (post-cap, post-render).
    pub facts_injected: usize,
    /// Procedures actually rendered into the preamble (post-cap, post-render).
    pub procedures_injected: usize,
    /// Byte length of the full rendered enrichment block injected into the
    /// preamble (memory facts + procedures **and** any domain knowledge).
    pub preamble_bytes: usize,
}

/// The bounded cause of a reader-launch degrade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradeReason {
    /// `crate::ooda_loop::connect_memory` failed (e.g. memory-ipc Broken pipe).
    MemoryIpc,
    /// `launch_knowledge_client_native` failed.
    KnowledgeLaunch,
}

impl DegradeReason {
    /// The bounded, low-cardinality metric-attribute string.
    pub fn as_str(&self) -> &'static str {
        match self {
            DegradeReason::MemoryIpc => "memory_ipc",
            DegradeReason::KnowledgeLaunch => "knowledge_launch",
        }
    }
}

/// Strip control characters and truncate `raw` to `max_bytes` on a UTF-8
/// boundary. Used for the `objective` log field and the DEBUG raw-error detail —
/// no control character or over-long value ever rides on an emitted line
/// (log-injection / forging defence).
fn sanitize_field(raw: &str, max_bytes: usize) -> String {
    let stripped: String = raw.chars().filter(|c| !c.is_control()).collect();
    if stripped.len() <= max_bytes {
        return stripped;
    }
    let mut end = max_bytes;
    while end > 0 && !stripped.is_char_boundary(end) {
        end -= 1;
    }
    stripped[..end].to_string()
}

/// Emit the per-decision enrichment observation: one structured tracing line
/// plus the `simard.enrichment.*` metrics for the *expected* population.
///
/// Level encodes the outcome so "any `WARN` under `simard::enrichment` is a real
/// degrade" is a safe operator rule:
/// * `attached` → INFO (`enrichment applied`),
/// * `expected && !attached` → **WARN** (memory reader expected but degraded),
/// * `!expected` → INFO (`enrichment not configured`), and **uncounted**.
pub fn observe(obs: EnrichmentObservation<'_>) {
    let objective = sanitize_field(obs.objective, MAX_OBJECTIVE_BYTES);
    // Widen to u64 so the tracing visitor records `facts=7`, not `facts=7usize`.
    let facts = obs.facts_injected as u64;
    let procedures = obs.procedures_injected as u64;
    let preamble_bytes = obs.preamble_bytes as u64;

    if obs.attached {
        tracing::info!(
            target: "simard::enrichment",
            attached = obs.attached,
            expected = obs.expected,
            facts,
            procedures,
            preamble_bytes,
            objective = %objective,
            "enrichment applied",
        );
    } else if obs.expected {
        tracing::warn!(
            target: "simard::enrichment",
            attached = obs.attached,
            expected = obs.expected,
            facts,
            procedures,
            preamble_bytes,
            objective = %objective,
            "enrichment degraded — memory reader expected but not attached; \
             decision proceeding without recalled memory",
        );
    } else {
        tracing::info!(
            target: "simard::enrichment",
            attached = obs.attached,
            expected = obs.expected,
            facts,
            procedures,
            preamble_bytes,
            objective = %objective,
            "enrichment not configured for this session",
        );
    }

    // The attach-rate population is the *expected* turns only: an adapter that
    // legitimately runs without enrichment must not drag the rate below 100%.
    if obs.expected {
        telemetry::counter_add(
            names::ENRICHMENT_DECISIONS,
            1,
            &[(
                names::ATTR_ATTACHED,
                if obs.attached { "true" } else { "false" },
            )],
        );
        telemetry::histogram_record(
            names::ENRICHMENT_FACTS_INJECTED,
            obs.facts_injected as f64,
            &[],
        );
        telemetry::histogram_record(
            names::ENRICHMENT_PROCEDURES_INJECTED,
            obs.procedures_injected as f64,
            &[],
        );
        telemetry::histogram_record(
            names::ENRICHMENT_PREAMBLE_BYTES,
            obs.preamble_bytes as f64,
            &[],
        );
        rollup_lock().record_decision(
            obs.attached,
            obs.facts_injected,
            obs.procedures_injected,
            obs.preamble_bytes,
        );
    }
}

/// Fail-LOUD reader-launch degrade: a WARN carrying the bounded `reason` enum
/// (the raw error goes to DEBUG only, never the WARN) plus a
/// `simard.enrichment.degraded{reason}` increment. Replaces the old silent
/// `eprintln!` degrade paths in `launch_enrichment_clients`.
pub fn observe_degrade(reason: DegradeReason, raw_error: &str) {
    let message = match reason {
        DegradeReason::MemoryIpc => {
            "cognitive-memory reader unavailable — memory enrichment disabled for this session"
        }
        DegradeReason::KnowledgeLaunch => {
            "knowledge reader unavailable — knowledge enrichment disabled for this session"
        }
    };
    tracing::warn!(
        target: "simard::enrichment",
        reason = reason.as_str(),
        "{message}",
    );
    // The raw error is DEBUG-only and sanitised — it never rides on the WARN, so
    // an attacker-influenced error string cannot forge structured WARN fields.
    tracing::debug!(
        target: "simard::enrichment",
        reason = reason.as_str(),
        raw = %sanitize_field(raw_error, MAX_RAW_ERROR_BYTES),
        "enrichment reader degrade detail",
    );
    telemetry::counter_add(
        names::ENRICHMENT_DEGRADED,
        1,
        &[(names::ATTR_REASON, reason.as_str())],
    );
    rollup_lock().record_degrade(reason);
}

// ── per-cycle rollup ────────────────────────────────────────────────────────

/// The most recent observed decision, surfaced for dashboard spot-checking.
#[derive(Clone)]
struct LastDecision {
    attached: bool,
    facts_injected: u64,
    procedures_injected: u64,
    preamble_bytes: u64,
    at: String,
}

/// Cumulative (since process start) enrichment aggregate. The daemon reads it
/// once per OODA cycle via [`snapshot_section`] and writes it into
/// `metrics_snapshot.json`; it is non-draining so the dashboard shows a stable
/// lifetime attach-rate rather than a thin single-cycle window.
#[derive(Default)]
struct Rollup {
    window_start: Option<String>,
    decisions: u64,
    attached: u64,
    degraded_memory_ipc: u64,
    degraded_knowledge_launch: u64,
    sum_facts: u64,
    sum_procedures: u64,
    sum_preamble_bytes: u64,
    last: Option<LastDecision>,
}

impl Rollup {
    fn record_decision(&mut self, attached: bool, facts: usize, procedures: usize, bytes: usize) {
        let now = now_rfc3339();
        if self.window_start.is_none() {
            self.window_start = Some(now.clone());
        }
        self.decisions = self.decisions.saturating_add(1);
        if attached {
            self.attached = self.attached.saturating_add(1);
        }
        self.sum_facts = self.sum_facts.saturating_add(facts as u64);
        self.sum_procedures = self.sum_procedures.saturating_add(procedures as u64);
        self.sum_preamble_bytes = self.sum_preamble_bytes.saturating_add(bytes as u64);
        self.last = Some(LastDecision {
            attached,
            facts_injected: facts as u64,
            procedures_injected: procedures as u64,
            preamble_bytes: bytes as u64,
            at: now,
        });
    }

    fn record_degrade(&mut self, reason: DegradeReason) {
        match reason {
            DegradeReason::MemoryIpc => {
                self.degraded_memory_ipc = self.degraded_memory_ipc.saturating_add(1);
            }
            DegradeReason::KnowledgeLaunch => {
                self.degraded_knowledge_launch = self.degraded_knowledge_launch.saturating_add(1);
            }
        }
    }

    fn to_section(&self) -> Option<Value> {
        // Nothing observed yet → omit the section entirely so the dashboard
        // reports "Not tracked yet" instead of a false 0%.
        if self.decisions == 0
            && self.degraded_memory_ipc == 0
            && self.degraded_knowledge_launch == 0
        {
            return None;
        }
        let avg = |sum: u64| -> Value {
            if self.decisions > 0 {
                json!(sum as f64 / self.decisions as f64)
            } else {
                Value::Null
            }
        };
        let attach_rate = if self.decisions > 0 {
            json!(self.attached as f64 / self.decisions as f64)
        } else {
            Value::Null
        };
        let last = self.last.as_ref().map(|l| {
            json!({
                "attached": l.attached,
                "facts_injected": l.facts_injected,
                "procedures_injected": l.procedures_injected,
                "preamble_bytes": l.preamble_bytes,
                "at": l.at,
            })
        });
        Some(json!({
            "window_start": self.window_start,
            "window_end": now_rfc3339(),
            "decisions": self.decisions,
            "attached": self.attached,
            "attach_rate": attach_rate,
            "degraded": {
                "memory_ipc": self.degraded_memory_ipc,
                "knowledge_launch": self.degraded_knowledge_launch,
            },
            "avg_facts_injected": avg(self.sum_facts),
            "avg_procedures_injected": avg(self.sum_procedures),
            "avg_preamble_bytes": avg(self.sum_preamble_bytes),
            "last": last,
        }))
    }
}

fn rollup() -> &'static Mutex<Rollup> {
    static ROLLUP: LazyLock<Mutex<Rollup>> = LazyLock::new(|| Mutex::new(Rollup::default()));
    &ROLLUP
}

fn rollup_lock() -> std::sync::MutexGuard<'static, Rollup> {
    // A poisoned lock only means a prior test panicked mid-update; the rollup is
    // still structurally valid, so recover rather than propagate into the hot
    // per-turn emit path.
    rollup().lock().unwrap_or_else(|e| e.into_inner())
}

/// The current enrichment rollup as the `enrichment` section of
/// `metrics_snapshot.json`, or `None` when nothing has been observed. The daemon
/// reads this once per OODA cycle (non-draining) and passes it to
/// [`crate::telemetry::flush_snapshot_with`]. Reading it never touches the recall
/// corpus.
pub fn snapshot_section() -> Option<Value> {
    rollup_lock().to_section()
}

/// Reset the rollup. Test-only isolation; the daemon never calls it.
#[cfg(test)]
pub fn reset_rollup() {
    *rollup_lock() = Rollup::default();
}

// ── ablation eval (the hard proof) ──────────────────────────────────────────

/// The verdict of an enrichment ablation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AblationVerdict {
    /// Recall-on differs measurably from recall-off (delta > 0 and the preambles
    /// differ) — recalled memory influences the decision input.
    Influences,
    /// Recall-on and recall-off are indistinguishable (empty store / no recall).
    NoInfluence,
}

impl AblationVerdict {
    /// The stable string form (`influences` | `no-influence`).
    pub fn as_str(&self) -> &'static str {
        match self {
            AblationVerdict::Influences => "influences",
            AblationVerdict::NoInfluence => "no-influence",
        }
    }
}

/// The reproducible outcome of a recall-on vs recall-off ablation.
#[derive(Clone, Debug)]
pub struct EnrichmentAblationOutcome {
    /// Bytes of the enrichment block rendered with recall ON.
    pub recall_on_bytes: usize,
    /// Bytes of the enrichment block rendered with recall suppressed (always 0).
    pub recall_off_bytes: usize,
    /// `recall_on_bytes - recall_off_bytes` — the positive magnitude of
    /// difference recall makes.
    pub delta_bytes: i64,
    /// Facts injected on the recall-on side.
    pub facts: usize,
    /// Procedures injected on the recall-on side.
    pub procedures: usize,
    /// Whether the two rendered prompt preambles are non-identical.
    pub preambles_differ: bool,
    /// The verdict derived from the delta + preamble difference.
    pub verdict: AblationVerdict,
}

/// Run the enrichment ablation for `objective` against `memory`: render the turn
/// WITH recall injected and WITHOUT (recall suppressed), and measure the delta.
///
/// Hermetic and deterministic — it queries the given store and renders through
/// the same [`render_enrichment_block`] the live seam uses, so a positive
/// `delta_bytes` is reproducible proof that recalled memory changes the
/// decision's prompt preamble.
pub fn run_enrichment_ablation(
    objective: &str,
    memory: &dyn CognitiveMemoryOps,
) -> SimardResult<EnrichmentAblationOutcome> {
    // recall ON: the memory reader attaches and recalls facts/procedures.
    let ctx_on = prepare_turn_context(objective, Some(memory), None)?;
    let on_block = render_enrichment_block(&ctx_on);

    // recall OFF: recall suppressed (no memory reader) → nothing is injected.
    let ctx_off = prepare_turn_context(objective, None, None)?;
    let off_block = render_enrichment_block(&ctx_off);

    let recall_on_bytes = on_block.len();
    let recall_off_bytes = off_block.len();
    let delta_bytes = recall_on_bytes as i64 - recall_off_bytes as i64;
    let preambles_differ = on_block != off_block;
    let verdict = if delta_bytes != 0 && preambles_differ {
        AblationVerdict::Influences
    } else {
        AblationVerdict::NoInfluence
    };

    Ok(EnrichmentAblationOutcome {
        recall_on_bytes,
        recall_off_bytes,
        delta_bytes,
        facts: ctx_on.memory_facts.len(),
        procedures: ctx_on.procedures.len(),
        preambles_differ,
        verdict,
    })
}

/// Feed an ablation `outcome` into the hybrid self-measurement (#2644): record
/// `delta_bytes` as the durable `enrichment_ablation_delta` self-metric, tagged
/// with the ablation site + verdict so #2644 can consume it.
pub fn record_ablation_feed(outcome: &EnrichmentAblationOutcome) -> SimardResult<()> {
    let context = json!({
        "site": "enrichment_ablation",
        "verdict": outcome.verdict.as_str(),
        "facts": outcome.facts,
        "procedures": outcome.procedures,
        "recall_on_bytes": outcome.recall_on_bytes,
        "recall_off_bytes": outcome.recall_off_bytes,
    })
    .to_string();
    crate::self_metrics::record_metric(
        "enrichment_ablation_delta",
        outcome.delta_bytes as f64,
        &context,
    )
    .map_err(|e| SimardError::ActionExecutionFailed {
        action: "record_enrichment_ablation_delta".to_string(),
        reason: e.to_string(),
    })
}
