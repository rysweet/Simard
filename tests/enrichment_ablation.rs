//! TDD (Step 7) — failing tests for the enrichment ablation eval (#2942).
//!
//! The hard proof that recalled memory *influences* decisions: run one
//! representative decision WITH recall injected vs WITHOUT (recall suppressed)
//! and assert a measurable, reproducible difference. Reference:
//! `docs/reference/enrichment-observability-api.md#ablation-eval-simard-gym-enrichment-ablation`.
//!
//! Contract under test (not-yet-implemented — the compile/assert failures are
//! the intended TDD red state):
//!
//! ```rust
//! // src/enrichment_observability/mod.rs
//! pub struct EnrichmentAblationOutcome {
//!     pub recall_on_bytes: usize,
//!     pub recall_off_bytes: usize,
//!     pub delta_bytes: i64,          // recall_on_bytes - recall_off_bytes
//!     pub facts: usize,
//!     pub procedures: usize,
//!     pub preambles_differ: bool,
//!     pub verdict: AblationVerdict,
//! }
//! pub enum AblationVerdict { Influences, NoInfluence }
//! impl AblationVerdict { pub fn as_str(&self) -> &'static str; } // "influences" | "no-influence"
//! pub fn run_enrichment_ablation(
//!     objective: &str,
//!     memory: &dyn CognitiveMemoryOps,
//! ) -> SimardResult<EnrichmentAblationOutcome>;
//! pub fn record_ablation_feed(outcome: &EnrichmentAblationOutcome) -> SimardResult<()>;
//! ```

use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;

use simard::BaseTypeTurnInput;
use simard::CognitiveMemoryOps;
use simard::enrich_turn_input;
use simard::enrichment_observability::{
    AblationVerdict, record_ablation_feed, run_enrichment_ablation,
};
use simard::memory_client::CognitiveMemoryClient;
use simard::rpc::RpcErrorPayload;
use simard::rpc_transport::InMemoryRpcTransport;

const OBJECTIVE: &str = "implement error handling";

/// A seeded, hermetic memory store: one fact + one procedure for any query.
fn seeded_memory_client() -> Box<dyn CognitiveMemoryOps> {
    let transport = InMemoryRpcTransport::new("ablation-memory", |method, params| match method {
        "memory.search_facts" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            Ok(
                json!({"facts": [{"node_id": "sem_001", "concept": "testing",
                "content": format!("relevant fact about '{query}'"),
                "confidence": 0.85, "source_id": "src_1", "tags": ["test"]}]}),
            )
        }
        "memory.recall_procedure" => Ok(json!({"procedures": [{"node_id": "proc_001",
            "name": "build-and-test", "steps": ["cargo build", "cargo test"],
            "prerequisites": ["rust toolchain"], "usage_count": 5}]})),
        "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
        _ => Err(RpcErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    Box::new(CognitiveMemoryClient::new(Box::new(transport)))
}

/// A live-but-empty store: bridge attaches, nothing recalled.
fn empty_memory_client() -> Box<dyn CognitiveMemoryOps> {
    let transport = InMemoryRpcTransport::new("ablation-empty", |method, _params| match method {
        "memory.search_facts" => Ok(json!({"facts": []})),
        "memory.recall_procedure" => Ok(json!({"procedures": []})),
        "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
        _ => Err(RpcErrorPayload {
            code: -32601,
            message: format!("unknown method: {method}"),
        }),
    });
    Box::new(CognitiveMemoryClient::new(Box::new(transport)))
}

// ── the hard proof: recall changes the decision input ───────────────────────

#[test]
fn ablation_with_seeded_store_shows_recall_influences_decisions() {
    let mem = seeded_memory_client();
    let outcome =
        run_enrichment_ablation(OBJECTIVE, &*mem).expect("ablation must run hermetically");

    assert!(
        outcome.recall_on_bytes > 0,
        "recall-on must inject a non-empty enrichment block"
    );
    assert_eq!(
        outcome.recall_off_bytes, 0,
        "recall-off (suppressed) must inject nothing"
    );
    assert!(
        outcome.delta_bytes > 0,
        "delta_bytes must be a positive, reproducible magnitude of difference"
    );
    assert_eq!(
        outcome.delta_bytes,
        outcome.recall_on_bytes as i64 - outcome.recall_off_bytes as i64,
        "delta_bytes is defined as recall_on_bytes - recall_off_bytes"
    );
    assert!(
        outcome.facts > 0,
        "the seeded store injects at least one fact"
    );
    assert!(
        outcome.procedures > 0,
        "the seeded store injects at least one procedure"
    );
    assert!(
        outcome.preambles_differ,
        "the two prompt preambles must be non-identical"
    );
    assert!(
        matches!(outcome.verdict, AblationVerdict::Influences),
        "delta_bytes>0 AND preambles_differ => verdict=influences"
    );
    assert_eq!(outcome.verdict.as_str(), "influences");
}

#[test]
fn ablation_with_empty_store_reports_no_influence() {
    // The bridge attaches but recalls nothing, so recall-on == recall-off.
    let mem = empty_memory_client();
    let outcome = run_enrichment_ablation(OBJECTIVE, &*mem).expect("ablation must run");

    assert_eq!(outcome.recall_on_bytes, 0);
    assert_eq!(outcome.recall_off_bytes, 0);
    assert_eq!(outcome.delta_bytes, 0);
    assert_eq!(outcome.facts, 0);
    assert_eq!(outcome.procedures, 0);
    assert!(
        !outcome.preambles_differ,
        "identical (empty) preambles must not be reported as differing"
    );
    assert!(
        matches!(outcome.verdict, AblationVerdict::NoInfluence),
        "no delta and no difference => verdict=no-influence"
    );
    assert_eq!(outcome.verdict.as_str(), "no-influence");
}

// ── the fallback bar (independent of the ablation harness) ──────────────────

/// The doc's minimum "fallback bar": `enrich_turn_input` produces a non-empty,
/// correctly-rendered preamble (both sections) when the bridge attaches and the
/// store has facts/procedures, and an EMPTY enrichment block when recall is
/// suppressed. This is the same yes/no the ablation encodes, proven directly at
/// the public seam (exercises the new 4-arg `expected` signature).
#[test]
fn fallback_bar_recall_on_renders_both_sections_recall_off_is_empty() {
    let mem = seeded_memory_client();
    let input = BaseTypeTurnInput::objective_only(OBJECTIVE);

    // recall ON: bridge attached, enrichment configured (expected=true).
    let on = enrich_turn_input(&input, Some(&*mem), None, true).expect("recall-on enrich");
    assert!(
        on.prompt_preamble.contains("## Relevant Memory Facts"),
        "recall-on must render the memory-facts section"
    );
    assert!(
        on.prompt_preamble.contains("## Known Procedures"),
        "recall-on must render the procedures section"
    );

    // recall OFF: recall suppressed (memory = None, expected=false).
    let off = enrich_turn_input(&input, None, None, false).expect("recall-off enrich");
    assert!(
        !off.prompt_preamble.contains("## Relevant Memory Facts"),
        "recall-off must not fabricate a memory-facts section"
    );
    assert!(
        off.prompt_preamble.trim().is_empty(),
        "recall-off must inject an empty enrichment block, got: {:?}",
        off.prompt_preamble
    );
    assert_ne!(
        on.prompt_preamble, off.prompt_preamble,
        "recall must change the decision's prompt preamble"
    );
}

// ── hybrid self-measurement feed (#2644) ────────────────────────────────────

/// Each ablation run feeds `delta_bytes` into the hybrid self-measurement via
/// `self_metrics` as `enrichment_ablation_delta`. `self_metrics` writes under
/// `$HOME/.simard/metrics`, so this test scopes `HOME` to a temp dir and is
/// serialised with the crate's env-mutation key.
#[test]
#[serial(cognitive_memory)]
fn ablation_feeds_delta_into_self_metrics() {
    let tmp = TempDir::new().unwrap();
    let prev_home = std::env::var_os("HOME");
    // SAFETY: env mutation is serialised by `#[serial(cognitive_memory)]`; HOME
    // is restored before the test returns / propagates any panic.
    unsafe {
        std::env::set_var("HOME", tmp.path());
    }

    let result = std::panic::catch_unwind(|| {
        let mem = seeded_memory_client();
        let outcome = run_enrichment_ablation(OBJECTIVE, &*mem).expect("ablation runs");
        record_ablation_feed(&outcome).expect("the ablation must feed self_metrics");

        let entries = simard::self_metrics::query_metrics("enrichment_ablation_delta", None)
            .expect("query self_metrics");
        assert_eq!(
            entries.len(),
            1,
            "exactly one enrichment_ablation_delta metric is recorded per run"
        );
        assert_eq!(
            entries[0].value, outcome.delta_bytes as f64,
            "the recorded value is the ablation delta_bytes"
        );
        assert!(
            entries[0].context.contains("enrichment_ablation"),
            "the feed context tags the site, got: {}",
            entries[0].context
        );
    });

    // SAFETY: restore HOME under the same serial key before resuming any panic.
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
