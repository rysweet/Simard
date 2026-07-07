//! TDD (Step 7) — creative-ideas goal routing is visible on the SAME store, and
//! read faults are surfaced, not silently swallowed (bug #2896).
//!
//! Bug #2896: an accepted [`CreativeIdea`] is routed to a goal, the write path
//! reports success, yet `simard goal list --tag source:creative-ideas` returns
//! zero. Two guarantees the fix must deliver, pinned here end-to-end through the
//! real routing seam ([`route_idea_to_goal`]) and the production
//! [`CognitiveMemoryGoalStore`]:
//!
//!   1. **Visibility (guard):** routing an accepted idea and then reading the
//!      SAME cognitive-memory store returns the goal, tagged
//!      `source:creative-ideas`. This is the persistence-and-visibility contract
//!      that #2896 says is broken in production.
//!   2. **Fail-closed read (RED before the fix):** when the write reports success
//!      ("0 review error(s)") but the subsequent read hits a transport fault, the
//!      store MUST surface `Err` — never a phantom empty list that makes the goal
//!      look lost.
//!
//! Hermetic: no network, injected clock (`now_epoch`), `TempDir` state roots.
//! Test 1 uses the real library-backed tier-2 store (no daemon). Test 2 injects a
//! fault backend as the in-process writer. Both mutate process-global memory
//! state, so they are `#[serial_test::serial(cognitive_memory)]` and reset it.

use std::sync::Arc;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::cognitive_memory::creative_idea::{CreativeIdea, IdeaContext, IdeaStatus};
use crate::creative_ideas::pipeline::{CognitiveMemoryGoalStoreFactory, GoalStoreFactory};
use crate::error::{SimardError, SimardResult};
use crate::goal_curation::labels::SOURCE_CREATIVE_IDEAS;
use crate::goals::{CognitiveMemoryGoalStore, GoalStatus, GoalStore, goal_slug};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::memory_ipc::{
    clear_in_process_writer, clear_tier2_store_cache, launch_writer_client,
    register_in_process_writer,
};

use super::routing::route_idea_to_goal;

fn accepted_idea(title: &str, node_id: &str) -> CreativeIdea {
    let mut idea = CreativeIdea::new(
        title,
        IdeaContext {
            source: "creative-ideas-thread".to_string(),
            goals_snapshot: vec!["improve recall".to_string()],
            observation_digest: "digest-2896".to_string(),
            rationale: "recall precision has plateaued for 3 days".to_string(),
        },
        1_700_000_000,
    );
    idea.node_id = node_id.to_string();
    idea.status = IdeaStatus::AcceptedForImplementation;
    idea
}

/// Guard: route an accepted idea through the REAL cognitive-memory goal store
/// (tier-2, no daemon) and prove the goal is visible via a subsequent `list()` on
/// the same state root, tagged `source:creative-ideas`. This is the end-to-end
/// persistence-and-visibility contract #2896 requires: the write and the read
/// must address the same store.
#[test]
#[serial_test::serial(cognitive_memory)]
fn routed_creative_idea_goal_is_visible_via_same_cognitive_memory_store() {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();

    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    let idea = accepted_idea("distill meeting transcripts into facts", "pro_idea_2896");

    let record = route_idea_to_goal(&idea, &store, 1_700_000_500).expect("route to goal");
    assert_eq!(record.status, GoalStatus::Proposed);

    // A SUBSEQUENT read of the same store must return the routed goal — this is
    // the read that returns 0 in production today.
    let goals = store.list().expect("list goals");
    clear_tier2_store_cache();
    clear_in_process_writer();

    let persisted = goals
        .iter()
        .find(|g| g.slug == goal_slug(&idea.idea))
        .unwrap_or_else(|| {
            panic!(
                "routed creative-idea goal must be visible on the same store, got {} goal(s): \
                 {:?} (bug #2896: goals silently lost)",
                goals.len(),
                goals.iter().map(|g| &g.slug).collect::<Vec<_>>(),
            )
        });
    assert!(
        persisted.labels.iter().any(|l| l == SOURCE_CREATIVE_IDEAS),
        "the visible goal must carry source:creative-ideas so `goal list --tag \
         source:creative-ideas` finds it; labels were {:?}",
        persisted.labels,
    );
}

/// Seam #3 (bug #2896): the production [`CognitiveMemoryGoalStoreFactory`]
/// reuses the caller's live in-process memory handle, so a routed goal is
/// visible via that SAME handle — visible **by construction**, independent of
/// any `state_root` tier-0 match. This is the "reuse `ctx.memory`" contract the
/// daemon and dashboard routing paths depend on to guarantee the write lands in
/// the store the daemon serves `goal list` from.
#[test]
#[serial_test::serial(cognitive_memory)]
fn production_factory_routes_to_the_caller_supplied_in_process_handle() {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();

    // A real, shared in-process cognitive-memory handle (the tier-2 library
    // store, no daemon), standing in for the daemon's live `ctx.memory`.
    let writer = launch_writer_client(&root).expect("open in-process writer");

    // Route through the PRODUCTION factory, handing it the live handle.
    let goals = CognitiveMemoryGoalStoreFactory
        .open(writer.ops(), &root)
        .expect("factory open");
    let idea = accepted_idea("route via the live handle", "pro_idea_seam3");
    let record = route_idea_to_goal(&idea, goals.as_ref(), 1_700_000_900).expect("route to goal");
    assert_eq!(record.status, GoalStatus::Proposed);

    // The goal is visible on the SAME handle the write went through.
    let listed = goals.list().expect("list via the same handle");
    clear_tier2_store_cache();
    clear_in_process_writer();

    let found = listed
        .iter()
        .find(|g| g.slug == goal_slug(&idea.idea))
        .unwrap_or_else(|| {
            panic!(
                "routed goal must be visible via the caller's own memory handle, got {} goal(s): \
                 {:?} (bug #2896: reuse ctx.memory)",
                listed.len(),
                listed.iter().map(|g| &g.slug).collect::<Vec<_>>(),
            )
        });
    assert!(
        found.labels.iter().any(|l| l == SOURCE_CREATIVE_IDEAS),
        "the visible goal must carry source:creative-ideas; labels were {:?}",
        found.labels,
    );
}

/// A backend that ACKS writes (so `put` reports success, mirroring the bug's
/// "0 review error(s)") but FAILS reads with a memory-ipc transport error,
/// mirroring the "memory 'memory-ipc' transport error: write-len: Broken pipe"
/// lines in the live logs.
struct WriteOkReadFaultMemory;

impl WriteOkReadFaultMemory {
    fn read_err() -> SimardError {
        SimardError::RpcCallFailed {
            endpoint: "memory-ipc".to_string(),
            method: "search_facts".to_string(),
            reason: "write-len: Broken pipe (os error 32)".to_string(),
        }
    }
}

impl CognitiveMemoryOps for WriteOkReadFaultMemory {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sensory".to_string())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("working".to_string())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        _c: &str,
        _s: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Ok("episode".to_string())
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        _c: &str,
        _co: &str,
        _cf: f64,
        _t: &[String],
        _s: &str,
    ) -> SimardResult<String> {
        Ok("fact".to_string())
    }
    fn store_fact_with_caller_key(
        &self,
        _caller_key: &str,
        _concept: &str,
        _content: &str,
        _confidence: f64,
        _tags: &[String],
        _source_id: &str,
    ) -> SimardResult<String> {
        // Write "succeeds" — the router will count routed_goal and report 0 errors.
        Ok("fact".to_string())
    }
    fn search_facts(&self, _q: &str, _l: u32, _m: f64) -> SimardResult<Vec<CognitiveFact>> {
        Err(Self::read_err())
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("procedure".to_string())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _tc: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("prospective".to_string())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }
}

/// RED (fails before the #2896 fix): the full silent-loss signature. Routing an
/// accepted idea reports SUCCESS (put returns Ok, matching "N → goal, 0 review
/// error(s)"), but the subsequent `list()` read hits a transport fault. Today
/// that fault is swallowed to `Ok(Vec::new())`, so the goal is silently lost.
/// After the fix, `list()` MUST surface `Err`, turning a phantom success into a
/// visible failure the caller can react to.
#[test]
#[serial_test::serial(cognitive_memory)]
fn routing_reports_success_but_read_fault_is_surfaced_not_silently_lost() {
    clear_in_process_writer();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let mem: Arc<dyn CognitiveMemoryOps> = Arc::new(WriteOkReadFaultMemory);
    register_in_process_writer(root.clone(), Arc::clone(&mem));

    let store = CognitiveMemoryGoalStore::new(root.clone()).expect("construct goal store");
    let idea = accepted_idea("cache distilled facts by concept", "pro_idea_lost");

    // The write path reports success — exactly what the daemon telemetry shows.
    let routed = route_idea_to_goal(&idea, &store, 1_700_000_777);
    assert!(
        routed.is_ok(),
        "precondition: the write path reports success (bug #2896 telemetry says \
         '0 review error(s)'); got {routed:?}",
    );

    // The read that backs `goal list` must NOT hide the transport fault behind an
    // empty result — that empty result is the silent data loss of #2896.
    let listed = store.list();

    clear_in_process_writer();
    assert!(
        listed.is_err(),
        "a broken-pipe read after a 'successful' route MUST surface as Err, not a \
         phantom empty list that drops the goal (bug #2896). Got: {listed:?}",
    );
}
