//! Durable, typed merge-verdict record store (issue #4721, WS-2).
//!
//! This module is the transport that replaces the forbidden
//! "recipe emits JSON → Rust scrapes stdout → Rust acts" pattern. The
//! agent-facing `simard merge record-verdict` tool WRITES a typed
//! [`MergeVerdictRecord`] here; the thin deterministic rail
//! ([`super::recipe_merge_judge`]) READS it back — freshness- and
//! identity-checked — instead of parsing prose. See
//! `docs/reference/merge-record-verdict-cli.md` for the full contract.
//!
//! ## Safety properties
//!
//! - **Deterministic, traversal-safe path.** A record for `(repo, pr)` lives at
//!   `<state_root>/merge_verdicts/<owner__name>/<pr>.json`. The repo slug is
//!   validated (`validate_repo_slug`) so a malicious `--repo` can never escape
//!   the `merge_verdicts/` subtree.
//! - **Atomic write.** [`write_record`] writes to a temp sibling then `rename`s
//!   over the final path, so a reader never observes a half-written record and
//!   no temp files are left beside it.
//! - **Fail-closed read.** [`read_verified`] is a total function — it never
//!   panics on malformed JSON and returns [`ReadOutcome::Mismatch`] on an
//!   unknown `schema_version`, a `(repo, pr)` identity disagreement, or a
//!   `run_token` mismatch. A safety-critical rail must never act on a stale,
//!   foreign, or corrupt verdict.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The only record schema this build understands. A record carrying any other
/// value is treated as un-trusted and fails closed on read (forward/backward
/// compatibility is intentionally NOT silent).
pub const SCHEMA_VERSION: u32 = 1;

/// The two typed verdicts the agent may record. Serialized lowercase
/// (`"merge"` / `"hold"`) so the on-disk form matches the CLI vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictKind {
    /// The agent judged the change merge-ready (advisory — the rail still
    /// independently re-verifies the hard safety gates before authorizing).
    Merge,
    /// The agent is holding the merge (a real defect / concern). The rail maps
    /// this to a non-merge verdict unconditionally.
    Hold,
}

/// A durably-recorded, freshness-tokened merge verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeVerdictRecord {
    /// Record schema version. Read paths fail closed on any value other than
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    pub pr: u32,
    pub repo: String,
    pub verdict: VerdictKind,
    /// A bounded, human-readable rationale (kept concise; large payloads go via
    /// files, never here).
    pub reason: String,
    /// RFC3339 UTC instant the record was stamped.
    pub recorded_at: String,
    /// Opaque single-run token. The rail generates a fresh token per run and
    /// requires the record to echo it, so a record from a previous run can
    /// never be mistaken for this run's verdict.
    pub run_token: String,
    /// Deliverable 2 (rework loop): `Some(true)` marks a *fixable* hold the
    /// autonomous rework rail may dispatch. **Absent (old records) or
    /// `Some(false)` ⇒ NOT reworkable** (fail-closed). `#[serde(default)]` keeps
    /// records written before this field existed readable (they deserialize to
    /// `None`), so `SCHEMA_VERSION` stays `1`.
    #[serde(default)]
    pub reworkable: Option<bool>,
    /// Deliverable 2: a concise plain-English description of exactly what the
    /// rework must change. Handed to the rework recipe via a ContextFile (never
    /// argv). Absent ⇒ `None`.
    #[serde(default)]
    pub concern: Option<String>,
}

impl MergeVerdictRecord {
    /// Build a record for the current run, stamping [`SCHEMA_VERSION`] and an
    /// RFC3339 UTC `recorded_at` internally.
    pub fn new(
        pr: u32,
        repo: &str,
        verdict: VerdictKind,
        reason: &str,
        run_token: &str,
    ) -> MergeVerdictRecord {
        MergeVerdictRecord {
            schema_version: SCHEMA_VERSION,
            pr,
            repo: repo.to_string(),
            verdict,
            reason: reason.to_string(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            run_token: run_token.to_string(),
            reworkable: None,
            concern: None,
        }
    }
}

/// The result of a freshness/identity-checked read. A total function's whole
/// codomain — the rail matches on all three arms and treats anything but
/// [`ReadOutcome::Found`] as "no valid verdict for this run".
#[derive(Debug)]
pub enum ReadOutcome {
    /// A record that matched the requested `(repo, pr, run_token)` and carries a
    /// known `schema_version`.
    Found(MergeVerdictRecord),
    /// No record file exists for `(repo, pr)`.
    Missing,
    /// A record exists but is stale/foreign/corrupt: malformed JSON, unknown
    /// `schema_version`, `(repo, pr)` identity disagreement, or `run_token`
    /// mismatch. The `String` names the reason for operator diagnostics.
    Mismatch(String),
}

/// Validate an `owner/name` repo slug and return its two components.
///
/// Rejects anything that could escape the `merge_verdicts/` subtree or is not a
/// well-formed slug: NUL bytes, a component count other than two, empty
/// components, `.`/`..` components, embedded path separators, or whitespace.
/// This is the single guard both [`record_path`] (store side) and the CLI
/// parser (`operator_cli::merge`) share.
pub fn validate_repo_slug(repo: &str) -> Result<(&str, &str), String> {
    if repo.contains('\0') {
        return Err(format!("repo {repo:?} contains a NUL byte"));
    }
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(format!(
            "repo {repo:?} must be exactly <owner>/<name> (one '/')"
        ));
    }
    for comp in &parts {
        if comp.is_empty()
            || *comp == "."
            || *comp == ".."
            || comp.contains(['/', '\\'])
            || comp.chars().any(char::is_whitespace)
        {
            return Err(format!(
                "repo {repo:?} has an invalid path component {comp:?}"
            ));
        }
    }
    Ok((parts[0], parts[1]))
}

/// Deterministic, traversal-safe path for the `(repo, pr)` record:
/// `<state_root>/merge_verdicts/<owner__name>/<pr>.json`.
pub fn record_path(state_root: &Path, repo: &str, pr: u32) -> Result<PathBuf, String> {
    let (owner, name) = validate_repo_slug(repo)?;
    let sanitized = format!("{owner}__{name}");
    Ok(state_root
        .join("merge_verdicts")
        .join(sanitized)
        .join(format!("{pr}.json")))
}

/// Atomically write `rec` to its deterministic path, creating parent dirs.
///
/// Delegates the temp-write + `rename` + owner-only `0o600` to the shared
/// [`super::record_io::atomic_write_0600`], so a concurrent reader never sees a
/// partial record and no temp file is left beside the record.
pub fn write_record(state_root: &Path, rec: &MergeVerdictRecord) -> Result<(), String> {
    let path = record_path(state_root, &rec.repo, rec.pr)?;
    let json =
        serde_json::to_vec_pretty(rec).map_err(|e| format!("serialize verdict record: {e}"))?;
    super::record_io::atomic_write_0600(&path, &json)
}

/// Read the record for `(repo, pr)` and verify it belongs to THIS run.
///
/// Total function: never panics. Returns [`ReadOutcome::Missing`] when no
/// record file exists, [`ReadOutcome::Mismatch`] when a record exists but is
/// unreadable / unknown-schema / identity-mismatched / stale-tokened, and
/// [`ReadOutcome::Found`] only when everything checks out.
pub fn read_verified(
    state_root: &Path,
    repo: &str,
    pr: u32,
    expected_run_token: &str,
) -> ReadOutcome {
    let path = match record_path(state_root, repo, pr) {
        Ok(p) => p,
        Err(e) => return ReadOutcome::Mismatch(e),
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadOutcome::Missing,
        Err(e) => return ReadOutcome::Mismatch(format!("read {path:?} failed: {e}")),
    };
    let rec: MergeVerdictRecord = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(e) => return ReadOutcome::Mismatch(format!("malformed record json: {e}")),
    };
    if rec.schema_version != SCHEMA_VERSION {
        return ReadOutcome::Mismatch(format!(
            "unknown schema_version {} (expected {SCHEMA_VERSION})",
            rec.schema_version
        ));
    }
    if rec.repo != repo || rec.pr != pr {
        return ReadOutcome::Mismatch(format!(
            "record identity (repo={:?}, pr={}) disagrees with key (repo={repo:?}, pr={pr})",
            rec.repo, rec.pr
        ));
    }
    if rec.run_token != expected_run_token {
        return ReadOutcome::Mismatch(format!(
            "run_token mismatch (record token does not match this run's token {expected_run_token:?})"
        ));
    }
    ReadOutcome::Found(rec)
}

/// Delete the record for `(repo, pr)` if present. Idempotent: a missing record
/// is a no-op success (the rail deletes any prior record before invoking the
/// recipe, so a fresh run never inherits a stale verdict).
pub fn delete_record(state_root: &Path, repo: &str, pr: u32) -> Result<(), String> {
    let path = record_path(state_root, repo, pr)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {path:?} failed: {e}")),
    }
}
