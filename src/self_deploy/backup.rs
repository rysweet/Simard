//! Dual protective backup: take BOTH a live cognitive-memory snapshot AND a
//! binary backup, together, after build + gates pass and immediately before any
//! daemon mutation. Either failure aborts the deploy loudly.
//!
//! Backups are not reinvented here — the binary backup reuses the safe-update
//! [`snapshot`](crate::safe_update::snapshot) phase (so the existing
//! [`do_rollback`](crate::safe_update::do_rollback) can restore it) and the
//! cognitive snapshot reuses
//! [`export_memory_snapshot`](crate::remote_transfer::export_memory_snapshot).
//! They are sequenced and made mandatory. See
//! `docs/reference/self-deploy-api.md#dual-protective-backup`.

use std::path::{Path, PathBuf};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::safe_update::SafeUpdateError;
use crate::safe_update::snapshot::{default_bin_dir, take_snapshot_of};
use crate::safe_update::state::DEFAULT_BACKUP_RETENTION;

/// Agent name recorded in the protective cognitive-memory snapshot.
const SNAPSHOT_AGENT: &str = "simard";

/// Paths of the two protective artifacts taken before the daemon is mutated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtectiveBackup {
    /// Path of the cognitive-memory snapshot (JSON export of the LIVE store).
    pub memory_snapshot: PathBuf,
    /// Path of the binary backup (`~/.simard/bin/simard.bak.<utc-iso8601>`).
    pub binary_backup: PathBuf,
}

/// Take BOTH backups. Returns [`SafeUpdateError::BackupFailed`] if either fails;
/// on partial success the function cleans up the partial artifact so a retry
/// starts clean. No daemon mutation is performed by this function.
///
/// Order: the **memory** snapshot first (the irreplaceable, live cognitive
/// store), then the **binary** backup (reproducible from source). If the memory
/// snapshot fails, no binary backup is taken. If the binary backup fails, the
/// just-written memory snapshot is removed so a retry is not fooled by a
/// half-written protective set.
pub fn take_protective_backup(
    mem: &dyn CognitiveMemoryOps,
    install_path: &Path,
    state_dir: &Path,
) -> Result<ProtectiveBackup, SafeUpdateError> {
    take_protective_backup_in(mem, install_path, state_dir, &default_bin_dir())
}

/// Test-friendly variant: lets the caller substitute the binary-backup
/// directory so a test never clobbers or prunes the operator's real
/// `~/.simard/bin` rollback backups.
pub(crate) fn take_protective_backup_in(
    mem: &dyn CognitiveMemoryOps,
    install_path: &Path,
    state_dir: &Path,
    bin_dir: &Path,
) -> Result<ProtectiveBackup, SafeUpdateError> {
    // 1) Live cognitive-memory snapshot → state_dir/self-deploy-memory.<utc>.json
    let memory_snapshot = take_memory_snapshot(mem, state_dir)?;

    // 2) Binary backup via the existing safe-update snapshot phase. On failure,
    //    roll back the memory snapshot so a retry starts from a clean slate.
    let snapshot = match take_snapshot_of(
        install_path,
        state_dir,
        DEFAULT_BACKUP_RETENTION,
        bin_dir.to_path_buf(),
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&memory_snapshot);
            return Err(SafeUpdateError::BackupFailed {
                which: "binary".to_string(),
                detail: e.to_string(),
            });
        }
    };

    Ok(ProtectiveBackup {
        memory_snapshot,
        binary_backup: snapshot.backup_path,
    })
}

/// Export the live cognitive store to a timestamped JSON file under `state_dir`.
/// A failure here aborts the deploy via [`SafeUpdateError::BackupFailed`].
#[allow(deprecated)] // export_memory_snapshot is the live-store export primitive
fn take_memory_snapshot(
    mem: &dyn CognitiveMemoryOps,
    state_dir: &Path,
) -> Result<PathBuf, SafeUpdateError> {
    std::fs::create_dir_all(state_dir).map_err(|e| SafeUpdateError::BackupFailed {
        which: "memory".to_string(),
        detail: format!("mkdir {}: {e}", state_dir.display()),
    })?;
    let path = state_dir.join(format!("self-deploy-memory.{}.json", now_path_safe()));
    crate::remote_transfer::export_memory_snapshot(mem, SNAPSHOT_AGENT, Some(&path)).map_err(
        |e| SafeUpdateError::BackupFailed {
            which: "memory".to_string(),
            detail: e.to_string(),
        },
    )?;
    Ok(path)
}

fn now_path_safe() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cognitive_memory::LibraryCognitiveMemory;
    use tempfile::tempdir;

    fn fake_binary(dir: &Path) -> PathBuf {
        let p = dir.join("simard");
        std::fs::write(&p, b"simard 9.9.9 fake-binary-payload").unwrap();
        p
    }

    #[test]
    fn takes_both_backups_and_returns_paths() {
        let state = tempdir().unwrap();
        let bin_dir = tempdir().unwrap();
        let bin_src = tempdir().unwrap();
        let install = fake_binary(bin_src.path());
        // A real in-memory cognitive store — the export reads the live facts.
        let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

        let backup =
            take_protective_backup_in(&mem, &install, state.path(), bin_dir.path()).unwrap();

        assert!(
            backup.memory_snapshot.exists(),
            "memory snapshot must be written"
        );
        assert!(
            backup.binary_backup.exists(),
            "binary backup must be written"
        );
        assert!(
            backup.binary_backup.starts_with(bin_dir.path()),
            "binary backup must live in the requested bin dir"
        );
    }

    #[test]
    fn memory_failure_aborts_without_binary_backup() {
        // Force the memory snapshot to fail by handing a *file* where a state
        // directory is expected: `create_dir_all` then fails → BackupFailed.
        let tmp = tempdir().unwrap();
        let not_a_dir = tmp.path().join("state-is-a-file");
        std::fs::write(&not_a_dir, b"x").unwrap();
        let bin_dir = tempdir().unwrap();
        let bin_src = tempdir().unwrap();
        let install = fake_binary(bin_src.path());
        let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

        let err =
            take_protective_backup_in(&mem, &install, &not_a_dir, bin_dir.path()).unwrap_err();
        match err {
            SafeUpdateError::BackupFailed { which, .. } => assert_eq!(which, "memory"),
            other => panic!("expected BackupFailed(memory), got {other:?}"),
        }
        // No binary backup should have been written.
        let entries: Vec<_> = std::fs::read_dir(bin_dir.path()).unwrap().collect();
        assert!(entries.is_empty(), "no binary backup on memory failure");
    }
}
