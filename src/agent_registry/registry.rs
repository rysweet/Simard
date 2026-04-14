use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::SimardError;

/// State of a registered Simard agent process.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Starting,
    Running,
    Idle,
    ShuttingDown,
    Dead,
}

/// Snapshot of resource usage at last heartbeat.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceUsage {
    pub rss_bytes: Option<u64>,
    pub cpu_percent: Option<f32>,
}

/// A single registered agent process.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentEntry {
    pub id: String,
    pub pid: u32,
    pub host: String,
    pub start_time: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    pub state: AgentState,
    pub role: String,
    pub resources: ResourceUsage,
}

/// Trait for registry backends.
pub trait AgentRegistry {
    fn register(&self, entry: AgentEntry) -> Result<(), SimardError>;
    fn heartbeat(
        &self,
        id: &str,
        state: AgentState,
        resources: ResourceUsage,
    ) -> Result<(), SimardError>;
    fn deregister(&self, id: &str) -> Result<(), SimardError>;
    fn list(&self) -> Result<Vec<AgentEntry>, SimardError>;
    fn get(&self, id: &str) -> Result<Option<AgentEntry>, SimardError>;
    /// Reap entries whose process is no longer alive (local only).
    fn reap_dead(&self) -> Result<usize, SimardError>;
}

/// File-backed registry stored as JSON in `~/.simard/agent_registry.json`.
pub struct FileBackedAgentRegistry {
    path: PathBuf,
}

impl FileBackedAgentRegistry {
    pub fn new(state_root: &Path) -> Self {
        Self {
            path: state_root.join("agent_registry.json"),
        }
    }

    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
        PathBuf::from(home).join(".simard")
    }

    fn load(&self) -> Result<Vec<AgentEntry>, SimardError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content =
            std::fs::read_to_string(&self.path).map_err(|e| SimardError::PersistentStoreIo {
                store: "agent_registry".into(),
                action: "read".into(),
                path: self.path.clone(),
                reason: e.to_string(),
            })?;
        if content.trim().is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(&content).map_err(|e| SimardError::PersistentStoreIo {
            store: "agent_registry".into(),
            action: "parse".into(),
            path: self.path.clone(),
            reason: e.to_string(),
        })
    }

    fn save(&self, entries: &[AgentEntry]) -> Result<(), SimardError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SimardError::PersistentStoreIo {
                store: "agent_registry".into(),
                action: "mkdir".into(),
                path: parent.to_path_buf(),
                reason: e.to_string(),
            })?;
        }
        let json =
            serde_json::to_string_pretty(&entries).map_err(|e| SimardError::PersistentStoreIo {
                store: "agent_registry".into(),
                action: "serialize".into(),
                path: self.path.clone(),
                reason: e.to_string(),
            })?;
        std::fs::write(&self.path, json).map_err(|e| SimardError::PersistentStoreIo {
            store: "agent_registry".into(),
            action: "write".into(),
            path: self.path.clone(),
            reason: e.to_string(),
        })
    }
}

impl AgentRegistry for FileBackedAgentRegistry {
    fn register(&self, entry: AgentEntry) -> Result<(), SimardError> {
        let mut entries = self.load()?;
        entries.retain(|e| e.id != entry.id);
        entries.push(entry);
        self.save(&entries)
    }

    fn heartbeat(
        &self,
        id: &str,
        state: AgentState,
        resources: ResourceUsage,
    ) -> Result<(), SimardError> {
        let mut entries = self.load()?;
        if let Some(entry) = entries.iter_mut().find(|e| e.id == id) {
            entry.last_heartbeat = Utc::now();
            entry.state = state;
            entry.resources = resources;
            self.save(&entries)
        } else {
            Err(SimardError::PersistentStoreIo {
                store: "agent_registry".into(),
                action: "heartbeat".into(),
                path: self.path.clone(),
                reason: format!("agent {id} not found in registry"),
            })
        }
    }

    fn deregister(&self, id: &str) -> Result<(), SimardError> {
        let mut entries = self.load()?;
        entries.retain(|e| e.id != id);
        self.save(&entries)
    }

    fn list(&self) -> Result<Vec<AgentEntry>, SimardError> {
        self.load()
    }

    fn get(&self, id: &str) -> Result<Option<AgentEntry>, SimardError> {
        let entries = self.load()?;
        Ok(entries.into_iter().find(|e| e.id == id))
    }

    fn reap_dead(&self) -> Result<usize, SimardError> {
        let mut entries = self.load()?;
        let before = entries.len();

        let local_hostname = hostname();
        entries.retain(|entry| {
            if entry.host != local_hostname {
                return true; // can't verify remote processes
            }
            is_pid_alive(entry.pid)
        });

        let reaped = before - entries.len();
        if reaped > 0 {
            self.save(&entries)?;
        }
        Ok(reaped)
    }
}

/// Read the local hostname.
pub fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_else(|_| "unknown".into())
        .trim()
        .to_string()
}

/// Check whether a PID is still alive on this host.
fn is_pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Gather resource usage for the current process via `/proc/self`.
pub fn self_resource_usage() -> ResourceUsage {
    let rss_bytes = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| {
            let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
            Some(pages * 4096) // page size
        });

    ResourceUsage {
        rss_bytes,
        cpu_percent: None,
    }
}

/// Create an `AgentEntry` for the current process.
pub fn self_entry(role: &str) -> AgentEntry {
    AgentEntry {
        id: format!("{}-{}", role, std::process::id()),
        pid: std::process::id(),
        host: hostname(),
        start_time: Utc::now(),
        last_heartbeat: Utc::now(),
        state: AgentState::Running,
        role: role.to_string(),
        resources: self_resource_usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
