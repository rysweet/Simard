//! TDD **failing** tests for the `simard cognition record-thread-reasoning` CLI
//! writer verb (WS-A.2, issue #4970) — the ACT step every cognitive-thread
//! recipe calls exactly once to record its per-invocation reasoning.
//!
//! These specify the dispatch arm reached through the public
//! `dispatch_operator_cli` entry point, exactly like the existing
//! `ooda record-decide` tests. The subcommand does not exist yet, so every test
//! here fails until the Builder phase adds
//! `cognition::dispatch_record_thread_reasoning`.
//!
//! Contract (see `docs/reference/simard-cognition-record-thread-reasoning-cli.md`):
//!   * Validates `--thread` (closed 13-variant enum, case-insensitive),
//!     `--domain` (must match the thread's expected domain), the per-domain
//!     fields (bounded + clamped), and the `reasoning_summary` through the shared
//!     `sanitize_reasoning_summary` chokepoint.
//!   * Hardens `--record-path` (absolute, no `..`), then writes EXACTLY ONE
//!     atomic `0o600` `ThreadReasoningRecord`. Any validation failure ⇒ non-zero
//!     exit AND **no file on disk** (validate-all-then-write-once).
//!   * Repeatable list flags (`--top-signal`, `--note`, …) accumulate; unknown or
//!     malformed flags are rejected — nothing is silently ignored.
//!   * The record it writes is accepted by `read_verified_thread_reasoning`
//!     (writer/reader parity — one shared type, one shared chokepoint, no drift).

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ooda_brain::{
    ThreadDomain, ThreadName, ThreadReasoningRecord, read_verified_thread_reasoning,
};
use crate::operator_cli::dispatch_operator_cli;

fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
}

fn read_record(path: &Path) -> ThreadReasoningRecord {
    let bytes = std::fs::read(path).expect("thread-reasoning record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into ThreadReasoningRecord")
}

// ===========================================================================
// Happy paths + writer/reader parity
// ===========================================================================

#[test]
fn record_thread_reasoning_valid_salience_writes_one_record() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("salience.json");
    let epoch = now_epoch();
    let before = SystemTime::now() - Duration::from_secs(5);

    run(&[
        "cognition",
        "record-thread-reasoning",
        "--thread",
        "salience",
        "--domain",
        "salience",
        "--reasoning-summary",
        "prioritising goal #4970 over #4812 because a release-blocking regression outranks docs polish",
        "--top-signal",
        "regression:#4970",
        "--top-signal",
        "docs:#4812",
        "--priority",
        "0.92",
        "--written-at-epoch",
        &epoch.to_string(),
        "--record-path",
        record_path.to_str().unwrap(),
    ])
    .expect("a valid salience reasoning record must exit Ok");

    let rec = read_record(&record_path);
    assert_eq!(rec.thread, ThreadName::Salience);
    assert!(
        rec.reasoning_summary
            .contains("release-blocking regression")
    );
    match rec.domain {
        ThreadDomain::Salience {
            top_signals,
            priority,
        } => {
            assert_eq!(top_signals, vec!["regression:#4970", "docs:#4812"]);
            assert!((priority - 0.92).abs() < 1e-6);
        }
        other => panic!("expected salience domain, got {other:?}"),
    }

    // Writer/reader parity — the reader accepts what the writer produced.
    read_verified_thread_reasoning(&record_path, ThreadName::Salience, before)
        .expect("the reader must accept the record the writer just produced (no drift)");
}

#[test]
fn record_thread_reasoning_valid_notes_writes_one_record() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("metacognition.json");
    let epoch = now_epoch();
    let before = SystemTime::now() - Duration::from_secs(5);

    run(&[
        "cognition",
        "record-thread-reasoning",
        "--thread",
        "metacognition",
        "--domain",
        "notes",
        "--reasoning-summary",
        "self-audit found a retry loop inflating latency; flagged the coverage step for review",
        "--note",
        "retry loop detected",
        "--note",
        "add a retry to the coverage-comment step",
        "--written-at-epoch",
        &epoch.to_string(),
        "--record-path",
        record_path.to_str().unwrap(),
    ])
    .expect("a valid notes-domain reasoning record must exit Ok");

    let rec = read_record(&record_path);
    assert_eq!(rec.thread, ThreadName::Metacognition);
    match rec.domain {
        ThreadDomain::Notes { notes } => assert_eq!(notes.len(), 2),
        other => panic!("expected notes domain, got {other:?}"),
    }
    read_verified_thread_reasoning(&record_path, ThreadName::Metacognition, before)
        .expect("reader parity on the shared notes domain");
}

#[test]
fn record_thread_reasoning_thread_matched_case_insensitively() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("salience.json");
    let epoch = now_epoch();
    run(&[
        "cognition",
        "record-thread-reasoning",
        "--thread",
        "SALIENCE",
        "--domain",
        "salience",
        "--reasoning-summary",
        "ranking goals by release impact for this cycle",
        "--priority",
        "0.4",
        "--written-at-epoch",
        &epoch.to_string(),
        "--record-path",
        record_path.to_str().unwrap(),
    ])
    .expect("an upper-case --thread must be accepted (matched case-insensitively)");
    assert_eq!(read_record(&record_path).thread, ThreadName::Salience);
}

#[test]
fn record_thread_reasoning_summary_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("reflection.json");
    let summary_path = dir.path().join("summary.txt");
    std::fs::write(
        &summary_path,
        "engineer #4801 stalled on flaky CI, not on the code itself; recommend a retry",
    )
    .unwrap();
    let epoch = now_epoch();

    run(&[
        "cognition",
        "record-thread-reasoning",
        "--thread",
        "reflection",
        "--domain",
        "notes",
        "--reasoning-summary-path",
        summary_path.to_str().unwrap(),
        "--note",
        "flaky CI, not code",
        "--written-at-epoch",
        &epoch.to_string(),
        "--record-path",
        record_path.to_str().unwrap(),
    ])
    .expect("reasoning summary supplied via --reasoning-summary-path must be accepted");
    assert!(
        read_record(&record_path)
            .reasoning_summary
            .contains("flaky CI")
    );
}

#[test]
fn record_thread_reasoning_priority_is_clamped() {
    // `--priority` is clamped into [0.0, 1.0] by the tool (defense in depth), so
    // an out-of-range value is corrected, never rejected outright.
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("salience.json");
    let epoch = now_epoch();
    run(&[
        "cognition",
        "record-thread-reasoning",
        "--thread",
        "salience",
        "--domain",
        "salience",
        "--reasoning-summary",
        "an over-eager appraisal that reported a priority above one",
        "--priority",
        "1.5",
        "--written-at-epoch",
        &epoch.to_string(),
        "--record-path",
        record_path.to_str().unwrap(),
    ])
    .expect("an out-of-range priority is clamped, not rejected");
    match read_record(&record_path).domain {
        ThreadDomain::Salience { priority, .. } => {
            assert!((priority - 1.0).abs() < 1e-6, "priority clamps to 1.0")
        }
        other => panic!("expected salience domain, got {other:?}"),
    }
}

// ===========================================================================
// Rejections — validate-all-then-write-once (NO file on disk on any failure)
// ===========================================================================

fn assert_rejected_no_file(args: &[&str], record_path: &Path) {
    assert!(run(args).is_err(), "invalid input must exit non-zero");
    assert!(
        !record_path.exists(),
        "a rejected invocation must leave NO file on disk (validate-all-then-write-once)"
    );
}

#[test]
fn record_thread_reasoning_unknown_thread_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "overseer",
            "--domain",
            "notes",
            "--reasoning-summary",
            "a thread that is not in the closed roster",
            "--note",
            "n",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_unknown_domain_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "bogus",
            "--reasoning-summary",
            "a domain tag that is not in the closed set",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_domain_thread_mismatch_rejected() {
    // salience must carry the `salience` domain, not the shared `notes` bucket.
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "notes",
            "--reasoning-summary",
            "a salience thread must not use the notes domain",
            "--note",
            "n",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_empty_summary_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "salience",
            "--reasoning-summary",
            "",
            "--priority",
            "0.5",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_requires_a_summary_source() {
    // Neither --reasoning-summary nor --reasoning-summary-path supplied.
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "salience",
            "--priority",
            "0.5",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_summary_inline_and_path_are_mutually_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    let summary_path = dir.path().join("s.txt");
    std::fs::write(
        &summary_path,
        "a summary from a file that should not be combined",
    )
    .unwrap();
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "salience",
            "--reasoning-summary",
            "an inline summary that conflicts with the file",
            "--reasoning-summary-path",
            summary_path.to_str().unwrap(),
            "--priority",
            "0.5",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_record_path_must_be_absolute() {
    // A relative --record-path is rejected by harden_path.
    assert!(
        run(&[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "salience",
            "--reasoning-summary",
            "a summary with a relative record path",
            "--priority",
            "0.5",
            "--written-at-epoch",
            &now_epoch().to_string(),
            "--record-path",
            "relative/salience.json",
        ])
        .is_err()
    );
}

#[test]
fn record_thread_reasoning_record_path_must_not_traverse() {
    let dir = tempfile::tempdir().unwrap();
    let traversal = dir.path().join("..").join("salience.json");
    assert!(
        run(&[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "salience",
            "--reasoning-summary",
            "a summary with a traversing record path",
            "--priority",
            "0.5",
            "--written-at-epoch",
            &now_epoch().to_string(),
            "--record-path",
            traversal.to_str().unwrap(),
        ])
        .is_err()
    );
}

#[test]
fn record_thread_reasoning_unknown_flag_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "salience",
            "--reasoning-summary",
            "a summary alongside an unknown flag",
            "--priority",
            "0.5",
            "--totally-unknown-flag",
            "x",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_missing_written_at_epoch_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("x.json");
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "salience",
            "--domain",
            "salience",
            "--reasoning-summary",
            "a summary without the required written-at-epoch",
            "--priority",
            "0.5",
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_creative_ideas_kept_le_considered_enforced() {
    // Domain invariant: kept_after_dedup <= ideas_considered.
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("creative_ideas.json");
    let epoch = now_epoch();
    assert_rejected_no_file(
        &[
            "cognition",
            "record-thread-reasoning",
            "--thread",
            "creative_ideas",
            "--domain",
            "creative_ideas",
            "--reasoning-summary",
            "kept more ideas than were considered, which is impossible",
            "--ideas-considered",
            "3",
            "--kept-after-dedup",
            "5",
            "--written-at-epoch",
            &epoch.to_string(),
            "--record-path",
            record_path.to_str().unwrap(),
        ],
        &record_path,
    );
}

#[test]
fn record_thread_reasoning_creative_ideas_valid_counts_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let record_path = dir.path().join("creative_ideas.json");
    let epoch = now_epoch();
    run(&[
        "cognition",
        "record-thread-reasoning",
        "--thread",
        "creative_ideas",
        "--domain",
        "creative_ideas",
        "--reasoning-summary",
        "considered five ideas and kept three after dedup against #4959 records",
        "--ideas-considered",
        "5",
        "--kept-after-dedup",
        "3",
        "--written-at-epoch",
        &epoch.to_string(),
        "--record-path",
        record_path.to_str().unwrap(),
    ])
    .expect("kept_after_dedup <= ideas_considered must be accepted");
    match read_record(&record_path).domain {
        ThreadDomain::CreativeIdeas {
            ideas_considered,
            kept_after_dedup,
        } => {
            assert_eq!(ideas_considered, 5);
            assert_eq!(kept_after_dedup, 3);
        }
        other => panic!("expected creative_ideas domain, got {other:?}"),
    }
}
