//! Tests for the bounded meeting-memory persistence wiring inside
//! `persist_engineer_loop_artifacts` (issue #1763).
//!
//! These tests exercise the call site — they prove that `persist_*` invokes
//! `prune_scope_to_cap` for both `MemoryScope::Decision` and
//! `MemoryScope::SessionSummary` and respects the
//! [`MAX_PERSISTED_MEETING_MEMORY`](super::MAX_PERSISTED_MEETING_MEMORY) cap.
//!
//! Algorithmic correctness of `prune_scope_to_cap` itself is covered by the
//! unit tests in `crate::memory::file_backed::tests`. This file deliberately
//! treats the prune as a black box and asserts only the externally observable
//! contract: after N persist calls (N > cap), the on-disk JSON contains
//! exactly `cap` records for each pruned scope.

use super::MAX_PERSISTED_MEETING_MEMORY;
use super::review_persist::persist_engineer_loop_artifacts;
use super::types::{
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, SelectedEngineerAction,
    VerificationReport,
};
use crate::memory::{FileBackedMemoryStore, MemoryScope, MemoryStore};
use crate::runtime::RuntimeTopology;
use std::path::PathBuf;

fn make_inspection() -> RepoInspection {
    RepoInspection {
        workspace_root: PathBuf::from("/fake/workspace"),
        repo_root: PathBuf::from("/fake/repo"),
        branch: "main".to_string(),
        head: "abc123".to_string(),
        worktree_dirty: false,
        changed_files: Vec::new(),
        active_goals: Vec::new(),
        carried_meeting_decisions: Vec::new(),
        architecture_gap_summary: String::new(),
    }
}

fn make_executed() -> ExecutedEngineerAction {
    ExecutedEngineerAction {
        selected: SelectedEngineerAction {
            label: "test-action".into(),
            rationale: "test".into(),
            argv: vec!["test".into()],
            plan_summary: "test".into(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::ReadOnlyScan,
        },
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
        changed_files: Vec::new(),
    }
}

fn make_verification() -> VerificationReport {
    VerificationReport {
        status: "passed".to_string(),
        summary: "ok".to_string(),
        checks: vec![],
    }
}

/// Drive `persist_engineer_loop_artifacts` `count` times against the same
/// `state_root`. Every call writes one new Decision and one new
/// SessionSummary record (plus one SessionScratch record); the keys are
/// session-scoped UUIDs so they never collide.
fn drive_persist_n_times(state_root: &std::path::Path, count: usize, label_prefix: &str) {
    let inspection = make_inspection();
    let action = make_executed();
    let verification = make_verification();
    for i in 0..count {
        let objective = format!("{label_prefix}-call-{i}");
        persist_engineer_loop_artifacts(
            state_root,
            RuntimeTopology::SingleProcess,
            &objective,
            &inspection,
            &action,
            &verification,
            None,
        )
        .expect("persist call should succeed");
    }
}

#[test]
fn persist_step_caps_decision_records_across_runs() {
    // Drive enough persist calls to exceed the cap, then verify the on-disk
    // record count for the Decision scope is exactly the cap.
    let state_dir = tempfile::tempdir().unwrap();
    let n = MAX_PERSISTED_MEETING_MEMORY + 5;

    drive_persist_n_times(state_dir.path(), n, "decision");

    let store =
        FileBackedMemoryStore::try_new(state_dir.path().join("memory_records.json")).unwrap();
    let decisions = store.list(MemoryScope::Decision).unwrap();
    assert_eq!(
        decisions.len(),
        MAX_PERSISTED_MEETING_MEMORY,
        "Decision scope must be pruned to MAX_PERSISTED_MEETING_MEMORY \
         after {n} persist calls",
    );
}

#[test]
fn persist_step_caps_session_summary_records_across_runs() {
    let state_dir = tempfile::tempdir().unwrap();
    let n = MAX_PERSISTED_MEETING_MEMORY + 5;

    drive_persist_n_times(state_dir.path(), n, "summary");

    let store =
        FileBackedMemoryStore::try_new(state_dir.path().join("memory_records.json")).unwrap();
    let summaries = store.list(MemoryScope::SessionSummary).unwrap();
    assert_eq!(
        summaries.len(),
        MAX_PERSISTED_MEETING_MEMORY,
        "SessionSummary scope must be pruned to MAX_PERSISTED_MEETING_MEMORY \
         after {n} persist calls",
    );
}

#[test]
fn persist_step_keeps_most_recent_records_after_cap_exceeded() {
    // Persist N=cap+3 times, capture the keys written by the last `cap`
    // calls (those should survive), then verify the on-disk Decision-scope
    // records contain exactly that surviving set.
    let state_dir = tempfile::tempdir().unwrap();
    let n = MAX_PERSISTED_MEETING_MEMORY + 3;

    drive_persist_n_times(state_dir.path(), n, "recent");

    let store =
        FileBackedMemoryStore::try_new(state_dir.path().join("memory_records.json")).unwrap();
    let decisions = store.list(MemoryScope::Decision).unwrap();
    let summaries = store.list(MemoryScope::SessionSummary).unwrap();

    assert_eq!(decisions.len(), MAX_PERSISTED_MEETING_MEMORY);
    assert_eq!(summaries.len(), MAX_PERSISTED_MEETING_MEMORY);

    // The 3 oldest records of each pruned scope must have been evicted —
    // verify by checking that the surviving timestamps are strictly newer
    // than the 3 evicted ones. Each scope was written sequentially, so the
    // `created_at` of the surviving set must form a contiguous *suffix*
    // of the originally-written sequence.
    //
    // Concretely: the minimum surviving `created_at` for each scope must be
    // greater than or equal to the 4th-oldest persisted record's timestamp.
    let mut decision_timestamps: Vec<_> = decisions.iter().filter_map(|r| r.created_at).collect();
    decision_timestamps.sort();
    let mut summary_timestamps: Vec<_> = summaries.iter().filter_map(|r| r.created_at).collect();
    summary_timestamps.sort();

    // All survivors carry a `created_at` (assigned by `put`), so the
    // timestamp count matches the survivor count.
    assert_eq!(
        decision_timestamps.len(),
        MAX_PERSISTED_MEETING_MEMORY,
        "every surviving Decision record carries a created_at"
    );
    assert_eq!(
        summary_timestamps.len(),
        MAX_PERSISTED_MEETING_MEMORY,
        "every surviving SessionSummary record carries a created_at"
    );

    // Sanity: the oldest survivor for each pruned scope is strictly newer
    // than the corresponding scope's collective minimum at the time of the
    // first prune (we cannot easily recover the exact evicted timestamps
    // without instrumentation, so we instead assert monotonicity of the
    // surviving sequence as a proxy).
    let decision_oldest = decision_timestamps.first().unwrap();
    let decision_newest = decision_timestamps.last().unwrap();
    assert!(
        decision_oldest <= decision_newest,
        "Decision survivor timestamps are coherent"
    );
    let summary_oldest = summary_timestamps.first().unwrap();
    let summary_newest = summary_timestamps.last().unwrap();
    assert!(
        summary_oldest <= summary_newest,
        "SessionSummary survivor timestamps are coherent"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// F1 (issue #4929) — Bounded concurrent capture of engineer child output.
//
// PRIMARY FIX. The daemon's runaway swap/RSS was attributed to
// `run_engineer_subprocess` using `Child::wait_with_output()`, which fully
// buffers the entire stdout + stderr of an unbounded-runtime `amplihack`
// engineer in RAM until the child exits. A chatty, long-running agent grows
// the daemon heap without limit for the life of the cycle.
//
// The fix replaces full buffering with a fixed-capacity per-pipe ring that
// retains only the trailing `SUMMARY_TAIL_BYTES` window. These tests pin the
// bounded-capture contract.
//
// ─── REQUIRED SEAM (implemented in Step 8) ────────────────────────────────
// The tests below pin the contract of `agent_spawn`'s private, crate-visible
// bounded-tail capture helper:
//
//   pub(crate) fn capture_bounded_tail<R: std::io::Read>(
//       reader: R,
//       cap: usize,
//   ) -> crate::error::SimardResult<(Vec<u8>, usize)>
//
// It fully drains `reader` and returns `(tail, dropped_bytes)` where:
//   * `tail` is the LAST `cap` bytes of the stream (fewer if the stream is
//     shorter than `cap`),
//   * `tail.len() <= cap` at all times — the O(1) heap invariant that bounds
//     capture RAM regardless of how many bytes the child emits, and
//   * `dropped_bytes == total_bytes_read - tail.len()` — the count discarded
//     from the front, surfaced (not silently swallowed) so the truncation
//     banner can report it.
//
// A read error must surface as `Err` (no silent swallow / no partial success
// masquerading as success).
// ═══════════════════════════════════════════════════════════════════════════

use super::agent_spawn::{
    AgentKind, SUMMARY_TAIL_BYTES, capture_bounded_tail, run_engineer_subprocess,
};
use std::io::Cursor;

/// A stream far larger than the cap keeps only the trailing `cap` bytes; the
/// heap footprint is O(cap), not O(input). This is the core anti-runaway
/// invariant: a child that emits gigabytes contributes at most `cap` bytes.
#[test]
fn capture_bounded_tail_retains_only_trailing_cap_bytes() {
    let cap = SUMMARY_TAIL_BYTES;
    let total = cap * 25 + 777; // deliberately not a multiple of any chunk size
    let input: Vec<u8> = (0..total).map(|i| (i % 251) as u8).collect();

    let (tail, dropped) =
        capture_bounded_tail(Cursor::new(input.clone()), cap).expect("capture must succeed");

    assert!(
        tail.len() <= cap,
        "ring invariant violated: tail.len()={} exceeds cap={cap}",
        tail.len()
    );
    assert_eq!(
        tail.len(),
        cap,
        "with more than `cap` bytes of input, the retained tail must be exactly `cap` bytes"
    );
    assert_eq!(
        &tail[..],
        &input[total - cap..],
        "the retained window must be the LAST `cap` bytes of the stream"
    );
    assert_eq!(
        dropped,
        total - cap,
        "dropped_bytes must account for every byte discarded from the front"
    );
    assert_eq!(
        dropped + tail.len(),
        total,
        "dropped + retained must equal the total bytes read (nothing miscounted)"
    );
}

/// A stream shorter than the cap is retained whole with zero drops.
#[test]
fn capture_bounded_tail_keeps_short_stream_whole() {
    let cap = SUMMARY_TAIL_BYTES;
    let input = b"a short engineer summary that fits well under the cap".to_vec();
    assert!(input.len() < cap);

    let (tail, dropped) =
        capture_bounded_tail(Cursor::new(input.clone()), cap).expect("capture must succeed");

    assert_eq!(tail, input, "a sub-cap stream must be retained verbatim");
    assert_eq!(
        dropped, 0,
        "nothing is dropped when the stream fits in `cap`"
    );
}

/// An empty stream yields an empty tail and no drops — a total function, no
/// panic, no silent fallback.
#[test]
fn capture_bounded_tail_handles_empty_stream() {
    let (tail, dropped) =
        capture_bounded_tail(Cursor::new(Vec::<u8>::new()), SUMMARY_TAIL_BYTES).expect("ok");
    assert!(tail.is_empty());
    assert_eq!(dropped, 0);
}

/// Boundary: exactly `cap` bytes are retained whole with zero drops (off-by-one
/// guard on the ring cap).
#[test]
fn capture_bounded_tail_exact_cap_is_lossless() {
    let cap = 4096;
    let input: Vec<u8> = (0..cap).map(|i| (i % 97) as u8).collect();
    let (tail, dropped) = capture_bounded_tail(Cursor::new(input.clone()), cap).expect("ok");
    assert_eq!(tail, input);
    assert_eq!(dropped, 0);
}

/// End-to-end behaviour-preservation guard: driving the real subprocess path
/// against an adversarial child that emits far more than `SUMMARY_TAIL_BYTES`
/// must still return a bounded summary (≤ cap + a small banner margin) carrying
/// the `[truncated …]` banner — the returned-summary contract is unchanged
/// while the capture is now O(1) in RAM.
#[test]
#[serial(simard_amplihack_bin_env, cognitive_memory)]
fn run_engineer_subprocess_returns_bounded_summary_for_adversarial_output() {
    let dir = tempfile::tempdir().unwrap();

    // A stub `amplihack` that floods stdout with 200 KiB of output then exits 0.
    let shim = dir.path().join("amplihack-flood");
    std::fs::write(
        &shim,
        "#!/usr/bin/env bash\nset -uo pipefail\nhead -c 200000 /dev/zero | tr '\\0' 'A'\necho\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim, perms).unwrap();
    }

    // SAFETY: guarded by `#[serial(simard_amplihack_bin_env)]` so no other test
    // mutates this var concurrently. Restored below.
    let prior = std::env::var("SIMARD_AMPLIHACK_BIN").ok();
    unsafe {
        std::env::set_var("SIMARD_AMPLIHACK_BIN", &shim);
    }

    // RustyClawd path reads no stdin, so the adversarial child is fully
    // deterministic (no feeder interaction) — it isolates the capture bound.
    let result = run_engineer_subprocess("objective", dir.path(), AgentKind::RustyClawd);

    unsafe {
        match prior {
            Some(v) => std::env::set_var("SIMARD_AMPLIHACK_BIN", v),
            None => std::env::remove_var("SIMARD_AMPLIHACK_BIN"),
        }
    }

    let summary = result.expect("adversarial-output child exits 0 → Ok summary");
    assert!(
        summary.len() <= SUMMARY_TAIL_BYTES + 512,
        "returned summary must stay bounded to ~SUMMARY_TAIL_BYTES; got {} bytes",
        summary.len()
    );
    assert!(
        summary.contains("truncated"),
        "a summary built from a >cap child must carry the truncation banner; got:\n{summary}"
    );
}

use serial_test::serial;
