//! TDD **failing** tests for the `ThreadReasoningRecord` typed record, its
//! shared `sanitize_reasoning_summary` chokepoint, the fail-CLOSED reader
//! `read_verified_thread_reasoning` (R1–R7), and the `recipe_rail`
//! `run_reflective_thread` helper that surfaces a thread's natural-language
//! reasoning into `ThreadOutcome.summary` instead of the boolean `"{recipe}: ok"`.
//!
//! Authored **tests-first** for WS-A (issue #4970). They pin the exact contract
//! documented in `docs/reference/simard-cognition-record-thread-reasoning-cli.md`:
//!
//!   * `THREAD_REASONING_SCHEMA = "thread-reasoning/v1"`, `MAX_AGE_SECS = 300`.
//!   * `ThreadName` — the closed 13-variant roster (snake_case wire tags).
//!   * `ThreadDomain` — internally-tagged (`"kind"`) closed enum; per-thread
//!     domain fields.
//!   * `sanitize_reasoning_summary` — one shared chokepoint (writer + reader):
//!     non-empty after sanitize, `>= 8` graphemes, `<= 600` bytes, C0/ANSI
//!     stripped, secrets scrubbed.
//!   * `read_verified_thread_reasoning(path, expected_thread, invoke_start)` —
//!     every failure mode is an `Err` (R1–R7).
//!   * `run_reflective_thread` — reads the record fail-closed and surfaces
//!     `reasoning_summary` (NEVER `"<recipe>: ok"`, NEVER scraped stdout);
//!     pre-truncates any stale record so a prior run cannot be replayed.
//!
//! None of that surface exists yet, so this whole module fails RED (does not
//! compile) until the Builder phase lands `thread_reasoning_record.rs` and the
//! rail helper. It turns GREEN once the implementation matches this contract.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::ooda_brain::{
    MAX_AGE_SECS, THREAD_REASONING_SCHEMA, ThreadDomain, ThreadName, ThreadReasoningRecord,
    read_verified_thread_reasoning, sanitize_reasoning_summary,
};

use super::recipe_rail::{self, InvokeResult, RecipeInvoker, run_reflective_thread};

/// Current unix-epoch seconds (records must be fresh for R7 to pass).
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
}

/// A fully-valid salience record, fresh as of `epoch`, that the reader accepts.
fn valid_salience_record(epoch: u64) -> ThreadReasoningRecord {
    ThreadReasoningRecord {
        schema: THREAD_REASONING_SCHEMA.to_string(),
        thread: ThreadName::Salience,
        reasoning_summary: "prioritising goal #4970 over #4812 because a \
             release-blocking regression outranks docs polish"
            .to_string(),
        written_at_epoch: epoch,
        domain: ThreadDomain::Salience {
            top_signals: vec!["regression:#4970".to_string(), "docs:#4812".to_string()],
            priority: 0.92,
        },
    }
}

/// Persist a `serde_json::Value` verbatim to `path` (used to author records the
/// typed writer would never produce — control-only text, unknown tags, an extra
/// top-level key — so the reader's fail-closed matrix is exercised directly).
fn write_raw(path: &Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, serde_json::to_vec(value).expect("serialize")).expect("write raw record");
}

/// A fresh, fully-valid salience record as raw JSON (a base to tweak per test).
fn valid_salience_json(epoch: u64) -> serde_json::Value {
    json!({
        "schema": "thread-reasoning/v1",
        "thread": "salience",
        "reasoning_summary": "prioritising #4970 over #4812 due to a release-blocking regression",
        "written_at_epoch": epoch,
        "domain": { "kind": "salience", "top_signals": ["a", "b"], "priority": 0.9 }
    })
}

// ===========================================================================
// Pins: schema + freshness constants + closed ThreadName roster
// ===========================================================================

#[test]
fn schema_pin_is_thread_reasoning_v1() {
    assert_eq!(THREAD_REASONING_SCHEMA, "thread-reasoning/v1");
}

#[test]
fn max_age_secs_is_five_minutes() {
    assert_eq!(MAX_AGE_SECS, 300);
}

#[test]
fn thread_name_roster_is_the_closed_thirteen() {
    // The full roster + their exact snake_case wire tags (R6 identity / R4 parse).
    let roster: &[(ThreadName, &str)] = &[
        (ThreadName::Salience, "salience"),
        (ThreadName::Metacognition, "metacognition"),
        (ThreadName::Reflection, "reflection"),
        (ThreadName::Prospection, "prospection"),
        (ThreadName::OperatorModel, "operator_model"),
        (ThreadName::Analogy, "analogy"),
        (ThreadName::Narrative, "narrative"),
        (ThreadName::ValuesDeliberation, "values_deliberation"),
        (ThreadName::Consolidation, "consolidation"),
        (ThreadName::CreativeIdeas, "creative_ideas"),
        (ThreadName::EngineerLogAnalysis, "engineer_log_analysis"),
        (ThreadName::Interoception, "interoception"),
        (ThreadName::Maintenance, "maintenance"),
    ];
    assert_eq!(roster.len(), 13, "exactly thirteen threads");
    for (variant, label) in roster {
        // Serializes to its snake_case tag …
        assert_eq!(
            serde_json::to_value(variant).unwrap(),
            json!(label),
            "{label} serializes to its snake_case tag"
        );
        // … and round-trips back.
        let back: ThreadName = serde_json::from_value(json!(label)).unwrap();
        assert_eq!(&back, variant, "{label} round-trips");
    }
}

#[test]
fn thread_name_rejects_an_unknown_tag() {
    assert!(serde_json::from_value::<ThreadName>(json!("overseer")).is_err());
    assert!(serde_json::from_value::<ThreadName>(json!("ooda")).is_err());
}

// ===========================================================================
// sanitize_reasoning_summary — the single shared chokepoint (R5)
// ===========================================================================

#[test]
fn sanitize_summary_accepts_a_normal_sentence() {
    let s = sanitize_reasoning_summary(
        "filed a capacity health-goal because disk use breaches the 85% guard",
    )
    .expect("a clean 1–3 sentence summary is accepted");
    assert!(s.contains("capacity health-goal"));
}

#[test]
fn sanitize_summary_rejects_empty_and_whitespace_only() {
    assert!(sanitize_reasoning_summary("").is_none());
    assert!(sanitize_reasoning_summary("   \t  ").is_none());
}

#[test]
fn sanitize_summary_rejects_control_only_text() {
    // A summary made up ENTIRELY of ANSI/C0 control bytes collapses to empty
    // after sanitize and must fail closed (never honored verbatim).
    assert!(sanitize_reasoning_summary("\u{1b}[31m\u{7}\u{0}").is_none());
}

#[test]
fn sanitize_summary_rejects_too_short_text() {
    // Below the >= 8 grapheme floor.
    assert!(sanitize_reasoning_summary("nope").is_none());
}

#[test]
fn sanitize_summary_rejects_oversized_text() {
    // Over the 600-byte hard bound — rejected, never silently truncated
    // (mirrors the concept-key "reject, don't truncate" precedent).
    let huge = "x".repeat(601);
    assert!(sanitize_reasoning_summary(&huge).is_none());
}

#[test]
fn sanitize_summary_strips_control_and_folds_whitespace() {
    let s = sanitize_reasoning_summary("line one\nline two\ttabbed\u{1b}[0m end")
        .expect("mixed control input still yields real content");
    assert!(!s.contains('\n') && !s.contains('\t') && !s.contains('\u{1b}'));
    assert!(s.contains("line one") && s.contains("end"));
}

#[test]
fn sanitize_summary_scrubs_secrets() {
    let s = sanitize_reasoning_summary(
        "leaked token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 while ranking goals",
    )
    .expect("the surrounding sentence is otherwise valid");
    assert!(
        !s.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"),
        "a GitHub token must be scrubbed before it can reach a durable record or log"
    );
}

// ===========================================================================
// ThreadReasoningRecord serde shape
// ===========================================================================

#[test]
fn record_serializes_with_internally_tagged_domain() {
    let rec = valid_salience_record(now_epoch());
    let v = serde_json::to_value(&rec).unwrap();
    assert_eq!(v["schema"], json!("thread-reasoning/v1"));
    assert_eq!(v["thread"], json!("salience"));
    // Domain is a nested, internally-tagged object keyed on "kind".
    assert_eq!(v["domain"]["kind"], json!("salience"));
    assert_eq!(v["domain"]["priority"], json!(0.92));
    assert!(v["domain"]["top_signals"].is_array());
}

#[test]
fn record_notes_domain_round_trips() {
    let rec = ThreadReasoningRecord {
        schema: THREAD_REASONING_SCHEMA.to_string(),
        thread: ThreadName::Metacognition,
        reasoning_summary: "self-audit found a retry loop inflating latency".to_string(),
        written_at_epoch: now_epoch(),
        domain: ThreadDomain::Notes {
            notes: vec![
                "retry loop detected".to_string(),
                "flagged for review".to_string(),
            ],
        },
    };
    let v = serde_json::to_value(&rec).unwrap();
    assert_eq!(v["domain"]["kind"], json!("notes"));
    let back: ThreadReasoningRecord = serde_json::from_value(v).unwrap();
    assert_eq!(back, rec);
}

// ===========================================================================
// read_verified_thread_reasoning — the fail-CLOSED R1–R7 matrix
// ===========================================================================

#[test]
fn r8_valid_record_reads_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let epoch = now_epoch();
    crate::persistence::persist_json("tr-test", &path, &valid_salience_record(epoch))
        .expect("persist");
    // invoke_start just before the write; the fresh file's mtime is >= it.
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    let rec = read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start)
        .expect("a fresh, identity-matched, schema-pinned record reads Ok");
    assert_eq!(rec.thread, ThreadName::Salience);
    assert!(
        rec.reasoning_summary
            .contains("release-blocking regression")
    );
}

#[test]
fn r1_absent_file_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json"); // never written
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    std::fs::write(&path, b"{ this is not json").unwrap();
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r3_schema_mismatch_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let mut v = valid_salience_json(now_epoch());
    v["schema"] = json!("thread-reasoning/v2");
    write_raw(&path, &v);
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r4_unknown_thread_name_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let mut v = valid_salience_json(now_epoch());
    v["thread"] = json!("overseer");
    write_raw(&path, &v);
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r4_unknown_domain_tag_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let mut v = valid_salience_json(now_epoch());
    v["domain"] = json!({ "kind": "bogus", "top_signals": [], "priority": 0.1 });
    write_raw(&path, &v);
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r4_unknown_top_level_key_fails_closed() {
    // SR-TR-7: `deny_unknown_fields` does not cross a `flatten` boundary, so the
    // reader performs an explicit unknown-top-level-key check. A crafted extra
    // field must not slip past.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let mut v = valid_salience_json(now_epoch());
    v["injected"] = json!(true);
    write_raw(&path, &v);
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r5_empty_summary_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let mut v = valid_salience_json(now_epoch());
    v["reasoning_summary"] = json!("");
    write_raw(&path, &v);
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r5_control_only_summary_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let mut v = valid_salience_json(now_epoch());
    v["reasoning_summary"] = json!("\u{1b}[0m\u{7}");
    write_raw(&path, &v);
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r6_thread_identity_mismatch_fails_closed() {
    // A record written by `salience` must never be honored when the rail invoked
    // a different thread (the only stable identity a thread has).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metacognition.json");
    write_raw(&path, &valid_salience_json(now_epoch()));
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(
        read_verified_thread_reasoning(&path, ThreadName::Metacognition, invoke_start).is_err(),
        "record.thread=salience read as metacognition must fail closed (R6)"
    );
}

#[test]
fn r7_stale_mtime_fails_closed() {
    // A file whose mtime predates invoke_start is a leftover from a prior run.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    crate::persistence::persist_json("tr-test", &path, &valid_salience_record(now_epoch()))
        .expect("persist");
    // Capture an invoke_start well AFTER the write → mtime < invoke_start.
    let invoke_start = SystemTime::now() + Duration::from_secs(600);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r7_future_epoch_skew_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    // Fresh file (mtime ok) but a written_at_epoch far in the future → R7 epoch
    // defense-in-depth rejects it.
    let rec = valid_salience_record(now_epoch() + 10_000);
    crate::persistence::persist_json("tr-test", &path, &rec).expect("persist");
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn r7_past_epoch_skew_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("salience.json");
    let rec = valid_salience_record(now_epoch().saturating_sub(10_000));
    crate::persistence::persist_json("tr-test", &path, &rec).expect("persist");
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(read_verified_thread_reasoning(&path, ThreadName::Salience, invoke_start).is_err());
}

#[test]
fn notes_domain_list_over_cap_fails_closed() {
    // SR-TR-9: the shared `notes` bucket is capped at <= 5 elements; the reader
    // re-validates the bound.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("metacognition.json");
    let epoch = now_epoch();
    let v = json!({
        "schema": "thread-reasoning/v1",
        "thread": "metacognition",
        "reasoning_summary": "self-audit surfaced six separate concerns to record",
        "written_at_epoch": epoch,
        "domain": { "kind": "notes", "notes": ["n1", "n2", "n3", "n4", "n5", "n6"] }
    });
    write_raw(&path, &v);
    let invoke_start = SystemTime::now() - Duration::from_secs(2);
    assert!(
        read_verified_thread_reasoning(&path, ThreadName::Metacognition, invoke_start).is_err(),
        "a 6-element notes list exceeds the <=5 cap and must fail closed"
    );
}

// ===========================================================================
// run_reflective_thread — the rail surfaces the record, never `"{recipe}: ok"`
// ===========================================================================

/// Recorded argv for every rail invocation (each call's `-c key=value` pairs).
type RecordedCalls = Arc<Mutex<Vec<Vec<(String, String)>>>>;

/// A fake [`RecipeInvoker`] that, on invoke, writes a canned
/// [`ThreadReasoningRecord`] to the `record_path` context var the rail passes,
/// then returns a canned [`InvokeResult`]. `record_to_write = None` models a
/// recipe that "ran" but wrote no valid record (fail-closed path). Records every
/// call's argv so the rail's `-c record_path=<abs>` discipline is testable.
#[derive(Clone)]
struct RecordWritingInvoker {
    result: InvokeResult,
    record_to_write: Option<ThreadReasoningRecord>,
    calls: RecordedCalls,
}

impl RecordWritingInvoker {
    fn new(result: InvokeResult, record_to_write: Option<ThreadReasoningRecord>) -> Self {
        Self {
            result,
            record_to_write,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<Vec<(String, String)>> {
        self.calls.lock().unwrap().clone()
    }
}

impl RecipeInvoker for RecordWritingInvoker {
    fn invoke(&self, _recipe_name: &str, ctx_vars: &[(&str, String)]) -> InvokeResult {
        let recorded: Vec<(String, String)> = ctx_vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        self.calls.lock().unwrap().push(recorded);

        if let Some(rec) = &self.record_to_write {
            let record_path = ctx_vars
                .iter()
                .find(|(k, _)| *k == "record_path")
                .map(|(_, v)| v.clone())
                .expect("the rail MUST pass the record_path context var to the recipe");
            crate::persistence::persist_json("tr-fake", Path::new(&record_path), rec)
                .expect("fake recipe write");
        }
        self.result.clone()
    }
}

/// The rail's record path convention (pinned by the reference doc).
fn expected_record_path(state_root: &Path, thread_label: &str) -> PathBuf {
    state_root
        .join("cognitive_threads")
        .join("reasoning")
        .join(format!("{thread_label}.json"))
}

#[test]
fn rail_surfaces_record_summary_not_the_ok_string() {
    let dir = tempfile::tempdir().unwrap();
    let state_root = dir.path();

    let rec = ThreadReasoningRecord {
        schema: THREAD_REASONING_SCHEMA.to_string(),
        thread: ThreadName::Salience,
        reasoning_summary: "prioritising goal #4970 over #4812 for the release".to_string(),
        written_at_epoch: now_epoch(),
        domain: ThreadDomain::Salience {
            top_signals: vec!["regression:#4970".to_string()],
            priority: 0.9,
        },
    };
    let fake = RecordWritingInvoker::new(InvokeResult::Ran, Some(rec.clone()));

    let outcome = run_reflective_thread(
        &fake,
        "salience-appraise",
        ThreadName::Salience,
        state_root,
        Vec::new(),
        Instant::now(),
    );

    assert!(
        outcome.ran && outcome.success,
        "a valid record => a successful tick"
    );
    assert_eq!(
        outcome.summary, rec.reasoning_summary,
        "the daemon log line must be the record's reasoning_summary"
    );
    assert_ne!(
        outcome.summary, "salience-appraise: ok",
        "the boolean `<recipe>: ok` collapse must be gone (sourced from the record, not stdout)"
    );
}

#[test]
fn rail_passes_the_record_path_context_var() {
    let dir = tempfile::tempdir().unwrap();
    let state_root = dir.path();
    let rec = valid_salience_record(now_epoch());
    let fake = RecordWritingInvoker::new(InvokeResult::Ran, Some(rec));

    let _ = run_reflective_thread(
        &fake,
        "salience-appraise",
        ThreadName::Salience,
        state_root,
        Vec::new(),
        Instant::now(),
    );

    let calls = fake.calls();
    assert_eq!(calls.len(), 1, "exactly one recipe invocation");
    let record_path = calls[0]
        .iter()
        .find(|(k, _)| k == "record_path")
        .map(|(_, v)| v.clone())
        .expect("the rail passes `-c record_path=<abs>`");
    assert_eq!(
        Path::new(&record_path),
        expected_record_path(state_root, "salience"),
        "record path = state_root/cognitive_threads/reasoning/<thread>.json"
    );
    assert!(
        Path::new(&record_path).is_absolute(),
        "record path must be absolute"
    );
}

#[test]
fn rail_fails_closed_when_recipe_ran_but_wrote_no_record() {
    // A recipe that exits 0 but writes no valid record is a FAILURE, never a
    // silent `"<recipe>: ok"` success.
    let dir = tempfile::tempdir().unwrap();
    let fake = RecordWritingInvoker::new(InvokeResult::Ran, None);

    let outcome = run_reflective_thread(
        &fake,
        "salience-appraise",
        ThreadName::Salience,
        dir.path(),
        Vec::new(),
        Instant::now(),
    );

    assert!(outcome.ran, "the tick ran");
    assert!(
        !outcome.success,
        "no valid record => a FAILED tick (fail-closed)"
    );
    assert_ne!(outcome.summary, "salience-appraise: ok");
}

#[test]
fn rail_fails_closed_on_recipe_failure() {
    let dir = tempfile::tempdir().unwrap();
    let fake = RecordWritingInvoker::new(
        InvokeResult::Failed {
            detail: "recipe-runner-rs exited 1".to_string(),
        },
        None,
    );

    let outcome = run_reflective_thread(
        &fake,
        "salience-appraise",
        ThreadName::Salience,
        dir.path(),
        Vec::new(),
        Instant::now(),
    );

    assert!(!outcome.success, "a failed recipe run => a failed tick");
}

#[test]
fn rail_pre_truncates_a_stale_record_before_invoking() {
    // Anti-replay: a leftover record from a prior run must be deleted BEFORE the
    // recipe is spawned, so a recipe that writes nothing this invocation cannot
    // have the stale reasoning surfaced as current.
    let dir = tempfile::tempdir().unwrap();
    let state_root = dir.path();
    let stale_path = expected_record_path(state_root, "salience");

    let stale = ThreadReasoningRecord {
        schema: THREAD_REASONING_SCHEMA.to_string(),
        thread: ThreadName::Salience,
        reasoning_summary: "STALE reasoning from a previous invocation".to_string(),
        written_at_epoch: now_epoch(),
        domain: ThreadDomain::Salience {
            top_signals: vec!["old".to_string()],
            priority: 0.5,
        },
    };
    crate::persistence::persist_json("tr-test", &stale_path, &stale).expect("seed stale record");

    // The recipe "ran" but writes NOTHING this invocation.
    let fake = RecordWritingInvoker::new(InvokeResult::Ran, None);
    let outcome = run_reflective_thread(
        &fake,
        "salience-appraise",
        ThreadName::Salience,
        state_root,
        Vec::new(),
        Instant::now(),
    );

    assert!(
        !outcome.success,
        "the stale record must have been pre-truncated → nothing to read → fail closed"
    );
    assert_ne!(
        outcome.summary, stale.reasoning_summary,
        "a prior run's reasoning must never be replayed as current"
    );
}

/// The rail helper must apply the same double env-gate every reflective thread
/// shares (kept accessible for the Builder to wire; this pins the helper exists).
#[test]
fn recipe_rail_still_exposes_the_env_gate_predicate() {
    assert!(recipe_rail::env_gate_open(None, None));
    assert!(!recipe_rail::env_gate_open(Some("0"), None));
}
