//! Subagent tmux session registry (WS-2).
//!
//! Tracks engineer subprocesses launched inside tmux sessions so the
//! dashboard can surface live and recently-ended sessions and offer
//! `tmux attach` deep-links from the Recent Actions feed.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Process-wide lock serializing read-modify-write cycles against the
/// on-disk session registry (`load` → mutate → `save_atomic`).
///
/// `save_atomic` keeps the file *valid* via temp-file+rename, but it does
/// not prevent a lost update: two callers that `load()` the same snapshot
/// will each `save_atomic` their own copy, and the second write clobbers
/// the first writer's appended/changed entries. Engineers are now spawned
/// concurrently within a single OODA round (see `dispatch_advance_goal_concurrent`),
/// so `record_spawn` can run in parallel with itself and with the daemon's
/// `poll_and_gc`. Holding this lock across the whole load→mutate→save
/// sequence makes those mutations atomic with respect to one another.
fn registry_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Sessions ended more than this many seconds ago are GC'd (default).
/// Configurable via `SIMARD_SUBAGENT_RETENTION_SECONDS`.
pub const RETENTION_SECONDS: i64 = 86_400;

/// Tight retention used during emergency memory shedding (1 hour).
pub const TIGHT_RETENTION_SECONDS: i64 = 3_600;

/// Hard cap on the number of registry entries. When exceeded during
/// `record_spawn`, ended sessions are pruned oldest-first until the
/// count is within the cap.
pub const MAX_SESSIONS: usize = 500;

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
    load_from(&registry_path())
}

/// Load the registry from an explicit `<state_root>/state/subagent_sessions.json`
/// path. Parameterized so the dashboard's live-engineer view (issue #2580) can
/// read a specific state root hermetically. Returns an empty `Registry` on a
/// missing or corrupt file (never panics).
pub fn load_from(path: &Path) -> Registry {
    let bytes = match fs::read(path) {
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

/// Path of the registry under an explicit state root:
/// `<state_root>/state/subagent_sessions.json`.
pub fn registry_path_under(state_root: &Path) -> PathBuf {
    state_root.join("state").join("subagent_sessions.json")
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
///
/// If the registry exceeds [`MAX_SESSIONS`] after the push, ended
/// sessions are pruned oldest-first until the count is within the cap.
/// Active (non-ended) sessions are never dropped by the cap.
pub fn record_spawn(session: SubagentSession) -> io::Result<()> {
    let _guard = registry_mutation_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let mut reg = load();
    reg.sessions.push(session);
    enforce_cap(&mut reg);
    save_atomic(&reg)
}

/// Enforce [`MAX_SESSIONS`] by dropping the oldest ended sessions.
fn enforce_cap(reg: &mut Registry) {
    if reg.sessions.len() <= MAX_SESSIONS {
        return;
    }
    // Sort ended sessions by ended_at ascending (oldest first) and drop
    // enough to get under the cap. Active sessions are never touched.
    let excess = reg.sessions.len() - MAX_SESSIONS;
    let mut ended_indices: Vec<usize> = reg
        .sessions
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.ended_at.map(|ts| (i, ts)))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|(i, _ts)| i)
        .collect();

    // Sort by ended_at ascending (oldest first).
    ended_indices.sort_by_key(|&i| reg.sessions[i].ended_at.unwrap_or(i64::MAX));

    let to_remove: std::collections::HashSet<usize> =
        ended_indices.into_iter().take(excess).collect();
    if !to_remove.is_empty() {
        let mut idx = 0;
        reg.sessions.retain(|_| {
            let keep = !to_remove.contains(&idx);
            idx += 1;
            keep
        });
        tracing::info!(
            target: "simard::subagent_sessions",
            removed = to_remove.len(),
            remaining = reg.sessions.len(),
            "enforced MAX_SESSIONS cap",
        );
    }
}

/// Mark dead sessions as ended; GC entries ended longer ago than
/// [`RETENTION_SECONDS`].
///
/// Retention is read from `SIMARD_SUBAGENT_RETENTION_SECONDS` env var at
/// call time, falling back to the compiled-in [`RETENTION_SECONDS`].
pub fn poll_and_gc<R: SessionProbe>(probe: &R) -> io::Result<()> {
    let retention = configured_retention();
    gc_with_retention(probe, retention).map(|_| ())
}

/// Like [`poll_and_gc`] but with an explicit retention threshold. Returns
/// the number of entries pruned.
pub fn gc_with_retention<R: SessionProbe>(probe: &R, retention_secs: i64) -> io::Result<usize> {
    let _guard = registry_mutation_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
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
        Some(end) => now - end <= retention_secs,
        None => true,
    });
    let pruned = before - reg.sessions.len();
    if pruned > 0 {
        tracing::info!(
            target: "simard::subagent_sessions",
            pruned,
            retention_secs,
            "GC'd subagent sessions older than retention threshold",
        );
    }

    save_atomic(&reg)?;
    Ok(pruned)
}

/// Read retention from env or return the compiled-in default.
fn configured_retention() -> i64 {
    std::env::var("SIMARD_SUBAGENT_RETENTION_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(RETENTION_SECONDS)
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
