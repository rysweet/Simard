//! Subagent tmux session registry (WS-2).
//!
//! Tracks engineer subprocesses launched inside tmux sessions so the
//! dashboard can surface live and recently-ended sessions and offer
//! `tmux attach` deep-links from the Recent Actions feed.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Sessions ended more than this many seconds ago are GC'd.
pub const RETENTION_SECONDS: i64 = 86_400;

/// One row in the registry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentSession {
    pub agent_id: String,
    pub session_name: String,
    pub host: String,
    pub pid: u32,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    pub goal_id: String,
}

/// On-disk registry shape.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub sessions: Vec<SubagentSession>,
}

/// Probe abstraction so polling can be unit-tested without a real tmux.
pub trait SessionProbe {
    fn alive(&self, session_name: &str) -> bool;
}

/// Real probe: shells out to `tmux has-session -t <name>`.
pub struct TmuxProbe;

impl SessionProbe for TmuxProbe {
    fn alive(&self, session_name: &str) -> bool {
        match Command::new("tmux")
            .args(["has-session", "-t", session_name])
            .output()
        {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }
}

/// Resolve the state root: `SIMARD_STATE_ROOT` env or `$HOME/.simard`.
fn state_root() -> PathBuf {
    if let Ok(v) = std::env::var("SIMARD_STATE_ROOT") {
        return PathBuf::from(v);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".simard")
}

/// Returns the on-disk registry path: `<state_root>/state/subagent_sessions.json`.
pub fn registry_path() -> PathBuf {
    state_root().join("state").join("subagent_sessions.json")
}

fn now_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Load the registry from disk. Returns empty `Registry` on missing/corrupt.
pub fn load() -> Registry {
    let path = registry_path();
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::warn!(
                    target: "simard::subagent_sessions",
                    path = %path.display(),
                    error = %e,
                    "failed to read subagent registry; returning empty",
                );
            }
            return Registry::default();
        }
    };
    match serde_json::from_slice::<Registry>(&bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                target: "simard::subagent_sessions",
                path = %path.display(),
                error = %e,
                "failed to parse subagent registry; returning empty",
            );
            Registry::default()
        }
    }
}

/// Atomic write: write to a uniquely-named temp file in the same directory,
/// then `rename`. Removes the temp file on any failure so no `.tmp.*`
/// stragglers are left behind.
pub fn save_atomic(reg: &Registry) -> io::Result<()> {
    let path = registry_path();
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "registry_path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;

    let tmp = parent.join(format!("subagent_sessions.json.tmp.{}", std::process::id()));

    let serialized = serde_json::to_vec_pretty(reg).map_err(io::Error::other)?;

    let write_result = (|| -> io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&serialized)?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Append a new session record (load → push → save_atomic).
pub fn record_spawn(session: SubagentSession) -> io::Result<()> {
    let mut reg = load();
    reg.sessions.push(session);
    save_atomic(&reg)
}

/// Mark dead sessions as ended; GC entries ended >24h ago.
pub fn poll_and_gc<R: SessionProbe>(probe: &R) -> io::Result<()> {
    let mut reg = load();
    let now = now_epoch_seconds();

    for s in reg.sessions.iter_mut() {
        if s.ended_at.is_some() {
            continue;
        }
        if !probe.alive(&s.session_name) {
            s.ended_at = Some(now);
            tracing::info!(
                target: "simard::subagent_sessions",
                agent_id = %s.agent_id,
                session_name = %s.session_name,
                "subagent session ended (tmux has-session = false)",
            );
        }
    }

    let before = reg.sessions.len();
    reg.sessions.retain(|s| match s.ended_at {
        Some(end) => now - end <= RETENTION_SECONDS,
        None => true,
    });
    let pruned = before - reg.sessions.len();
    if pruned > 0 {
        tracing::info!(
            target: "simard::subagent_sessions",
            pruned,
            "GC'd subagent sessions ended >24h ago",
        );
    }

    save_atomic(&reg)
}

/// Sanitize an agent_id for use in a tmux session name.
/// Replaces `[^A-Za-z0-9_-]` with `-`. Empty input becomes `"engineer"`.
pub fn sanitize_id(raw: &str) -> String {
    if raw.is_empty() {
        return "engineer".to_string();
    }
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Convenience: produce the canonical tmux session name for an agent id.
pub fn session_name_for(agent_id: &str) -> String {
    format!("simard-engineer-{}", sanitize_id(agent_id))
}

#[cfg(test)]
mod tests;
