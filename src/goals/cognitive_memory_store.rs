//! Cognitive-memory backed [`GoalStore`] adapter.
//!
//! See `docs/reference/cognitive-memory-goal-store.md` for the design.
//!
//! `bootstrap::assembly` is the production caller: previously it constructed
//! an `Arc<FileBackedGoalStore>` for `RuntimePorts.goal_store`, which left
//! a half-migration gap (every other consumer reads/writes through
//! cognitive memory; only the bootstrap-assembled local sessions still
//! touched `goal_records.json`). Issue #1590 closes that gap.

use std::path::PathBuf;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::goal_curation::{GoalProgress, load_goal_board, save_goal_board};
use crate::memory_ipc::launch_writer_bridge;
use crate::metadata::{BackendDescriptor, Freshness};

use super::{GoalRecord, GoalStatus, GoalStore};

/// Concept prefix used to namespace serialized `GoalRecord` facts in
/// cognitive memory. The full concept is
/// `goal-record:{slug}` so a `search_facts("goal-record:", …)` returns
/// every record across all slugs.
const GOAL_RECORD_PREFIX: &str = "goal-record:";

/// Owner-identity to attribute board snapshots to when projecting from
/// `GoalRecord`s. Boards persisted by the goal-curation pipeline use
/// `"goal-curator"`; we intentionally use a different label here so the
/// origin of each snapshot is auditable.
const GOAL_STORE_SOURCE: &str = "cognitive-memory-goal-store";

/// `GoalStore` implementation backed by cognitive memory through the
/// bridge helpers (`launch_writer_bridge` / `open_reader_bridge`).
///
/// Each method opens a fresh bridge for the duration of one call and
/// drops it afterwards. Inside the OODA daemon the tier-0 in-process Arc
/// shortcut means this is a single `RwLock::read` plus `Arc::clone`; in
/// out-of-process callers (operator probes spawned from cargo tests, the
/// `simard-operator-probe` binary, etc.) it walks the IPC / direct-open
/// ladder.
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

    /// Read every persisted `GoalRecord` through `bridge`. Records are
    /// deduplicated by `slug` — the lexicographically-largest `node_id`
    /// wins (uuid-v7 ⇒ time-ordered, so largest = most-recent put).
    fn list_via_bridge(&self, bridge: &dyn CognitiveMemoryOps) -> SimardResult<Vec<GoalRecord>> {
        let facts = bridge.search_facts(GOAL_RECORD_PREFIX, 1024, 0.0)?;
        let mut latest_by_slug: std::collections::BTreeMap<String, (String, GoalRecord)> =
            std::collections::BTreeMap::new();
        for fact in facts {
            if !fact.concept.starts_with(GOAL_RECORD_PREFIX) {
                continue;
            }
            let record: GoalRecord = match serde_json::from_str(&fact.content) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!(
                        "[simard] CognitiveMemoryGoalStore::list_via_bridge: skipping \
                         malformed goal-record fact (node_id={}): {e}",
                        fact.node_id
                    );
                    continue;
                }
            };
            match latest_by_slug.get(&record.slug) {
                Some((existing_id, _)) if existing_id >= &fact.node_id => continue,
                _ => {
                    latest_by_slug.insert(record.slug.clone(), (fact.node_id.clone(), record));
                }
            }
        }
        Ok(latest_by_slug.into_values().map(|(_, r)| r).collect())
    }
}

impl GoalStore for CognitiveMemoryGoalStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        let bridge = launch_writer_bridge(&self.state_root)?;
        let ops = bridge.ops();

        // (1) Persist the full record as a fact for fidelity. The
        // concept is `goal-record:{slug}` so list/queries can prefix-scan.
        let serialized = serde_json::to_string(&record).map_err(|e| {
            crate::error::SimardError::InvalidGoalRecord {
                field: "record".to_string(),
                reason: format!("failed to serialize GoalRecord: {e}"),
            }
        })?;
        let concept = format!("{GOAL_RECORD_PREFIX}{}", record.slug);
        ops.store_fact(
            &concept,
            &serialized,
            1.0,
            &[
                "goal-record".to_string(),
                record.status.to_string(),
                record.owner_identity.clone(),
            ],
            GOAL_STORE_SOURCE,
        )?;

        // (2) Project the active set into `GoalBoard.active` and persist
        // a fresh snapshot. This is what `engineer-loop-run` reads via
        // `load_goal_board` + `active_goals_as_records`. Without this
        // projection, a `goal_store.put(active record)` would be invisible
        // to the engineer-loop probe and the regression tests in
        // `tests/improvement_curation.rs` would fail their cross-subprocess
        // assertion (`Active goals count: 1`).
        let all = self.list_via_bridge(ops)?;
        let mut board = load_goal_board(ops)?;
        board.active = all
            .iter()
            .filter(|r| r.status == GoalStatus::Active)
            .map(record_to_active_goal)
            .collect();
        save_goal_board(&board, ops)?;
        Ok(())
    }

    fn list(&self) -> SimardResult<Vec<GoalRecord>> {
        let bridge = launch_writer_bridge(&self.state_root)?;
        self.list_via_bridge(bridge.ops())
    }

    fn top_goals_by_status(
        &self,
        status: GoalStatus,
        limit: usize,
    ) -> SimardResult<Vec<GoalRecord>> {
        let mut records: Vec<GoalRecord> = self
            .list()?
            .into_iter()
            .filter(|r| r.status == status)
            .collect();
        sort_goal_records(&mut records);
        records.truncate(limit);
        Ok(records)
    }

    fn active_top_goals(&self, limit: usize) -> SimardResult<Vec<GoalRecord>> {
        self.top_goals_by_status(GoalStatus::Active, limit)
    }
}

fn sort_goal_records(records: &mut [GoalRecord]) {
    records.sort_by(|a, b| {
        a.status
            .rank()
            .cmp(&b.status.rank())
            .then(a.priority.cmp(&b.priority))
            .then(a.title.cmp(&b.title))
            .then(a.slug.cmp(&b.slug))
    });
}

/// Project a `GoalRecord` into the `ActiveGoal` shape consumed by
/// `goal_curation::save_goal_board` / `load_goal_board`.
///
/// This is the inverse of `goal_curation::active_goals_as_records`, but
/// the two mappings are deliberately not symmetric: `ActiveGoal.id` is
/// the goal slug here so a subsequent `active_goals_as_records` round
/// trip preserves the slug.
fn record_to_active_goal(record: &GoalRecord) -> crate::goal_curation::ActiveGoal {
    crate::goal_curation::ActiveGoal {
        id: record.slug.clone(),
        description: record.title.clone(),
        priority: u32::from(record.priority),
        status: if record.status == GoalStatus::Completed {
            GoalProgress::Completed
        } else {
            GoalProgress::NotStarted
        },
        assigned_to: Some(record.owner_identity.clone()),
        current_activity: if record.rationale.is_empty() {
            None
        } else {
            Some(record.rationale.clone())
        },
        wip_refs: Vec::new(),
    }
}

// `GoalStatus::rank` is `pub(super)` in `super::types`; we are inside
// the `goals` module so the visibility is satisfied.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goals::{GoalRecord, GoalStatus, GoalUpdate};
    use crate::memory_ipc::unregister_in_process_writer_for_test;
    use crate::session::{SessionId, SessionPhase};
    use serial_test::serial;
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
    #[serial]
    fn cognitive_memory_goal_store_round_trips_active_goal() {
        unregister_in_process_writer_for_test();
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
    #[serial]
    fn cognitive_memory_goal_store_active_top_goals_returns_active_only() {
        unregister_in_process_writer_for_test();
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
    #[serial]
    fn cognitive_memory_goal_store_top_goals_by_status_filters_correctly() {
        unregister_in_process_writer_for_test();
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
