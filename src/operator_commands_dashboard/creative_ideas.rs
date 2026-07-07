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

use axum::Json;
use serde_json::{Value, json};

use super::routes::resolve_state_root;
use crate::cognitive_memory::creative_idea::{
    CreativeIdea, CreativeIdeaStore, IdeaStatus, ProspectiveCreativeIdeaStore, parse_idea_status,
};
use crate::error::SimardResult;
use crate::memory_ipc::open_reader_client;

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
    // Single O(n) tally instead of one full scan per status; every status is
    // still emitted in `ALL` order (zero-filled).
    let mut tally: std::collections::HashMap<IdeaStatus, usize> = std::collections::HashMap::new();
    for idea in ideas {
        *tally.entry(idea.status).or_insert(0) += 1;
    }
    let mut map = serde_json::Map::new();
    for status in IdeaStatus::ALL {
        let n = tally.get(&status).copied().unwrap_or(0);
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
    use crate::memory_ipc::{clear_in_process_writer, register_in_process_writer};
    use crate::test_support::HermeticState;

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

    fn seed(ops: &dyn CognitiveMemoryOps, text: &str, status: IdeaStatus) {
        let store = ProspectiveCreativeIdeaStore::new(ops);
        let mut idea = CreativeIdea::new(text, ctx(), 1);
        idea.node_id = store.store(&idea).expect("store");
        if status != IdeaStatus::New {
            idea.try_transition(status).expect("transition");
            store.update(&idea).expect("update");
        }
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
}
