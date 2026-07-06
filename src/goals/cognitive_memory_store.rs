//! Cognitive-memory backed [`GoalStore`] adapter.
//!
//! See `docs/reference/cognitive-memory-goal-store.md` for the design.
//!
//! `bootstrap::assembly` and `meeting_backend` are the production callers:
//! every per-record write flows through cognitive memory via the bridge
//! helpers (`launch_writer_client` / `open_reader_client`). The legacy
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
use crate::memory_ipc::{launch_writer_client, open_reader_client};
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
/// Pull window for `list()` reads. The board enforces a small
/// `MAX_ACTIVE_GOALS` active cap; even with status churn the per-process
/// record count stays modest, so 256 covers realistic deployments without
/// risking truncation.
pub(crate) const GOAL_STORE_LIST_LIMIT: u32 = 256;

/// `GoalStore` implementation backed by cognitive memory through the
/// bridge helpers (`launch_writer_client` / `open_reader_client`).
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
        let reader = match open_reader_client(&self.state_root) {
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

        let writer = launch_writer_client(&self.state_root)?;

        // De-fork Phase 2b (issue #2307): the library backend's `check_triggers`
        // is a FIRE-ONCE mutator (marks matches `"triggered"`) that matches on
        // ANY shared whole word — including the ubiquitous token `goal` — rather
        // than the deleted native backend's read-only whole-substring match. A
        // single mixed loop that probed/resolved one goal would therefore consume
        // another goal's freshly-stored prospective (they share `goal`). Split
        // the work into two phases so stored prospectives are never re-probed:
        //   1. Resolve every goal-prospective (clears stale + drift entries).
        //   2. Store exactly one fresh PENDING prospective per Active goal.
        for goal in &goals {
            resolve_goal_prospectives(&goal.slug, writer.ops())?;
        }
        for goal in &goals {
            if goal.status != GoalStatus::Active {
                continue;
            }
            let trigger = prospective_trigger_for(goal);
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

        Ok(())
    }
}

impl GoalStore for CognitiveMemoryGoalStore {
    fn descriptor(&self) -> BackendDescriptor {
        self.descriptor.clone()
    }

    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        let content = Self::encode(&record)?;
        let writer = launch_writer_client(&self.state_root)?;

        // Primary storage: semantic fact (authoritative record). Issue #2329:
        // route through CallerKey dedup keyed per goal slug so each goal's record
        // supersedes its own previous revision instead of appending a fresh fact
        // every `put`. The read-side "max node_id per slug" dedup remains as a
        // defensive guard.
        writer.ops().store_fact_with_caller_key(
            &format!("{GOAL_STORE_FACT_CONCEPT}:{}", record.slug),
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

/// Mirror a live [`GoalBoard`](crate::goal_curation::GoalBoard)'s active goals
/// into prospective memory so they fire as triggers during OODA preparation
/// (issue #2308 follow-up).
///
/// The daemon persists its goals through the `GoalBoard` snapshot path, not
/// through [`CognitiveMemoryGoalStore::put`] — so the only live prospective
/// writer never ran and `check_triggers` had nothing to match ("0 triggers"
/// every cycle). This board-sourced reconcile closes that gap without
/// migrating the daemon's goal persistence: it runs every cycle, before
/// preparation, and ensures exactly one `pending` prospective per active goal.
///
/// Operates on the caller's existing memory handle (the daemon's own writer)
/// rather than opening a fresh bridge, so it never contends for the store lock.
///
/// Implemented as a two-phase pass for the same reason
/// [`CognitiveMemoryGoalStore::reconcile_prospectives`] is: the library's
/// `check_triggers` is a fire-once mutator that matches on any shared whole
/// word, so resolving one goal's prospect mid-loop could consume another's
/// freshly-stored one. Phase 1 resolves every goal-prospective for the active
/// slugs; phase 2 stores one fresh `pending` prospective per Active goal.
pub fn reconcile_board_prospectives(
    board: &crate::goal_curation::GoalBoard,
    ops: &dyn crate::cognitive_memory::CognitiveMemoryOps,
) -> SimardResult<()> {
    // Project the live board's active goals to records (slug = goal_slug(id),
    // status mapped). This is the same projection the snapshot→record path
    // uses, so the slug-phrase trigger is byte-identical with the read-side
    // `build_objective_probe`.
    let records = crate::goal_curation::active_goals_as_records(board);
    if records.is_empty() {
        return Ok(());
    }

    // Phase 1: resolve every existing goal-prospective for these slugs first.
    // The library's `check_triggers` is a fire-once mutator that matches on any
    // shared whole word, so resolving one goal's prospect mid-loop could
    // consume another's freshly-stored one — splitting the work guarantees a
    // stored prospect is never re-probed (mirrors `reconcile_prospectives`).
    for record in &records {
        resolve_goal_prospectives(&record.slug, ops)?;
    }

    // Phase 2: store exactly one fresh PENDING prospective per Active goal so
    // `check_triggers` fires it during the same cycle's preparation pass.
    // Non-Active goals (paused/completed/proposed) were resolved in phase 1 and
    // are intentionally not re-created.
    for record in &records {
        if record.status != GoalStatus::Active {
            continue;
        }
        let trigger = prospective_trigger_for(record);
        let description = format!("{}{}", GOAL_PROSPECTIVE_PREFIX, record.title);
        let action = format!(
            "Pursue goal: {} (p{}, {})",
            record.title, record.priority, record.rationale,
        );
        ops.store_prospective(&description, &trigger, &action, i64::from(record.priority))?;
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
    let writer = match launch_writer_client(state_root) {
        Ok(w) => w,
        Err(e) => {
            eprintln!(
                "[simard] goal_store migration: launch_writer_client failed ({e}) — \
                 leaving file in place for next retry"
            );
            return;
        }
    };

    // Read existing slugs from cognitive memory to skip duplicates.
    // If the store exists but we cannot read it, abort the migration
    // to avoid overwriting newer cognitive-memory records with stale
    // legacy data. De-fork Phase 2b (#2307): the library backend persists
    // at `<state_root>/cognitive`, not the native `cognitive_memory.ladybug`.
    let db_file = state_root.join("cognitive");
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
            crate::memory_ipc::open_reader_client(&root).expect("reader bridge should open");
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
            crate::memory_ipc::open_reader_client(&root).expect("reader bridge should open");
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
            crate::memory_ipc::open_reader_client(&root).expect("reader bridge should open");

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
            let writer = crate::memory_ipc::launch_writer_client(&root).expect("writer bridge");
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

        let reader = crate::memory_ipc::open_reader_client(&root).expect("reader bridge");
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

    /// Issue #2280 Gap 1: put(Active) must increment prospective_count in
    /// get_statistics(). This is the quantitative assertion that complements
    /// the qualitative `put_active_goal_creates_prospective_trigger` test.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn put_active_goal_increments_prospective_count() {
        let root = fresh_state_root("prospective-count");
        let store = CognitiveMemoryGoalStore::new(root.clone()).expect("store should build");

        // Put a Proposed goal first to create the DB without a prospective node.
        store
            .put(record("Setup baseline", GoalStatus::Proposed, 2))
            .expect("put proposed goal");

        {
            let reader = crate::memory_ipc::open_reader_client(&root).expect("reader bridge");
            let before = reader.ops().get_statistics().expect("get_statistics");
            assert_eq!(
                before.prospective_count, 0,
                "Proposed goal must not create a prospective node"
            );
        }

        // Put an Active goal — this must create a prospective memory node.
        store
            .put(record("Implement caching layer", GoalStatus::Active, 1))
            .expect("put active goal");

        // Open a fresh reader to see the write.
        let reader = crate::memory_ipc::open_reader_client(&root).expect("reader bridge");
        let after = reader.ops().get_statistics().expect("get_statistics");
        assert!(
            after.prospective_count > 0,
            "put(Active) must increment prospective_count; got {}",
            after.prospective_count
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

    // ───────────────────────────────────────────────────────────────────────
    // Issue #2308 follow-up: board-sourced prospective reconcile.
    //
    // The daemon persists goals through the GoalBoard snapshot path, not
    // through CognitiveMemoryGoalStore::put — so no prospects were ever
    // written and OODA preparation reported "0 triggers" every cycle even
    // with active goals. `reconcile_board_prospectives` mirrors the live
    // board into prospective memory so check_triggers fires during prep.
    // ───────────────────────────────────────────────────────────────────────

    /// TDD red: seed one Active goal on a `GoalBoard`, reconcile it into
    /// prospective memory, then run the real preparation recall path against
    /// the daemon-shaped objective probe. A trigger MUST fire.
    #[test]
    fn board_reconcile_fires_trigger_for_active_goal_in_preparation() {
        use crate::cognitive_memory::LibraryCognitiveMemory;
        use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open library store");

        let mut board = GoalBoard::new();
        board.active.push(ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: "fix-episode-recall".to_string(),
            description: "Fix episode recall during OODA preparation".to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: Vec::new(),
            last_progress_update_at: None,
        });

        // Mirror the active board into prospective memory.
        reconcile_board_prospectives(&board, &mem).expect("reconcile");

        // Build the objective probe exactly as `build_objective_probe` does in
        // the daemon: free-text description + the slug-phrase trigger.
        let goal = &board.active[0];
        let probe = format!(
            "{}; {}",
            goal.description.trim(),
            crate::goals::goal_slug(&goal.id).replace('-', " "),
        );

        let session = SessionId::parse("session-018f1f7e-4c5d-7b2a-8f10-b5c0d4f7b123")
            .expect("valid session id");
        let ctx =
            crate::memory_consolidation::preparation_memory_operations(&probe, &session, &mem)
                .expect("preparation");

        assert!(
            !ctx.triggered_prospectives.is_empty(),
            "an active board goal must fire a prospective trigger during preparation; \
             reconcile wrote no matching prospective"
        );
        assert!(
            ctx.triggered_prospectives
                .iter()
                .any(|p| p.description.starts_with("goal:")),
            "the fired trigger must be the goal-prefixed prospective; got: {:?}",
            ctx.triggered_prospectives
                .iter()
                .map(|p| &p.description)
                .collect::<Vec<_>>()
        );
    }

    /// The reconcile is idempotent: running it twice for the same single
    /// active goal must leave exactly one `pending` prospective (the fresh
    /// one), not accumulate duplicates within a cycle.
    #[test]
    fn board_reconcile_is_idempotent_for_a_stable_active_goal() {
        use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
        use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};

        let tmp = tempfile::tempdir().expect("tempdir");
        let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open library store");

        let mut board = GoalBoard::new();
        board.active.push(ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: "ship-introspection-cli".to_string(),
            description: "Ship the memory introspection CLI".to_string(),
            priority: 2,
            status: GoalProgress::InProgress { percent: 40 },
            assigned_to: None,
            current_activity: None,
            wip_refs: Vec::new(),
            last_progress_update_at: None,
        });

        reconcile_board_prospectives(&board, &mem).expect("first reconcile");
        reconcile_board_prospectives(&board, &mem).expect("second reconcile");

        // After two reconciles the probe must still surface exactly one
        // pending goal prospective (the latest fresh one), not two.
        let probe = crate::goals::goal_slug(&board.active[0].id).replace('-', " ");
        let pending = mem.check_triggers(&probe).expect("check_triggers");
        let goal_pending: Vec<_> = pending
            .iter()
            .filter(|p| p.description.starts_with("goal:"))
            .collect();
        assert_eq!(
            goal_pending.len(),
            1,
            "idempotent reconcile must leave exactly one pending goal prospective; got {}",
            goal_pending.len()
        );
    }
}
