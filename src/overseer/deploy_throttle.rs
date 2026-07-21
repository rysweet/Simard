//! Durable, fail-closed self-deploy anti-thrash ledger (#4390).
//!
//! The process-global min-interval guard in [`crate::overseer::deploy_trigger`]
//! ([`global_deploy_throttle_allow`](crate::overseer::deploy_trigger::global_deploy_throttle_allow))
//! is commit-agnostic and lives in a process `static`, so it forgets *which*
//! commit failed and resets on every overseer restart. That is exactly the seam
//! behind the observed thrash: commit `56b10bef5057` failed the canary
//! `deploy_gate` on five consecutive ticks and was re-attempted every tick
//! because the daemon never remembered — across a restart — that this specific
//! SHA was known-bad.
//!
//! [`DeployAttemptLedger`] closes that seam. It records, **per target SHA**, the
//! consecutive-failure count and an exponential (capped) `backoff_until`,
//! persisted atomically to `deploy-attempt-ledger.json` so it **survives an
//! overseer restart**. It is **fail-closed per-SHA**: a ledger this tick cannot
//! trust for the candidate SHA (corrupt/unknown-version file, or a record present
//! with no terminal result) refuses that SHA rather than re-admitting it. A
//! *missing* file loads empty and `Allow`s, so the guard can never deadlock the
//! literal first-ever deploy.
//!
//! See [`docs/reference/overseer-deploy-throttle-api.md`] for the full contract.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::overseer::deploy_trigger::deploy_min_interval_secs;

/// File name of the durable ledger inside the state dir.
const LEDGER_FILE_NAME: &str = "deploy-attempt-ledger.json";

/// Current on-disk schema version. A greater/unknown version loads poisoned
/// (fail-closed) rather than being silently migrated.
const SCHEMA_VERSION: u32 = 1;

/// Fixed cap on the exponential backoff window: 6 hours. Not operator-tunable —
/// the effective aggressiveness is lowered via the backoff `base`
/// (`SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS`) instead.
pub const DEPLOY_BACKOFF_CAP_SECS: u64 = 6 * 60 * 60;

/// Upper bound on the number of per-SHA records retained. The oldest terminal
/// records are evicted first so the ledger cannot grow without bound.
const MAX_ENTRIES: usize = 256;

/// The [`DeployAttemptLedger`]'s verdict for one candidate target SHA. A pure
/// function of durable ledger state and `now` — it needs no live "is-the-canary-
/// red" signal because the ledger *is* the durable memory of a past red canary.
/// Fails closed per-SHA: a ledger this tick cannot trust for the candidate SHA
/// yields [`FailClosed`](ThrottleDecision::FailClosed), never
/// [`Allow`](ThrottleDecision::Allow).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThrottleDecision {
    /// The SHA is new (no record ⇒ never attempted) or its backoff window has
    /// elapsed — admit the deploy attempt.
    Allow,
    /// The SHA failed a recent attempt and is inside its exponential backoff
    /// window. Suppress the attempt until `retry_after_unix_secs`.
    BackingOff {
        /// The SHA being suppressed.
        target_sha: String,
        /// Consecutive canary/deploy failures recorded for this SHA.
        failure_count: u32,
        /// Epoch-seconds after which this SHA becomes eligible again.
        retry_after_unix_secs: u64,
    },
    /// The ledger could not be trusted for the candidate SHA (corrupt/unreadable
    /// file, or a record present but with no terminal result). Refuse the deploy
    /// and surface the stuck state.
    FailClosed {
        /// The SHA being refused.
        target_sha: String,
        /// Why the ledger declined to admit (for the surfaced warning).
        reason: FailClosedReason,
    },
}

/// Why a [`ThrottleDecision::FailClosed`] decision was reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailClosedReason {
    /// Ledger file present but unreadable / deserialize error / unknown schema
    /// version (torn or corrupt state). A **missing** file is not this — it loads
    /// an empty ledger and yields `Allow` — so `Unreadable` can only occur once at
    /// least one attempt has already been persisted, and therefore never blocks
    /// the literal first-ever deploy.
    Unreadable,
    /// A record exists for the SHA (so it *was* attempted) but its
    /// `last_deploy_result` is unset — the outcome is ambiguous, so don't
    /// re-attempt it.
    Ambiguous,
}

/// Terminal outcome of the last recorded deploy attempt for a SHA.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DeployResult {
    Failed,
    Succeeded,
}

/// One durable per-SHA record.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LedgerEntry {
    /// Consecutive canary/deploy failures for this SHA. Reset to `0` on success.
    failure_count: u32,
    /// Epoch-seconds of the last recorded attempt.
    last_attempt_unix_secs: u64,
    /// Epoch-seconds before which the SHA is suppressed (`BackingOff`).
    backoff_until_unix_secs: u64,
    /// Terminal result of the last attempt. **Unset ⇒ ambiguous ⇒ `FailClosed`.**
    #[serde(default)]
    last_deploy_result: Option<DeployResult>,
}

impl LedgerEntry {
    /// A fresh record for a SHA seen for the first time this tick: zero failures,
    /// no backoff, and no terminal result yet (the caller sets the outcome).
    fn new(now_secs: u64) -> Self {
        Self {
            failure_count: 0,
            last_attempt_unix_secs: now_secs,
            backoff_until_unix_secs: now_secs,
            last_deploy_result: None,
        }
    }
}

/// The serialized ledger file shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LedgerFile {
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, LedgerEntry>,
}

/// Durable, per-target-SHA anti-thrash ledger for the autonomous self-deploy rail
/// (#4390). Persisted to `<state_dir>/deploy-attempt-ledger.json` (atomic
/// tmp+rename, `0600`) so a red-canary commit is not re-attempted every tick even
/// across an overseer restart. Fail-closed per-SHA.
#[derive(Clone, Debug)]
pub struct DeployAttemptLedger {
    /// State dir the ledger persists under (records nothing until `record_*`).
    state_dir: PathBuf,
    /// Live records keyed by full target SHA.
    entries: BTreeMap<String, LedgerEntry>,
    /// A present-but-corrupt/unknown-version file loads poisoned: `consult`
    /// refuses the candidate SHA (`FailClosed(Unreadable)`) rather than silently
    /// re-admitting a commit that had already been persisted as bad.
    poisoned: bool,
}

impl DeployAttemptLedger {
    /// Load the ledger from `state_dir`. A **missing** file loads an empty ledger
    /// (a first-ever run is not an error, and yields `Allow`). A present-but-
    /// corrupt or unknown-schema-version file loads a `poisoned` ledger that
    /// returns `FailClosed(Unreadable)` for the candidate SHA. Never panics on IO
    /// or deserialize.
    pub fn load(state_dir: &Path) -> Self {
        let path = Self::ledger_path(state_dir);
        let mut ledger = Self {
            state_dir: state_dir.to_path_buf(),
            entries: BTreeMap::new(),
            poisoned: false,
        };

        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            // A missing file is the first-ever run: stay empty (⇒ Allow).
            Err(e) if e.kind() == io::ErrorKind::NotFound => return ledger,
            // Any other IO error (unreadable perms, etc.) is a present-but-
            // untrusted file: fail closed.
            Err(_) => {
                ledger.poisoned = true;
                return ledger;
            }
        };

        match serde_json::from_slice::<LedgerFile>(&bytes) {
            Ok(file) if file.version == SCHEMA_VERSION => {
                ledger.entries = file
                    .entries
                    .into_iter()
                    .filter(|(sha, _)| is_full_hex_sha(sha))
                    .collect();
            }
            // Corrupt JSON, or a known file shape at an unknown schema version:
            // never silently migrate — load poisoned (fail-closed).
            _ => ledger.poisoned = true,
        }
        ledger
    }

    /// Path of the ledger file inside `state_dir`
    /// (`<state_dir>/deploy-attempt-ledger.json`).
    pub fn ledger_path(state_dir: &Path) -> PathBuf {
        state_dir.join(LEDGER_FILE_NAME)
    }

    /// Consult the ledger for `target_sha` at `now_secs`. A pure, read-only
    /// function of durable ledger state — records nothing and needs no live
    /// red-canary signal (the ledger record *is* that memory).
    pub fn consult(&self, target_sha: &str, now_secs: u64) -> ThrottleDecision {
        // Poisoned durable state cannot be trusted for the in-flight SHA: refuse
        // *this* candidate (per-SHA, never a global deadlock).
        if self.poisoned {
            return ThrottleDecision::FailClosed {
                target_sha: target_sha.to_string(),
                reason: FailClosedReason::Unreadable,
            };
        }

        let Some(entry) = self.entries.get(target_sha) else {
            // Never seen ⇒ never attempted ⇒ admit (first deploy of a new SHA).
            return ThrottleDecision::Allow;
        };

        match entry.last_deploy_result {
            // A record with no terminal result is ambiguous: it *was* attempted
            // but we don't know the outcome — don't re-attempt it, even once the
            // recorded backoff window has elapsed.
            None => ThrottleDecision::FailClosed {
                target_sha: target_sha.to_string(),
                reason: FailClosedReason::Ambiguous,
            },
            // A green commit is immediately eligible again.
            Some(DeployResult::Succeeded) => ThrottleDecision::Allow,
            // A failed commit is suppressed until its backoff window elapses
            // (inclusive boundary: eligible once now >= backoff_until).
            Some(DeployResult::Failed) => {
                if now_secs >= entry.backoff_until_unix_secs {
                    ThrottleDecision::Allow
                } else {
                    ThrottleDecision::BackingOff {
                        target_sha: target_sha.to_string(),
                        failure_count: entry.failure_count,
                        retry_after_unix_secs: entry.backoff_until_unix_secs,
                    }
                }
            }
        }
    }

    /// Record a failed deploy of `target_sha`: increments `failure_count`, sets
    /// `last_attempt_unix_secs = now_secs`, computes the next
    /// `backoff_until_unix_secs` (exponential, capped), and persists atomically.
    pub fn record_failure(&mut self, target_sha: &str, now_secs: u64) -> io::Result<()> {
        // A record clears the poisoned flag: we now hold a known, valid state and
        // overwrite the untrusted file with it.
        self.poisoned = false;
        let entry = self
            .entries
            .entry(target_sha.to_string())
            .or_insert_with(|| LedgerEntry::new(now_secs));
        entry.failure_count = entry.failure_count.saturating_add(1);
        entry.last_attempt_unix_secs = now_secs;
        entry.backoff_until_unix_secs = now_secs.saturating_add(backoff_secs(entry.failure_count));
        entry.last_deploy_result = Some(DeployResult::Failed);

        self.evict_if_over_cap(target_sha);
        self.persist()
    }

    /// Record a successful deploy of `target_sha`: clears the SHA's failure count
    /// and backoff (a green commit is immediately eligible again) and persists
    /// atomically. Idempotent.
    pub fn record_success(&mut self, target_sha: &str, now_secs: u64) -> io::Result<()> {
        self.poisoned = false;
        let entry = self
            .entries
            .entry(target_sha.to_string())
            .or_insert_with(|| LedgerEntry::new(now_secs));
        entry.failure_count = 0;
        entry.last_attempt_unix_secs = now_secs;
        entry.backoff_until_unix_secs = now_secs;
        entry.last_deploy_result = Some(DeployResult::Succeeded);

        self.evict_if_over_cap(target_sha);
        self.persist()
    }

    /// Bound the entry map: when over the cap, drop the oldest terminal records
    /// first (never the SHA just touched).
    fn evict_if_over_cap(&mut self, keep: &str) {
        while self.entries.len() > MAX_ENTRIES {
            let victim = self
                .entries
                .iter()
                .filter(|(sha, _)| sha.as_str() != keep)
                .min_by_key(|(_, e)| e.last_attempt_unix_secs)
                .map(|(sha, _)| sha.clone());
            match victim {
                Some(sha) => {
                    self.entries.remove(&sha);
                }
                None => break,
            }
        }
    }

    /// Persist the ledger atomically (write sibling tmp, `0600`, then `rename`).
    fn persist(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        harden_dir_perms(&self.state_dir);

        let path = Self::ledger_path(&self.state_dir);
        refuse_symlink(&path)?;

        let tmp = self.state_dir.join(format!("{LEDGER_FILE_NAME}.tmp"));
        let file = LedgerFile {
            version: SCHEMA_VERSION,
            entries: self.entries.clone(),
        };
        let body = serde_json::to_vec_pretty(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        write_private(&tmp, &body)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// The exponential backoff window after `failure_count` consecutive failures:
/// `min(base * 2^(n-1), cap)`, where `base` is the resolved deploy min-interval
/// and `cap` is [`DEPLOY_BACKOFF_CAP_SECS`].
fn backoff_secs(failure_count: u32) -> u64 {
    let base = deploy_min_interval_secs();
    let shift = failure_count.saturating_sub(1).min(63);
    let grown = base.saturating_mul(1u64 << shift);
    grown.min(DEPLOY_BACKOFF_CAP_SECS)
}

/// The documented trust shape for an autonomous deploy target: a fully-resolved
/// 40/64-char lowercase hex SHA (mirrors `deploy_trigger::is_full_hex_sha`).
fn is_full_hex_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Refuse to write through a symlink at the target path (defense against a
/// planted link that would redirect the durable write).
fn refuse_symlink(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "refusing to write ledger through a symlink",
        )),
        _ => Ok(()),
    }
}

/// Write `bytes` to `path`, owner-only (`0600`) on unix.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        // Enforce 0600 even if the file pre-existed with looser perms.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

/// Best-effort tighten the state dir to `0700` on unix.
fn harden_dir_perms(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}
