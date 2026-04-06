use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::session::{SessionId, SessionPhase};

use super::store::MemoryStore;
use super::types::{CognitiveMemoryType, MemoryRecord};

/// SQLite-backed memory store for durable cognitive memory persistence.
///
/// Uses a single `memory_records` table with indexed columns for scope and
/// session_id. Upserts by key so duplicate puts overwrite existing records.
#[derive(Debug)]
pub struct SqliteMemoryStore {
    conn: Mutex<Connection>,
    descriptor: BackendDescriptor,
}

impl SqliteMemoryStore {
    pub fn new(path: impl AsRef<Path>) -> SimardResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SimardError::PersistentStoreIo {
                store: "memory-sqlite".to_string(),
                action: "create-dir".to_string(),
                path: parent.to_path_buf(),
                reason: e.to_string(),
            })?;
        }

        let conn = Connection::open(path).map_err(|e| SimardError::PersistentStoreIo {
            store: "memory-sqlite".to_string(),
            action: "open".to_string(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memory_records (
                key TEXT PRIMARY KEY,
                scope TEXT NOT NULL,
                value TEXT NOT NULL,
                session_id TEXT NOT NULL,
                recorded_in TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_memory_scope ON memory_records(scope);
            CREATE INDEX IF NOT EXISTS idx_memory_session ON memory_records(session_id);",
        )
        .map_err(|e| SimardError::PersistentStoreIo {
            store: "memory-sqlite".to_string(),
            action: "init-schema".to_string(),
            path: path.to_path_buf(),
            reason: e.to_string(),
        })?;

        Ok(Self {
            conn: Mutex::new(conn),
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "memory::sqlite-store",
                "runtime-port:memory-store:sqlite",
                Freshness::now()?,
            ),
        })
    }

    fn map_sqlite_err(e: rusqlite::Error) -> SimardError {
        SimardError::PersistentStoreIo {
            store: "memory-sqlite".to_string(),
            action: "query".to_string(),
            path: PathBuf::from("<sqlite>"),
            reason: e.to_string(),
        }
    }

    fn lock_conn(&self) -> SimardResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| SimardError::StoragePoisoned {
            store: "memory-sqlite".to_string(),
        })
    }
}

impl MemoryStore for SqliteMemoryStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: MemoryRecord) -> SimardResult<()> {
        let conn = self.lock_conn()?;
        let scope_str = serde_json::to_string(&record.memory_type).unwrap_or_default();
        let phase_str = serde_json::to_string(&record.recorded_in).unwrap_or_default();
        conn.execute(
            "INSERT INTO memory_records (key, scope, value, session_id, recorded_in)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET
                scope = excluded.scope,
                value = excluded.value,
                session_id = excluded.session_id,
                recorded_in = excluded.recorded_in",
            rusqlite::params![
                record.key,
                scope_str,
                record.value,
                record.session_id.as_str(),
                phase_str,
            ],
        )
        .map_err(Self::map_sqlite_err)?;
        Ok(())
    }

    fn list(&self, memory_type: CognitiveMemoryType) -> SimardResult<Vec<MemoryRecord>> {
        let conn = self.lock_conn()?;
        let scope_str = serde_json::to_string(&memory_type).unwrap_or_default();
        let mut stmt = conn
            .prepare("SELECT key, scope, value, session_id, recorded_in FROM memory_records WHERE scope = ?1")
            .map_err(Self::map_sqlite_err)?;
        let rows = stmt
            .query_map(rusqlite::params![scope_str], |row| {
                Ok(RawRow {
                    key: row.get(0)?,
                    scope: row.get(1)?,
                    value: row.get(2)?,
                    session_id: row.get(3)?,
                    recorded_in: row.get(4)?,
                })
            })
            .map_err(Self::map_sqlite_err)?;
        rows.map(|r| {
            let raw = r.map_err(Self::map_sqlite_err)?;
            raw.into_record()
        })
        .collect()
    }

    fn list_for_session(&self, session_id: &SessionId) -> SimardResult<Vec<MemoryRecord>> {
        let conn = self.lock_conn()?;
        let mut stmt = conn
            .prepare("SELECT key, scope, value, session_id, recorded_in FROM memory_records WHERE session_id = ?1")
            .map_err(Self::map_sqlite_err)?;
        let rows = stmt
            .query_map(rusqlite::params![session_id.as_str()], |row| {
                Ok(RawRow {
                    key: row.get(0)?,
                    scope: row.get(1)?,
                    value: row.get(2)?,
                    session_id: row.get(3)?,
                    recorded_in: row.get(4)?,
                })
            })
            .map_err(Self::map_sqlite_err)?;
        rows.map(|r| {
            let raw = r.map_err(Self::map_sqlite_err)?;
            raw.into_record()
        })
        .collect()
    }

    fn count_for_session(&self, session_id: &SessionId) -> SimardResult<usize> {
        let conn = self.lock_conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory_records WHERE session_id = ?1",
                rusqlite::params![session_id.as_str()],
                |row| row.get(0),
            )
            .map_err(Self::map_sqlite_err)?;
        Ok(count as usize)
    }
}

/// Helper for reading raw SQLite rows before deserializing enums.
struct RawRow {
    key: String,
    scope: String,
    value: String,
    session_id: String,
    recorded_in: String,
}

impl RawRow {
    fn into_record(self) -> SimardResult<MemoryRecord> {
        let memory_type: CognitiveMemoryType =
            serde_json::from_str(&self.scope).map_err(|e| SimardError::PersistentStoreIo {
                store: "memory-sqlite".to_string(),
                action: "deserialize-scope".to_string(),
                path: PathBuf::from("<sqlite>"),
                reason: e.to_string(),
            })?;
        let recorded_in: SessionPhase = serde_json::from_str(&self.recorded_in).map_err(|e| {
            SimardError::PersistentStoreIo {
                store: "memory-sqlite".to_string(),
                action: "deserialize-phase".to_string(),
                path: PathBuf::from("<sqlite>"),
                reason: e.to_string(),
            }
        })?;
        Ok(MemoryRecord {
            key: self.key,
            memory_type,
            value: self.value,
            session_id: SessionId::parse(&self.session_id)?,
            recorded_in,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionPhase;
    use uuid::Uuid;

    fn make_record(key: &str, memory_type: CognitiveMemoryType) -> MemoryRecord {
        MemoryRecord {
            key: key.to_string(),
            memory_type,
            value: format!("value-{key}"),
            session_id: SessionId::from_uuid(Uuid::nil()),
            recorded_in: SessionPhase::Execution,
        }
    }

    fn temp_db_path(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("simard-sqlite-{label}-{unique}.db"))
    }

    #[test]
    fn sqlite_put_and_list_by_scope() {
        let store = SqliteMemoryStore::new(temp_db_path("scope")).unwrap();
        store
            .put(make_record("a", CognitiveMemoryType::Semantic))
            .unwrap();
        store
            .put(make_record("b", CognitiveMemoryType::Semantic))
            .unwrap();
        store
            .put(make_record("c", CognitiveMemoryType::Semantic))
            .unwrap();

        let decisions = store.list(CognitiveMemoryType::Semantic).unwrap();
        assert_eq!(decisions.len(), 2);
        let projects = store.list(CognitiveMemoryType::Semantic).unwrap();
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn sqlite_upsert_deduplicates_by_key() {
        let store = SqliteMemoryStore::new(temp_db_path("upsert")).unwrap();
        store
            .put(make_record("dup", CognitiveMemoryType::Semantic))
            .unwrap();
        store
            .put(make_record("dup", CognitiveMemoryType::Semantic))
            .unwrap();

        let all = store.list(CognitiveMemoryType::Semantic).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn sqlite_list_for_session() {
        let store = SqliteMemoryStore::new(temp_db_path("session")).unwrap();
        store
            .put(make_record("x", CognitiveMemoryType::Working))
            .unwrap();

        let session = SessionId::from_uuid(Uuid::nil());
        assert_eq!(store.list_for_session(&session).unwrap().len(), 1);

        let other = SessionId::from_uuid(Uuid::from_u128(1));
        assert_eq!(store.list_for_session(&other).unwrap().len(), 0);
    }

    #[test]
    fn sqlite_count_for_session() {
        let store = SqliteMemoryStore::new(temp_db_path("count")).unwrap();
        store
            .put(make_record("p", CognitiveMemoryType::Procedural))
            .unwrap();
        store
            .put(make_record("q", CognitiveMemoryType::Procedural))
            .unwrap();

        let session = SessionId::from_uuid(Uuid::nil());
        assert_eq!(store.count_for_session(&session).unwrap(), 2);
    }

    #[test]
    fn sqlite_descriptor_identifies_sqlite() {
        let store = SqliteMemoryStore::new(temp_db_path("desc")).unwrap();
        assert!(store.descriptor().identity.contains("sqlite"));
    }

    #[test]
    fn sqlite_persists_across_reopen() {
        let path = temp_db_path("reopen");
        {
            let store = SqliteMemoryStore::new(&path).unwrap();
            store
                .put(make_record("durable", CognitiveMemoryType::Semantic))
                .unwrap();
        }
        let store = SqliteMemoryStore::new(&path).unwrap();
        let records = store.list(CognitiveMemoryType::Semantic).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key, "durable");
    }
}
