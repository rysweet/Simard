//! The Overseer's autonomous PR **rework loop** rail (issue #4911, Deliverable 2).
//!
//! This module is a THIN, deterministic rail. ALL judgment — "is this hold
//! fixable, and what exactly must change?" — lives in the `merge-readiness-judge`
//! agentic recipe and reaches the rail ONLY as a typed [`MergeVerdictRecord`]
//! carrying `reworkable = Some(true)` plus a plain-English `concern`. The rail
//! makes no independent decision about fixability. It only enforces the numeric
//! safety envelope:
//!
//!   * read the typed verdict **fail-closed** (`read_verified`);
//!   * admit a rework ONLY when it is a reworkable hold, the per-PR attempt cap is
//!     not hit, it is not a duplicate of an already-dispatched rework, and the PR
//!     is not the Overseer's OWN (the recursion guard, fail-closed when the
//!     Overseer identity is unconfigured);
//!   * on admission write the recorded concern to a durable ContextFile (never
//!     argv → no E2BIG) and return [`Intervention::ReworkPr`], bumping a durable,
//!     MONOTONIC per-PR attempt counter;
//!   * at the cap — or on corrupt attempt state — return
//!     [`Intervention::Escalate`] (the human backstop), never another rework;
//!   * otherwise a no-op [`ReworkOutcome::Skip`].
//!
//! The loop itself is emergent: the SAME judge re-reviews the reworked PR on the
//! next Overseer tick. There is no bespoke state machine.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::overseer::intervention::Intervention;
use crate::stewardship::merge_verdict_store::{
    ReadOutcome, VerdictKind, read_verified, validate_repo_slug,
};

/// The three terminal shapes of one `poll_rework`.
#[derive(Debug)]
pub enum ReworkOutcome {
    /// Dispatch an autonomous rework ([`Intervention::ReworkPr`]).
    Rework(Intervention),
    /// The cap is hit (or the durable state is corrupt): hand off to a human
    /// ([`Intervention::Escalate`]) — the final backstop, never the first
    /// response to a fixable hold.
    Escalate(Intervention),
    /// No action this tick, with a diagnostic reason (not reworkable, deduped,
    /// own PR, unverified verdict, …).
    Skip(String),
}

/// Durable, monotonic per-PR attempt state. Written owner-only `0o600`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AttemptState {
    schema_version: u32,
    /// The number of distinct reworks dispatched for this PR. Never decremented.
    attempts: u32,
    updated_at: String,
    /// Fingerprint of the last-dispatched `(run_token, concern)` so an identical
    /// verdict re-observed on the next tick is deduped (one rework in flight).
    #[serde(default)]
    last_dispatch_key: Option<String>,
}

const ATTEMPT_SCHEMA_VERSION: u32 = 1;

/// Read the typed verdict fail-closed and, if it is a reworkable hold, enforce
/// the numeric safety envelope. See the module docs for the full contract.
pub fn poll_rework(
    state_root: &Path,
    repo: &str,
    pr: u32,
    pr_author: &str,
    run_token: &str,
    max_attempts: u32,
    overseer_author_login: Option<&str>,
) -> ReworkOutcome {
    // 1. Read the typed verdict fail-closed — the ONLY source of the rework
    //    decision. Anything but a matching, fresh record is a no-op.
    let rec = match read_verified(state_root, repo, pr, run_token) {
        ReadOutcome::Found(r) => r,
        ReadOutcome::Missing => {
            return ReworkOutcome::Skip("no merge verdict recorded for this run".to_string());
        }
        ReadOutcome::Mismatch(reason) => {
            return ReworkOutcome::Skip(format!("verdict not usable (fail-closed): {reason}"));
        }
    };

    // 2. The judge's recorded decision — NOT any Rust heuristic — gates the loop.
    if rec.verdict != VerdictKind::Hold {
        return ReworkOutcome::Skip("verdict is not a hold; nothing to rework".to_string());
    }
    if rec.reworkable != Some(true) {
        return ReworkOutcome::Skip(
            "hold is not marked reworkable by the judge (fail-closed)".to_string(),
        );
    }
    let concern = match rec.concern.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => {
            return ReworkOutcome::Skip("reworkable hold carries no concern to act on".to_string());
        }
    };

    // 3. Recursion / own-PR guard (fail CLOSED): without a configured, DISTINCT
    //    Overseer identity the rail cannot prove the PR is foreign, so it refuses.
    let overseer = match overseer_author_login {
        Some(o) if !o.trim().is_empty() => o.trim(),
        _ => {
            return ReworkOutcome::Skip(
                "unconfigured Overseer identity — cannot prove the PR is foreign (fail-closed)"
                    .to_string(),
            );
        }
    };
    if pr_author == overseer {
        return ReworkOutcome::Skip(
            "refusing to rework the Overseer's own PR (recursion guard)".to_string(),
        );
    }

    // 4. Durable attempt state. Corrupt state escalates rather than looping.
    let state = match read_attempt_state(state_root, repo, pr) {
        Ok(s) => s,
        Err(reason) => {
            return ReworkOutcome::Escalate(escalate(
                repo,
                pr,
                &concern,
                &format!("corrupt rework attempt state ({reason})"),
            ));
        }
    };

    let dispatch_key = dispatch_key(run_token, &concern);

    // Dedup: an identical rework (same token + concern) already dispatched is
    // still in flight — do not relaunch it.
    if let Some(s) = &state
        && s.last_dispatch_key.as_deref() == Some(dispatch_key.as_str())
    {
        return ReworkOutcome::Skip(
            "identical rework already dispatched (in-flight dedup)".to_string(),
        );
    }

    let attempts = state.as_ref().map(|s| s.attempts).unwrap_or(0);

    // Cap: a new rework beyond the cap escalates to a human (final backstop).
    if attempts >= max_attempts {
        return ReworkOutcome::Escalate(escalate(
            repo,
            pr,
            &concern,
            &format!("rework attempt cap ({max_attempts}) reached"),
        ));
    }

    // 5. Admit: write the concern to a durable ContextFile (never argv) and bump
    //    the monotonic counter, then dispatch the reused rework intervention.
    let concern_path = match write_concern_file(state_root, repo, pr, &concern) {
        Ok(p) => p,
        Err(reason) => {
            return ReworkOutcome::Skip(format!(
                "could not stage the rework concern file (retry next tick): {reason}"
            ));
        }
    };
    if let Err(reason) = write_attempt_state(state_root, repo, pr, attempts + 1, &dispatch_key) {
        return ReworkOutcome::Skip(format!(
            "could not persist rework attempt state (retry next tick): {reason}"
        ));
    }

    ReworkOutcome::Rework(Intervention::ReworkPr {
        repo: repo.to_string(),
        pr,
        concern_path: concern_path.to_string_lossy().into_owned(),
    })
}

/// Build the human-backstop escalation carrying the plain-English concern.
fn escalate(repo: &str, pr: u32, concern: &str, why: &str) -> Intervention {
    Intervention::Escalate {
        reason: format!(
            "Autonomous rework of {repo}#{pr} handed off to a human: {why}. \
             Outstanding concern: {concern}"
        ),
    }
}

/// Deterministic per-PR attempt-state path:
/// `<state_root>/overseer/rework_attempts/<owner__name>/<pr>.json`.
fn attempt_state_path(state_root: &Path, repo: &str, pr: u32) -> Result<PathBuf, String> {
    let (owner, name) = validate_repo_slug(repo)?;
    Ok(state_root
        .join("overseer")
        .join("rework_attempts")
        .join(format!("{owner}__{name}"))
        .join(format!("{pr}.json")))
}

/// Deterministic per-PR concern ContextFile path:
/// `<state_root>/overseer/rework_concerns/<owner__name>/<pr>.txt`. Durable (the
/// dispatched intervention references it), owner-only `0o600`.
fn concern_file_path(state_root: &Path, repo: &str, pr: u32) -> Result<PathBuf, String> {
    let (owner, name) = validate_repo_slug(repo)?;
    Ok(state_root
        .join("overseer")
        .join("rework_concerns")
        .join(format!("{owner}__{name}"))
        .join(format!("{pr}.txt")))
}

/// Fingerprint a `(run_token, concern)` pair into a compact, path-free key used
/// for in-flight dedup.
fn dispatch_key(run_token: &str, concern: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(run_token.as_bytes());
    hasher.update([0u8]);
    hasher.update(concern.as_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Read the durable attempt state. `Ok(None)` when absent; `Err` when a record
/// exists but is unreadable/corrupt (⇒ escalate, never loop forever).
fn read_attempt_state(
    state_root: &Path,
    repo: &str,
    pr: u32,
) -> Result<Option<AttemptState>, String> {
    let path = attempt_state_path(state_root, repo, pr)?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            let state: AttemptState = serde_json::from_slice(&bytes)
                .map_err(|e| format!("malformed attempt state json: {e}"))?;
            if state.schema_version != ATTEMPT_SCHEMA_VERSION {
                return Err(format!(
                    "unknown attempt-state schema_version {}",
                    state.schema_version
                ));
            }
            Ok(Some(state))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {path:?} failed: {e}")),
    }
}

/// Atomically persist the monotonic attempt counter + last dispatch key.
fn write_attempt_state(
    state_root: &Path,
    repo: &str,
    pr: u32,
    attempts: u32,
    dispatch_key: &str,
) -> Result<(), String> {
    let path = attempt_state_path(state_root, repo, pr)?;
    let rec = AttemptState {
        schema_version: ATTEMPT_SCHEMA_VERSION,
        attempts,
        updated_at: chrono::Utc::now().to_rfc3339(),
        last_dispatch_key: Some(dispatch_key.to_string()),
    };
    let json =
        serde_json::to_vec_pretty(&rec).map_err(|e| format!("serialize attempt state: {e}"))?;
    atomic_write(&path, &json)
}

/// Write the recorded concern to its durable ContextFile and return the path.
fn write_concern_file(
    state_root: &Path,
    repo: &str,
    pr: u32,
    concern: &str,
) -> Result<PathBuf, String> {
    let path = concern_file_path(state_root, repo, pr)?;
    atomic_write(&path, concern.as_bytes())?;
    Ok(path)
}

/// Atomic temp-write + rename, creating parent dirs, owner-only `0o600`.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("path {path:?} has no parent directory"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all {dir:?} failed: {e}"))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".rework.tmp.{}.{nanos}", std::process::id()));
    {
        let mut f =
            std::fs::File::create(&tmp).map_err(|e| format!("create temp {tmp:?} failed: {e}"))?;
        f.write_all(bytes)
            .map_err(|e| format!("write temp {tmp:?} failed: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("fsync temp {tmp:?} failed: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("chmod 0o600 temp {tmp:?} failed: {e}"))?;
        }
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {tmp:?} -> {path:?} failed: {e}")
    })
}

/// One held PR the [`ReworkPort`] surfaced this tick as a rework candidate,
/// already projected to exactly the fields [`poll_rework`] needs. The port
/// (production) lists the Overseer's held-with-reason PRs and the `run_token`
/// the judge stamped on each, so the rail can read the typed verdict fail-closed.
#[derive(Debug, Clone)]
pub struct ReworkCandidate {
    /// Validated `owner/name` repo slug.
    pub repo: String,
    /// The PR number.
    pub pr: u32,
    /// The PR's author login (the recursion guard refuses the Overseer's own).
    pub pr_author: String,
    /// The freshness token the judge stamped on this PR's verdict record.
    pub run_token: String,
}

/// The external-I/O seam for the rework loop: enumerate the held PRs that MIGHT
/// be reworkable this tick, paired with the judge's freshness token. The rail
/// keeps ALL judgment out of Rust — it only reads each candidate's typed verdict
/// fail-closed via [`poll_rework`]. This mirrors the established
/// `ecosystem_observe::EcosystemObserver` seam and is `None` (inert) until
/// `build_overseer` wires the production implementation.
pub trait ReworkPort: Send + Sync {
    /// The held-with-reason PRs to evaluate for autonomous rework this tick.
    fn candidates(&self) -> Vec<ReworkCandidate>;
}
