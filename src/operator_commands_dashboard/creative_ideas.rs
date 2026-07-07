//! Dashboard **Creative Ideas** tab handlers.
//!
//! Two read-only endpoints back the Creative Ideas tab, reading the durable
//! creative-idea prospective memories out of the *same* cognitive-memory store
//! the rest of the dashboard reads (via [`open_reader_client`]) — no parallel
//! datastore:
//!
//! * `GET  /api/creative-ideas`        — the current idea pool (latest revision
//!   per idea), newest first, plus a per-status count summary.
//! * `POST /api/creative-ideas/search` — filter the pool by status and/or a
//!   free-text query over the idea text and rationale.
//!
//! The pool is browseable and **searchable by status**: every [`IdeaStatus`]
//! value is enumerable.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Json;
use axum::extract::Path as AxumPath;
use serde_json::{Value, json};

use super::routes::resolve_state_root;
use crate::cognitive_memory::creative_idea::{
    CreativeIdea, CreativeIdeaStore, IdeaStatus, ProspectiveCreativeIdeaStore, parse_idea_status,
};
use crate::cognitive_threads::ThreadContext;
use crate::cognitive_threads::threads::creative_ideas::{CreativeIdeasThread, GenerationReport};
use crate::creative_ideas::pipeline::{CognitiveMemoryGoalStoreFactory, GoalStoreFactory};
use crate::creative_ideas::routing::route_idea_to_goal;
use crate::error::SimardResult;
use crate::memory_ipc::{launch_writer_client, open_reader_client};

/// Read window for the idea pool (bounded; the pool stays modest).
const IDEA_LIST_LIMIT: u32 = 512;

/// `GET /api/creative-ideas` — the current idea pool, newest first, with a
/// per-status count summary.
pub(crate) async fn creative_ideas() -> Json<Value> {
    let state_root = resolve_state_root();
    match load_ideas(&state_root) {
        Ok(ideas) => Json(json!({
            "counts": status_counts(&ideas),
            "ideas": ideas.iter().map(idea_summary).collect::<Vec<_>>(),
        })),
        Err(e) => Json(json!({ "error": e.to_string(), "ideas": [], "counts": {} })),
    }
}

/// `POST /api/creative-ideas/search` — filter the pool by `status` (one of the
/// [`IdeaStatus`] names) and/or a free-text `query`. Body: `{status?, query?}`.
pub(crate) async fn creative_ideas_search(Json(body): Json<Value>) -> Json<Value> {
    let status = body.get("status").and_then(Value::as_str).map(str::trim);
    let query = body
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let state_root = resolve_state_root();

    // An explicit but unknown status is a fail-closed error (never silently
    // treated as "all").
    let status_filter = match status {
        Some(s) if !s.is_empty() => match parse_idea_status(s) {
            Ok(v) => Some(v),
            Err(e) => return Json(json!({ "error": e.to_string(), "results": [] })),
        },
        _ => None,
    };

    match load_ideas(&state_root) {
        Ok(ideas) => {
            let results: Vec<Value> = ideas
                .iter()
                .filter(|i| status_filter.is_none_or(|s| i.status == s))
                .filter(|i| query.is_empty() || idea_matches(i, &query))
                .map(idea_summary)
                .collect();
            Json(json!({ "results": results }))
        }
        Err(e) => Json(json!({ "error": e.to_string(), "results": [] })),
    }
}

// ---------------------------------------------------------------------------
// Operator write controls: manual generation ("Run now") + per-idea
// Promote/Prune. These resolve the LIVE writer store via `launch_writer_client`
// (the same store the daemon writes to) — no parallel datastore, no stale data.
// ---------------------------------------------------------------------------

/// Process-wide re-entrancy guard so an operator double-click (or two operators)
/// cannot launch overlapping generation ticks.
static RUN_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// RAII lease over [`RUN_IN_PROGRESS`]. Acquired via [`RunGuard::try_acquire`]
/// and released on `Drop`, so the flag is cleared even if the acquiring scope
/// unwinds (panic) — never a permanently-stuck "already running" lock.
struct RunGuard;

impl RunGuard {
    /// Take the run lease, or `None` if a run is already in flight.
    fn try_acquire() -> Option<Self> {
        RUN_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then_some(Self)
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUN_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// `POST /api/creative-ideas/run` — manually trigger one creative-ideas
/// generation tick against the live daemon store, persisting any new ideas and
/// returning a report. Useful because the thread otherwise only ticks on a 24h
/// schedule (which every daemon restart pushes 24h out). Guarded against
/// overlapping runs; any failure is surfaced loudly (never a silent no-op).
pub(crate) async fn creative_ideas_run() -> Json<Value> {
    let state_root = resolve_state_root();
    let runtime = tokio::runtime::Handle::current();
    // Generation may invoke agents and block, so run it off the async executor.
    // The re-entrancy lease is taken *inside* the blocking task so its lifetime
    // is bound to the actual work, not this handler future: a client disconnect
    // that cancels the handler cannot leak the lock (the blocking task still
    // runs to completion and drops the guard), and the lock stays held for the
    // full run so no overlapping tick can start. `None` => a run is already in
    // flight.
    let outcome = tokio::task::spawn_blocking(move || -> Option<SimardResult<GenerationReport>> {
        let _guard = RunGuard::try_acquire()?;
        let mut thread = CreativeIdeasThread::from_env();
        Some(run_generation_tick(
            &state_root,
            &mut thread,
            now_epoch(),
            runtime,
        ))
    })
    .await;

    match outcome {
        Ok(Some(Ok(report))) => Json(json!({
            "ok": true,
            "report": serde_json::to_value(report).unwrap_or(Value::Null),
        })),
        Ok(Some(Err(e))) => Json(error_json(e)),
        Ok(None) => Json(json!({
            "error": "a creative-ideas generation run is already in progress",
            "running": true,
        })),
        Err(join_err) => Json(error_json(format!("generation run failed: {join_err}"))),
    }
}

/// Env-free core of [`creative_ideas_run`] (test seam): run one generation tick
/// with the given `thread` against the live writer store for `state_root`.
///
/// Bypasses the thread's 24h schedule + opt-out gate via
/// [`CreativeIdeasThread::run_now`]. Tests inject a hermetic thread
/// (`FakeIdeaSource` + a no-op pipeline); production passes
/// [`CreativeIdeasThread::from_env`].
fn run_generation_tick(
    state_root: &Path,
    thread: &mut CreativeIdeasThread,
    now_epoch: u64,
    runtime: tokio::runtime::Handle,
) -> SimardResult<GenerationReport> {
    let writer = launch_writer_client(state_root)?;
    let shutdown = AtomicBool::new(false);
    let mut ctx = ThreadContext {
        state_root,
        repo_root: state_root,
        memory: writer.ops(),
        runtime,
        shutdown: &shutdown,
        now_epoch,
        dry_run: false,
    };
    thread.run_now(&mut ctx)
}

/// `POST /api/creative-ideas/{id}/promote` — accept one idea (`{id}` is its
/// stable `idea_id`) by transitioning it to
/// [`IdeaStatus::AcceptedForImplementation`], then (unless `route_to_goal` is
/// `false`) best-effort route it onto the goal board.
///
/// Body (optional): `{"route_to_goal"?: bool}` (default **`true`**). Acceptance
/// is persisted **before** routing, so a routing failure surfaces `goal_error`
/// without rolling back the acceptance.
pub(crate) async fn creative_ideas_promote(
    AxumPath(id): AxumPath<String>,
    body: Option<Json<Value>>,
) -> Json<Value> {
    let route_to_goal = body
        .as_ref()
        .and_then(|Json(v)| v.get("route_to_goal"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Json(promote_idea(&resolve_state_root(), &id, route_to_goal))
}

/// `POST /api/creative-ideas/{id}/prune` — reject one idea (`{id}` is its stable
/// `idea_id`) by transitioning it to the terminal [`IdeaStatus::Rejected`].
pub(crate) async fn creative_ideas_prune(AxumPath(id): AxumPath<String>) -> Json<Value> {
    Json(prune_idea(&resolve_state_root(), &id))
}

/// Standard operator error payload: `{"error": "<message>"}`.
fn error_json(e: impl std::fmt::Display) -> Value {
    json!({ "error": e.to_string() })
}

/// Load `idea_id` from the given live writer `store`, apply the `target`
/// transition, and persist the resulting revision. Returns the updated idea, or
/// an operator error `Value`.
///
/// Takes an already-opened `store` (rather than a `state_root`) so a caller that
/// performs several writes in one request — e.g. Promote's accept-then-route —
/// pays the writer-client setup cost once and shares a single handle.
///
/// Fail-closed: an unknown id, or an edge not in the [`IdeaStatus`] transition
/// table, surfaces loudly as `Err(json!({"error": ...}))` — never a silent
/// no-op or a corrupted store.
fn transition_and_persist(
    store: &ProspectiveCreativeIdeaStore<'_>,
    idea_id: &str,
    target: IdeaStatus,
) -> Result<CreativeIdea, Value> {
    let mut idea = match load_idea(store, idea_id) {
        Ok(Some(i)) => i,
        Ok(None) => return Err(json!({ "error": "idea not found" })),
        Err(e) => return Err(error_json(e)),
    };
    idea.try_transition(target).map_err(error_json)?;
    store.update(&idea).map_err(error_json)?;
    Ok(idea)
}

/// Accept `idea_id` and (optionally) route it to a goal. Returns the operator
/// response value (`{ok, idea, goal?/goal_error?}` or `{error}`).
fn promote_idea(state_root: &Path, idea_id: &str, route_to_goal: bool) -> Value {
    // Open ONE writer handle for the whole accept(+route) sequence. The accept
    // persist and the post-route persist reuse it, so the (potentially IPC)
    // writer-client/socket setup is paid once per request, not per write.
    let writer = match launch_writer_client(state_root) {
        Ok(w) => w,
        Err(e) => return error_json(e),
    };
    let store = ProspectiveCreativeIdeaStore::new(writer.ops());

    // Accept + persist FIRST. Fail-closed: an invalid edge surfaces loudly.
    let mut idea =
        match transition_and_persist(&store, idea_id, IdeaStatus::AcceptedForImplementation) {
            Ok(i) => i,
            Err(e) => return e,
        };

    if !route_to_goal {
        return json!({ "ok": true, "idea": idea_summary(&idea) });
    }

    // Best-effort route to a goal. Failure surfaces `goal_error` WITHOUT rolling
    // back the (already-persisted) acceptance.
    match route_accepted_idea_to_goal(state_root, &store, &mut idea) {
        Ok(goal) => json!({ "ok": true, "idea": idea_summary(&idea), "goal": goal }),
        Err(e) => json!({ "ok": true, "idea": idea_summary(&idea), "goal_error": e.to_string() }),
    }
}

/// Reject `idea_id` (terminal [`IdeaStatus::Rejected`]). Returns `{ok, idea}` or
/// `{error}`.
fn prune_idea(state_root: &Path, idea_id: &str) -> Value {
    let writer = match launch_writer_client(state_root) {
        Ok(w) => w,
        Err(e) => return error_json(e),
    };
    let store = ProspectiveCreativeIdeaStore::new(writer.ops());
    match transition_and_persist(&store, idea_id, IdeaStatus::Rejected) {
        Ok(idea) => json!({ "ok": true, "idea": idea_summary(&idea) }),
        Err(e) => e,
    }
}

/// Look up one idea in the live pool by its stable `idea_id` (latest revision).
fn load_idea(
    store: &ProspectiveCreativeIdeaStore<'_>,
    idea_id: &str,
) -> SimardResult<Option<CreativeIdea>> {
    Ok(store
        .list(IDEA_LIST_LIMIT)?
        .into_iter()
        .find(|i| i.idea_id == idea_id))
}

/// Route an already-`AcceptedForImplementation` idea to a `Proposed` goal on the
/// live goal board, advance it to `ImplementationStarted`, and persist. Returns
/// a compact goal summary. Any failure is returned as `Err` (surfaced as
/// `goal_error` by the caller, without rolling back the acceptance).
fn route_accepted_idea_to_goal(
    state_root: &Path,
    store: &ProspectiveCreativeIdeaStore<'_>,
    idea: &mut CreativeIdea,
) -> SimardResult<Value> {
    let goals = CognitiveMemoryGoalStoreFactory.open(state_root)?;
    let record = route_idea_to_goal(idea, goals.as_ref(), now_epoch())?;
    idea.try_transition(IdeaStatus::ImplementationStarted)?;
    store.update(idea)?;
    Ok(json!({
        "id": record.slug,
        "title": record.title,
        "status": format!("{:?}", record.status),
    }))
}

/// Current unix-epoch seconds (generation/routing provenance clock).
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Pure helpers (state_root injected) — the async handlers are thin shells over
// these so they are unit-testable against a hermetic store.
// ---------------------------------------------------------------------------

/// Load the current idea pool (latest revision per idea, newest first).
fn load_ideas(state_root: &Path) -> SimardResult<Vec<CreativeIdea>> {
    let reader = open_reader_client(state_root)?;
    let store = ProspectiveCreativeIdeaStore::new(reader.ops());
    store.list(IDEA_LIST_LIMIT)
}

/// Per-status count map over every [`IdeaStatus`] value (zero-filled).
fn status_counts(ideas: &[CreativeIdea]) -> Value {
    let mut map = serde_json::Map::new();
    for status in IdeaStatus::ALL {
        let n = ideas.iter().filter(|i| i.status == status).count();
        map.insert(status.as_str().to_string(), json!(n));
    }
    Value::Object(map)
}

/// Compact per-idea summary for the pool list.
fn idea_summary(idea: &CreativeIdea) -> Value {
    json!({
        "idea_id": idea.idea_id,
        "idea": idea.idea,
        "status": idea.status.as_str(),
        "rationale": idea.context.rationale,
        "links": idea.links.len(),
        "reviews": idea.reviews.len(),
        "has_metric": idea.success_metric.is_some(),
        "metric": idea.success_metric.as_ref().map(|m| m.name.clone()),
        "created_epoch": idea.created_epoch,
    })
}

/// Case-insensitive match over the idea text + rationale (`query` is lowercased).
fn idea_matches(idea: &CreativeIdea, query: &str) -> bool {
    idea.idea.to_ascii_lowercase().contains(query)
        || idea.context.rationale.to_ascii_lowercase().contains(query)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::cognitive_memory::creative_idea::IdeaContext;
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::cognitive_threads::threads::creative_ideas::{
        FakeIdeaSource, GenerationInputs, RawIdea,
    };
    use crate::creative_ideas::CreativeIdeasConfig;
    use crate::creative_ideas::pipeline::{IdeaPipeline, RouteOutcome};
    use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
    use crate::test_support::HermeticState;

    /// Hermetic pipeline double: reviews/routes nothing so a generation tick
    /// persists its `New` ideas without any agent/network dependency.
    struct NoopPipeline;
    impl IdeaPipeline for NoopPipeline {
        fn review_and_route(
            &self,
            _idea: &mut CreativeIdea,
            _inputs: &GenerationInputs,
            _ctx: &ThreadContext<'_>,
        ) -> SimardResult<RouteOutcome> {
            Ok(RouteOutcome::Parked)
        }
    }

    struct MemGuard {
        writer: Arc<dyn CognitiveMemoryOps>,
    }

    impl MemGuard {
        fn register(state: &HermeticState) -> Self {
            let writer: Arc<dyn CognitiveMemoryOps> =
                Arc::new(LibraryCognitiveMemory::open(state.state_root()).expect("open store"));
            register_in_process_writer(state.state_root().to_path_buf(), Arc::clone(&writer));
            Self { writer }
        }
        fn ops(&self) -> &dyn CognitiveMemoryOps {
            self.writer.as_ref()
        }
    }

    impl Drop for MemGuard {
        fn drop(&mut self) {
            clear_in_process_writer();
        }
    }

    fn ctx() -> IdeaContext {
        IdeaContext {
            source: "creative-ideas-thread".to_string(),
            goals_snapshot: vec![],
            observation_digest: "d".to_string(),
            rationale: "recall precision plateaued".to_string(),
        }
    }

    /// Seed one idea at `status`, returning its stable `idea_id`.
    fn seed(ops: &dyn CognitiveMemoryOps, text: &str, status: IdeaStatus) -> String {
        let store = ProspectiveCreativeIdeaStore::new(ops);
        let mut idea = CreativeIdea::new(text, ctx(), 1);
        idea.node_id = store.store(&idea).expect("store");
        if status != IdeaStatus::New {
            idea.try_transition(status).expect("transition");
            store.update(&idea).expect("update");
        }
        idea.idea_id
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn lists_pool_with_status_counts() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        seed(mem.ops(), "improve recall ranking", IdeaStatus::New);
        seed(
            mem.ops(),
            "auto-delete stale worktrees",
            IdeaStatus::NeedsHumanReview,
        );
        seed(mem.ops(), "a rejected idea", IdeaStatus::Rejected);

        let Json(v) = creative_ideas().await;
        assert_eq!(v["ideas"].as_array().expect("ideas").len(), 3);
        assert_eq!(v["counts"]["New"], 1);
        assert_eq!(v["counts"]["NeedsHumanReview"], 1);
        assert_eq!(v["counts"]["Rejected"], 1);
        assert_eq!(v["counts"]["Deferred"], 0);
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn search_filters_by_status_and_text() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        seed(mem.ops(), "improve recall ranking", IdeaStatus::New);
        seed(
            mem.ops(),
            "auto-delete stale worktrees",
            IdeaStatus::NeedsHumanReview,
        );
        seed(mem.ops(), "another new recall idea", IdeaStatus::New);

        // By status.
        let Json(new_only) = creative_ideas_search(Json(json!({ "status": "New" }))).await;
        assert_eq!(new_only["results"].as_array().expect("arr").len(), 2);

        // By status + text.
        let Json(both) =
            creative_ideas_search(Json(json!({ "status": "New", "query": "ranking" }))).await;
        assert_eq!(both["results"].as_array().expect("arr").len(), 1);

        // Text only, across statuses.
        let Json(text) = creative_ideas_search(Json(json!({ "query": "worktrees" }))).await;
        assert_eq!(text["results"].as_array().expect("arr").len(), 1);

        // An unknown status is a fail-closed error, not "all".
        let Json(bad) = creative_ideas_search(Json(json!({ "status": "Bogus" }))).await;
        assert!(bad.get("error").is_some());
    }

    // -----------------------------------------------------------------------
    // Feature 1 (display): every persisted idea renders with its status + key
    // metadata (created time, review/link counts).
    // -----------------------------------------------------------------------
    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn renders_each_idea_with_status_and_metadata() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        seed(mem.ops(), "improve recall ranking", IdeaStatus::New);
        seed(mem.ops(), "park this one", IdeaStatus::Deferred);

        let Json(v) = creative_ideas().await;
        let ideas = v["ideas"].as_array().expect("ideas");
        assert_eq!(ideas.len(), 2, "N persisted ideas -> N rendered");
        for idea in ideas {
            // Each rendered idea carries a valid status string and metadata.
            let status = idea["status"].as_str().expect("status string");
            assert!(
                parse_idea_status(status).is_ok(),
                "rendered status must be an enumerable IdeaStatus, got {status:?}"
            );
            assert!(idea["idea"].as_str().is_some_and(|s| !s.is_empty()));
            assert!(idea["created_epoch"].is_number(), "created time present");
            assert!(idea["reviews"].is_number());
            assert!(idea["links"].is_number());
        }
        // Statuses are surfaced accurately per idea.
        let statuses: Vec<&str> = ideas.iter().filter_map(|i| i["status"].as_str()).collect();
        assert!(statuses.contains(&"New"));
        assert!(statuses.contains(&"Deferred"));
    }

    // -----------------------------------------------------------------------
    // Feature 2 (Run now): a manual run triggers a generation tick that
    // persists new ideas to the live store (hermetic source + no-op pipeline).
    // -----------------------------------------------------------------------
    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn run_now_triggers_generation_and_persists() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let store = ProspectiveCreativeIdeaStore::new(mem.ops());
        assert_eq!(store.list(u32::MAX).expect("list").len(), 0, "starts empty");

        let raws = vec![
            RawIdea {
                idea: "cache distilled facts by concept".to_string(),
                links: vec![],
                rationale: "recall precision plateaued".to_string(),
            },
            RawIdea {
                idea: "auto-delete stale worktrees nightly".to_string(),
                links: vec![],
                rationale: "disk pressure".to_string(),
            },
            RawIdea {
                idea: "prefetch goal board on tab open".to_string(),
                links: vec![],
                rationale: "dashboard latency".to_string(),
            },
        ];
        let cfg = CreativeIdeasConfig {
            enabled: true,
            batch: 3,
            ..CreativeIdeasConfig::default()
        };
        let mut thread = CreativeIdeasThread::with_pipeline(
            cfg,
            Box::new(FakeIdeaSource::with_ideas(raws)),
            Box::new(NoopPipeline),
        );

        let report = run_generation_tick(
            state.state_root(),
            &mut thread,
            1_000,
            tokio::runtime::Handle::current(),
        )
        .expect("manual generation run");

        assert!(report.persisted >= 1, "run persisted at least one idea");
        let ideas = store.list(u32::MAX).expect("list");
        assert_eq!(
            ideas.len(),
            report.persisted,
            "every persisted idea is readable from the live store"
        );
        assert!(
            ideas.iter().all(|i| i.status == IdeaStatus::New),
            "freshly generated ideas are New"
        );
    }

    /// The re-entrancy guard refuses a second overlapping run (no double-run).
    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn run_now_guard_blocks_reentrant_run() {
        // Simulate a run already in flight.
        RUN_IN_PROGRESS.store(true, Ordering::SeqCst);
        let Json(v) = creative_ideas_run().await;
        RUN_IN_PROGRESS.store(false, Ordering::SeqCst);
        assert_eq!(v["running"], true);
        assert!(
            v["error"]
                .as_str()
                .is_some_and(|e| e.contains("already in progress")),
            "re-entrant run must surface a clear error, got {v:?}"
        );
    }

    /// The run lease is RAII: it releases on drop (even on panic/cancellation),
    /// so it can never leave the feature stuck in a permanent "already running"
    /// state — a second acquire after the first drops succeeds.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn run_guard_releases_on_drop() {
        assert!(!RUN_IN_PROGRESS.load(Ordering::SeqCst), "starts free");
        {
            let _lease = RunGuard::try_acquire().expect("first acquire");
            assert!(RUN_IN_PROGRESS.load(Ordering::SeqCst), "held while leased");
            assert!(
                RunGuard::try_acquire().is_none(),
                "second acquire is refused while the first is held"
            );
        }
        assert!(
            !RUN_IN_PROGRESS.load(Ordering::SeqCst),
            "released once the lease drops"
        );
        drop(RunGuard::try_acquire().expect("re-acquire after release"));
        assert!(!RUN_IN_PROGRESS.load(Ordering::SeqCst), "free again");
    }

    // -----------------------------------------------------------------------
    // Feature 3 (Promote / Prune): valid edges persist, invalid edges error.
    // -----------------------------------------------------------------------
    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn promote_transitions_new_to_accepted_and_persists() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let id = seed(mem.ops(), "promote me", IdeaStatus::New);

        // route_to_goal=false keeps the outcome deterministic at Accepted.
        let Json(v) = creative_ideas_promote(
            AxumPath(id.clone()),
            Some(Json(json!({ "route_to_goal": false }))),
        )
        .await;
        assert_eq!(v["ok"], true, "{v:?}");
        assert_eq!(v["idea"]["status"], "AcceptedForImplementation");
        assert_eq!(v["idea"]["idea_id"], id);

        // Persisted to the live store.
        let store = ProspectiveCreativeIdeaStore::new(mem.ops());
        let idea = store
            .list(u32::MAX)
            .expect("list")
            .into_iter()
            .find(|i| i.idea_id == id)
            .expect("idea present");
        assert_eq!(idea.status, IdeaStatus::AcceptedForImplementation);
    }

    /// Promote with the default `route_to_goal` accepts the idea and best-effort
    /// routes it to a goal: on success the idea advances to `ImplementationStarted`
    /// and a `goal` is returned; if routing fails the idea stays accepted and a
    /// `goal_error` is surfaced — never a silent failure, always at least accepted.
    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn promote_default_routes_to_goal_best_effort() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let id = seed(mem.ops(), "route me to a goal", IdeaStatus::New);

        // No body ⇒ route_to_goal defaults to true.
        let Json(v) = creative_ideas_promote(AxumPath(id.clone()), None).await;
        assert_eq!(v["ok"], true, "{v:?}");

        let status = v["idea"]["status"].as_str().expect("status");
        if v.get("goal").is_some() {
            assert_eq!(status, "ImplementationStarted", "routed ⇒ in flight: {v:?}");
            assert!(v["goal"]["id"].as_str().is_some_and(|s| !s.is_empty()));
        } else {
            // Best-effort routing failed: acceptance still stands, loudly reported.
            assert_eq!(status, "AcceptedForImplementation", "{v:?}");
            assert!(
                v.get("goal_error").is_some(),
                "must report goal_error: {v:?}"
            );
        }

        // Whatever the routing outcome, the idea is at least Accepted (persisted).
        let store = ProspectiveCreativeIdeaStore::new(mem.ops());
        let idea = store
            .list(u32::MAX)
            .expect("list")
            .into_iter()
            .find(|i| i.idea_id == id)
            .expect("idea present");
        assert!(matches!(
            idea.status,
            IdeaStatus::AcceptedForImplementation | IdeaStatus::ImplementationStarted
        ));
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn prune_transitions_new_to_rejected_and_persists() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        let id = seed(mem.ops(), "prune me", IdeaStatus::New);

        let Json(v) = creative_ideas_prune(AxumPath(id.clone())).await;
        assert_eq!(v["ok"], true, "{v:?}");
        assert_eq!(v["idea"]["status"], "Rejected");

        let store = ProspectiveCreativeIdeaStore::new(mem.ops());
        let idea = store
            .list(u32::MAX)
            .expect("list")
            .into_iter()
            .find(|i| i.idea_id == id)
            .expect("idea present");
        assert_eq!(idea.status, IdeaStatus::Rejected);
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn invalid_transition_surfaces_error() {
        let state = HermeticState::new();
        let mem = MemGuard::register(&state);
        // A terminal Rejected idea cannot be pruned again (Rejected -> Rejected
        // is not in the transition table).
        let id = seed(mem.ops(), "already rejected", IdeaStatus::Rejected);

        let Json(v) = creative_ideas_prune(AxumPath(id.clone())).await;
        assert!(v.get("ok").is_none(), "must not succeed: {v:?}");
        assert!(
            v["error"]
                .as_str()
                .is_some_and(|e| e.contains("invalid creative-idea transition")),
            "invalid edge must surface a clear error, got {v:?}"
        );

        // The persisted status is unchanged (no silent corruption).
        let store = ProspectiveCreativeIdeaStore::new(mem.ops());
        let idea = store
            .list(u32::MAX)
            .expect("list")
            .into_iter()
            .find(|i| i.idea_id == id)
            .expect("idea present");
        assert_eq!(idea.status, IdeaStatus::Rejected);
    }

    #[tokio::test]
    #[serial_test::serial(cognitive_memory)]
    async fn transition_missing_idea_errors() {
        let state = HermeticState::new();
        let _mem = MemGuard::register(&state);
        let Json(v) = creative_ideas_promote(AxumPath("does-not-exist".to_string()), None).await;
        assert!(v.get("ok").is_none());
        assert_eq!(v["error"], "idea not found", "{v:?}");

        let Json(p) = creative_ideas_prune(AxumPath("does-not-exist".to_string())).await;
        assert_eq!(p["error"], "idea not found", "{p:?}");
    }
}
