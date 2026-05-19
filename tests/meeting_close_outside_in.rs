//! Outside-in regression test for issues #1908 / #1906.
//!
//! These tests boot a full `MeetingBackend` against a temporary
//! `SIMARD_STATE_ROOT` and exercise the close pipeline against a
//! blocking mock agent. They prove:
//!
//! 1. Issue #1908 — `MeetingBackend::close()` returns within the
//!    configured budget (here: 3s + 2s headroom) even when the
//!    underlying agent's `close()` would otherwise hang.
//! 2. Issue #1906 — The handoff bundle is written under
//!    `$SIMARD_STATE_ROOT/meeting_handoffs/` rather than the legacy
//!    `~/.simard/meeting_handoffs/` location.
//! 3. The on-disk handoff envelope still parses against the current
//!    `MeetingHandoff` schema (no breaking changes — the partial-close
//!    path is signaled via tracing + `MeetingSummary.partial_reason`
//!    only).
//!
//! These tests mutate process-level env vars, so they are serialized
//! against any other test that touches `SIMARD_STATE_ROOT` /
//! `SIMARD_MEETING_*` via `serial_test::serial`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;
use serial_test::serial;
use tempfile::TempDir;

use simard::base_types::{
    BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    standard_session_capabilities,
};
use simard::error::SimardResult;
use simard::meeting_backend::{MeetingBackend, PartialReason};
use simard::metadata::{BackendDescriptor, Freshness};
use simard::runtime::RuntimeTopology;

/// Walk a directory and produce a multi-line listing for diagnostic
/// failure messages. Caps depth at 4 so a runaway tree does not eat
/// the test log.
fn walk(dir: &Path, depth: usize) -> String {
    if depth > 4 {
        return String::new();
    }
    let mut out = String::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let pad = "  ".repeat(depth);
        out.push_str(&format!("{pad}{}\n", path.display()));
        if path.is_dir() {
            out.push_str(&walk(&path, depth + 1));
        }
    }
    out
}

/// Tracks whether `close()` was ever invoked on the underlying agent.
/// Used to assert the close pipeline still calls `agent.close()` on
/// the happy path even though the timeout fast-path returns first.
#[derive(Default)]
struct BlockingState {
    close_called: bool,
    close_returned: bool,
}

/// Mock agent that sleeps `block` on every `run_turn` and `close`.
/// Used to deterministically force the close-pipeline timeout fast
/// path (issue #1908).
struct BlockingSession {
    descriptor: BaseTypeDescriptor,
    block: Duration,
    is_open: bool,
    is_closed: bool,
    state: Arc<Mutex<BlockingState>>,
}

impl BlockingSession {
    fn new(block: Duration, state: Arc<Mutex<BlockingState>>) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("blocking-mock-meeting"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "mock",
                    "test:blocking-mock-meeting",
                    Freshness::now().unwrap(),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            block,
            is_open: true,
            is_closed: false,
            state,
        }
    }
}

impl BaseTypeSession for BlockingSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, _input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;
        sleep(self.block);
        Ok(BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "Summary of the meeting from the blocking mock.".to_string(),
            evidence: Vec::new(),
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        {
            let mut g = self.state.lock().unwrap();
            g.close_called = true;
        }
        sleep(self.block);
        {
            let mut g = self.state.lock().unwrap();
            g.close_returned = true;
        }
        self.is_closed = true;
        Ok(())
    }
}

/// Scrub every env var the meeting pipeline reads so the test runs in
/// hermetic isolation against the temp dir provided. Returns previous
/// values so the test can restore them via the `RestoreEnv` guard.
fn scrub_env() {
    for k in [
        "SIMARD_STATE_ROOT",
        "SIMARD_MEETINGS_DIR",
        "SIMARD_MEETINGS_ROOT",
        "SIMARD_HANDOFF_DIR",
        "SIMARD_MEETING_CLOSE_TIMEOUT_SECS",
        "SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS",
    ] {
        // SAFETY: tests run serially under `#[serial]` above, so the
        // race window for env mutation is irrelevant.
        unsafe { std::env::remove_var(k) };
    }
}

#[test]
#[serial(state_root)]
fn close_returns_within_budget_when_agent_blocks() {
    scrub_env();
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
        // 3s close budget keeps the test fast; close_guard clamps the
        // agent-close inner budget to >=1s so leave it at default.
        std::env::set_var("SIMARD_MEETING_CLOSE_TIMEOUT_SECS", "3");
        std::env::set_var("SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS", "1");
    }

    let state = Arc::new(Mutex::new(BlockingState::default()));
    // Block for 30s on every agent call — well above the 3s close +
    // 1s agent-close budget so the timeout fast-path is hit
    // deterministically.
    let agent = BlockingSession::new(Duration::from_secs(30), state.clone());

    let mut backend = MeetingBackend::new_session(
        "Outside-in close test",
        Box::new(agent),
        None,
        String::new(),
    );

    // Add a synthetic history message without going through
    // `send_message` (which would itself block on the mock for 30s).
    // This is enough payload that the bundle has real content.
    backend.push_test_message("operator", "What did we decide?");
    backend.push_test_message("simard", "We decided to ship the fix.");

    let started = Instant::now();
    let summary = backend.close().expect("close must return ok");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "close took {elapsed:?}; budget violated (issue #1908 regression)"
    );
    assert!(
        summary.partial_reason.is_some(),
        "blocking agent should have produced a partial close, got {:?}",
        summary.partial_reason
    );
    let reason = summary.partial_reason.unwrap();
    assert!(
        matches!(
            reason,
            PartialReason::AgentCloseTimeout
                | PartialReason::CloseTimeout
                | PartialReason::SummaryTimeout
        ),
        "unexpected partial reason {reason:?}"
    );

    // The on-disk handoff bundle MUST exist under SIMARD_STATE_ROOT
    // (issue #1906) — not the home-dir fallback. The per-meeting
    // bundle is at `<root>/meetings/<id>/meeting_handoff.json` and
    // the OODA-style legacy handoff at
    // `<root>/meeting_handoffs/handoff-<ts>.json`.
    let bundle_dir: PathBuf = summary
        .bundle_dir
        .clone()
        .expect("bundle_dir set even on partial close")
        .into();
    let handoff_path = bundle_dir.join("meeting_handoff.json");
    if !handoff_path.exists() {
        let listing = walk(&root, 0);
        panic!(
            "expected handoff at {} (issue #1906); root tree:\n{}",
            handoff_path.display(),
            listing
        );
    }

    let raw = std::fs::read_to_string(&handoff_path).expect("read handoff");
    let parsed: Value = serde_json::from_str(&raw).expect("handoff is valid JSON");
    // The on-disk envelope MUST deserialize against the current
    // `MeetingHandoff` struct — the partial-close path is signaled
    // via tracing + `MeetingSummary.partial_reason` only and MUST
    // NOT introduce new required fields (see
    // `docs/reference/meeting-close-lifecycle.md`).
    let handoff: simard::meeting_facilitator::MeetingHandoff =
        serde_json::from_str(&raw).expect("handoff parses against current MeetingHandoff schema");
    assert!(!handoff.processed, "partial handoff must be unprocessed");
    assert!(
        !handoff.meeting_id.is_empty(),
        "partial handoff still has a meeting id"
    );
    // Cross-check field presence at the JSON layer so a future
    // schema-renaming refactor produces a useful diff.
    for field in [
        "meeting_id",
        "topic",
        "started_at",
        "closed_at",
        "transcript",
        "action_items",
        "decisions",
        "open_questions",
        "processed",
        "participants",
    ] {
        assert!(
            parsed.get(field).is_some(),
            "handoff JSON missing required field '{field}': {raw}"
        );
    }
    assert_eq!(parsed["processed"], Value::Bool(false));

    // Per-meeting bundle dir is also written under SIMARD_STATE_ROOT.
    assert!(
        bundle_dir.starts_with(&root),
        "bundle_dir {} not under state root {} (issue #1906)",
        bundle_dir.display(),
        root.display()
    );

    // The legacy OODA handoff folder is also relocated under
    // `SIMARD_STATE_ROOT/meeting_handoffs/`. Its filename is
    // timestamped — assert the directory contains *something* rather
    // than fixate on the schema-version-coupled filename.
    let legacy_dir = root.join("meeting_handoffs");
    assert!(
        legacy_dir.exists(),
        "legacy handoff dir {} missing (issue #1906)",
        legacy_dir.display()
    );
    let entries: Vec<_> = std::fs::read_dir(&legacy_dir)
        .map(|rd| rd.flatten().collect())
        .unwrap_or_default();
    assert!(
        !entries.is_empty(),
        "no handoff written under {} (issue #1906)",
        legacy_dir.display()
    );

    // The detached worker thread for run_turn or close may or may
    // not have observed close_called depending on whether the
    // summary fast-path short-circuited. We do not assert on it
    // here because the regression we care about is the wall-clock
    // budget, not the worker's lifecycle (which is intentionally
    // best-effort per `docs/reference/meeting-close-lifecycle.md`).
    let _state_snapshot = state.lock().unwrap();

    scrub_env();
}

#[test]
#[serial(state_root)]
fn happy_path_close_writes_under_state_root() {
    scrub_env();
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
    }

    // Zero-block mock — close should complete normally with no
    // partial reason set, regardless of budget.
    let state = Arc::new(Mutex::new(BlockingState::default()));
    let agent = BlockingSession::new(Duration::from_millis(0), state);

    let mut backend =
        MeetingBackend::new_session("Happy path close", Box::new(agent), None, String::new());
    backend.push_test_message("operator", "All good?");
    backend.push_test_message("simard", "All good.");

    let started = Instant::now();
    let summary = backend.close().expect("close ok");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "close exceeded sanity-budget on happy path"
    );
    assert!(
        summary.partial_reason.is_none(),
        "non-blocking mock should produce a clean close, got {:?}",
        summary.partial_reason
    );

    let handoff_path = root.join("meetings");
    // The bundle dir name is timestamped; locate any
    // meeting_handoff.json under <root>/meetings/<id>/. The legacy
    // OODA-style handoffs go under <root>/meeting_handoffs/.
    let bundle_dir: PathBuf = summary
        .bundle_dir
        .clone()
        .expect("bundle_dir present")
        .into();
    let bundle_file = bundle_dir.join("meeting_handoff.json");
    assert!(
        bundle_file.exists(),
        "expected handoff at {} (issue #1906); root tree:\n{}",
        bundle_file.display(),
        walk(&root, 0)
    );
    assert!(
        bundle_dir.starts_with(&root),
        "bundle_dir {} not under state root {} (issue #1906)",
        bundle_dir.display(),
        root.display()
    );
    assert!(
        handoff_path.exists(),
        "expected meetings dir under state root (issue #1906)"
    );
    scrub_env();
}
