//! Automated memory backup with verification.
//!
//! Creates timestamped backups of both cognitive memory (facts + procedures)
//! and file-backed memory records, with SHA-256 integrity verification and
//! configurable retention policies.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryStore};
use crate::remote_transfer::MemorySnapshot;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Manifest describing the contents and integrity of a single backup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupManifest {
    pub backup_dir: PathBuf,
    pub created_at: DateTime<Utc>,
    pub cognitive_snapshot_path: PathBuf,
    pub memory_records_path: PathBuf,
    pub cognitive_facts_count: usize,
    pub cognitive_procedures_count: usize,
    pub memory_records_count: usize,
    /// SHA-256 hex digest of concatenated backup file contents.
    pub checksum: String,
}

/// Result of verifying a backup against its manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BackupStatus {
    Valid,
    Corrupted { reason: String },
    Incomplete { missing: Vec<String> },
}

/// Full verification report for a backup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupVerification {
    pub manifest: BackupManifest,
    pub status: BackupStatus,
    pub verified_at: DateTime<Utc>,
}

/// Configuration for backup location and retention.
#[derive(Clone, Debug)]
pub struct BackupConfig {
    pub backup_dir: PathBuf,
    pub retention_days: u32,
    pub min_backups_to_keep: usize,
}

impl Default for BackupConfig {
    fn default() -> Self {
        let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            backup_dir: base.join(".simard").join("backups"),
            retention_days: 30,
            min_backups_to_keep: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const MANIFEST_FILE: &str = "manifest.json";
const SNAPSHOT_FILE: &str = "cognitive_snapshot.json";
const RECORDS_FILE: &str = "memory_records.json";

fn store_error(action: &str, path: &Path, reason: String) -> SimardError {
    SimardError::PersistentStoreIo {
        store: "memory-backup".to_string(),
        action: action.to_string(),
        path: path.to_path_buf(),
        reason,
    }
}

/// Compute SHA-256 hex digest over the concatenation of `data` slices.
fn sha256_hex(data: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for chunk in data {
        hasher.update(chunk);
    }
    format!("{:x}", hasher.finalize())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> SimardResult<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| store_error("serialize", path, e.to_string()))?;
    // Route through the durable persistence pipeline (temp + fsync + rename +
    // parent fsync). The checksum below is computed over `bytes`, which is the
    // exact payload written to disk, so verification stays bit-exact.
    crate::persistence::persist_bytes("memory-backup", path, &bytes)?;
    Ok(bytes)
}

fn read_bytes(path: &Path) -> SimardResult<Vec<u8>> {
    fs::read(path).map_err(|e| store_error("read", path, e.to_string()))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a timestamped backup of cognitive and file-backed memory.
pub fn backup_memory(
    bridge: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    agent_name: &str,
    config: &BackupConfig,
) -> SimardResult<BackupManifest> {
    let now = Utc::now();
    let dir_name = now.format("%Y%m%d_%H%M%S").to_string();
    let backup_dir = config.backup_dir.join(&dir_name);

    fs::create_dir_all(&backup_dir)
        .map_err(|e| store_error("create-dir", &backup_dir, e.to_string()))?;

    // Export the FULL cognitive snapshot (no replication truncation caps) so a
    // store larger than MAX_EXPORT_FACTS is captured faithfully (issue #2420).
    let snapshot = crate::remote_transfer::export_full_memory_snapshot(bridge, agent_name)?;
    let snapshot_path = backup_dir.join(SNAPSHOT_FILE);
    let snapshot_bytes = write_json(&snapshot_path, &snapshot)?;

    // Export file-backed memory records.
    let records = store.list_all()?;
    let records_path = backup_dir.join(RECORDS_FILE);
    let records_bytes = write_json(&records_path, &records)?;

    let checksum = sha256_hex(&[&snapshot_bytes, &records_bytes]);

    let manifest = BackupManifest {
        backup_dir: backup_dir.clone(),
        created_at: now,
        cognitive_snapshot_path: snapshot_path,
        memory_records_path: records_path,
        cognitive_facts_count: snapshot.facts.len(),
        cognitive_procedures_count: snapshot.procedures.len(),
        memory_records_count: records.len(),
        checksum,
    };

    let manifest_path = backup_dir.join(MANIFEST_FILE);
    write_json(&manifest_path, &manifest)?;

    Ok(manifest)
}

/// Verify that a backup is complete and uncorrupted.
pub fn verify_backup(backup_dir: &Path) -> SimardResult<BackupVerification> {
    let manifest_path = backup_dir.join(MANIFEST_FILE);
    if !manifest_path.exists() {
        return Ok(BackupVerification {
            manifest: BackupManifest {
                backup_dir: backup_dir.to_path_buf(),
                created_at: Utc::now(),
                cognitive_snapshot_path: PathBuf::new(),
                memory_records_path: PathBuf::new(),
                cognitive_facts_count: 0,
                cognitive_procedures_count: 0,
                memory_records_count: 0,
                checksum: String::new(),
            },
            status: BackupStatus::Incomplete {
                missing: vec![MANIFEST_FILE.to_string()],
            },
            verified_at: Utc::now(),
        });
    }

    let manifest_bytes = read_bytes(&manifest_path)?;
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| store_error("deserialize-manifest", &manifest_path, e.to_string()))?;

    // Check for missing files.
    let mut missing = Vec::new();
    if !manifest.cognitive_snapshot_path.exists() {
        missing.push(SNAPSHOT_FILE.to_string());
    }
    if !manifest.memory_records_path.exists() {
        missing.push(RECORDS_FILE.to_string());
    }
    if !missing.is_empty() {
        return Ok(BackupVerification {
            manifest,
            status: BackupStatus::Incomplete { missing },
            verified_at: Utc::now(),
        });
    }

    // Verify checksum.
    let snapshot_bytes = read_bytes(&manifest.cognitive_snapshot_path)?;
    let records_bytes = read_bytes(&manifest.memory_records_path)?;
    let actual_checksum = sha256_hex(&[&snapshot_bytes, &records_bytes]);

    if actual_checksum != manifest.checksum {
        let reason = format!(
            "checksum mismatch: expected {}, got {}",
            manifest.checksum, actual_checksum
        );
        return Ok(BackupVerification {
            manifest,
            status: BackupStatus::Corrupted { reason },
            verified_at: Utc::now(),
        });
    }

    // Verify record counts.
    let snapshot: MemorySnapshot = serde_json::from_slice(&snapshot_bytes).map_err(|e| {
        store_error(
            "deserialize-snapshot",
            &manifest.cognitive_snapshot_path,
            e.to_string(),
        )
    })?;
    let records: Vec<MemoryRecord> = serde_json::from_slice(&records_bytes).map_err(|e| {
        store_error(
            "deserialize-records",
            &manifest.memory_records_path,
            e.to_string(),
        )
    })?;

    if snapshot.facts.len() != manifest.cognitive_facts_count
        || snapshot.procedures.len() != manifest.cognitive_procedures_count
        || records.len() != manifest.memory_records_count
    {
        return Ok(BackupVerification {
            manifest,
            status: BackupStatus::Corrupted {
                reason: "record counts do not match manifest".to_string(),
            },
            verified_at: Utc::now(),
        });
    }

    Ok(BackupVerification {
        manifest,
        status: BackupStatus::Valid,
        verified_at: Utc::now(),
    })
}

/// Restore memory from a verified backup.
///
/// Verifies the backup first. Returns the total count of restored items.
#[allow(deprecated)] // import_memory_snapshot is deprecated but needed here
pub fn restore_from_backup(
    bridge: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    backup_dir: &Path,
) -> SimardResult<usize> {
    let verification = verify_backup(backup_dir)?;
    match &verification.status {
        BackupStatus::Valid => {}
        BackupStatus::Corrupted { reason } => {
            return Err(SimardError::MemoryIntegrityError {
                path: backup_dir.to_path_buf(),
                reason: format!("cannot restore from corrupted backup: {reason}"),
            });
        }
        BackupStatus::Incomplete { missing } => {
            return Err(SimardError::MemoryIntegrityError {
                path: backup_dir.to_path_buf(),
                reason: format!(
                    "cannot restore from incomplete backup, missing: {}",
                    missing.join(", ")
                ),
            });
        }
    }

    let manifest = &verification.manifest;

    // Restore cognitive memory.
    let snapshot_bytes = read_bytes(&manifest.cognitive_snapshot_path)?;
    let snapshot: MemorySnapshot = serde_json::from_slice(&snapshot_bytes).map_err(|e| {
        store_error(
            "deserialize-snapshot",
            &manifest.cognitive_snapshot_path,
            e.to_string(),
        )
    })?;
    let cognitive_count = crate::remote_transfer::import_memory_snapshot(bridge, &snapshot)?;

    // Restore file-backed memory records.
    let records_bytes = read_bytes(&manifest.memory_records_path)?;
    let records: Vec<MemoryRecord> = serde_json::from_slice(&records_bytes).map_err(|e| {
        store_error(
            "deserialize-records",
            &manifest.memory_records_path,
            e.to_string(),
        )
    })?;
    let mut record_count = 0;
    for record in records {
        store.put(record)?;
        record_count += 1;
    }

    Ok(cognitive_count + record_count)
}

/// List available backups sorted newest-first, each with verification status.
pub fn list_backups(config: &BackupConfig) -> SimardResult<Vec<BackupVerification>> {
    let dir = &config.backup_dir;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| store_error("list-dir", dir, e.to_string()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();

    // Sort descending by directory name (timestamp-based).
    entries.sort_by(|a, b| b.cmp(a));

    entries.iter().map(|p| verify_backup(p)).collect()
}

/// Remove backups older than `retention_days`, keeping at least `min_backups_to_keep`.
pub fn prune_old_backups(config: &BackupConfig) -> SimardResult<usize> {
    let dir = &config.backup_dir;
    if !dir.exists() {
        return Ok(0);
    }

    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| store_error("list-dir", dir, e.to_string()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.path())
        .collect();

    // Sort descending (newest first) so we can protect the most recent N.
    entries.sort_by(|a, b| b.cmp(a));

    let cutoff = Utc::now() - chrono::Duration::days(i64::from(config.retention_days));
    let mut pruned = 0;

    for (i, entry) in entries.iter().enumerate() {
        if i < config.min_backups_to_keep {
            continue;
        }

        let manifest_path = entry.join(MANIFEST_FILE);
        let should_prune = if manifest_path.exists() {
            match fs::read(&manifest_path)
                .ok()
                .and_then(|b| serde_json::from_slice::<BackupManifest>(&b).ok())
            {
                Some(m) => m.created_at < cutoff,
                None => true,
            }
        } else {
            true
        };

        if should_prune && fs::remove_dir_all(entry).is_ok() {
            pruned += 1;
        }
    }

    Ok(pruned)
}

// ---------------------------------------------------------------------------
// Scheduled daemon backup + corrupt-artifact bounding (issue #2420)
// ---------------------------------------------------------------------------

/// Source agent label recorded in scheduled-backup snapshots.
pub const BACKUP_AGENT_NAME: &str = "simard-ooda-daemon";

/// Filename of the file-backed memory-record store under `state_root`. Mirrors
/// the path `simard meeting read` and the meeting close-path writer use.
const MEMORY_RECORDS_FILENAME: &str = "memory_records.json";

/// Number of most-recent corrupt/shadow quarantine artifacts to retain for
/// forensics before pruning. Bounds the unbounded accumulation described in
/// issue #2420 (112 artifacts / ~88 MB observed on the live host).
pub const CORRUPT_ARTIFACTS_KEEP: usize = 5;

/// Outcome of one scheduled daemon backup pass.
#[derive(Clone, Debug)]
pub struct ScheduledBackupOutcome {
    pub manifest: BackupManifest,
    pub status: BackupStatus,
    pub backups_pruned: usize,
    pub corrupt_artifacts_pruned: usize,
}

impl ScheduledBackupOutcome {
    /// Whether the freshly written backup verified clean.
    pub fn is_valid(&self) -> bool {
        matches!(self.status, BackupStatus::Valid)
    }

    /// One-line, log-friendly summary.
    pub fn summary(&self) -> String {
        let status = match &self.status {
            BackupStatus::Valid => "verified".to_string(),
            BackupStatus::Corrupted { reason } => format!("CORRUPT ({reason})"),
            BackupStatus::Incomplete { missing } => {
                format!("INCOMPLETE (missing {})", missing.join(", "))
            }
        };
        format!(
            "memory backup {status}: {} facts, {} procedures, {} records -> {} \
             (pruned {} old backup(s), {} corrupt artifact(s))",
            self.manifest.cognitive_facts_count,
            self.manifest.cognitive_procedures_count,
            self.manifest.memory_records_count,
            self.manifest.backup_dir.display(),
            self.backups_pruned,
            self.corrupt_artifacts_pruned,
        )
    }
}

/// Run one scheduled, verified backup of the **live** cognitive store fronted by
/// `bridge`, plus the file-backed memory records under `state_root`.
///
/// This is the de-fork-era replacement for the removed native lbug file-copy
/// backup (`create_verified_backup` / `prune_old_backups`, issue #2307/#2308).
/// Crucially the snapshot is taken from the **live** store the daemon actually
/// opened (the `bridge`), not from a stale on-disk path such as the
/// pre-migration `state_root/cognitive_memory.ladybug` (issue #2420 gap #1).
///
/// Order of operations:
/// 1. Write a fresh timestamped logical snapshot under `~/.simard/backups/`.
/// 2. Verify it opens and its counts match the manifest.
/// 3. **Only if the fresh backup verified clean**, prune old verified backups
///    and bound the corrupt/shadow quarantine artifacts. Pruning is gated on a
///    good fresh backup so a bad write can never delete the prior good copy.
pub fn run_scheduled_backup(
    bridge: &dyn CognitiveMemoryOps,
    state_root: &Path,
    agent_name: &str,
) -> SimardResult<ScheduledBackupOutcome> {
    let config = BackupConfig::default();
    let store = FileBackedMemoryStore::try_new(state_root.join(MEMORY_RECORDS_FILENAME))?;
    run_scheduled_backup_with(bridge, &store, state_root, agent_name, &config)
}

/// Dependency-injected core of [`run_scheduled_backup`]: the caller supplies the
/// record store and backup location so the pass can be exercised against a
/// temporary directory in tests without touching the operator's real
/// `~/.simard`.
pub(crate) fn run_scheduled_backup_with(
    bridge: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    state_root: &Path,
    agent_name: &str,
    config: &BackupConfig,
) -> SimardResult<ScheduledBackupOutcome> {
    let manifest = backup_memory(bridge, store, agent_name, config)?;
    let verification = verify_backup(&manifest.backup_dir)?;

    let (backups_pruned, corrupt_artifacts_pruned) = match &verification.status {
        BackupStatus::Valid => (
            prune_old_backups(config)?,
            prune_corrupt_artifacts(state_root, CORRUPT_ARTIFACTS_KEEP)?,
        ),
        // Bad fresh backup: keep every prior backup and every forensic artifact.
        _ => (0, 0),
    };

    Ok(ScheduledBackupOutcome {
        manifest,
        status: verification.status,
        backups_pruned,
        corrupt_artifacts_pruned,
    })
}

/// Names that belong to the **live** lbug store and must never be pruned, even
/// if a future naming scheme makes one of them look quarantine-like. The live
/// store (lbug 0.17.x) is a single file `cognitive`; its active write sidecars
/// are listed defensively. Transient checkpoint shadow files (`cognitive.shadow`)
/// are deliberately *not* protected here — stale `.shadow` leftovers are exactly
/// the cruft issue #2420 asks to bound, and any genuinely-active shadow file is
/// the newest artifact and is shielded by the keep-newest-N retention instead.
fn is_live_store_name(name: &str) -> bool {
    matches!(
        name,
        "cognitive" | "cognitive.wal" | "cognitive.shm" | "cognitive-wal" | "cognitive-shm"
    )
}

/// Whether `name` is a corrupt/shadow quarantine artifact left behind by the
/// library's corrupt-WAL recovery (quarantine + rebuild). Matched as substrings
/// so concatenated rename chains — e.g.
/// `cognitive.wal.corrupt-…cognitive.corrupt-…cognitive.shadow` (issue #2420
/// gap #2) — are caught regardless of how deeply the name has been chained.
fn is_corrupt_artifact(name: &str) -> bool {
    if is_live_store_name(name) {
        return false;
    }
    name.contains(".corrupt-") || name.contains(".corrupt.") || name.ends_with(".shadow")
}

/// Bound corrupt/shadow quarantine artifacts directly under `state_root`,
/// retaining the `keep` newest (by mtime) for forensics and removing the rest.
///
/// Handles both files and directories (the migrated lbug store quarantines as a
/// flat file today, but historic native stores quarantined directories). The
/// live store file and its active sidecars are never eligible. Returns the
/// number of artifacts removed.
pub fn prune_corrupt_artifacts(state_root: &Path, keep: usize) -> SimardResult<usize> {
    if !state_root.exists() {
        return Ok(0);
    }

    let mut artifacts: Vec<(PathBuf, SystemTime)> = fs::read_dir(state_root)
        .map_err(|e| store_error("list-dir", state_root, e.to_string()))?
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_corrupt_artifact(&name) {
                return None;
            }
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((entry.path(), mtime))
        })
        .collect();

    // Newest first so the most-recent `keep` artifacts are protected.
    artifacts.sort_by_key(|entry| std::cmp::Reverse(entry.1));

    let mut pruned = 0;
    for (path, _) in artifacts.into_iter().skip(keep) {
        let removed = if path.is_dir() {
            fs::remove_dir_all(&path).is_ok()
        } else {
            fs::remove_file(&path).is_ok()
        };
        if removed {
            pruned += 1;
        }
    }

    Ok(pruned)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
