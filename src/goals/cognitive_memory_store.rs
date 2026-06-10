//! Cognitive-memory backed [`GoalStore`] adapter.
//!
//! See `docs/reference/cognitive-memory-goal-store.md` for the design.
//!
//! `bootstrap::assembly` and `meeting_backend` are the production callers:
//! every per-record write flows through cognitive memory via the bridge
//! helpers (`launch_writer_bridge` / `open_reader_bridge`). The legacy
//! on-disk `goal_records.json` and `state/goal_store.json` files are no
//! longer produced — closing the half-migration gap that PR #1593 /
//! PR #1600 / issue #1668 left behind.
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
pub(crate) const GOAL_STORE_FACT_CONCEPT: &str = "goal-store:record";
/// Source label recorded with every fact.
const GOAL_STORE_SOURCE: &str = "goal-store";
/// Tag recorded with every fact.
const GOAL_STORE_TAG: &str = "goal-store";
/// Description prefix for goal-related prospective memories, used to
/// distinguish them from non-goal prospective entries (e.g. meeting
/// action items).
const GOAL_PROSPECTIVE_PREFIX: &str = "goal:";
/// Pull window for `list()` reads. The board enforces
/// `MAX_ACTIVE_GOALS = 5`; even with status churn the per-process record
/// count stays modest, so 256 covers realistic deployments without
/// risking truncation.
pub(crate) const GOAL_STORE_LIST_LIMIT: u32 = 256;

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

    /// Reconcile drift between goal state and prospective memory entries.
    ///
    /// Loads all current goals via `list_via_reader()`, then for each:
    /// - **Active** goals: ensures a prospective trigger exists; creates one
    ///   if missing (e.g. due to a prior dual-write failure).
    /// - **Non-Active** goals: resolves any stale prospective triggers so
    ///   they no longer fire.
    ///
    /// Stops on the first error encountered. Callers should retry on
    /// transient failures. A future improvement may collect all errors
    /// instead of short-circuiting (see issue #2207).
    pub fn reconcile_prospectives(&self) -> SimardResult<()> {
        let goals = self.list_via_reader()?;
        if goals.is_empty() {
            return Ok(());
        }

        let writer = launch_writer_bridge(&self.state_root)?;

        for goal in &goals {
            let trigger = prospective_trigger_for(goal);

            // Check whether a prospective entry already exists for this slug.
            let existing = writer.ops().check_triggers(&trigger)?;
            let has_goal_prospective = existing.iter().any(|p| {
                p.description.starts_with(GOAL_PROSPECTIVE_PREFIX) && p.trigger_condition == trigger
            });

            if goal.status == GoalStatus::Active {
                if !has_goal_prospective {
                    // Drift: Active goal is missing its prospective trigger.
                    let description = format!("{}{}", GOAL_PROSPECTIVE_PREFIX, goal.title);
                    let action = format!(
                        "Pursue goal: {} (p{}, {})",
                        goal.title, goal.priority, goal.rationale,
                    );
                    writer.ops().store_prospective(
                        &description,
                        &trigger,
                        &action,
                        i64::from(goal.priority),
                    )?;
                }
            } else {
                // Non-Active goal: resolve any stale prospective entries.
                if has_goal_prospective {
                    resolve_goal_prospectives(&goal.slug, writer.ops())?;
                }
            }
        }

        Ok(())
    }
}

impl GoalStore for CognitiveMemoryGoalStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        let content = Self::encode(&record)?;
        let writer = launch_writer_bridge(&self.state_root)?;

        // Primary storage: semantic fact (authoritative record).
        writer.ops().store_fact(
            GOAL_STORE_FACT_CONCEPT,
            &content,
            1.0,
            &[GOAL_STORE_TAG.to_string()],
            GOAL_STORE_SOURCE,
        )?;

        // Resolve any previous prospective memory for this goal so stale
        // entries don't accumulate. Part of put()'s success contract (#2207):
        // if the mirror update fails, the caller must know.
        resolve_goal_prospectives(&record.slug, writer.ops())?;

        // Dual-write: store Active goals as prospective memories so they
        // surface via `check_triggers` during OODA preparation (#2207).
        if record.status == GoalStatus::Active {
            let description = format!("{}{}", GOAL_PROSPECTIVE_PREFIX, record.title);
            let trigger_condition = prospective_trigger_for(&record);
            let action_on_trigger = format!(
                "Pursue goal: {} (p{}, {})",
                record.title, record.priority, record.rationale,
            );
            writer.ops().store_prospective(
                &description,
                &trigger_condition,
                &action_on_trigger,
                i64::from(record.priority),
            )?;
        }

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

/// Build a trigger condition string for a goal's prospective memory.
///
/// Uses the goal slug with dashes replaced by spaces so that substring
/// matching in `check_triggers` fires when the OODA objective summary
/// mentions similar terms. For example, slug `"fix-broken-features"`
/// produces trigger `"fix broken features"`.
fn prospective_trigger_for(record: &GoalRecord) -> String {
    record.slug.replace('-', " ")
}

/// Resolve (mark as `"resolved"`) any pending prospective memories whose
/// description starts with the `GOAL_PROSPECTIVE_PREFIX` and matches the
/// given goal slug. This prevents accumulation of stale prospective entries
/// when a goal is re-put or transitions to Completed/Paused.
///
/// Uses `check_triggers` with the slug-derived trigger phrase, then filters
/// by the `goal:` description prefix and matching trigger_condition.
fn resolve_goal_prospectives(
    slug: &str,
    ops: &dyn crate::cognitive_memory::CognitiveMemoryOps,
) -> crate::error::SimardResult<()> {
    let trigger = slug.replace('-', " ");
    // check_triggers returns pending entries whose trigger_condition is a
    // substring of the probe string. We probe with the trigger itself so
    // an exact-match will surface the old entry.
    let candidates = ops.check_triggers(&trigger)?;
    for p in candidates {
        if p.description.starts_with(GOAL_PROSPECTIVE_PREFIX) && p.trigger_condition == trigger {
            ops.resolve_prospective(&p.node_id)?;
        }
    }
    Ok(())
}

/// One-time migration: if a legacy `state/goal_store.json` exists on disk
/// (from the `FileBackedGoalStore` era), read its records, write them into
/// cognitive memory, and rename the file to `state/goal_store.json.migrated`.
///
/// The migration is idempotent: once the file is renamed the `exists()` gate
/// short-circuits. Records whose slug already exists in cognitive memory are
/// skipped to avoid overwriting newer writes with stale file data.
///
/// All failures are logged and non-fatal — a corrupt or unreadable file is
/// left in place for operator inspection and the caller proceeds to the
/// cognitive-memory code path. The file is only renamed after ALL records
/// are successfully written so a partial failure leaves the file intact for
/// retry on next startup.
pub fn migrate_file_backed_goal_store_if_present(state_root: &std::path::Path) {
    let goal_store_path = state_root.join("state").join("goal_store.json");
    if !goal_store_path.exists() {
        return;
    }

    // Read the raw file to avoid `FileBackedGoalStore::try_new` side
    // effects (it copies `goal_records.json` → `goal_store.json`).
    let content = match std::fs::read_to_string(&goal_store_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[simard] goal_store migration: failed to read {} ({e}) — \
                 leaving file in place for next retry",
                goal_store_path.display()
            );
            return;
        }
    };

    let records: Vec<GoalRecord> = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[simard] goal_store migration: failed to parse {} ({e}) — \
                 leaving corrupt file in place for operator inspection",
                goal_store_path.display()
            );
            return;
        }
    };

    if records.is_empty() {
        // Nothing to migrate — rename and move on.
        rename_to_migrated(&goal_store_path);
        return;
    }

    // Open a single writer bridge for both reading existing slugs and
    // writing migrated records.  Using the writer bridge (not the
    // read-only reader bridge) is deliberate: write-mode opens replay
    // the WAL, handling the edge case where a prior writer left an
    // un-checkpointed WAL that read-only mode cannot recover.
    let writer = match launch_writer_bridge(state_root) {
        Ok(w) => w,
        Err(e) => {
            eprintln!(
                "[simard] goal_store migration: launch_writer_bridge failed ({e}) — \
                 leaving file in place for next retry"
            );
            return;
        }
    };

    // Read existing slugs from cognitive memory to skip duplicates.
    // If the DB file exists but we cannot read it, abort the migration
    // to avoid overwriting newer cognitive-memory records with stale
    // legacy data.
    let db_file = state_root.join("cognitive_memory.ladybug");
    let existing_slugs: std::collections::HashSet<String> = if db_file.exists() {
        match writer
            .ops()
            .search_facts(GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, 0.0)
        {
            Ok(facts) => facts
                .into_iter()
                .filter_map(|f| {
                    serde_json::from_str::<GoalRecord>(&f.content)
                        .ok()
                        .map(|r| r.slug)
                })
                .collect(),
            Err(e) => {
                eprintln!(
                    "[simard] goal_store migration: cannot read existing records ({e}) — \
                     leaving file in place for safety"
                );
                return;
            }
        }
    } else {
        std::collections::HashSet::new()
    };

    let mut all_ok = true;
    let mut migrated_count = 0usize;
    let mut skipped_count = 0usize;
    for record in &records {
        if existing_slugs.contains(&record.slug) {
            skipped_count += 1;
            continue;
        }
        let content = match serde_json::to_string(record) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[simard] goal_store migration: serialise failed for slug={} ({e}) — \
                     aborting migration, leaving file in place",
                    record.slug
                );
                all_ok = false;
                break;
            }
        };
        if let Err(e) = writer.ops().store_fact(
            GOAL_STORE_FACT_CONCEPT,
            &content,
            1.0,
            &[GOAL_STORE_TAG.to_string()],
            GOAL_STORE_SOURCE,
        ) {
            eprintln!(
                "[simard] goal_store migration: put failed for slug={} ({e}) — \
                 aborting migration, leaving file in place",
                record.slug
            );
            all_ok = false;
            break;
        }
        migrated_count += 1;
    }

    if all_ok {
        eprintln!(
            "[simard] goal_store migration: migrated {migrated_count} records, \
             skipped {skipped_count} already-present slugs from {}",
            goal_store_path.display()
        );
        rename_to_migrated(&goal_store_path);
    }
}

fn rename_to_migrated(path: &std::path::Path) {
    let migrated_path = path.with_extension("json.migrated");
    if let Err(e) = std::fs::rename(path, &migrated_path) {
        eprintln!(
            "[simard] goal_store migration: rename failed ({e}) — \
             data is in cognitive memory but {} remains on disk; \
             next startup will retry (idempotent)",
            path.display()
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
    #[serial_test::serial(cognitive_memory)]
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
    #[serial_test::serial(cognitive_memory)]
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
    #[serial_test::serial(cognitive_memory)]
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

    // ───────────────────────────────────────────────────────────────────────
    // Issue #2207 Finding 2: put() must propagate prospective-mirror errors
    // and reconcile_prospectives() must exist to fix drift.
    // ───────────────────────────────────────────────────────────────────────

    /// After put() of an Active goal, check_triggers must find the prospective
    /// entry — verifying that the dual-write is part of put()'s success contract.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn put_active_goal_creates_prospective_trigger() {
        let root = fresh_state_root("prospective-trigger");
        let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store should build");
        store
            .put(record("Fix authentication bug", GoalStatus::Active, 1))
            .expect("put active goal");

        // The prospective trigger condition is the slug with dashes→spaces.
        // Open a reader bridge and check triggers with the expected phrase.
        let reader =
            crate::memory_ipc::open_reader_bridge(&root).expect("reader bridge should open");
        let triggered = reader
            .ops()
            .check_triggers("fix authentication bug")
            .expect("check_triggers");

        assert!(
            triggered.iter().any(|p| p.description.starts_with("goal:")),
            "put() of an Active goal must create a goal: prospective entry; \
             found {} triggers: {:?}",
            triggered.len(),
            triggered.iter().map(|p| &p.description).collect::<Vec<_>>()
        );
    }

    /// After put() with a non-Active status, old prospective entries for that
    /// slug must be resolved (no longer triggerable).
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn put_completed_goal_resolves_stale_prospective() {
        let root = fresh_state_root("resolve-prospective");
        let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store should build");

        // First put: Active → creates prospective.
        store
            .put(record("Deploy CI pipeline", GoalStatus::Active, 1))
            .expect("put active");
        std::thread::sleep(std::time::Duration::from_millis(2));

        // Second put: Completed → should resolve the old prospective.
        store
            .put(record("Deploy CI pipeline", GoalStatus::Completed, 1))
            .expect("put completed");

        // The trigger should no longer fire for the completed goal.
        let reader =
            crate::memory_ipc::open_reader_bridge(&root).expect("reader bridge should open");
        let triggered = reader
            .ops()
            .check_triggers("deploy ci pipeline")
            .expect("check_triggers");

        let goal_triggers: Vec<_> = triggered
            .iter()
            .filter(|p| p.description.starts_with("goal:"))
            .collect();

        assert!(
            goal_triggers.is_empty(),
            "put() of a Completed goal must resolve old prospective entries; \
             found {} stale triggers: {:?}",
            goal_triggers.len(),
            goal_triggers
                .iter()
                .map(|p| &p.description)
                .collect::<Vec<_>>()
        );
    }

    /// `reconcile_prospectives()` must exist as a public method on the store.
    /// It should ensure Active goals have prospective entries and non-Active
    /// goals don't have stale ones.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn reconcile_prospectives_fixes_drift() {
        let root = fresh_state_root("reconcile");
        let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store should build");

        // Create two goals: one Active, one Completed.
        store
            .put(record("Active goal", GoalStatus::Active, 1))
            .expect("put active");
        std::thread::sleep(std::time::Duration::from_millis(2));
        store
            .put(record("Done goal", GoalStatus::Completed, 2))
            .expect("put completed");

        // Call reconcile — it must not error on a consistent store.
        store
            .reconcile_prospectives()
            .expect("reconcile_prospectives must succeed on a consistent store");

        // After reconciliation: Active goal should still have a trigger,
        // Completed goal should not.
        let reader =
            crate::memory_ipc::open_reader_bridge(&root).expect("reader bridge should open");

        let active_triggers = reader
            .ops()
            .check_triggers("active goal")
            .expect("check_triggers for active");
        assert!(
            active_triggers
                .iter()
                .any(|p| p.description.starts_with("goal:")),
            "reconcile must ensure Active goals have prospective entries"
        );

        let done_triggers = reader
            .ops()
            .check_triggers("done goal")
            .expect("check_triggers for done");
        let stale: Vec<_> = done_triggers
            .iter()
            .filter(|p| p.description.starts_with("goal:"))
            .collect();
        assert!(
            stale.is_empty(),
            "reconcile must resolve prospective entries for non-Active goals"
        );
    }

    /// reconcile_prospectives() must re-create a missing prospective for an
    /// Active goal (simulating drift where the dual-write was lost).
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn reconcile_prospectives_recreates_missing_active_prospective() {
        let root = fresh_state_root("reconcile-recreate");
        let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store should build");

        // Write an Active goal — this creates a prospective entry.
        store
            .put(record("Drifted goal", GoalStatus::Active, 1))
            .expect("put active");

        // Manually resolve the prospective to simulate drift.
        {
            let writer = crate::memory_ipc::launch_writer_bridge(&root).expect("writer bridge");
            let triggered = writer
                .ops()
                .check_triggers("drifted goal")
                .expect("check_triggers");
            for p in &triggered {
                if p.description.starts_with("goal:") {
                    writer
                        .ops()
                        .resolve_prospective(&p.node_id)
                        .expect("resolve");
                }
            }
            // Confirm it's gone.
            let after = writer
                .ops()
                .check_triggers("drifted goal")
                .expect("check_triggers after resolve");
            assert!(
                after.iter().all(|p| !p.description.starts_with("goal:")),
                "test setup: prospective should be resolved before reconcile"
            );
        }

        // Reconcile should detect the missing prospective and recreate it.
        store
            .reconcile_prospectives()
            .expect("reconcile_prospectives");

        let reader = crate::memory_ipc::open_reader_bridge(&root).expect("reader bridge");
        let triggered = reader
            .ops()
            .check_triggers("drifted goal")
            .expect("check_triggers post-reconcile");
        assert!(
            triggered.iter().any(|p| p.description.starts_with("goal:")),
            "reconcile must recreate prospective entries for Active goals \
             that lost their trigger due to drift"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
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
