//! Automated backup and verification for cognitive memory (LadybugDB).
//!
//! Provides file-copy backup with timestamped filenames and verification
//! that the backup can be opened by LadybugDB.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::error::{SimardError, SimardResult};

/// Result of a backup operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupResult {
    pub source_path: PathBuf,
    pub backup_path: PathBuf,
    pub size_bytes: u64,
    pub verified: bool,
}

/// Default backup directory: `~/.simard/backups/`.
pub fn default_backup_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".simard/backups")
}

/// Create a timestamped backup of the cognitive memory database file.
///
/// Copies the DB file to `<backup_dir>/cognitive_memory_<timestamp>.ladybug`.
/// If `backup_dir` is `None`, uses `~/.simard/backups/`.
pub fn create_backup(db_path: &Path, backup_dir: Option<&Path>) -> SimardResult<BackupResult> {
    let dir = backup_dir
        .map(PathBuf::from)
        .unwrap_or_else(default_backup_dir);

    std::fs::create_dir_all(&dir).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory-backup".into(),
        action: "create_backup_dir".into(),
        path: dir.clone(),
        reason: e.to_string(),
    })?;

    if !db_path.exists() {
        return Err(SimardError::PersistentStoreIo {
            store: "cognitive-memory-backup".into(),
            action: "check_source".into(),
            path: db_path.to_path_buf(),
            reason: "Database file does not exist.".into(),
        });
    }

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!("cognitive_memory_{timestamp}.ladybug");
    let backup_path = dir.join(&filename);

    std::fs::copy(db_path, &backup_path).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory-backup".into(),
        action: "copy".into(),
        path: backup_path.clone(),
        reason: e.to_string(),
    })?;

    let size_bytes = std::fs::metadata(&backup_path)
        .map(|m| m.len())
        .unwrap_or(0);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&backup_path, perms) {
            warn!("Failed to set backup permissions: {e}");
        }
    }

    info!(
        source = %db_path.display(),
        backup = %backup_path.display(),
        size_bytes,
        "Cognitive memory backup created"
    );

    Ok(BackupResult {
        source_path: db_path.to_path_buf(),
        backup_path,
        size_bytes,
        verified: false,
    })
}

/// Verify a backup by attempting to open it with LadybugDB.
///
/// Opens the backup file, runs a basic query, and closes it.
/// Returns the `BackupResult` with `verified = true` on success.
pub fn verify_backup(mut result: BackupResult) -> SimardResult<BackupResult> {
    let db =
        lbug::Database::new(&result.backup_path, lbug::SystemConfig::default()).map_err(|e| {
            SimardError::PersistentStoreIo {
                store: "cognitive-memory-backup".into(),
                action: "verify_open".into(),
                path: result.backup_path.clone(),
                reason: format!("Failed to open backup: {e}"),
            }
        })?;

    let conn = lbug::Connection::new(&db).map_err(|e| SimardError::PersistentStoreIo {
        store: "cognitive-memory-backup".into(),
        action: "verify_connect".into(),
        path: result.backup_path.clone(),
        reason: format!("Failed to connect to backup: {e}"),
    })?;

    conn.query("RETURN 1")
        .map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory-backup".into(),
            action: "verify_query".into(),
            path: result.backup_path.clone(),
            reason: format!("Backup query failed: {e}"),
        })?;

    result.verified = true;
    info!(backup = %result.backup_path.display(), "Backup verified successfully");
    Ok(result)
}

/// Create a backup and verify it in one call.
pub fn backup_and_verify(db_path: &Path, backup_dir: Option<&Path>) -> SimardResult<BackupResult> {
    let result = create_backup(db_path, backup_dir)?;
    verify_backup(result)
}

/// Prune old backups, keeping at most `keep` newest files.
///
/// Only removes files matching `cognitive_memory_*.ladybug` in the backup dir.
pub fn prune_backups(backup_dir: &Path, keep: usize) -> SimardResult<usize> {
    let entries: Vec<_> = std::fs::read_dir(backup_dir)
        .map_err(|e| SimardError::PersistentStoreIo {
            store: "cognitive-memory-backup".into(),
            action: "list_backups".into(),
            path: backup_dir.to_path_buf(),
            reason: e.to_string(),
        })?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("cognitive_memory_") && name.ends_with(".ladybug")
        })
        .collect();

    if entries.len() <= keep {
        return Ok(0);
    }

    // Sort by modification time (newest first)
    let mut with_time: Vec<_> = entries
        .into_iter()
        .filter_map(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| (e, t))
        })
        .collect();
    with_time.sort_by(|a, b| b.1.cmp(&a.1));

    let mut removed = 0;
    for (entry, _) in with_time.into_iter().skip(keep) {
        if let Err(e) = std::fs::remove_file(entry.path()) {
            warn!(path = %entry.path().display(), "Failed to prune backup: {e}");
        } else {
            removed += 1;
        }
    }

    info!(removed, keep, "Pruned old backups");
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_nonexistent_source_fails() {
        let dir = tempfile::tempdir().unwrap();
        let fake_db = dir.path().join("nonexistent.ladybug");
        let result = create_backup(&fake_db, Some(dir.path()));
        assert!(result.is_err());
    }

    #[test]
    fn backup_and_verify_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.ladybug");

        // Create a real LadybugDB file
        let _db = lbug::Database::new(&db_path, lbug::SystemConfig::default())
            .expect("should create test DB");
        drop(_db);

        let backup_dir = dir.path().join("backups");
        let result = create_backup(&db_path, Some(&backup_dir)).unwrap();
        assert!(result.backup_path.exists());
        assert!(result.size_bytes > 0);
        assert!(!result.verified);

        let verified = verify_backup(result).unwrap();
        assert!(verified.verified);
    }

    #[test]
    fn prune_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        for i in 0..5 {
            let name = format!("cognitive_memory_2025010{i}_120000.ladybug");
            std::fs::write(backup_dir.join(&name), format!("data{i}")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let removed = prune_backups(&backup_dir, 2).unwrap();
        assert_eq!(removed, 3);

        let remaining: Vec<_> = std::fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn prune_no_op_when_under_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        let removed = prune_backups(dir.path(), 10).unwrap();
        assert_eq!(removed, 0);
    }
}
