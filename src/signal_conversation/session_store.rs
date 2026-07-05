//! Durable, resumable per-operator session store for the Signal continuous
//! conversation (issue #2577).
//!
//! This is the Signal-channel analogue of the dashboard's
//! [`crate::operator_commands_dashboard::chat_store`]. It reuses the **same**
//! session-file envelope shape and the same crash-durable
//! [`crate::persistence::persist_json`] write pipeline, but lives on the Signal
//! side (lower blast radius — no re-pointing of the axum-scoped dashboard store)
//! and adds one thing the dashboard does not need: a per-operator index that
//! maps an operator's Signal address (E.164) to their **active** session id.
//!
//! On-disk layout under the resolved durable state root:
//!
//! ```text
//! <state_root>/signal_sessions/
//! ├── operators.json         # operator E.164 -> active session_id (the index)
//! ├── <session_id>.json      # full, uncapped turn history for one session
//! └── …
//! ```
//!
//! Design contract:
//!
//!   * Every `session_id` that reaches a filesystem path is validated by the
//!     shared [`crate::session_id::validate_session_id`] guard
//!     (`^[A-Za-z0-9_-]{1,64}$`) **before any path join**. A raw E.164 fails the
//!     guard, so the operator number is only ever a *lookup value* inside
//!     `operators.json` — never a path component.
//!   * The full turn history is persisted **uncapped** — independent of the
//!     in-memory `MeetingBackend` `MAX_HISTORY` working-set cap — so a daemon
//!     restart can replay the complete conversation into a fresh backend.
//!   * `append_turn_at` creates the session file lazily on the first turn.
//!   * `set_active_session` is what `/new` uses to rotate an operator onto a
//!     fresh session id while the previous session file is **retained** on disk.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::meeting_backend::ConversationMessage;
use crate::persistence::persist_json;
use crate::session_id::validate_session_id;

/// On-disk schema version for the Signal session + index envelopes. Bump when
/// the shape of [`SignalSession`] / [`OperatorIndex`] changes in a breaking way.
const SCHEMA_VERSION: u32 = 1;

/// Fixed static subdirectory (under the state root) that holds every Signal
/// session file plus the operator index. Joined onto the resolved state root;
/// never derived from operator data.
const SIGNAL_SESSIONS_DIR: &str = "signal_sessions";

/// Persistence "store" label used in [`SimardError::PersistentStoreIo`].
const STORE: &str = "signal_sessions";

/// Filename of the operator -> active-session index inside the sessions dir.
const OPERATORS_INDEX: &str = "operators.json";

/// Serializes concurrent `operators.json` upserts so two `set_active_session`
/// read-modify-writes can never lose an entry (writes themselves are atomic
/// temp+rename via `persist_json`, but the read-modify-write is not).
static INDEX_LOCK: Mutex<()> = Mutex::new(());

/// The `operators.json` envelope: operator Signal address (E.164) -> active
/// session id. The E.164 is a lookup **value/key** here — it is never joined
/// onto a filesystem path.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct OperatorIndex {
    schema_version: u32,
    #[serde(default)]
    operators: BTreeMap<String, String>,
}

impl Default for OperatorIndex {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            operators: BTreeMap::new(),
        }
    }
}

/// One durable Signal session: metadata plus the complete, uncapped turn
/// history for a single conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalSession {
    /// On-disk schema version for forward-compatible migrations.
    pub schema_version: u32,
    /// Operator Signal address (E.164) that owns this session.
    pub operator: String,
    /// Stable session id (UUIDv7). Validated against the traversal guard.
    pub session_id: String,
    /// RFC3339 timestamp of the first turn; set once, never changes.
    pub created_at: String,
    /// RFC3339 timestamp of the most recently appended turn.
    pub updated_at: String,
    /// The complete, uncapped conversation history (both roles).
    pub history: Vec<ConversationMessage>,
}

/// Metadata for one stored session, surfaced by [`list_sessions_at`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignalSessionMeta {
    pub session_id: String,
    pub operator: String,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Path + IO helpers
// ---------------------------------------------------------------------------

fn invalid_session_id_err(id: &str) -> SimardError {
    SimardError::InvalidSessionId {
        value: id.to_string(),
        reason: "must match ^[A-Za-z0-9_-]{1,64}$".to_string(),
    }
}

fn io_err(action: &str, path: &Path, source: impl std::fmt::Display) -> SimardError {
    SimardError::PersistentStoreIo {
        store: STORE.to_string(),
        action: action.to_string(),
        path: path.to_path_buf(),
        reason: source.to_string(),
    }
}

fn sessions_dir(state_root: &Path) -> PathBuf {
    state_root.join(SIGNAL_SESSIONS_DIR)
}

fn session_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn operators_index_path(dir: &Path) -> PathBuf {
    dir.join(OPERATORS_INDEX)
}

/// Create `signal_sessions/` if absent and lock it down to owner-only
/// (`0o700`). `persist_json` creates parent dirs subject to the process umask,
/// so the directory is chmod'd explicitly to keep the whole tree owner-only —
/// operator conversations are sensitive.
fn ensure_dir_secure(dir: &Path) -> SimardResult<()> {
    fs::create_dir_all(dir).map_err(|e| io_err("create-dir", dir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| io_err("chmod-dir", dir, e))?;
    }
    Ok(())
}

/// Read a `<session_id>.json` file (caller has already validated the id).
/// Returns `Ok(None)` when the file is absent.
fn read_session_file(path: &Path) -> SimardResult<Option<SignalSession>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|e| io_err("read", path, e))?;
    let session: SignalSession =
        serde_json::from_slice(&bytes).map_err(|e| io_err("deserialize", path, e))?;
    Ok(Some(session))
}

fn read_operator_index(path: &Path) -> SimardResult<OperatorIndex> {
    if !path.exists() {
        return Ok(OperatorIndex::default());
    }
    let bytes = fs::read(path).map_err(|e| io_err("read", path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| io_err("deserialize", path, e))
}

/// Reverse-resolve the operator that currently owns `session_id` from the
/// index (best-effort denormalization used to stamp `SignalSession.operator`).
/// Returns `""` when no operator currently points at this session.
fn operator_owning(dir: &Path, session_id: &str) -> String {
    read_operator_index(&operators_index_path(dir))
        .ok()
        .and_then(|idx| {
            idx.operators
                .into_iter()
                .find(|(_op, sid)| sid == session_id)
                .map(|(op, _sid)| op)
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Per-operator active-session index (operators.json)
// ---------------------------------------------------------------------------

/// Append one conversation turn to a session, creating the session file lazily
/// on the first turn. Persists the full `<session_id>.json` history (uncapped).
///
/// `state_root` is trusted-internal; `session_id` is validated against the
/// shared traversal guard **before any path is constructed**, so an invalid id
/// (including a raw E.164) returns `Err` and writes nothing.
pub fn append_turn_at(
    state_root: &Path,
    session_id: &str,
    message: &ConversationMessage,
) -> SimardResult<()> {
    if !validate_session_id(session_id) {
        return Err(invalid_session_id_err(session_id));
    }
    let dir = sessions_dir(state_root);
    ensure_dir_secure(&dir)?;
    let path = session_path(&dir, session_id);

    let mut session = match read_session_file(&path)? {
        Some(existing) => existing,
        None => {
            let created_at = message.timestamp.clone();
            SignalSession {
                schema_version: SCHEMA_VERSION,
                // Best-effort: stamp the owning operator from the index if one
                // already points here (the real run sets the index before the
                // first append). Left empty for direct store-unit use.
                operator: operator_owning(&dir, session_id),
                session_id: session_id.to_string(),
                created_at: created_at.clone(),
                updated_at: created_at,
                history: Vec::new(),
            }
        }
    };

    session.updated_at = message.timestamp.clone();
    session.history.push(message.clone());

    persist_json(STORE, &path, &session)?;
    tracing::debug!(
        target: "signal",
        session_id,
        role = ?message.role,
        turns = session.history.len(),
        "session.append"
    );
    Ok(())
}

/// Load one session's complete history by id from an explicit state root.
///
/// Returns `Ok(None)` when no session with that (valid) id exists, and
/// `Err` when the id fails the traversal guard — **before** any path join.
pub fn load_session_at(state_root: &Path, session_id: &str) -> SimardResult<Option<SignalSession>> {
    if !validate_session_id(session_id) {
        return Err(invalid_session_id_err(session_id));
    }
    let dir = sessions_dir(state_root);
    read_session_file(&session_path(&dir, session_id))
}

/// List every stored session's metadata. Returns `Ok(vec![])` for an
/// empty/missing store — never an error for "nothing persisted yet".
pub fn list_sessions_at(state_root: &Path) -> SimardResult<Vec<SignalSessionMeta>> {
    let dir = sessions_dir(state_root);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err("read-dir", &dir, e)),
    };

    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| io_err("read-dir-entry", &dir, e))?;
        let path = entry.path();
        // Every session file is `<session_id>.json`; skip the operator index
        // and anything else (e.g. lock/temp files).
        if path.file_name().and_then(|n| n.to_str()) == Some(OPERATORS_INDEX) {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Some(session) = read_session_file(&path)? {
            out.push(SignalSessionMeta {
                session_id: session.session_id,
                operator: session.operator,
                created_at: session.created_at,
                updated_at: session.updated_at,
            });
        }
    }
    // RFC3339 UTC ("…Z") timestamps of equal shape sort lexicographically in
    // chronological order, so a reverse string sort yields newest-first.
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

/// Look up the operator's currently-active session id from `operators.json`.
///
/// Returns `Ok(None)` when the operator has no active session yet (first
/// contact). The `operator` argument is an opaque lookup key (the E.164); it is
/// never joined onto a path.
pub fn active_session_for(state_root: &Path, operator: &str) -> SimardResult<Option<String>> {
    let dir = sessions_dir(state_root);
    let index = read_operator_index(&operators_index_path(&dir))?;
    Ok(index.operators.get(operator).cloned())
}

/// Point an operator at `session_id` in the `operators.json` index (creating or
/// overwriting the mapping). Used on first contact and on `/new` rotation.
///
/// `session_id` is validated against the shared traversal guard before it is
/// recorded, so the index can never point at a hostile id.
pub fn set_active_session(state_root: &Path, operator: &str, session_id: &str) -> SimardResult<()> {
    if !validate_session_id(session_id) {
        return Err(invalid_session_id_err(session_id));
    }
    let dir = sessions_dir(state_root);
    ensure_dir_secure(&dir)?;
    let path = operators_index_path(&dir);

    let _guard = INDEX_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut index = read_operator_index(&path)?;
    index
        .operators
        .insert(operator.to_string(), session_id.to_string());
    persist_json(STORE, &path, &index)
}
