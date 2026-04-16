use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::persistence::{load_json_or_default, persist_json};

use super::{GoalRecord, GoalStatus};

const GOAL_STORE_NAME: &str = "goals";

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
        let mut records = self
            .records
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: GOAL_STORE_NAME.to_string(),
            })?;
        upsert_record(&mut records, record);
        self.persist(&records)
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

    fn make_descriptor() -> BackendDescriptor {
        BackendDescriptor::new(
            "goals::test",
            Provenance::injected("test:goal-store"),
            Freshness::now().expect("freshness should be observable"),
        )
    }

    // ---- InMemoryGoalStore ----

    #[test]
    fn in_memory_list_empty_initially() {
        let store = InMemoryGoalStore::new(make_descriptor());
        let list = store.list().expect("list should succeed");
        assert!(list.is_empty());
    }

    #[test]
    fn in_memory_try_default_creates_valid_store() {
        let store = InMemoryGoalStore::try_default().expect("try_default should succeed");
        assert!(store.list().expect("list should succeed").is_empty());
    }

    #[test]
    fn in_memory_put_and_list_round_trip() {
        let store = InMemoryGoalStore::new(make_descriptor());
        store
            .put(goal_record("Goal A", GoalStatus::Active, 1))
            .unwrap();
        store
            .put(goal_record("Goal B", GoalStatus::Proposed, 2))
            .unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn in_memory_upsert_replaces_existing_record_by_slug() {
        let store = InMemoryGoalStore::new(make_descriptor());
        store
            .put(goal_record("Goal A", GoalStatus::Active, 3))
            .unwrap();
        store
            .put(goal_record("Goal A", GoalStatus::Completed, 1))
            .unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, GoalStatus::Completed);
    }

    #[test]
    fn in_memory_top_goals_by_status_filters_correctly() {
        let store = InMemoryGoalStore::new(make_descriptor());
        store
            .put(goal_record("Active 1", GoalStatus::Active, 1))
            .unwrap();
        store
            .put(goal_record("Proposed 1", GoalStatus::Proposed, 1))
            .unwrap();
        store
            .put(goal_record("Active 2", GoalStatus::Active, 2))
            .unwrap();
        let active = store.top_goals_by_status(GoalStatus::Active, 10).unwrap();
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|r| r.status == GoalStatus::Active));
    }

    #[test]
    fn in_memory_top_goals_respects_limit() {
        let store = InMemoryGoalStore::new(make_descriptor());
        for i in 1..=5 {
            store
                .put(goal_record(&format!("G{i}"), GoalStatus::Active, i))
                .unwrap();
        }
        let top2 = store.active_top_goals(2).unwrap();
        assert_eq!(top2.len(), 2);
    }

    #[test]
    fn in_memory_descriptor_returns_stored_descriptor() {
        let desc = make_descriptor();
        let store = InMemoryGoalStore::new(desc.clone());
        assert_eq!(store.descriptor().identity, desc.identity);
    }

    // ---- FileBackedGoalStore ----

    #[test]
    fn file_backed_try_new_creates_store() {
        let dir = tempfile::tempdir().expect("tempdir should create");
        let path = dir.path().join("goals.json");
        let store = FileBackedGoalStore::try_new(&path).expect("try_new should succeed");
        assert!(store.list().unwrap().is_empty());
        assert_eq!(store.path(), path);
    }

    #[test]
    fn file_backed_put_persists_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir should create");
        let path = dir.path().join("goals.json");
        let store = FileBackedGoalStore::try_new(&path).unwrap();
        store
            .put(goal_record("Persist me", GoalStatus::Active, 1))
            .unwrap();
        drop(store);
        // Reload from same path
        let store2 = FileBackedGoalStore::try_new(&path).unwrap();
        let list = store2.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "Persist me");
    }

    #[test]
    fn file_backed_upsert_persists_update() {
        let dir = tempfile::tempdir().expect("tempdir should create");
        let path = dir.path().join("goals.json");
        let store = FileBackedGoalStore::try_new(&path).unwrap();
        store
            .put(goal_record("Goal X", GoalStatus::Active, 1))
            .unwrap();
        store
            .put(goal_record("Goal X", GoalStatus::Completed, 1))
            .unwrap();
        drop(store);
        let store2 = FileBackedGoalStore::try_new(&path).unwrap();
        let list = store2.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].status, GoalStatus::Completed);
    }

    #[test]
    fn file_backed_top_goals_by_status_filters() {
        let dir = tempfile::tempdir().expect("tempdir should create");
        let path = dir.path().join("goals.json");
        let store = FileBackedGoalStore::try_new(&path).unwrap();
        store.put(goal_record("A", GoalStatus::Active, 1)).unwrap();
        store
            .put(goal_record("B", GoalStatus::Proposed, 1))
            .unwrap();
        let proposed = store.top_goals_by_status(GoalStatus::Proposed, 5).unwrap();
        assert_eq!(proposed.len(), 1);
        assert_eq!(proposed[0].title, "B");
    }

    // ---- sorting ----

    #[test]
    fn sorted_goal_records_orders_by_status_then_priority_then_title() {
        let records = vec![
            goal_record("Z Active", GoalStatus::Active, 2),
            goal_record("A Active", GoalStatus::Active, 1),
            goal_record("Proposed", GoalStatus::Proposed, 1),
        ];
        let sorted = sorted_goal_records(records);
        assert_eq!(sorted[0].title, "A Active");
        assert_eq!(sorted[1].title, "Z Active");
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
