//! Cognitive-memory backed [`GoalStore`] adapter.
//!
//! See `docs/reference/cognitive-memory-goal-store.md` for the design.
//!
//! `bootstrap::assembly` is the production caller: every per-record write
//! flows through cognitive memory via the bridge helpers
//! (`launch_writer_bridge` / `open_reader_bridge`). The legacy on-disk
//! `goal_records.json` file is no longer produced — closing the
//! half-migration gap that PR #1593 / PR #1600 left behind for the
//! bootstrap-assembled local sessions.
//!
//! Storage encoding: each [`GoalRecord`] is serialised as a
//! `goal-store:record` fact whose content is the JSON record. Reads
//! gather every `goal-store:record` fact, group by slug, keep the latest
//! by node_id (UUID-v7 — time-ordered), and deserialise. This mirrors
//! the [`crate::goal_curation::load_goal_board`] pattern and is robust
//! against the trait's append-only semantics (no UPDATE / DELETE).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{SimardError, SimardResult};
use crate::memory_ipc::{launch_writer_bridge, open_reader_bridge};
use crate::metadata::{BackendDescriptor, Freshness};

use super::{GoalRecord, GoalStatus, GoalStore};

/// Concept under which goal records are filed in cognitive memory.
const GOAL_STORE_FACT_CONCEPT: &str = "goal-store:record";
/// Source label recorded with every fact.
const GOAL_STORE_SOURCE: &str = "goal-store";
/// Tag recorded with every fact.
const GOAL_STORE_TAG: &str = "goal-store";
/// Pull window for `list()` reads. The board enforces
/// `MAX_ACTIVE_GOALS = 5`; even with status churn the per-process record
/// count stays modest, so 256 covers realistic deployments without
/// risking truncation.
const GOAL_STORE_LIST_LIMIT: u32 = 256;

/// `GoalStore` implementation backed by cognitive memory through the
/// bridge helpers (`launch_writer_bridge` / `open_reader_bridge`).
///
/// Each method opens a fresh bridge for the duration of one call and
/// drops it afterwards. With the tier-0 in-process Arc shortcut
/// registered by the OODA daemon (issue #1590 follow-up), per-call
/// acquisition inside the daemon process is a single `RwLock` read plus
/// `Arc::clone` — no IPC, no disk re-open.
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

    /// Serialise `record` to JSON for `store_fact` content.
    fn encode(record: &GoalRecord) -> SimardResult<String> {
        serde_json::to_string(record).map_err(|e| SimardError::InvalidGoalRecord {
            field: "goal_record".to_string(),
            reason: format!("failed to serialise goal record: {e}"),
        })
    }

    /// Read all goal records currently visible in cognitive memory and
    /// dedup by slug, keeping the latest write per slug.
    fn list_via_reader(&self) -> SimardResult<Vec<GoalRecord>> {
        // The reader bridge resolves through the in-process Arc shortcut
        // first (zero-cost for daemon callers), then the IPC socket,
        // then `open_read_only`. If none succeed (e.g. an uninitialised
        // state_root), `list()` returns an empty Vec rather than
        // surfacing the error — `GoalStore::list` is best-effort and the
        // FileBackedGoalStore behaved the same way (`load_json_or_default`).
        let reader = match open_reader_bridge(&self.state_root) {
            Ok(r) => r,
            Err(_) => return Ok(Vec::new()),
        };
        let facts =
            match reader
                .ops()
                .search_facts(GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, 0.0)
            {
                Ok(f) => f,
                Err(e) => {
                    eprintln!(
                        "[simard] CognitiveMemoryGoalStore::list: search_facts failed ({e}) — \
                     returning empty record set"
                    );
                    return Ok(Vec::new());
                }
            };

        // For each slug, keep the fact with the largest node_id (most
        // recent UUID-v7).
        let mut latest_by_slug: HashMap<String, (String, GoalRecord)> = HashMap::new();
        for fact in facts {
            if fact.concept != GOAL_STORE_FACT_CONCEPT {
                continue;
            }
            let record: GoalRecord = match serde_json::from_str(&fact.content) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!(
                        "[simard] CognitiveMemoryGoalStore::list: skipping unparseable record \
                         (node_id={}): {e}",
                        fact.node_id
                    );
                    continue;
                }
            };
            let slug = record.slug.clone();
            match latest_by_slug.get(&slug) {
                Some((existing_id, _)) if existing_id >= &fact.node_id => {}
                _ => {
                    latest_by_slug.insert(slug, (fact.node_id, record));
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
        let content = Self::encode(&record)?;
        let writer = launch_writer_bridge(&self.state_root)?;
        writer.ops().store_fact(
            GOAL_STORE_FACT_CONCEPT,
            &content,
            1.0,
            &[GOAL_STORE_TAG.to_string()],
            GOAL_STORE_SOURCE,
        )?;
        Ok(())
    }

    fn list(&self) -> SimardResult<Vec<GoalRecord>> {
        self.list_via_reader()
    }

    fn top_goals_by_status(
        &self,
        status: GoalStatus,
        limit: usize,
    ) -> SimardResult<Vec<GoalRecord>> {
        let mut records = self.list()?;
        records.retain(|r| r.status == status);
        records.sort_by(compare_goal_records);
        records.truncate(limit);
        Ok(records)
    }

    fn active_top_goals(&self, limit: usize) -> SimardResult<Vec<GoalRecord>> {
        self.top_goals_by_status(GoalStatus::Active, limit)
    }
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

    #[test]
    fn cognitive_memory_goal_store_put_overwrites_existing_slug_with_latest_record() {
        let root = fresh_state_root("upsert");
        let store = CognitiveMemoryGoalStore::new(root).expect("store should build");
        store
            .put(record("Same goal", GoalStatus::Proposed, 3))
            .unwrap();
        // Re-put with a different status / priority — UUID-v7 ordering
        // ensures the second write is "latest" and `list()` returns it.
        // Sleep a hair to guarantee monotonic timestamps.
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .put(record("Same goal", GoalStatus::Active, 1))
            .unwrap();

        let listed = store.list().unwrap();
        let same_goal: Vec<_> = listed.iter().filter(|r| r.slug == "same-goal").collect();
        assert_eq!(
            same_goal.len(),
            1,
            "list() must dedup by slug and surface the latest record only"
        );
        assert_eq!(same_goal[0].status, GoalStatus::Active);
        assert_eq!(same_goal[0].priority, 1);
    }
}
