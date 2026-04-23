//! TDD tests for the subagent_sessions registry module.
//!
//! These are written FIRST per the TDD discipline and will fail until
//! Step 8 implements the module. They define the contract for:
//!  - atomic write semantics (no `.tmp.*` siblings left behind)
//!  - round-trip serialization
//!  - GC of entries ended > 24h ago
//!  - probe-driven `ended_at` marking
//!  - id sanitization rules

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use super::*;

/// Serialize tests that mutate the SIMARD_STATE_ROOT env var.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fresh_tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-subagent-test-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn with_state_root<F: FnOnce(&PathBuf) -> R, R>(tag: &str, f: F) -> R {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let root = fresh_tempdir(tag);
    // SAFETY: serialized via ENV_LOCK; tests are single-threaded under this guard.
    unsafe { std::env::set_var("SIMARD_STATE_ROOT", &root) };
    let result = f(&root);
    unsafe { std::env::remove_var("SIMARD_STATE_ROOT") };
    result
}

fn sample(agent_id: &str, ended_at: Option<i64>) -> SubagentSession {
    SubagentSession {
        agent_id: agent_id.to_string(),
        session_name: format!("simard-engineer-{agent_id}"),
        host: "local".to_string(),
        pid: 12345,
        created_at: 1_700_000_000,
        ended_at,
        goal_id: "goal-abc".to_string(),
    }
}

#[test]
fn registry_path_honors_state_root_env() {
    with_state_root("path-env", |root| {
        let p = registry_path();
        assert!(
            p.starts_with(root),
            "registry_path() {p:?} should start with SIMARD_STATE_ROOT {root:?}"
        );
        assert!(
            p.ends_with("subagent_sessions.json"),
            "registry_path() basename must be subagent_sessions.json: {p:?}"
        );
    });
}

#[test]
fn load_returns_empty_when_missing() {
    with_state_root("empty", |_root| {
        let reg = load();
        assert!(reg.sessions.is_empty(), "missing file → empty registry");
    });
}

#[test]
fn save_atomic_round_trips_and_creates_parent_dir() {
    with_state_root("rt", |_root| {
        let reg = Registry {
            sessions: vec![
                sample("engineer-aaa", None),
                sample("engineer-bbb", Some(1_700_000_500)),
            ],
        };
        save_atomic(&reg).expect("save_atomic must succeed");
        let loaded = load();
        assert_eq!(loaded, reg, "round-trip must preserve registry exactly");
    });
}

#[test]
fn save_atomic_leaves_no_tmp_siblings() {
    with_state_root("atomic", |_root| {
        let reg = Registry {
            sessions: vec![sample("engineer-x", None)],
        };
        save_atomic(&reg).expect("save_atomic must succeed");

        let path = registry_path();
        let parent = path.parent().expect("registry path must have parent");
        let stragglers: Vec<_> = fs::read_dir(parent)
            .expect("parent dir must be readable")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("subagent_sessions.json.tmp"))
            .collect();
        assert!(
            stragglers.is_empty(),
            "atomic write must not leave .tmp.* siblings, found: {stragglers:?}"
        );
    });
}

#[test]
fn record_spawn_appends_to_registry() {
    with_state_root("rec", |_root| {
        record_spawn(sample("engineer-1", None)).unwrap();
        record_spawn(sample("engineer-2", None)).unwrap();
        let reg = load();
        let ids: HashSet<_> = reg.sessions.iter().map(|s| s.agent_id.clone()).collect();
        assert!(ids.contains("engineer-1"));
        assert!(ids.contains("engineer-2"));
    });
}

struct StubProbe {
    alive_set: HashSet<String>,
    queries: RefCell<Vec<String>>,
}

impl SessionProbe for StubProbe {
    fn alive(&self, session_name: &str) -> bool {
        self.queries.borrow_mut().push(session_name.to_string());
        self.alive_set.contains(session_name)
    }
}

#[test]
fn poll_and_gc_marks_dead_sessions_with_ended_at() {
    with_state_root("poll", |_root| {
        record_spawn(sample("engineer-live", None)).unwrap();
        record_spawn(sample("engineer-dead", None)).unwrap();
        let alive_set: HashSet<_> = ["simard-engineer-engineer-live".to_string()]
            .into_iter()
            .collect();
        let probe = StubProbe {
            alive_set,
            queries: RefCell::new(Vec::new()),
        };
        poll_and_gc(&probe).expect("poll_and_gc must succeed");

        let reg = load();
        let live = reg
            .sessions
            .iter()
            .find(|s| s.agent_id == "engineer-live")
            .expect("live session must remain");
        assert!(
            live.ended_at.is_none(),
            "live session must NOT have ended_at"
        );

        let dead = reg
            .sessions
            .iter()
            .find(|s| s.agent_id == "engineer-dead")
            .expect("dead session must remain (within 24h)");
        assert!(
            dead.ended_at.is_some(),
            "dead session must have ended_at populated"
        );
    });
}

#[test]
fn poll_and_gc_drops_entries_ended_more_than_24h_ago() {
    with_state_root("gc", |_root| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let stale = SubagentSession {
            ended_at: Some(now - 86_400 - 60), // > 24h ago
            ..sample("engineer-stale", Some(now - 86_400 - 60))
        };
        let recent = SubagentSession {
            ended_at: Some(now - 60),
            ..sample("engineer-recent", Some(now - 60))
        };
        save_atomic(&Registry {
            sessions: vec![stale, recent],
        })
        .unwrap();

        let probe = StubProbe {
            alive_set: HashSet::new(),
            queries: RefCell::new(Vec::new()),
        };
        poll_and_gc(&probe).expect("poll_and_gc must succeed");

        let reg = load();
        let ids: HashSet<_> = reg.sessions.iter().map(|s| s.agent_id.clone()).collect();
        assert!(
            !ids.contains("engineer-stale"),
            "entry ended >24h ago must be GC'd"
        );
        assert!(
            ids.contains("engineer-recent"),
            "entry ended <24h ago must be retained"
        );
    });
}

#[test]
fn sanitize_id_replaces_unsafe_chars_with_dash() {
    assert_eq!(sanitize_id("engineer-abc_123"), "engineer-abc_123");
    assert_eq!(sanitize_id("engineer/with spaces"), "engineer-with-spaces");
    assert_eq!(sanitize_id("foo$bar*baz"), "foo-bar-baz");
}

#[test]
fn sanitize_id_empty_input_becomes_engineer() {
    assert_eq!(sanitize_id(""), "engineer");
}

#[test]
fn sanitize_id_output_matches_safe_charset() {
    let out = sanitize_id("weird!@#$%^&*()chars");
    assert!(
        out.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "sanitize_id output must only contain [A-Za-z0-9_-]: got {out:?}"
    );
    assert!(!out.is_empty());
}
