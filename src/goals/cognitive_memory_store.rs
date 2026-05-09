//! Cognitive-memory backed [`GoalStore`] adapter.
//!
//! See `docs/reference/cognitive-memory-goal-store.md` for the design.
//!
//! `bootstrap::assembly` is the production caller: today it constructs an
//! `Arc<FileBackedGoalStore>` for `RuntimePorts.goal_store`, which leaves
//! a half-migration gap (every other consumer reads/writes through
//! cognitive memory; only the bootstrap-assembled local sessions still
//! touch `goal_records.json`). Step 8 of the issue #1590 follow-up
//! replaces that instantiation with `Arc::new(CognitiveMemoryGoalStore::new(...))`.
//!
//! **TDD stub** — every `GoalStore` method body is `unimplemented!()` so
//! the failing tests in this module pin the contract before the
//! implementation lands.

use std::path::PathBuf;

use crate::error::SimardResult;
use crate::metadata::{BackendDescriptor, Freshness};

use super::{GoalRecord, GoalStatus, GoalStore};

/// `GoalStore` implementation backed by cognitive memory through the
/// bridge helpers (`launch_writer_bridge` / `open_reader_bridge`).
///
/// Each method opens a fresh bridge for the duration of one call and
/// drops it afterwards. With the planned tier-0 in-process Arc shortcut
/// (issue #1590 follow-up), per-call acquisition inside the daemon
/// process is a single `OnceLock::get` plus `Arc::clone`.
#[derive(Debug)]
pub struct CognitiveMemoryGoalStore {
    state_root: PathBuf,
    descriptor: BackendDescriptor,
}

impl CognitiveMemoryGoalStore {
    /// Construct a store rooted at `state_root`.
    ///
    /// The path must be the same `SIMARD_STATE_ROOT`-resolved directory
    /// the rest of the runtime addresses (i.e. `default_state_root()`).
    pub fn new(state_root: PathBuf) -> SimardResult<Self> {
        Ok(Self {
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "goals::cognitive-memory-store",
                "runtime-port:goal-store:cognitive-memory",
                Freshness::now()?,
            ),
            state_root,
        })
    }

    /// State root this store is bound to (used by tests and diagnostics).
    pub fn state_root(&self) -> &PathBuf {
        &self.state_root
    }
}

impl GoalStore for CognitiveMemoryGoalStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        let _ = record;
        unimplemented!(
            "CognitiveMemoryGoalStore::put: TDD stub — implementation lands in \
             step 8 of issue #1590 follow-up"
        );
    }

    fn list(&self) -> SimardResult<Vec<GoalRecord>> {
        unimplemented!(
            "CognitiveMemoryGoalStore::list: TDD stub — implementation lands in \
             step 8 of issue #1590 follow-up"
        );
    }

    fn top_goals_by_status(
        &self,
        status: GoalStatus,
        limit: usize,
    ) -> SimardResult<Vec<GoalRecord>> {
        let _ = (status, limit);
        unimplemented!(
            "CognitiveMemoryGoalStore::top_goals_by_status: TDD stub — implementation \
             lands in step 8 of issue #1590 follow-up"
        );
    }

    fn active_top_goals(&self, limit: usize) -> SimardResult<Vec<GoalRecord>> {
        let _ = limit;
        unimplemented!(
            "CognitiveMemoryGoalStore::active_top_goals: TDD stub — implementation \
             lands in step 8 of issue #1590 follow-up"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goals::{GoalRecord, GoalStatus, GoalUpdate};
    use crate::session::{SessionId, SessionPhase};
    use std::path::PathBuf;

    fn fresh_state_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "simard-cognitive-goal-store-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir should be created");
        dir
    }

    fn record(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
        GoalRecord::from_update(
            GoalUpdate::new(title, "tdd rationale", status, priority).expect("valid update"),
            "tdd-1590-cognitive-store",
            SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")
                .expect("valid session id"),
            SessionPhase::Persistence,
        )
        .expect("valid record")
    }

    #[test]
    fn cognitive_memory_goal_store_round_trips_active_goal() {
        let root = fresh_state_root("round-trip");
        let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store should build");

        store
            .put(record(
                "Cognitive store round-trip goal",
                GoalStatus::Active,
                1,
            ))
            .expect("put must persist through cognitive memory");

        let listed = store
            .list()
            .expect("list must read through cognitive memory");
        assert!(
            listed
                .iter()
                .any(|r| r.title == "Cognitive store round-trip goal"),
            "round-tripped goal must appear in list(); got {} records",
            listed.len()
        );

        // Every write must flow through cognitive memory — never the
        // legacy file. This is the half-migration we're closing.
        let legacy = root.join("goal_records.json");
        assert!(
            !legacy.exists(),
            "CognitiveMemoryGoalStore must NOT create {}",
            legacy.display()
        );
    }

    #[test]
    fn cognitive_memory_goal_store_active_top_goals_returns_active_only() {
        let root = fresh_state_root("active-top");
        let store = CognitiveMemoryGoalStore::new(root).expect("store should build");
        store
            .put(record("Active alpha", GoalStatus::Active, 2))
            .unwrap();
        store
            .put(record("Active beta", GoalStatus::Active, 1))
            .unwrap();
        store
            .put(record("Proposed gamma", GoalStatus::Proposed, 1))
            .unwrap();

        let top = store.active_top_goals(5).expect("active_top_goals");
        assert_eq!(
            top.len(),
            2,
            "active_top_goals must filter to active records only"
        );
        assert!(top.iter().all(|r| r.status == GoalStatus::Active));
        // Sort key is (status_rank, priority, title, slug); priority 1
        // wins over priority 2.
        assert_eq!(top[0].title, "Active beta");
        assert_eq!(top[1].title, "Active alpha");
    }

    #[test]
    fn cognitive_memory_goal_store_top_goals_by_status_filters_correctly() {
        let root = fresh_state_root("top-by-status");
        let store = CognitiveMemoryGoalStore::new(root).expect("store should build");
        store
            .put(record("Active alpha", GoalStatus::Active, 1))
            .unwrap();
        store
            .put(record("Proposed beta", GoalStatus::Proposed, 1))
            .unwrap();
        store
            .put(record("Proposed gamma", GoalStatus::Proposed, 2))
            .unwrap();

        let proposed = store
            .top_goals_by_status(GoalStatus::Proposed, 5)
            .expect("top_goals_by_status");
        assert_eq!(proposed.len(), 2);
        assert!(proposed.iter().all(|r| r.status == GoalStatus::Proposed));
    }
}
