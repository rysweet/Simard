//! Durable, resumable chat-session store for the operator dashboard Chat tab
//! (issue #2577).
//!
//! Chat sessions are persisted under the resolved durable state root in a
//! dedicated `chat_sessions/` subdirectory, keyed strictly by `session_id`:
//!
//! ```text
//! <state_root>/chat_sessions/
//! ├── index.json            # session list (metadata only)
//! ├── <session_id>.json      # full, uncapped turn history for one session
//! └── …
//! ```
//!
//! Design contract (see `docs/reference/dashboard-chat.md`):
//!
//!   * Every `session_id` that reaches a filesystem path is validated against
//!     `^[A-Za-z0-9_-]{1,64}$` **before any path join** (path-traversal guard).
//!   * The full turn history is persisted **uncapped** — independent of the
//!     in-memory `MeetingBackend` `MAX_HISTORY = 500` working-set cap — so a
//!     restart restores the complete conversation.
//!   * A session record is created **lazily on the first turn**; its title is
//!     derived from the first message (truncated to ~60 chars, timestamp
//!     fallback). `created_at` is set once; `updated_at` advances every turn.
//!   * The `<session_id>.json` file is written **before** the `index.json`
//!     upsert; concurrent index upserts are serialized by a process-global lock.
//!   * All writes go through [`crate::persistence::persist_json`] — a
//!     crash-durable atomic pipeline that chmods each file to `0o600`; the
//!     `chat_sessions/` directory itself is chmod'd to `0o700` on first use.
//!
//! The `_at(state_root)` cores take an EXPLICIT path (the `goals.rs`
//! convention), so callers thread a trusted-internal state root through rather
//! than resolving `SIMARD_STATE_ROOT` ambiently.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::Json;
use axum::extract::Path as AxumPath;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::routes::{resolve_state_root, truncate_with_ellipsis};
use crate::error::{SimardError, SimardResult};
use crate::meeting_backend::ConversationMessage;
use crate::persistence::persist_json;

/// On-disk schema version for the chat-session envelopes. Bump when the shape
/// of [`ChatSession`] / [`ChatSessionIndex`] changes in a breaking way.
const SCHEMA_VERSION: u32 = 1;

/// Fixed static subdirectory (under the state root) that holds every chat
/// session. Joined onto the resolved state root; never derived from request
/// data.
const CHAT_SESSIONS_DIR: &str = "chat_sessions";

/// Persistence "store" label used in [`SimardError::PersistentStoreIo`].
const STORE: &str = "chat_sessions";

/// Maximum number of characters kept from the first message when deriving a
/// session title. Longer titles are truncated with a trailing ellipsis.
const TITLE_MAX_CHARS: usize = 60;

/// Serializes concurrent `index.json` upserts. A partial reader is impossible
/// (writes are atomic temp+rename), but two concurrent read-modify-write
/// upserts could otherwise lose an entry; this lock makes the upsert
/// read-modify-write atomic across the process.
static INDEX_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// On-disk types
// ---------------------------------------------------------------------------

/// Metadata for one chat session, surfaced in `index.json` and the REST list.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChatSessionMeta {
    /// Stable session id (UUIDv7). Validated against `^[A-Za-z0-9_-]{1,64}$`.
    pub id: String,
    /// Human-readable title derived from the first message (timestamp fallback).
    pub title: String,
    /// RFC3339 timestamp of the first turn; set once, never changes.
    pub created_at: String,
    /// RFC3339 timestamp of the most recently appended turn.
    pub updated_at: String,
}

/// One durable chat session: metadata plus the complete, uncapped turn history.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSession {
    pub schema_version: u32,
    pub meta: ChatSessionMeta,
    pub history: Vec<ConversationMessage>,
}

/// The `index.json` envelope: every session's metadata for the sidebar.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSessionIndex {
    pub schema_version: u32,
    #[serde(default)]
    pub sessions: Vec<ChatSessionMeta>,
}

impl Default for ChatSessionIndex {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            sessions: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Session-id validation & generation (path-traversal guard)
// ---------------------------------------------------------------------------

// The security-critical id guard + generator now live in the shared,
// channel-agnostic `crate::session_id` module (issue #2577) so the dashboard
// chat store and the Signal session store validate ids identically, in exactly
// one place. Re-exported here to preserve the `chat_store::validate_session_id`
// / `chat_store::new_session_id` public surface.
pub use crate::session_id::{new_session_id, validate_session_id};

fn invalid_session_id_err(id: &str) -> SimardError {
    SimardError::InvalidSessionId {
        value: id.to_string(),
        reason: "must match ^[A-Za-z0-9_-]{1,64}$".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn chat_sessions_dir(state_root: &Path) -> PathBuf {
    state_root.join(CHAT_SESSIONS_DIR)
}

fn session_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn index_path(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

fn io_err(action: &str, path: &Path, source: impl std::fmt::Display) -> SimardError {
    SimardError::PersistentStoreIo {
        store: STORE.to_string(),
        action: action.to_string(),
        path: path.to_path_buf(),
        reason: source.to_string(),
    }
}

/// Create `chat_sessions/` if absent and lock it down to owner-only (`0o700`).
/// `persist_json` creates parent dirs subject to the process umask, so the
/// directory is chmod'd explicitly to keep the whole tree owner-only.
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

// ---------------------------------------------------------------------------
// Load / list
// ---------------------------------------------------------------------------

/// Read a `<session_id>.json` file without re-validating the id (the caller
/// has already validated). Returns `Ok(None)` when the file is absent.
fn read_session_file(path: &Path) -> SimardResult<Option<ChatSession>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|e| io_err("read", path, e))?;
    let session: ChatSession =
        serde_json::from_slice(&bytes).map_err(|e| io_err("deserialize", path, e))?;
    Ok(Some(session))
}

fn read_index(path: &Path) -> SimardResult<ChatSessionIndex> {
    if !path.exists() {
        return Ok(ChatSessionIndex::default());
    }
    let bytes = fs::read(path).map_err(|e| io_err("read", path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| io_err("deserialize", path, e))
}

/// Load one session's complete history by id from an explicit state root.
///
/// Returns `Ok(None)` when no session with that (valid) id exists, and
/// `Err(InvalidSessionId)` when the id fails the traversal guard — **before**
/// any path join.
pub fn load_session_at(state_root: &Path, id: &str) -> SimardResult<Option<ChatSession>> {
    if !validate_session_id(id) {
        return Err(invalid_session_id_err(id));
    }
    let dir = chat_sessions_dir(state_root);
    read_session_file(&session_path(&dir, id))
}

/// List every saved session's metadata, newest first (`updated_at` descending).
///
/// Returns `Ok(vec![])` when the store directory or index is absent — never an
/// error for an empty/missing store.
pub fn list_sessions_at(state_root: &Path) -> SimardResult<Vec<ChatSessionMeta>> {
    let dir = chat_sessions_dir(state_root);
    let mut index = read_index(&index_path(&dir))?;
    // RFC3339 UTC ("…Z") timestamps of equal shape sort lexicographically in
    // chronological order, so a reverse string sort yields newest-first.
    index
        .sessions
        .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(index.sessions)
}

// ---------------------------------------------------------------------------
// Append (lazy create + persist + index upsert)
// ---------------------------------------------------------------------------

/// Derive a session title from the first message content.
///
/// Non-empty content is truncated (on a UTF-8 char boundary) to
/// [`TITLE_MAX_CHARS`] with a trailing ellipsis; empty content falls back to
/// the creation timestamp so the sidebar never shows a blank title.
fn derive_title(first_content: &str, created_at: &str) -> String {
    if first_content.is_empty() {
        created_at.to_string()
    } else {
        truncate_with_ellipsis(first_content, TITLE_MAX_CHARS)
    }
}

/// Append one conversation turn to a session, creating the session lazily on
/// the first turn. Persists the full `<session_id>.json` history (uncapped)
/// and upserts the `index.json` entry.
///
/// The `state_root` is trusted-internal; the `id` is validated against the
/// traversal guard before any path is constructed, so an invalid id returns
/// `Err` and writes nothing.
pub fn append_turn_at(
    state_root: &Path,
    id: &str,
    message: &ConversationMessage,
) -> SimardResult<()> {
    if !validate_session_id(id) {
        return Err(invalid_session_id_err(id));
    }
    let dir = chat_sessions_dir(state_root);
    ensure_dir_secure(&dir)?;
    let path = session_path(&dir, id);

    let mut session = match read_session_file(&path)? {
        Some(existing) => existing,
        None => {
            let created_at = message.timestamp.clone();
            let title = derive_title(&message.content, &created_at);
            ChatSession {
                schema_version: SCHEMA_VERSION,
                meta: ChatSessionMeta {
                    id: id.to_string(),
                    title,
                    created_at: created_at.clone(),
                    updated_at: created_at,
                },
                history: Vec::new(),
            }
        }
    };

    session.meta.updated_at = message.timestamp.clone();
    session.history.push(message.clone());

    // Write the per-session file first so a crash before the index upsert
    // leaves a recoverable session, not a dangling index entry.
    persist_json(STORE, &path, &session)?;
    upsert_index(&dir, &session.meta)
}

fn upsert_index(dir: &Path, meta: &ChatSessionMeta) -> SimardResult<()> {
    let _guard = INDEX_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = index_path(dir);
    let mut index = read_index(&path)?;
    match index.sessions.iter_mut().find(|s| s.id == meta.id) {
        Some(entry) => *entry = meta.clone(),
        None => index.sessions.push(meta.clone()),
    }
    persist_json(STORE, &path, &index)
}

// ---------------------------------------------------------------------------
// REST handler cores (thin `_at(state_root)` + ambient-resolving wrappers)
// ---------------------------------------------------------------------------

/// Core of `GET /api/chat/sessions`: list all sessions newest-first from an
/// explicit state root. An empty/missing store is `200 {"sessions": []}`.
pub async fn chat_sessions_at(state_root: &Path) -> (StatusCode, Json<Value>) {
    match list_sessions_at(state_root) {
        Ok(sessions) => (StatusCode::OK, Json(json!({ "sessions": sessions }))),
        Err(e) => {
            // Log the specific filesystem detail server-side only; the client
            // sees a generic error with no on-disk path (SR-E1).
            eprintln!("[simard] chat sessions list failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list chat sessions" })),
            )
        }
    }
}

/// Core of `GET /api/chat/sessions/{id}`: return one session's full history.
///
/// `400` when the id fails validation (before store access), `404` when no
/// session with that valid id exists, `500` when the file is present but
/// corrupt. Error bodies never leak the on-disk state-root path.
pub async fn chat_session_by_id_at(state_root: &Path, id: &str) -> (StatusCode, Json<Value>) {
    if !validate_session_id(id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid session id" })),
        );
    }
    match load_session_at(state_root, id) {
        Ok(Some(session)) => (
            StatusCode::OK,
            Json(json!({
                "id": session.meta.id,
                "title": session.meta.title,
                "created_at": session.meta.created_at,
                "updated_at": session.meta.updated_at,
                "history": session.history,
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "session not found" })),
        ),
        Err(e) => {
            eprintln!("[simard] chat session load failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to load chat session" })),
            )
        }
    }
}

/// `GET /api/chat/sessions` — thin wrapper resolving the ambient state root.
pub(crate) async fn chat_sessions() -> (StatusCode, Json<Value>) {
    chat_sessions_at(&resolve_state_root()).await
}

/// `GET /api/chat/sessions/{id}` — thin wrapper resolving the ambient state
/// root.
pub(crate) async fn chat_session_by_id(
    AxumPath(id): AxumPath<String>,
) -> (StatusCode, Json<Value>) {
    chat_session_by_id_at(&resolve_state_root(), &id).await
}
