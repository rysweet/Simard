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
