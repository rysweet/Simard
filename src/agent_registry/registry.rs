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

/// Gather resource usage for the current process via /proc/self.
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
