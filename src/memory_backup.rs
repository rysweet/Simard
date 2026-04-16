//! Automated memory backup with verification.
//!
//! Creates timestamped backups of both cognitive memory (facts + procedures)
//! and file-backed memory records, with SHA-256 integrity verification and
//! configurable retention policies.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory::{MemoryRecord, MemoryStore};
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
    fs::write(path, &bytes).map_err(|e| store_error("write", path, e.to_string()))?;
    Ok(bytes)
}

fn read_bytes(path: &Path) -> SimardResult<Vec<u8>> {
    fs::read(path).map_err(|e| store_error("read", path, e.to_string()))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a timestamped backup of cognitive and file-backed memory.
#[allow(deprecated)] // export_memory_snapshot is deprecated but needed here
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

    // Export cognitive snapshot.
    let snapshot = crate::remote_transfer::export_memory_snapshot(bridge, agent_name, None)?;
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
        .filter_map(Result::ok)
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
        .filter_map(Result::ok)
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
    use crate::memory_bridge::CognitiveMemoryBridge;
    use crate::memory_cognitive::{CognitiveFact, CognitiveProcedure};
    use crate::session::{SessionId, SessionPhase};
    use serde_json::json;
    use std::sync::Mutex;
    use uuid::Uuid;

    struct MockStore {
        facts: Vec<CognitiveFact>,
        procedures: Vec<CognitiveProcedure>,
    }

    fn mock_bridge() -> CognitiveMemoryBridge {
        let store: &'static Mutex<MockStore> = Box::leak(Box::new(Mutex::new(MockStore {
            facts: vec![],
            procedures: vec![],
        })));

        let transport =
            InMemoryBridgeTransport::new("test-backup", move |method, params| match method {
                "memory.search_facts" => {
                    let s = store.lock().unwrap();
                    let facts: Vec<serde_json::Value> = s
                        .facts
                        .iter()
                        .map(|f| {
                            json!({
                                "node_id": f.node_id, "concept": f.concept,
                                "content": f.content, "confidence": f.confidence,
                                "source_id": f.source_id, "tags": f.tags,
                            })
                        })
                        .collect();
                    Ok(json!({"facts": facts}))
                }
                "memory.recall_procedure" => {
                    let s = store.lock().unwrap();
                    let procs: Vec<serde_json::Value> = s
                        .procedures
                        .iter()
                        .map(|p| {
                            json!({
                                "node_id": p.node_id, "name": p.name,
                                "steps": p.steps, "prerequisites": p.prerequisites,
                                "usage_count": p.usage_count,
                            })
                        })
                        .collect();
                    Ok(json!({"procedures": procs}))
                }
                "memory.store_fact" => {
                    let mut s = store.lock().unwrap();
                    let id = format!("fact-{}", s.facts.len() + 1);
                    s.facts.push(CognitiveFact {
                        node_id: id.clone(),
                        concept: params["concept"].as_str().unwrap_or("").to_string(),
                        content: params["content"].as_str().unwrap_or("").to_string(),
                        confidence: params["confidence"].as_f64().unwrap_or(0.0),
                        source_id: params["source_id"].as_str().unwrap_or("").to_string(),
                        tags: params["tags"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                    });
                    Ok(json!({"id": id}))
                }
                "memory.store_procedure" => {
                    let mut s = store.lock().unwrap();
                    let id = format!("proc-{}", s.procedures.len() + 1);
                    s.procedures.push(CognitiveProcedure {
                        node_id: id.clone(),
                        name: params["name"].as_str().unwrap_or("").to_string(),
                        steps: params["steps"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                        prerequisites: params["prerequisites"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect(),
                        usage_count: 0,
                    });
                    Ok(json!({"id": id}))
                }
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown method: {method}"),
                }),
            });
        CognitiveMemoryBridge::new(Box::new(transport))
    }

    fn test_session_id() -> SessionId {
        SessionId::from_uuid(Uuid::nil())
    }

    fn make_record(key: &str) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            scope: MemoryScope::Project,
            value: format!("val-{key}"),
            session_id: test_session_id(),
            recorded_in: SessionPhase::Execution,
            created_at: None,
        }
    }

    fn test_config(dir: &Path) -> BackupConfig {
        BackupConfig {
            backup_dir: dir.to_path_buf(),
            retention_days: 30,
            min_backups_to_keep: 2,
        }
    }

    #[test]
    fn backup_and_verify_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let backup_root = tmp.path().join("backups");
        let store_path = tmp.path().join("memory.json");
        let config = test_config(&backup_root);

        let bridge = mock_bridge();
        bridge
            .store_fact("rust", "fast lang", 0.9, &[], "ep1")
            .unwrap();
        bridge
            .store_procedure("build", &["compile".into()], &[])
            .unwrap();

        let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
        file_store.put(make_record("rec1")).unwrap();
        file_store.put(make_record("rec2")).unwrap();

        let manifest = backup_memory(&bridge, &file_store, "test-agent", &config).unwrap();
        assert_eq!(manifest.cognitive_facts_count, 1);
        assert_eq!(manifest.cognitive_procedures_count, 1);
        assert_eq!(manifest.memory_records_count, 2);
        assert!(!manifest.checksum.is_empty());

        let verification = verify_backup(&manifest.backup_dir).unwrap();
        assert!(matches!(verification.status, BackupStatus::Valid));
    }

    #[test]
    fn verify_detects_missing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("empty_backup");
        fs::create_dir_all(&dir).unwrap();

        let v = verify_backup(&dir).unwrap();
        assert!(matches!(v.status, BackupStatus::Incomplete { .. }));
    }

    #[test]
    fn verify_detects_corrupted_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let backup_root = tmp.path().join("backups");
        let store_path = tmp.path().join("memory.json");
        let config = test_config(&backup_root);

        let bridge = mock_bridge();
        let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

        let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();

        // Tamper with the snapshot file.
        fs::write(&manifest.cognitive_snapshot_path, b"tampered").unwrap();

        let v = verify_backup(&manifest.backup_dir).unwrap();
        assert!(matches!(v.status, BackupStatus::Corrupted { .. }));
    }

    #[test]
    fn verify_detects_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let backup_root = tmp.path().join("backups");
        let store_path = tmp.path().join("memory.json");
        let config = test_config(&backup_root);

        let bridge = mock_bridge();
        let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

        let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();

        // Remove one backup file.
        fs::remove_file(&manifest.memory_records_path).unwrap();

        let v = verify_backup(&manifest.backup_dir).unwrap();
        assert!(matches!(v.status, BackupStatus::Incomplete { .. }));
    }

    #[test]
    fn restore_from_valid_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let backup_root = tmp.path().join("backups");
        let store_path = tmp.path().join("memory.json");
        let config = test_config(&backup_root);

        let bridge = mock_bridge();
        bridge.store_fact("rust", "systems", 0.9, &[], "").unwrap();

        let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
        file_store.put(make_record("r1")).unwrap();

        let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();

        // Restore into fresh targets.
        let target_bridge = mock_bridge();
        let target_store_path = tmp.path().join("restored.json");
        let target_store = FileBackedMemoryStore::try_new(&target_store_path).unwrap();

        let count =
            restore_from_backup(&target_bridge, &target_store, &manifest.backup_dir).unwrap();
        assert_eq!(count, 2); // 1 fact + 1 record
        assert_eq!(target_store.list_all().unwrap().len(), 1);
    }

    #[test]
    fn restore_rejects_corrupted_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let backup_root = tmp.path().join("backups");
        let store_path = tmp.path().join("memory.json");
        let config = test_config(&backup_root);

        let bridge = mock_bridge();
        let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

        let manifest = backup_memory(&bridge, &file_store, "agent", &config).unwrap();
        fs::write(&manifest.cognitive_snapshot_path, b"bad").unwrap();

        let target_bridge = mock_bridge();
        let target_store = FileBackedMemoryStore::try_new(tmp.path().join("t.json")).unwrap();

        let err = restore_from_backup(&target_bridge, &target_store, &manifest.backup_dir);
        assert!(err.is_err());
    }

    #[test]
    fn list_backups_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let config = test_config(&tmp.path().join("no-such-dir"));
        let backups = list_backups(&config).unwrap();
        assert!(backups.is_empty());
    }

    #[test]
    fn prune_old_backups_respects_min_keep() {
        let tmp = tempfile::tempdir().unwrap();
        let backup_root = tmp.path().join("backups");
        let store_path = tmp.path().join("memory.json");
        let mut config = test_config(&backup_root);
        config.retention_days = 0;
        config.min_backups_to_keep = 1;

        let bridge = mock_bridge();
        let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();

        // Create two backups with distinct timestamp directories.
        backup_memory(&bridge, &file_store, "a", &config).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        backup_memory(&bridge, &file_store, "a", &config).unwrap();

        let before = list_backups(&config).unwrap().len();
        assert_eq!(before, 2);

        let pruned = prune_old_backups(&config).unwrap();
        assert_eq!(pruned, 1);

        let after = list_backups(&config).unwrap().len();
        assert_eq!(after, 1);
    }

    #[test]
    fn prune_nonexistent_dir_returns_zero() {
        let config = test_config(Path::new("/nonexistent/path"));
        assert_eq!(prune_old_backups(&config).unwrap(), 0);
    }

    #[test]
    fn backup_config_default_points_to_home() {
        let config = BackupConfig::default();
        assert!(config.backup_dir.to_string_lossy().contains(".simard"));
        assert!(config.backup_dir.to_string_lossy().contains("backups"));
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.min_backups_to_keep, 3);
    }

    #[test]
    fn backup_restore_round_trip_searchable() {
        let tmp = tempfile::tempdir().unwrap();
        let backup_root = tmp.path().join("backups");
        let store_path = tmp.path().join("memory.json");
        let config = test_config(&backup_root);

        let bridge = mock_bridge();
        bridge
            .store_fact("algorithms", "sorting and searching", 0.85, &[], "ep1")
            .unwrap();
        bridge
            .store_fact("databases", "relational storage", 0.9, &[], "ep2")
            .unwrap();
        bridge
            .store_procedure(
                "deploy",
                &["build".into(), "test".into(), "ship".into()],
                &[],
            )
            .unwrap();

        let file_store = FileBackedMemoryStore::try_new(&store_path).unwrap();
        let manifest = backup_memory(&bridge, &file_store, "test-agent", &config).unwrap();
        assert_eq!(manifest.cognitive_facts_count, 2);
        assert_eq!(manifest.cognitive_procedures_count, 1);

        // Restore into a fresh bridge and verify searchability.
        let target_bridge = mock_bridge();
        let target_store_path = tmp.path().join("restored.json");
        let target_store = FileBackedMemoryStore::try_new(&target_store_path).unwrap();

        let count =
            restore_from_backup(&target_bridge, &target_store, &manifest.backup_dir).unwrap();
        assert!(count >= 3, "should restore at least 2 facts + 1 procedure");

        // Verify facts are searchable.
        let facts = target_bridge.search_facts("algorithms", 10, 0.0).unwrap();
        assert!(!facts.is_empty(), "restored facts should be searchable");

        // Verify procedures are recallable.
        let procs = target_bridge.recall_procedure("deploy", 5).unwrap();
        assert!(
            !procs.is_empty(),
            "restored procedures should be recallable"
        );
        assert_eq!(procs[0].steps, vec!["build", "test", "ship"]);
    }
}
