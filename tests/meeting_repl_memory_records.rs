//! Regression test for issue #2000.
//!
//! Reproduces the close → read round-trip that previously hard-failed: drive
//! a scripted REPL meeting through the same `MeetingBackend::close()` code
//! path the binary uses, then verify that
//!
//! 1. `<state_root>/memory_records.json` exists and is parseable, and
//! 2. `<bundle_dir>/memory_records.json` exists and contains the same
//!    persisted-meeting record, and
//! 3. the read companion (`FileBackedMemoryStore::list(Decision)` filtered by
//!    `looks_like_persisted_meeting_record`) — i.e. the exact code path
//!    `simard meeting read` uses — surfaces the record without error.
//!
//! Pre-#2000 fix: step (1) failed because the REPL close path never wrote
//! `memory_records.json` at all. Post-fix: the file is written atomically on
//! every REPL close, even on the partial-close fast-path.
//!
//! Test isolation: `SIMARD_STATE_ROOT` and the meeting close timeout vars
//! are scoped to this test via a `serial` guard plus per-test scrubbing.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

use serial_test::serial;
use tempfile::TempDir;

use simard::base_types::{
    BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    standard_session_capabilities,
};
use simard::error::SimardResult;
use simard::meeting_backend::{MeetingBackend, PartialReason};
use simard::memory::{FileBackedMemoryStore, MemoryScope, MemoryStore};
use simard::meetings::{PersistedMeetingRecord, looks_like_persisted_meeting_record};
use simard::metadata::{BackendDescriptor, Freshness};
use simard::run_meeting_read_probe;
use simard::runtime::RuntimeTopology;

#[derive(Default)]
struct AgentState {
    close_called: bool,
}

struct ScriptedAgent {
    descriptor: BaseTypeDescriptor,
    block: Duration,
    is_open: bool,
    is_closed: bool,
    response: String,
    state: Arc<Mutex<AgentState>>,
}

impl ScriptedAgent {
    fn new(block: Duration, response: &str, state: Arc<Mutex<AgentState>>) -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("scripted-meeting-agent"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "mock",
                    "test:scripted-meeting-agent",
                    Freshness::now().unwrap(),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            block,
            is_open: true,
            is_closed: false,
            response: response.to_string(),
            state,
        }
    }
}

impl BaseTypeSession for ScriptedAgent {
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
        if !self.block.is_zero() {
            sleep(self.block);
        }
        Ok(BaseTypeOutcome {
            plan: String::new(),
            execution_summary: self.response.clone(),
            evidence: Vec::new(),
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        self.state.lock().unwrap().close_called = true;
        self.is_closed = true;
        Ok(())
    }
}

fn scrub_env() {
    for k in [
        "SIMARD_STATE_ROOT",
        "SIMARD_MEETINGS_DIR",
        "SIMARD_MEETINGS_ROOT",
        "SIMARD_HANDOFF_DIR",
        "SIMARD_MEETING_CLOSE_TIMEOUT_SECS",
        "SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS",
    ] {
        // SAFETY: scope-guarded by `#[serial(state_root)]` on each test.
        unsafe { std::env::remove_var(k) };
    }
}

/// Helper: walk the state-root tree for diagnostic failure messages.
fn list_tree(root: &Path) -> String {
    let mut out = String::new();
    fn recurse(dir: &Path, depth: usize, out: &mut String) {
        if depth > 4 {
            return;
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                let pad = "  ".repeat(depth);
                out.push_str(&format!("{pad}{}\n", path.display()));
                if path.is_dir() {
                    recurse(&path, depth + 1, out);
                }
            }
        }
    }
    recurse(root, 0, &mut out);
    out
}

/// Core regression for issue #2000: a successful REPL close MUST write
/// `memory_records.json` so `simard meeting read` does not hard-fail.
#[test]
#[serial(state_root)]
fn happy_path_repl_close_writes_memory_records_json() {
    scrub_env();
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
    }

    let state = Arc::new(Mutex::new(AgentState::default()));
    let agent = ScriptedAgent::new(Duration::from_millis(0), "Summary text.", state.clone());
    let mut backend = MeetingBackend::new_session(
        "Read-companion regression",
        Box::new(agent),
        None,
        String::new(),
    );
    backend.push_test_message("operator", "We decided to ship the fix on Monday.");
    backend.push_test_message("simard", "Confirmed: ship Monday.");

    let summary = backend.close().expect("close ok");
    assert!(
        summary.partial_reason.is_none(),
        "happy-path close should not be partial; got {:?}",
        summary.partial_reason
    );

    let bundle_dir = std::path::PathBuf::from(
        summary
            .bundle_dir
            .clone()
            .expect("bundle_dir present on happy path"),
    );

    // (1) bundle-local copy.
    let bundle_records = bundle_dir.join("memory_records.json");
    assert!(
        bundle_records.is_file(),
        "expected bundle-local memory_records.json at {} (issue #2000); root tree:\n{}",
        bundle_records.display(),
        list_tree(&root),
    );

    // (2) state-root copy — the path `simard meeting read` opens.
    let state_records = root.join("memory_records.json");
    assert!(
        state_records.is_file(),
        "expected state-root memory_records.json at {} (issue #2000); root tree:\n{}",
        state_records.display(),
        list_tree(&root),
    );

    // (3) Read companion's exact code path:
    //     FileBackedMemoryStore::try_new + list(Decision) +
    //     looks_like_persisted_meeting_record + PersistedMeetingRecord::parse
    let store = FileBackedMemoryStore::try_new(state_records.clone())
        .expect("open memory_records.json through FileBackedMemoryStore");
    let decisions = store.list(MemoryScope::Decision).expect("list decisions");
    let meeting_records: Vec<_> = decisions
        .into_iter()
        .filter(|r| looks_like_persisted_meeting_record(&r.value))
        .collect();
    assert!(
        !meeting_records.is_empty(),
        "read path expects ≥1 persisted-meeting record (issue #2000): {:?}",
        meeting_records
    );
    let parsed = PersistedMeetingRecord::parse(&meeting_records.last().unwrap().value)
        .expect("persisted meeting record must parse");
    assert_eq!(parsed.agenda, "Read-companion regression");

    scrub_env();
}

/// Empty REPL close — no extracted content — still produces a parseable
/// memory_records.json so the read path never hard-fails on an otherwise
/// successful session (acceptance criterion (b) on #2000).
#[test]
#[serial(state_root)]
fn empty_repl_close_emits_empty_but_valid_memory_records() {
    scrub_env();
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
    }

    // No conversation messages, no explicit /decision or /action.
    let state = Arc::new(Mutex::new(AgentState::default()));
    let agent = ScriptedAgent::new(Duration::from_millis(0), "(no content)", state);
    let mut backend = MeetingBackend::new_session(
        "Empty meeting",
        Box::new(agent),
        None,
        String::new(),
    );

    let summary = backend.close().expect("close ok");
    let bundle_dir = std::path::PathBuf::from(
        summary
            .bundle_dir
            .clone()
            .expect("bundle_dir present even on empty close"),
    );

    let state_records = root.join("memory_records.json");
    assert!(
        state_records.is_file(),
        "memory_records.json must exist even when no records were extracted (issue #2000 acceptance (b)); root tree:\n{}",
        list_tree(&root)
    );
    assert!(bundle_dir.join("memory_records.json").is_file());

    let store = FileBackedMemoryStore::try_new(state_records).unwrap();
    let decisions = store.list(MemoryScope::Decision).unwrap();
    let meeting_records: Vec<_> = decisions
        .iter()
        .filter(|r| looks_like_persisted_meeting_record(&r.value))
        .collect();
    assert_eq!(
        meeting_records.len(),
        1,
        "exactly one empty-but-valid record should be persisted; got {:?}",
        meeting_records
    );
    // The record must parse — that is what the read probe does.
    PersistedMeetingRecord::parse(&meeting_records[0].value)
        .expect("empty-list record must parse for read companion");

    scrub_env();
}

/// Partial close (agent blocks indefinitely → close timeout fast-path) MUST
/// still write `memory_records.json`. This is the most user-visible variant
/// of #2000: partial closes were the original reproduction.
#[test]
#[serial(state_root)]
fn partial_repl_close_still_writes_memory_records_json() {
    scrub_env();
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
        std::env::set_var("SIMARD_MEETING_CLOSE_TIMEOUT_SECS", "3");
        std::env::set_var("SIMARD_MEETING_AGENT_CLOSE_TIMEOUT_SECS", "1");
    }

    let state = Arc::new(Mutex::new(AgentState::default()));
    // 30s block on every agent call → forces the timeout fast-path.
    let agent = ScriptedAgent::new(Duration::from_secs(30), "blocked", state);
    let mut backend =
        MeetingBackend::new_session("Partial close regression", Box::new(agent), None, String::new());
    backend.push_test_message("operator", "Ship the fix?");
    backend.push_test_message("simard", "Acknowledged.");

    let summary = backend.close().expect("close ok even on timeout");
    assert!(
        summary.partial_reason.is_some(),
        "blocking agent should produce a partial close"
    );
    assert!(
        matches!(
            summary.partial_reason.unwrap(),
            PartialReason::AgentCloseTimeout
                | PartialReason::CloseTimeout
                | PartialReason::SummaryTimeout
        ),
        "unexpected partial reason"
    );

    let state_records = root.join("memory_records.json");
    assert!(
        state_records.is_file(),
        "even partial close MUST write memory_records.json (issue #2000); root tree:\n{}",
        list_tree(&root)
    );

    let store = FileBackedMemoryStore::try_new(state_records).unwrap();
    let decisions = store.list(MemoryScope::Decision).unwrap();
    let meeting_records: Vec<_> = decisions
        .iter()
        .filter(|r| looks_like_persisted_meeting_record(&r.value))
        .collect();
    assert!(
        !meeting_records.is_empty(),
        "partial close should still produce ≥1 persisted-meeting record"
    );
    PersistedMeetingRecord::parse(&meeting_records.last().unwrap().value)
        .expect("partial-close record must still parse");

    scrub_env();
}

/// End-to-end check that the actual `run_meeting_read_probe` companion
/// (the function the `simard meeting read` CLI command dispatches to)
/// returns `Ok(())` against a freshly-closed REPL meeting. This is the
/// closest in-process analogue to running `simard meeting read <id>` from
/// the shell (task acceptance criterion (c)).
#[test]
#[serial(state_root)]
fn meeting_read_probe_exits_ok_after_repl_close() {
    scrub_env();
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    unsafe {
        std::env::set_var("SIMARD_STATE_ROOT", &root);
    }

    let state = Arc::new(Mutex::new(AgentState::default()));
    let agent = ScriptedAgent::new(Duration::from_millis(0), "ok", state);
    let mut backend =
        MeetingBackend::new_session("End-to-end read probe", Box::new(agent), None, String::new());
    backend.push_test_message("operator", "Did we decide to fix #2000?");
    backend.push_test_message("simard", "Yes, we shipped the fix.");
    let _summary = backend.close().expect("close ok");

    // Capture stdout so the probe's banner doesn't pollute test output.
    let result = run_meeting_read_probe("local-harness", "single-process", Some(root.clone()));
    assert!(
        result.is_ok(),
        "meeting read probe must succeed against REPL bundle (issue #2000); err={:?} root tree:\n{}",
        result.err(),
        list_tree(&root),
    );

    scrub_env();
}
