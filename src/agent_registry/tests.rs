use super::*;
use chrono::Utc;
use tempfile::TempDir;

fn test_registry() -> (TempDir, FileBackedAgentRegistry) {
    let dir = TempDir::new().unwrap();
    let reg = FileBackedAgentRegistry::new(dir.path());
    (dir, reg)
}

fn sample_entry(id: &str) -> AgentEntry {
    AgentEntry {
        id: id.to_string(),
        pid: std::process::id(),
        host: "test-host".into(),
        start_time: Utc::now(),
        last_heartbeat: Utc::now(),
        state: AgentState::Running,
        role: "operator".into(),
        resources: ResourceUsage {
            rss_bytes: Some(1024),
            cpu_percent: None,
        },
    }
}

#[test]
fn register_and_list() {
    let (_dir, reg) = test_registry();
    reg.register(sample_entry("a1")).unwrap();
    reg.register(sample_entry("a2")).unwrap();

    let entries = reg.list().unwrap();
    assert_eq!(entries.len(), 2);
}

#[test]
fn deregister_removes_entry() {
    let (_dir, reg) = test_registry();
    reg.register(sample_entry("a1")).unwrap();
    reg.deregister("a1").unwrap();

    let entries = reg.list().unwrap();
    assert!(entries.is_empty());
}

#[test]
fn heartbeat_updates_state() {
    let (_dir, reg) = test_registry();
    reg.register(sample_entry("a1")).unwrap();
    reg.heartbeat(
        "a1",
        AgentState::Idle,
        ResourceUsage {
            rss_bytes: Some(2048),
            cpu_percent: Some(1.5),
        },
    )
    .unwrap();

    let entry = reg.get("a1").unwrap().unwrap();
    assert_eq!(entry.state, AgentState::Idle);
    assert_eq!(entry.resources.rss_bytes, Some(2048));
}

#[test]
fn heartbeat_missing_entry_returns_error() {
    let (_dir, reg) = test_registry();
    let result = reg.heartbeat(
        "nonexistent",
        AgentState::Running,
        ResourceUsage {
            rss_bytes: None,
            cpu_percent: None,
        },
    );
    assert!(result.is_err());
}

#[test]
fn reap_dead_removes_defunct_pids() {
    let (_dir, reg) = test_registry();
    let mut entry = sample_entry("dead-proc");
    entry.pid = 999_999_999; // very unlikely to be alive
    entry.host = super::registry::hostname();
    reg.register(entry).unwrap();

    let reaped = reg.reap_dead().unwrap();
    assert_eq!(reaped, 1);
    assert!(reg.list().unwrap().is_empty());
}

#[test]
fn empty_registry_loads_clean() {
    let (_dir, reg) = test_registry();
    let entries = reg.list().unwrap();
    assert!(entries.is_empty());
}

#[test]
fn get_returns_none_for_missing() {
    let (_dir, reg) = test_registry();
    assert!(reg.get("nope").unwrap().is_none());
}

#[test]
fn duplicate_register_replaces() {
    let (_dir, reg) = test_registry();
    let e1 = sample_entry("dup");
    reg.register(e1).unwrap();
    let mut e2 = sample_entry("dup");
    e2.role = "engineer".into();
    reg.register(e2).unwrap();

    let entries = reg.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].role, "engineer");
}

#[test]
fn default_path_is_sensible() {
    let p = FileBackedAgentRegistry::default_path();
    assert!(p.to_string_lossy().contains(".simard"));
}

#[test]
fn self_entry_creates_valid_entry() {
    let entry = super::registry::self_entry("operator");
    assert_eq!(entry.pid, std::process::id());
    assert_eq!(entry.role, "operator");
    assert_eq!(entry.state, AgentState::Running);
}

#[test]
fn self_resource_usage_returns_some_rss() {
    let usage = super::registry::self_resource_usage();
    // On Linux /proc/self/statm should be readable
    if cfg!(target_os = "linux") {
        assert!(usage.rss_bytes.is_some());
    }
}

#[cfg(test)]
mod registry_inline {
    use super::registry::*;
    use super::*;
    use std::path::{Path, PathBuf};

    fn temp_registry_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "simard-registry-test-{}-{}",
            label,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn make_entry(id: &str, pid: u32) -> AgentEntry {
        AgentEntry {
            id: id.to_string(),
            pid,
            host: hostname(),
            start_time: Utc::now(),
            last_heartbeat: Utc::now(),
            state: AgentState::Running,
            role: "test".to_string(),
            resources: ResourceUsage {
                rss_bytes: None,
                cpu_percent: None,
            },
        }
    }

    // --- AgentState ---

    #[test]
    fn agent_state_serde_round_trip() {
        for state in [
            AgentState::Starting,
            AgentState::Running,
            AgentState::Idle,
            AgentState::ShuttingDown,
            AgentState::Dead,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let back: AgentState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn agent_state_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&AgentState::ShuttingDown).unwrap(),
            "\"shutting_down\""
        );
    }

    // --- hostname / self_resource_usage / self_entry ---

    #[test]
    fn hostname_returns_nonempty_string() {
        let h = hostname();
        assert!(!h.is_empty(), "hostname should not be empty");
    }

    #[test]
    fn self_resource_usage_returns_valid_struct() {
        let usage = self_resource_usage();
        // rss_bytes should be Some on Linux with /proc
        if Path::new("/proc/self/statm").exists() {
            assert!(usage.rss_bytes.is_some(), "expected rss_bytes on Linux");
        }
    }

    #[test]
    fn self_entry_has_correct_role_and_pid() {
        let entry = self_entry("tester");
        assert!(entry.id.starts_with("tester-"));
        assert_eq!(entry.pid, std::process::id());
        assert_eq!(entry.role, "tester");
        assert_eq!(entry.state, AgentState::Running);
    }

    // --- FileBackedAgentRegistry ---

    #[test]
    fn empty_registry_lists_nothing() {
        let dir = temp_registry_dir("empty");
        let reg = FileBackedAgentRegistry::new(&dir);
        let entries = reg.list().unwrap();
        assert!(entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn register_and_get() {
        let dir = temp_registry_dir("register-get");
        let reg = FileBackedAgentRegistry::new(&dir);

        let entry = make_entry("agent-1", std::process::id());
        reg.register(entry.clone()).unwrap();

        let found = reg.get("agent-1").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "agent-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn register_replaces_existing_entry() {
        let dir = temp_registry_dir("upsert");
        let reg = FileBackedAgentRegistry::new(&dir);

        let entry1 = make_entry("agent-1", std::process::id());
        reg.register(entry1).unwrap();

        let mut entry2 = make_entry("agent-1", std::process::id());
        entry2.state = AgentState::Idle;
        reg.register(entry2).unwrap();

        let entries = reg.list().unwrap();
        assert_eq!(entries.len(), 1, "should replace, not duplicate");
        assert_eq!(entries[0].state, AgentState::Idle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deregister_removes_entry() {
        let dir = temp_registry_dir("deregister");
        let reg = FileBackedAgentRegistry::new(&dir);

        reg.register(make_entry("agent-1", std::process::id()))
            .unwrap();
        reg.register(make_entry("agent-2", std::process::id()))
            .unwrap();

        reg.deregister("agent-1").unwrap();
        let entries = reg.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "agent-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn heartbeat_updates_state_and_resources() {
        let dir = temp_registry_dir("heartbeat");
        let reg = FileBackedAgentRegistry::new(&dir);

        reg.register(make_entry("agent-1", std::process::id()))
            .unwrap();

        let new_resources = ResourceUsage {
            rss_bytes: Some(1024),
            cpu_percent: Some(50.0),
        };
        reg.heartbeat("agent-1", AgentState::Idle, new_resources)
            .unwrap();

        let entry = reg.get("agent-1").unwrap().unwrap();
        assert_eq!(entry.state, AgentState::Idle);
        assert_eq!(entry.resources.rss_bytes, Some(1024));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn heartbeat_fails_for_unknown_agent() {
        let dir = temp_registry_dir("heartbeat-missing");
        let reg = FileBackedAgentRegistry::new(&dir);

        let err = reg
            .heartbeat(
                "nonexistent",
                AgentState::Running,
                ResourceUsage {
                    rss_bytes: None,
                    cpu_percent: None,
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_returns_none_for_unknown_agent() {
        let dir = temp_registry_dir("get-none");
        let reg = FileBackedAgentRegistry::new(&dir);
        assert!(reg.get("nonexistent").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_path_ends_with_simard() {
        let path = FileBackedAgentRegistry::default_path();
        assert!(
            path.ends_with(".simard"),
            "expected .simard suffix, got: {path:?}"
        );
    }

    #[test]
    fn reap_dead_removes_dead_pids() {
        let dir = temp_registry_dir("reap-dead");
        let reg = FileBackedAgentRegistry::new(&dir);

        // Register an entry with a PID that almost certainly doesn't exist
        let dead_entry = make_entry("dead-agent", 4_000_000);
        reg.register(dead_entry).unwrap();

        // Register an entry with our own PID (alive)
        let alive_entry = make_entry("alive-agent", std::process::id());
        reg.register(alive_entry).unwrap();

        let reaped = reg.reap_dead().unwrap();
        assert_eq!(reaped, 1, "should reap the dead PID entry");

        let remaining = reg.list().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "alive-agent");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
