use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::{load_json_or_default, persist_json};

use super::{GoalRecord, GoalStatus};

const GOAL_STORE_NAME: &str = "goals";

/// RAII guard that acquires an `flock` on construction and releases on drop.
#[cfg(unix)]
struct FlockGuard {
    file: std::fs::File,
}

#[cfg(unix)]
impl FlockGuard {
    fn exclusive(path: &Path) -> SimardResult<Self> {
        let file = Self::open_lockfile(path)?;
        Self::flock_op(&file, path, libc::LOCK_EX)?;
        Ok(Self { file })
    }

    fn shared(path: &Path) -> SimardResult<Self> {
        let file = Self::open_lockfile(path)?;
        Self::flock_op(&file, path, libc::LOCK_SH)?;
        Ok(Self { file })
    }

    fn open_lockfile(store_path: &Path) -> SimardResult<std::fs::File> {
        let lock_path = lockfile_path(store_path);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| SimardError::PersistentStoreIo {
                store: GOAL_STORE_NAME.to_string(),
                action: "open-lockfile".to_string(),
                path: lock_path,
                reason: e.to_string(),
            })
    }

    fn flock_op(file: &std::fs::File, store_path: &Path, op: libc::c_int) -> SimardResult<()> {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(file.as_raw_fd(), op) };
        if ret != 0 {
            return Err(SimardError::PersistentStoreIo {
                store: GOAL_STORE_NAME.to_string(),
                action: "flock".to_string(),
                path: store_path.to_path_buf(),
                reason: format!("flock failed: {}", std::io::Error::last_os_error()),
            });
        }
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for FlockGuard {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn lockfile_path(store_path: &Path) -> PathBuf {
    store_path.with_extension("json.lock")
}

pub trait GoalStore: Send + Sync {
    fn descriptor(&self) -> BackendDescriptor;

    fn put(&self, record: GoalRecord) -> SimardResult<()>;

    fn list(&self) -> SimardResult<Vec<GoalRecord>>;

    fn top_goals_by_status(
        &self,
        status: GoalStatus,
        limit: usize,
    ) -> SimardResult<Vec<GoalRecord>>;

    fn active_top_goals(&self, limit: usize) -> SimardResult<Vec<GoalRecord>>;
}

#[derive(Debug)]
pub struct InMemoryGoalStore {
    records: Mutex<Vec<GoalRecord>>,
    descriptor: BackendDescriptor,
}

impl InMemoryGoalStore {
    pub fn new(descriptor: BackendDescriptor) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            descriptor,
        }
    }

    pub fn try_default() -> SimardResult<Self> {
        Ok(Self::new(BackendDescriptor::for_runtime_type::<Self>(
            "goals::in-memory",
            "runtime-port:goal-store",
            Freshness::now()?,
        )))
    }
}

#[derive(Debug)]
pub struct FileBackedGoalStore {
    records: Mutex<Vec<GoalRecord>>,
    path: PathBuf,
    descriptor: BackendDescriptor,
}

impl FileBackedGoalStore {
    pub fn new(path: impl Into<PathBuf>, descriptor: BackendDescriptor) -> SimardResult<Self> {
        let path = path.into();
        Ok(Self {
            records: Mutex::new(load_json_or_default(GOAL_STORE_NAME, &path)?),
            path,
            descriptor,
        })
    }

    pub fn try_new(path: impl Into<PathBuf>) -> SimardResult<Self> {
        let path = path.into();
        Self::new(
            path,
            BackendDescriptor::for_runtime_type::<Self>(
                "goals::json-file-store",
                "runtime-port:goal-store:file-json",
                Freshness::now()?,
            ),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&self, records: &[GoalRecord]) -> SimardResult<()> {
        persist_json(GOAL_STORE_NAME, &self.path, &records.to_vec())
    }

    fn reload_from_disk(&self) -> SimardResult<Vec<GoalRecord>> {
        load_json_or_default(GOAL_STORE_NAME, &self.path)
    }
}

impl GoalStore for InMemoryGoalStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: GOAL_STORE_NAME.to_string(),
            })?;
        upsert_record(&mut records, record);
        Ok(())
    }

    fn list(&self) -> SimardResult<Vec<GoalRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: GOAL_STORE_NAME.to_string(),
            })?
            .clone())
    }

    fn top_goals_by_status(
        &self,
        status: GoalStatus,
        limit: usize,
    ) -> SimardResult<Vec<GoalRecord>> {
        let records = self.list()?;
        Ok(sorted_goal_records(records)
            .into_iter()
            .filter(|record| record.status == status)
            .take(limit)
            .collect())
    }

    fn active_top_goals(&self, limit: usize) -> SimardResult<Vec<GoalRecord>> {
        self.top_goals_by_status(GoalStatus::Active, limit)
    }
}

impl GoalStore for FileBackedGoalStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        // Acquire exclusive flock for cross-process safety.
        #[cfg(unix)]
        let _lock = FlockGuard::exclusive(&self.path)?;

        let mut cache = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: GOAL_STORE_NAME.to_string(),
            })?;

        // Reload from disk to pick up writes from other processes.
        let mut records = self.reload_from_disk()?;
        upsert_record(&mut records, record);

        // Persist first; only update cache on success.
        self.persist(&records)?;
        *cache = records;
        Ok(())
    }

    fn list(&self) -> SimardResult<Vec<GoalRecord>> {
        // Acquire shared flock for cross-process consistency.
        #[cfg(unix)]
        let _lock = FlockGuard::shared(&self.path)?;

        let mut cache = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: GOAL_STORE_NAME.to_string(),
            })?;

        // Reload from disk to see writes from other processes.
        let records = self.reload_from_disk()?;
        *cache = records.clone();
        Ok(records)
    }

    fn top_goals_by_status(
        &self,
        status: GoalStatus,
        limit: usize,
    ) -> SimardResult<Vec<GoalRecord>> {
        let records = self.list()?;
        Ok(sorted_goal_records(records)
            .into_iter()
            .filter(|record| record.status == status)
            .take(limit)
            .collect())
    }

    fn active_top_goals(&self, limit: usize) -> SimardResult<Vec<GoalRecord>> {
        self.top_goals_by_status(GoalStatus::Active, limit)
    }
}

fn upsert_record(records: &mut Vec<GoalRecord>, record: GoalRecord) {
    if let Some(existing) = records
        .iter_mut()
        .find(|existing| existing.slug == record.slug)
    {
        *existing = record;
    } else {
        records.push(record);
    }
}

fn sorted_goal_records(mut records: Vec<GoalRecord>) -> Vec<GoalRecord> {
    records.sort_by(compare_goal_records);
    records
}

fn compare_goal_records(left: &GoalRecord, right: &GoalRecord) -> Ordering {
    left.status
        .rank()
        .cmp(&right.status.rank())
        .then(left.priority.cmp(&right.priority))
        .then(left.title.cmp(&right.title))
        .then(left.slug.cmp(&right.slug))
}

#[cfg(test)]
mod tests {
    use crate::goals::{GoalRecord, GoalStatus, GoalUpdate};
    use crate::metadata::{Freshness, Provenance};
    use crate::session::{SessionId, SessionPhase};

    use super::*;

    fn goal_record(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
        GoalRecord::from_update(
            GoalUpdate::new(title, "keep Simard pointed at user goals", status, priority)
                .expect("goal update should be valid"),
            "simard-goal-curator",
            SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")
                .expect("session id should parse"),
            SessionPhase::Persistence,
        )
        .expect("goal record should be valid")
    }

    #[test]
    fn in_memory_goal_store_upserts_and_orders_active_goals() {
        let store = InMemoryGoalStore::new(BackendDescriptor::new(
            "goals::test",
            Provenance::injected("test:goal-store"),
            Freshness::now().expect("freshness should be observable"),
        ));
        store
            .put(goal_record(
                "Improve meeting handoff",
                GoalStatus::Active,
                2,
            ))
            .expect("active goal should store");
        store
            .put(goal_record("Keep backlog curated", GoalStatus::Active, 1))
            .expect("active goal should store");
        store
            .put(goal_record(
                "Future remote orchestration",
                GoalStatus::Proposed,
                1,
            ))
            .expect("proposed goal should store");
        store
            .put(goal_record("Keep backlog curated", GoalStatus::Active, 1))
            .expect("goal upsert should succeed");

        let active = store
            .active_top_goals(5)
            .expect("active top goals should be readable");
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].title, "Keep backlog curated");
        assert_eq!(active[1].title, "Improve meeting handoff");
    }
}
