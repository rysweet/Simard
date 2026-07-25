use std::collections::HashSet;

use axum::Json;
use axum::extract::Path;
use serde_json::{Value, json};

use super::goals_status::render_status_and_detail;
use super::routes::resolve_state_root;
use super::{
    dashboard_goal_board_snapshot, dashboard_live_goal_board, dashboard_save_goal_board,
    dashboard_save_goal_board_with_removals,
};
use crate::goal_curation::{ActiveGoal, BacklogItem, GoalBoard, GoalProgress, MAX_ACTIVE_GOALS};
use crate::goals::goal_slug;
use crate::memory_ipc::open_reader_client;

/// Load the dashboard's view of the goal board from the EXPLICIT `state_root`
/// instead of resolving `SIMARD_STATE_ROOT` ambiently. Returns an empty
/// `GoalBoard` when the snapshot is missing or the memory cannot be opened —
/// the dashboard always renders rather than 500ing.
///
/// `state_root` is trusted-internal: it originates only from a handler
/// wrapper's `resolve_state_root()` or a test's `HermeticState`, NEVER from
/// request data, so threading it through carries no path-traversal risk
/// (#2408 / #2384).
fn load_board_or_empty_at(state_root: &std::path::Path) -> GoalBoard {
    dashboard_goal_board_snapshot(state_root).unwrap_or_default()
}

/// Returns `true` when `s` looks like a debug key=value dump rather than
/// prose — e.g. `"priority=3 status=proposed rationale=Action…"`. Heuristic:
/// three or more `word=` patterns in one short string (#1686).
fn looks_like_debug_string(s: &str) -> bool {
    let kv_count = s
        .split_whitespace()
        .filter(|w| {
            let eq = w.find('=');
            // The key must be at least 2 chars long and the value at least 1.
            matches!(eq, Some(pos) if pos >= 2 && pos + 1 < w.len())
        })
        .count();
    kv_count >= 3
}

/// Derive a short human-readable ID from content + concept instead of
/// exposing the raw `sem_019e18ac…` node ID (#1686). Takes the first few
/// words of the content, or falls back to the concept label.
fn human_backlog_id(content: &str, concept: &str) -> String {
    let words: Vec<&str> = content.split_whitespace().take(6).collect();
    if words.len() >= 2 {
        let slug = words.join(" ");
        if slug.len() > 50 {
            // Char-boundary safe: `&slug[..50]` panics when byte 50 splits a
            // multi-byte char, and the slug is arbitrary memory-graph title text.
            let mut t = slug;
            crate::util::string_truncate::truncate_to_char_boundary(&mut t, 50);
            format!("{}…", t.trim_end())
        } else {
            slug
        }
    } else {
        human_source_label(concept).to_string()
    }
}

/// Plain-English label for a cognitive-memory concept path, replacing the
/// raw `cognitive-memory/<concept>` prefix (#1686).
fn human_source_label(concept: &str) -> &'static str {
    let c = concept.to_lowercase();
    if c.contains("goal") {
        "From goals"
    } else if c.contains("action") {
        "From actions"
    } else if c.contains("decision") {
        "From decisions"
    } else if c.contains("meeting") {
        "From meeting"
    } else if c.contains("episode") {
        "From past event"
    } else {
        "From memory"
    }
}

pub(crate) async fn goals() -> Json<Value> {
    goals_at(&resolve_state_root()).await
}

/// Env-free core of [`goals`]: build the dashboard goal-board view from the
/// EXPLICIT `state_root` rather than resolving `SIMARD_STATE_ROOT` ambiently
/// (#2408 / #2384). See [`load_board_or_empty_at`] for the trusted-internal
/// `state_root` invariant.
pub(crate) async fn goals_at(state_root: &std::path::Path) -> Json<Value> {
    // Issue #2922: read the LIVE goal board (snapshot base unioned with the live
    // `CognitiveMemoryGoalStore` overlay), fail-closed. A live-read failure
    // surfaces an explicit error payload with zeroed counts and empty lists —
    // NEVER a silently-empty or stale board that a client could not distinguish
    // from "no goals". The underlying error chain is logged server-side only
    // (tracing), never returned to the client.
    let board = match dashboard_live_goal_board(state_root) {
        Ok(board) => board,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "dashboard live goal-board read failed; serving fail-closed error payload"
            );
            return Json(json!({
                "active": [],
                "backlog": [],
                "active_count": 0,
                "backlog_count": 0,
                "error": "goal-board read failed",
            }));
        }
    };

    // Issue #2695 follow-up: emit active goals ordered by priority ASCENDING
    // (p1 = highest first) with a stable id tiebreak, so the Goals tab renders a
    // priority-ordered tree and priority is both visible AND actionable. The
    // ordering is a display concern here; the SUBSTANCE (differentiating flat
    // priorities) is the prioritization pass on the curation/decompose path.
    let mut active_goals = board.active;
    active_goals.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.id.cmp(&b.id)));

    // Lifecycle breakdown of the active board so the Goals tab can distinguish
    // goals genuinely in progress from ones that are blocked, paused, or
    // already `Completed` but not yet archived off the board. Computed BEFORE
    // `active_goals` is consumed below. Additive: `active_count` is unchanged.
    let active_status_breakdown = active_status_breakdown(active_goals.iter().map(|g| &g.status));

    let active: Vec<Value> = active_goals
        .into_iter()
        .map(|g| {
            // Issue #1684: render the raw brain-log `current_activity` string
            // into a plain-English `status_chip` + `detail` pair, plus the
            // unredacted `detail_full` for click-to-expand. `current_activity`
            // is kept as-is (alias) so existing consumers do not break.
            let (chip, detail, detail_full) =
                render_status_and_detail(g.current_activity.as_deref());
            let mut obj = json!({
                "id": g.id,
                "description": g.description,
                "priority": g.priority,
                // Issue #2695 follow-up: additively expose the structured
                // decomposition back-reference so the Goals tab can NEST a
                // sub-goal under its active parent from durable board data (G3),
                // never by parsing the description. `null` for a top-level goal.
                "parent_goal_id": g.parent_goal_id,
                // Issue #2695 follow-up: additively expose operator-set priority
                // provenance so the client (and the prioritization pass) can tell
                // a hand-pinned priority from a differentiate-eligible default.
                "priority_explicit": g.priority_explicit,
                "status": g.status.to_string(),
                // Issue #20: additively expose the SERIALIZED `GoalProgress`
                // enum so the Goals tab can render a distinct, correctly-labeled
                // lifecycle badge (and surface a block reason) per goal instead
                // of dumping the free-form `status` string — which, paired with
                // the red activity chip, made every goal read as "failed". The
                // legacy `status` Display string above is left untouched
                // (additive-only); consumers parse the enum by variant (G3),
                // never the Display string.
                "status_progress": &g.status,
                "assigned_to": g.assigned_to,
                "repo": g.repo,
                "current_activity": g.current_activity,
                "status_chip": chip.as_str(),
                "detail": detail,
                "detail_full": detail_full,
                "wip_refs": g.wip_refs,
            });
            // Issue #2743: additively expose the goal's labels (tags) so the
            // Goals tab can render label chips and filter by tag. Omitted when
            // empty (mirrors the serde `skip_serializing_if` contract), so
            // existing `/api/goals` consumers are unaffected.
            if !g.labels.is_empty() {
                obj["labels"] = json!(g.labels);
            }
            obj
        })
        .collect();

    let mut backlog: Vec<Value> = board
        .backlog
        .into_iter()
        .map(|g| {
            json!({
                "id": g.id,
                "description": g.description,
                "source": g.source,
                "score": g.score,
            })
        })
        .collect();

    // Pull meeting-captured actions and decisions from cognitive memory (#415)
    // (#1686: filter out raw memory IDs and debug strings, provide clean labels)
    if let Ok(reader) = open_reader_client(state_root) {
        // Build an O(1) id index once so the per-fact dedup below is O(facts)
        // instead of re-scanning the whole active+backlog list (with a serde
        // map lookup per element) for every fact — this endpoint is polled on
        // every dashboard refresh. Ids of newly-listed facts are inserted as we
        // go, so later facts still dedup against earlier ones (unchanged
        // behavior, just without the quadratic scan).
        let mut seen_ids: HashSet<String> = active
            .iter()
            .chain(backlog.iter())
            .filter_map(|g| g.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        let mem = reader.ops();
        for tag in &["goal", "action", "decision"] {
            if let Ok(facts) = mem.search_facts(tag, 20, 0.0) {
                for fact in facts {
                    // Skip goal-board snapshots — they contain the entire
                    // serialized GoalBoard, not individual backlog items.
                    if fact.concept.contains("snapshot") || fact.concept.contains("goal-board") {
                        continue;
                    }
                    // Skip facts whose content looks like serialized JSON objects
                    let trimmed = fact.content.trim();
                    if trimmed.starts_with('{') || trimmed.starts_with('[') {
                        continue;
                    }
                    // (#1686) Skip facts whose content looks like debug key=value
                    // strings (e.g. "priority=3 status=proposed rationale=Action…")
                    if looks_like_debug_string(trimmed) {
                        continue;
                    }
                    if seen_ids.contains(fact.node_id.as_str()) {
                        continue;
                    }
                    // (#1686) Derive a human-readable title from the content
                    // instead of exposing the raw `sem_019e18ac…` node ID.
                    let display_id = human_backlog_id(&fact.content, &fact.concept);
                    let source_label = human_source_label(&fact.concept);
                    seen_ids.insert(fact.node_id.clone());
                    backlog.push(json!({
                        "id": fact.node_id,
                        "display_id": display_id,
                        "description": fact.content,
                        "source": source_label,
                        "score": fact.confidence,
                    }));
                }
            }
        }
    }

    Json(json!({
        "active": active,
        "backlog": backlog,
        "active_count": active.len(),
        "backlog_count": backlog.len(),
        // Issue #4270: additive per-lifecycle breakdown of the active board so
        // the Goals tab (and API clients) can tell in-progress goals apart from
        // blocked / paused / not-yet-archived Completed ones, instead of a
        // single `active_count` that reads e.g. "20 active goal(s)" when half
        // are finished. Existing consumers are unaffected (fields untouched).
        "active_status_breakdown": Value::Object(active_status_breakdown),
    }))
}

/// Per-variant lifecycle breakdown of the active goal board.
///
/// Returns a JSON object with a faithful count for every [`GoalProgress`]
/// variant (`proposed`, `not_started`, `in_progress`, `blocked`, `paused`,
/// `completed`). No bucketing surprises: an `InProgress { percent: 100 }` goal
/// counts as `in_progress`, not `completed` — callers that want the terminal
/// view use [`GoalProgress::is_terminal`] additively. All six keys are always
/// present (zero when unused) so clients can render a stable layout without
/// null-checking each field.
///
/// This exists because the Goals tab previously showed only `active_count`,
/// which conflates goals genuinely in progress with ones that are blocked,
/// paused, or already `Completed` but not yet archived off the board — hiding
/// what Simard is *actually working on right now*.
pub(crate) fn active_status_breakdown<'a>(
    statuses: impl IntoIterator<Item = &'a GoalProgress>,
) -> serde_json::Map<String, Value> {
    let mut proposed = 0u64;
    let mut not_started = 0u64;
    let mut in_progress = 0u64;
    let mut blocked = 0u64;
    let mut paused = 0u64;
    let mut completed = 0u64;
    for status in statuses {
        match status {
            GoalProgress::Proposed => proposed += 1,
            GoalProgress::NotStarted => not_started += 1,
            GoalProgress::InProgress { .. } => in_progress += 1,
            GoalProgress::Blocked(_) => blocked += 1,
            GoalProgress::Paused => paused += 1,
            GoalProgress::Completed => completed += 1,
        }
    }
    let mut breakdown = serde_json::Map::new();
    breakdown.insert("proposed".to_string(), json!(proposed));
    breakdown.insert("not_started".to_string(), json!(not_started));
    breakdown.insert("in_progress".to_string(), json!(in_progress));
    breakdown.insert("blocked".to_string(), json!(blocked));
    breakdown.insert("paused".to_string(), json!(paused));
    breakdown.insert("completed".to_string(), json!(completed));
    breakdown
}

pub(crate) async fn seed_goals() -> Json<Value> {
    seed_goals_at(&resolve_state_root()).await
}

/// Env-free core of [`seed_goals`]: seed the EXPLICIT `state_root`. See
/// [`load_board_or_empty_at`] for the trusted-internal `state_root` invariant
/// (#2408 / #2384).
pub(crate) async fn seed_goals_at(state_root: &std::path::Path) -> Json<Value> {
    let existing = dashboard_goal_board_snapshot(state_root).unwrap_or_default();
    if !existing.active.is_empty() {
        return Json(json!({"status": "already_seeded", "message": "Goals already exist"}));
    }

    let mut board = GoalBoard::new();
    let now = chrono::Utc::now().to_rfc3339();
    board.active.push(ActiveGoal {
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "self-improvement".to_string(),
        description:
            "Continuously improve own capabilities through gym scenarios and self-evaluation"
                .to_string(),
        priority: 1,
        status: GoalProgress::InProgress { percent: 0 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some(format!("Goal seeded via dashboard at {now}")),
        wip_refs: vec![],
        last_progress_update_at: None,
        labels: vec![crate::goal_curation::labels::SOURCE_SEED.to_string()],
    });
    board.active.push(ActiveGoal {
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: "knowledge-growth".to_string(),
        description:
            "Expand knowledge base through meetings, research, and cognitive memory consolidation"
                .to_string(),
        priority: 2,
        status: GoalProgress::InProgress { percent: 0 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some(format!("Goal seeded via dashboard at {now}")),
        wip_refs: vec![],
        last_progress_update_at: None,
        labels: vec![crate::goal_curation::labels::SOURCE_SEED.to_string()],
    });
    board.active.push(ActiveGoal {
        parent_goal_id: None,
        priority_explicit: false,
            repo: None,
        id: "operational-health".to_string(),
        description: "Maintain system health: budget compliance, resource usage, and error rates within thresholds".to_string(),
        priority: 3,
        status: GoalProgress::InProgress { percent: 0 },
        assigned_to: Some("simard".to_string()),
        current_activity: Some(format!("Goal seeded via dashboard at {now}")),
        wip_refs: vec![],
        last_progress_update_at: None,
        labels: vec![crate::goal_curation::labels::SOURCE_SEED.to_string()],
    });
    board.backlog.push(BacklogItem {
        id: "distributed-sync".to_string(),
        description: "Establish hive mind sync with remote Simard instances for cross-agent knowledge sharing".to_string(),
        source: "dashboard-seed".to_string(),
        score: 0.7,
    });
    board.backlog.push(BacklogItem {
        id: "meeting-quality".to_string(),
        description: "Improve meeting facilitation quality and actionable outcome generation"
            .to_string(),
        source: "dashboard-seed".to_string(),
        score: 0.6,
    });

    match dashboard_save_goal_board(state_root, &board) {
        Ok(()) => {
            Json(json!({"status": "ok", "message": "Seeded 3 active goals and 2 backlog items"}))
        }
        Err(e) => Json(json!({"status": "error", "error": format!("save failed: {e}")})),
    }
}

pub(crate) async fn add_goal(Json(body): Json<Value>) -> Json<Value> {
    add_goal_at(&resolve_state_root(), Json(body)).await
}

/// Env-free core of [`add_goal`]. BOTH the load (`load_board_or_empty_at`) and
/// the save honor the EXPLICIT `state_root`, closing the #2408 double-resolution
/// (the handler previously read `SIMARD_STATE_ROOT` once directly and again
/// inside `load_board_or_empty`). See [`load_board_or_empty_at`] for the
/// trusted-internal `state_root` invariant.
pub(crate) async fn add_goal_at(
    state_root: &std::path::Path,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut board = load_board_or_empty_at(state_root);

    let desc = match body.get("description").and_then(|v| v.as_str()) {
        Some(d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => return Json(json!({"error": "description is required"})),
    };

    let goal_type = body
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("active");
    let id = goal_slug(&desc);

    if goal_type == "backlog" {
        let score = body.get("score").and_then(|v| v.as_f64()).unwrap_or(0.5);
        board.backlog.push(BacklogItem {
            id: id.clone(),
            description: desc,
            source: "dashboard".to_string(),
            score,
        });
    } else {
        if board.active.len() >= MAX_ACTIVE_GOALS {
            return Json(json!({"error": format!(
                "Maximum {} active goals reached. Remove one first or add to backlog.",
                MAX_ACTIVE_GOALS
            )}));
        }
        let priority = body.get("priority").and_then(|v| v.as_u64()).unwrap_or(3) as u32;
        // SR-V1 (#2695 follow-up): validate priority at ingress rather than
        // silently persisting a p0 goal. p0 has no meaning (priorities are
        // 1 = highest .. n) and would poison the priority ordering/tiering.
        if priority < 1 {
            return Json(json!({"error": "priority must be >= 1"}));
        }
        // SR-V2 (#2695 follow-up): `priority_explicit` is SERVER-DERIVED
        // provenance — only the operator `simard goal set-priority` path sets it.
        // A dashboard-added goal is NOT operator-set-priority, so it stays
        // non-explicit (differentiate-eligible) regardless of any client-supplied
        // `priority_explicit`, which is ignored so a client cannot forge
        // provenance and exempt a goal from the prioritization pass.
        //
        // Issue #2359 (BUG 1): an optional target-repo slug routes the goal's
        // engineer to ~/src/<slug>. Shape-only validation here; the
        // existence/git-repo check happens later in `resolve_goal_repo` at
        // spawn time, so a goal can be created for a repo cloned shortly after.
        let repo = match body.get("repo").and_then(|v| v.as_str()) {
            Some(slug) if !slug.trim().is_empty() => {
                let slug = slug.trim();
                if let Err(e) =
                    crate::ooda_actions::advance_goal::repo_resolver::validate_repo_slug(slug)
                {
                    return Json(json!({"error": format!("invalid repo slug '{slug}': {e}")}));
                }
                Some(slug.to_string())
            }
            _ => None,
        };
        board.active.push(ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            repo,
            id: id.clone(),
            description: desc,
            priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
            labels: vec![crate::goal_curation::labels::SOURCE_OPERATOR.to_string()],
        });
    }

    match dashboard_save_goal_board(state_root, &board) {
        Ok(()) => Json(json!({"status": "ok", "id": id})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn remove_goal(Path(id): Path<String>) -> Json<Value> {
    remove_goal_at(&resolve_state_root(), Path(id)).await
}

/// Env-free core of [`remove_goal`]. See [`load_board_or_empty_at`] for the
/// trusted-internal `state_root` invariant (#2408 / #2384).
///
/// Persists through [`dashboard_save_goal_board_with_removals`] rather than the
/// plain merge-on-write save: a removed goal is absent from the in-flight
/// board, so merge-on-write would resurrect it from the persisted snapshot.
/// Force-removing the id defeats that resurrection, matching the CLI
/// `simard goal remove` contract (#1923 / #1925 / #1926).
pub(crate) async fn remove_goal_at(
    state_root: &std::path::Path,
    Path(id): Path<String>,
) -> Json<Value> {
    let mut board = load_board_or_empty_at(state_root);

    let before_active = board.active.len();
    let before_backlog = board.backlog.len();
    board.active.retain(|g| g.id != id);
    board.backlog.retain(|g| g.id != id);

    if board.active.len() == before_active && board.backlog.len() == before_backlog {
        return Json(json!({"error": "goal not found"}));
    }

    match dashboard_save_goal_board_with_removals(state_root, &board, std::slice::from_ref(&id)) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn update_goal_status(
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    update_goal_status_at(&resolve_state_root(), Path(id), Json(body)).await
}

/// Env-free core of [`update_goal_status`]. See [`load_board_or_empty_at`] for
/// the trusted-internal `state_root` invariant (#2408 / #2384).
pub(crate) async fn update_goal_status_at(
    state_root: &std::path::Path,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut board = load_board_or_empty_at(state_root);

    let status_str = match body.get("status").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Json(json!({"error": "status is required"})),
    };

    let new_status = match status_str {
        "proposed" => GoalProgress::Proposed,
        "not-started" => GoalProgress::NotStarted,
        "in-progress" => GoalProgress::InProgress { percent: 0 },
        "blocked" => {
            let reason = body
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unspecified")
                .to_string();
            GoalProgress::Blocked(reason)
        }
        "paused" => GoalProgress::Paused,
        "completed" => GoalProgress::Completed,
        other => return Json(json!({"error": format!("unknown status: {other}")})),
    };

    let mut found = false;
    for goal in &mut board.active {
        if goal.id == id {
            goal.status = new_status.clone();
            found = true;
            break;
        }
    }

    if !found {
        return Json(json!({"error": "goal not found in active goals"}));
    }

    match dashboard_save_goal_board(state_root, &board) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn promote_backlog_item(Path(id): Path<String>) -> Json<Value> {
    promote_backlog_item_at(&resolve_state_root(), Path(id)).await
}

/// Env-free core of [`promote_backlog_item`]. See [`load_board_or_empty_at`]
/// for the trusted-internal `state_root` invariant (#2408 / #2384).
pub(crate) async fn promote_backlog_item_at(
    state_root: &std::path::Path,
    Path(id): Path<String>,
) -> Json<Value> {
    let mut board = load_board_or_empty_at(state_root);

    if board.active.len() >= MAX_ACTIVE_GOALS {
        return Json(json!({"error": format!(
            "Maximum {} active goals reached. Remove one first.",
            MAX_ACTIVE_GOALS
        )}));
    }

    let pos = board.backlog.iter().position(|g| g.id == id);
    let item = match pos {
        Some(i) => board.backlog.remove(i),
        None => return Json(json!({"error": "backlog item not found"})),
    };
    let promoted_source = crate::goal_curation::labels::source_for_backlog(&item.source);

    board.active.push(ActiveGoal {
        parent_goal_id: None,
        priority_explicit: false,
        repo: None,
        id: item.id,
        description: item.description,
        priority: 3,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
        labels: vec![promoted_source.to_string()],
    });

    match dashboard_save_goal_board(state_root, &board) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

pub(crate) async fn demote_goal(Path(id): Path<String>) -> Json<Value> {
    demote_goal_at(&resolve_state_root(), Path(id)).await
}

/// Env-free core of [`demote_goal`]. See [`load_board_or_empty_at`] for the
/// trusted-internal `state_root` invariant (#2408 / #2384).
pub(crate) async fn demote_goal_at(
    state_root: &std::path::Path,
    Path(id): Path<String>,
) -> Json<Value> {
    let mut board = load_board_or_empty_at(state_root);

    let pos = board.active.iter().position(|g| g.id == id);
    let goal = match pos {
        Some(i) => board.active.remove(i),
        None => return Json(json!({"error": "active goal not found"})),
    };

    board.backlog.push(BacklogItem {
        id: goal.id,
        description: goal.description,
        source: "demoted".to_string(),
        score: 0.0,
    });

    match dashboard_save_goal_board(state_root, &board) {
        Ok(()) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

#[cfg(test)]
mod tests_backlog_helpers {
    use super::*;

    #[test]
    fn debug_string_detected() {
        assert!(looks_like_debug_string(
            "priority=3 status=proposed rationale=Action needed"
        ));
    }

    #[test]
    fn normal_prose_not_detected_as_debug() {
        assert!(!looks_like_debug_string("Fix the login page CSS"));
        assert!(!looks_like_debug_string(
            "Improve error handling in the auth module"
        ));
    }

    #[test]
    fn two_kv_pairs_not_flagged() {
        assert!(!looks_like_debug_string("status=active priority=1"));
    }

    #[test]
    fn human_backlog_id_from_long_content() {
        let id = human_backlog_id(
            "Improve the login page to handle OAuth better",
            "goal-action",
        );
        assert_eq!(id, "Improve the login page to handle");
    }

    #[test]
    fn human_backlog_id_from_short_content() {
        let id = human_backlog_id("OK", "goal-action");
        assert_eq!(id, "From goals");
    }

    #[test]
    fn human_source_label_maps_concepts() {
        assert_eq!(human_source_label("goal-action"), "From goals");
        assert_eq!(human_source_label("action-item"), "From actions");
        assert_eq!(human_source_label("decision-record"), "From decisions");
        assert_eq!(human_source_label("meeting-note"), "From meeting");
        assert_eq!(human_source_label("other-concept"), "From memory");
    }

    // ---- active_status_breakdown -----------------------------------------

    #[test]
    fn breakdown_always_has_all_six_keys() {
        let empty: Vec<GoalProgress> = vec![];
        let b = active_status_breakdown(empty.iter());
        for key in [
            "proposed",
            "not_started",
            "in_progress",
            "blocked",
            "paused",
            "completed",
        ] {
            assert_eq!(b.get(key).and_then(Value::as_u64), Some(0), "missing {key}");
        }
    }

    #[test]
    fn breakdown_counts_each_variant_faithfully() {
        let statuses = vec![
            GoalProgress::Proposed,
            GoalProgress::NotStarted,
            GoalProgress::NotStarted,
            GoalProgress::InProgress { percent: 40 },
            GoalProgress::Blocked("some reason".to_string()),
            GoalProgress::Blocked("another reason".to_string()),
            GoalProgress::Blocked("third".to_string()),
            GoalProgress::Paused,
            GoalProgress::Completed,
            GoalProgress::Completed,
        ];
        let b = active_status_breakdown(statuses.iter());
        assert_eq!(b["proposed"].as_u64(), Some(1));
        assert_eq!(b["not_started"].as_u64(), Some(2));
        assert_eq!(b["in_progress"].as_u64(), Some(1));
        assert_eq!(b["blocked"].as_u64(), Some(3));
        assert_eq!(b["paused"].as_u64(), Some(1));
        assert_eq!(b["completed"].as_u64(), Some(2));
    }

    #[test]
    fn breakdown_in_progress_at_100_percent_is_not_completed() {
        // Faithful per-variant counting: an InProgress goal at 100% is still
        // `in_progress`, never bucketed as `completed` (that terminal view is
        // GoalProgress::is_terminal, layered additively by callers).
        let statuses = [GoalProgress::InProgress { percent: 100 }];
        let b = active_status_breakdown(statuses.iter());
        assert_eq!(b["in_progress"].as_u64(), Some(1));
        assert_eq!(b["completed"].as_u64(), Some(0));
    }

    #[test]
    fn breakdown_sum_equals_input_len() {
        let statuses = [
            GoalProgress::Proposed,
            GoalProgress::InProgress { percent: 10 },
            GoalProgress::Completed,
            GoalProgress::Blocked("x".to_string()),
            GoalProgress::Paused,
        ];
        let b = active_status_breakdown(statuses.iter());
        let sum: u64 = [
            "proposed",
            "not_started",
            "in_progress",
            "blocked",
            "paused",
            "completed",
        ]
        .iter()
        .map(|k| b[*k].as_u64().unwrap_or(0))
        .sum();
        assert_eq!(sum as usize, statuses.len());
    }
}
