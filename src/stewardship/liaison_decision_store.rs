//! Durable, typed operator-liaison decision store (issue #4911, Deliverable 1).
//!
//! A **sibling** of [`super::merge_verdict_store`] with the identical, proven
//! agentic-recipe-first transport shape — but a distinct identity and lifecycle.
//! Where a merge verdict is keyed by a PR, a liaison decision is keyed by the
//! operator message it answers `(group_id, message_id)`. The agent-facing
//! `simard liaison record-decision` tool WRITEs a typed
//! [`LiaisonDecisionRecord`]; the thin deterministic rail
//! ([`crate::overseer::signal_liaison`]) READs it back — freshness- and
//! identity-checked — instead of scraping prose.
//!
//! ## Safety properties
//!
//! - **Deterministic, traversal-safe path.** A record for `(group_id,
//!   message_id)` lives at
//!   `<state_root>/liaison_decisions/<group_id_hash>/<message_id>.json`. The
//!   `group_id` is opaque (base64 — may hold `/`, `+`, `=`), so it is HASHED
//!   (SHA-256) into a single path-safe segment; the `message_id` is validated so
//!   it can never escape the subtree.
//! - **Atomic write.** [`write_record`] writes a temp sibling then `rename`s over
//!   the final path, and sets owner-only `0o600` before the rename so no
//!   permissions window ever exists.
//! - **Fail-closed read.** [`read_verified`] is a total function — it never
//!   panics and returns [`ReadOutcome::Mismatch`] on malformed JSON, an unknown
//!   `schema_version`, a `(group_id, message_id)` identity disagreement, or a
//!   `run_token` mismatch. The liaison rail must never act on a stale, foreign,
//!   or corrupt decision.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The only record schema this build understands. Any other value fails closed
/// on read (forward/backward compatibility is intentionally NOT silent).
pub const SCHEMA_VERSION: u32 = 1;

/// A go-ahead the liaison agent recorded: dispatch an intervention. Only one
/// directive kind exists in this PR — reuse the existing recipe-launch path
/// (default-workflow). Large context rides `context_path` (a ContextFile),
/// never argv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directive {
    /// The recipe to launch (e.g. `"default-workflow"`).
    pub recipe: String,
    /// The task description handed to the recipe (bounded; large payloads ride
    /// `context_path`).
    pub task_description: String,
    /// Validated `owner/name` repo the recipe targets.
    pub target_repo: String,
    /// Absolute path to a ContextFile carrying the full operator context, so the
    /// payload never touches argv.
    pub context_path: String,
}

/// A durably-recorded, freshness-tokened liaison decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiaisonDecisionRecord {
    /// Record schema version. Read paths fail closed on any value other than
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Identity — the operator group this decision answers.
    pub group_id: String,
    /// Identity — the operator message high-water-mark id this decision answers.
    pub message_id: String,
    /// Opaque single-run token; the rail requires the record to echo this run's
    /// token so a previous run's decision can never be mistaken for this one's.
    pub run_token: String,
    /// RFC3339 UTC instant the record was stamped.
    pub recorded_at: String,
    /// Optional plain-English reply to post back to the operator group.
    #[serde(default)]
    pub reply: Option<String>,
    /// Optional intervention directive. `reply` and `directive` are independent —
    /// a record may carry either, both, or (a valid no-op) neither.
    #[serde(default)]
    pub directive: Option<Directive>,
}

impl LiaisonDecisionRecord {
    /// Build a record for the current run, stamping [`SCHEMA_VERSION`] and an
    /// RFC3339 UTC `recorded_at` internally.
    pub fn new(
        group_id: &str,
        message_id: &str,
        run_token: &str,
        reply: Option<String>,
        directive: Option<Directive>,
    ) -> LiaisonDecisionRecord {
        LiaisonDecisionRecord {
            schema_version: SCHEMA_VERSION,
            group_id: group_id.to_string(),
            message_id: message_id.to_string(),
            run_token: run_token.to_string(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            reply,
            directive,
        }
    }
}

/// The result of a freshness/identity-checked read. A total function's whole
/// codomain — the rail treats anything but [`ReadOutcome::Found`] as "no valid
/// decision for this run".
#[derive(Debug)]
pub enum ReadOutcome {
    /// A record that matched `(group_id, message_id, run_token)` and carries a
    /// known `schema_version`.
    Found(LiaisonDecisionRecord),
    /// No record file exists for `(group_id, message_id)`.
    Missing,
    /// A record exists but is stale/foreign/corrupt. The `String` names the
    /// reason for operator diagnostics.
    Mismatch(String),
}

/// Hash the opaque `group_id` into a single path-safe hex segment. The raw id
/// may contain `/`, `+`, `=`; the hash never does and never reveals the id
/// verbatim in the path.
fn group_id_segment(group_id: &str) -> String {
    super::record_io::sha256_hex(group_id.as_bytes())
}

/// Validate a `message_id` so it can never escape the store subtree: rejects
/// NUL bytes, path separators, empty, and `.`/`..`.
fn validate_message_id(message_id: &str) -> Result<(), String> {
    if message_id.is_empty() {
        return Err("message_id must be non-empty".to_string());
    }
    if message_id == "." || message_id == ".." {
        return Err(format!(
            "message_id {message_id:?} is a reserved path component"
        ));
    }
    if message_id.contains('\0') || message_id.contains('/') || message_id.contains('\\') {
        return Err(format!(
            "message_id {message_id:?} contains an unsafe path character"
        ));
    }
    Ok(())
}

/// Deterministic, traversal-safe path for the `(group_id, message_id)` record:
/// `<state_root>/liaison_decisions/<group_id_hash>/<message_id>.json`.
pub fn record_path(state_root: &Path, group_id: &str, message_id: &str) -> Result<PathBuf, String> {
    if group_id.is_empty() {
        return Err("group_id must be non-empty".to_string());
    }
    validate_message_id(message_id)?;
    Ok(state_root
        .join("liaison_decisions")
        .join(group_id_segment(group_id))
        .join(format!("{message_id}.json")))
}

/// Atomically write `rec` to its deterministic path, creating parent dirs, and
/// set owner-only `0o600`. Delegates the temp-write + `rename` to the shared
/// [`super::record_io::atomic_write_0600`] (last writer wins), so a concurrent
/// reader never sees a partial record and no temp file is left beside it.
pub fn write_record(state_root: &Path, rec: &LiaisonDecisionRecord) -> Result<(), String> {
    let path = record_path(state_root, &rec.group_id, &rec.message_id)?;
    let json =
        serde_json::to_vec_pretty(rec).map_err(|e| format!("serialize liaison record: {e}"))?;
    super::record_io::atomic_write_0600(&path, &json)
}

/// Read the record for `(group_id, message_id)` and verify it belongs to THIS
/// run. Total function: never panics. Returns [`ReadOutcome::Missing`] when no
/// record exists, [`ReadOutcome::Mismatch`] when a record is
/// unreadable / unknown-schema / identity-mismatched / stale-tokened, and
/// [`ReadOutcome::Found`] only when everything checks out.
pub fn read_verified(
    state_root: &Path,
    group_id: &str,
    message_id: &str,
    expected_run_token: &str,
) -> ReadOutcome {
    let path = match record_path(state_root, group_id, message_id) {
        Ok(p) => p,
        Err(e) => return ReadOutcome::Mismatch(e),
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::Missing,
        Err(e) => return ReadOutcome::Mismatch(format!("read {path:?} failed: {e}")),
    };
    let rec: LiaisonDecisionRecord = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(e) => return ReadOutcome::Mismatch(format!("malformed record json: {e}")),
    };
    if rec.schema_version != SCHEMA_VERSION {
        return ReadOutcome::Mismatch(format!(
            "unknown schema_version {} (expected {SCHEMA_VERSION})",
            rec.schema_version
        ));
    }
    if rec.group_id != group_id || rec.message_id != message_id {
        return ReadOutcome::Mismatch(format!(
            "record identity (group_id={:?}, message_id={:?}) disagrees with key \
             (group_id={group_id:?}, message_id={message_id:?})",
            rec.group_id, rec.message_id
        ));
    }
    if rec.run_token != expected_run_token {
        return ReadOutcome::Mismatch(format!(
            "run_token mismatch (record token does not match this run's token \
             {expected_run_token:?})"
        ));
    }
    ReadOutcome::Found(rec)
}

/// Delete the record for `(group_id, message_id)` if present. Idempotent: a
/// missing record is a no-op success (the rail deletes any prior record before
/// invoking the recipe, so a fresh run never inherits a stale decision).
pub fn delete_record(state_root: &Path, group_id: &str, message_id: &str) -> Result<(), String> {
    let path = record_path(state_root, group_id, message_id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {path:?} failed: {e}")),
    }
}
