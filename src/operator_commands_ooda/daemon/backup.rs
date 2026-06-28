//! Periodic verified backup of the LIVE cognitive store (issue #2420).
//!
//! The OODA daemon opens its cognitive store through the library backend at
//! `state_root/cognitive` (see [`crate::cognitive_memory::live_store_path`]).
//! The de-fork (#2307) removed the old file-copy backup, and the surviving
//! file-copy backup targeted the stale legacy path — so no fresh verified
//! backup was produced from Jun 20 onward while the live store grew past 10k
//! memories.
//!
//! This module reintroduces a periodic backup as a **verified** routine: it
//! snapshots the live store through the bridge (inherently the migrated path),
//! re-opens and verifies the backup before pruning, and only then trims old
//! backups. It is best-effort — a failure is surfaced loudly to the caller and
//! never aborts the OODA cycle.
//!
//! The execution is split out as a pure function ([`run_verified_backup`]) plus
//! an env parser ([`backup_interval_secs_from_env`]) so both are unit-testable
//! without driving the full 988-line daemon loop.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::meeting_backend::persist::MEMORY_RECORDS_FILENAME;
use crate::memory::FileBackedMemoryStore;
use crate::memory_backup::{
    BackupConfig, BackupManifest, backup_memory_verified, prune_old_backups,
};

/// Agent label stamped on backup snapshots. Matches the live store's agent
/// (`LIBRARY_AGENT_NAME` / `SNAPSHOT_AGENT`), so a restored snapshot is
/// attributed to the same agent that produced it.
const BACKUP_AGENT: &str = "simard";

// File-backed memory records live next to the cognitive store at
// `state_root/`[`MEMORY_RECORDS_FILENAME`] — the single canonical filename
// `simard meeting read` also writes/reads (`meeting_backend::persist`). Imported
// rather than re-declared so the backup and the writer can never disagree.

/// Default backup interval: once per day. The live store changes continuously
/// but a daily verified backup plus the library backend's own WAL durability is
/// an adequate floor; operators tighten it with `SIMARD_BACKUP_INTERVAL_SECS`.
pub const DEFAULT_BACKUP_INTERVAL_SECS: u64 = 86_400;

/// Parse the verified-backup interval (seconds) from the
/// `SIMARD_BACKUP_INTERVAL_SECS` env value, falling back to
/// [`DEFAULT_BACKUP_INTERVAL_SECS`].
///
/// A missing, unparseable, or zero value falls back to the default: zero would
/// busy-loop the backup every cycle, which is never intended.
pub fn backup_interval_secs_from_env(raw: Option<&str>) -> u64 {
    match raw.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(n) if n > 0 => n,
        _ => DEFAULT_BACKUP_INTERVAL_SECS,
    }
}

/// Whether a verified backup is due, given when the last one ran (issue #2420).
///
/// `last` is `None` until the first backup completes, so the **first** call
/// always returns `true` — the daemon backs up within its first cycle after
/// start, regardless of host uptime. Thereafter it returns `true` once
/// `interval_secs` has elapsed since `last`.
///
/// Extracted (and tested) so the "first backup runs promptly" guarantee cannot
/// regress: deriving the initial state from a monotonic-clock back-date instead
/// would silently defer the first backup a full interval on any freshly-booted
/// host (Linux `Instant` is boot-relative), which is exactly the post-restart
/// window this feature protects.
pub fn should_run_backup(last: Option<Instant>, interval_secs: u64) -> bool {
    last.is_none_or(|t| t.elapsed() >= Duration::from_secs(interval_secs))
}

/// Build the backup configuration rooted at `state_root` (issue #2420).
///
/// The backup directory is `state_root/backups` so it tracks the same root the
/// daemon was launched with (production `~/.simard`, a `TempDir` in tests),
/// rather than [`BackupConfig::default`]'s home-relative path which would ignore
/// a `state_root` override.
fn backup_config_for(state_root: &Path) -> BackupConfig {
    BackupConfig {
        backup_dir: state_root.join("backups"),
        ..BackupConfig::default()
    }
}

/// Produce one verified backup of the live store under `state_root`, then prune
/// old backups (issue #2420).
///
/// Steps:
/// 1. Snapshot the live cognitive store (`bridge`) plus the file-backed records
///    at `state_root/memory_records.json` into `state_root/backups/<ts>/`.
/// 2. Re-open and verify the backup (checksum + manifest self-consistency) —
///    [`backup_memory_verified`] returns `Err` if it does not verify.
/// 3. Only on a verified backup, prune old backups (retention).
///
/// Returns the verified manifest. Any failure is returned as `Err` so the caller
/// logs it and skips the prune, leaving prior good backups intact.
pub fn run_verified_backup(
    bridge: &dyn CognitiveMemoryOps,
    state_root: &Path,
) -> SimardResult<BackupManifest> {
    let records_path = state_root.join(MEMORY_RECORDS_FILENAME);
    let file_store = FileBackedMemoryStore::try_new(&records_path)?;
    let config = backup_config_for(state_root);

    let manifest = backup_memory_verified(bridge, &file_store, BACKUP_AGENT, &config)?;

    // Prune only AFTER a verified backup so a failed/partial backup can never
    // cause the last-known-good backup to be reclaimed.
    prune_old_backups(&config)?;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cognitive_memory::LibraryCognitiveMemory;
    use crate::memory_backup::{BackupStatus, verify_backup};

    #[test]
    fn interval_defaults_when_missing_or_invalid() {
        assert_eq!(
            backup_interval_secs_from_env(None),
            DEFAULT_BACKUP_INTERVAL_SECS
        );
        assert_eq!(
            backup_interval_secs_from_env(Some("not-a-number")),
            DEFAULT_BACKUP_INTERVAL_SECS
        );
        // Zero would busy-loop — must fall back, not be honoured.
        assert_eq!(
            backup_interval_secs_from_env(Some("0")),
            DEFAULT_BACKUP_INTERVAL_SECS
        );
    }

    #[test]
    fn interval_parses_positive_override() {
        assert_eq!(backup_interval_secs_from_env(Some("3600")), 3600);
        assert_eq!(backup_interval_secs_from_env(Some("  120  ")), 120);
    }

    /// The first backup (no prior run) must always be due, so a freshly
    /// started/rebooted daemon backs up within its first cycle rather than
    /// deferring a full interval.
    #[test]
    fn first_backup_is_always_due() {
        assert!(
            should_run_backup(None, DEFAULT_BACKUP_INTERVAL_SECS),
            "the first backup must run immediately regardless of interval"
        );
    }

    /// A backup that just ran is not due again until the interval elapses.
    #[test]
    fn recent_backup_is_not_due() {
        assert!(
            !should_run_backup(Some(Instant::now()), DEFAULT_BACKUP_INTERVAL_SECS),
            "a just-completed backup must not immediately re-run"
        );
        // A zero interval makes every cycle due (even a just-run backup).
        assert!(should_run_backup(Some(Instant::now()), 0));
    }

    /// A live store with facts produces a verified backup under
    /// `state_root/backups`, and that backup verifies `Valid` on disk.
    #[test]
    fn run_verified_backup_produces_valid_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let state_root = tmp.path();

        let bridge = LibraryCognitiveMemory::in_memory().unwrap();
        bridge
            .store_fact("rust", "memory-safe", 0.9, &[], "ep1")
            .unwrap();
        bridge
            .store_fact("backups", "must be verified", 0.95, &[], "ep2")
            .unwrap();

        let manifest = run_verified_backup(&bridge, state_root).expect("verified backup");

        assert_eq!(manifest.cognitive_facts_count, 2);
        assert!(
            manifest.backup_dir.starts_with(state_root.join("backups")),
            "backup must land under state_root/backups"
        );

        let v = verify_backup(&manifest.backup_dir).unwrap();
        assert!(
            matches!(v.status, BackupStatus::Valid),
            "daemon-produced backup must be Valid on disk"
        );
    }

    /// An empty live store still produces a verified (empty) backup rather than
    /// erroring — the gate is about integrity, not non-emptiness.
    #[test]
    fn run_verified_backup_on_empty_store_is_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let bridge = LibraryCognitiveMemory::in_memory().unwrap();

        let manifest = run_verified_backup(&bridge, tmp.path()).expect("verified empty backup");
        assert_eq!(manifest.cognitive_facts_count, 0);

        let v = verify_backup(&manifest.backup_dir).unwrap();
        assert!(matches!(v.status, BackupStatus::Valid));
    }
}
