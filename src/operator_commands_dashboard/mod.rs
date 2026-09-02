mod activity;
mod agent_log;
mod auth;
mod brain_failures;
mod chat;
mod chat_store;
mod creative_ideas;
mod current_work;
pub(crate) mod cycle_source;
mod distributed;
mod enrichment;
mod feedback;
mod goals;
mod goals_status;
mod hosts;
mod index_html;
mod journal;
mod live_engineers;
mod logs;
mod memory;
mod merge_judge;
mod merge_readiness;
mod metrics;
mod monitoring;
mod ooda_cycles;
mod overseer;
mod pr_readiness;
mod registry;
pub(crate) mod routes;
mod status;
mod subagent;
mod thinking_collapse;
mod tmux;
mod workboard;

#[cfg(test)]
mod tests_activity;
#[cfg(test)]
mod tests_attach;
#[cfg(test)]
mod tests_chat_routes;
#[cfg(test)]
mod tests_chat_store;
#[cfg(test)]
mod tests_feedback;
#[cfg(test)]
mod tests_goal_records_migration;
#[cfg(test)]
mod tests_goals_crud;
// Issue #2679: the Memory dashboard reported "remembered 0 items in the last
// hour" from a hardcoded placeholder in `memory_recent`. These tests pin the
// corrected trailing-hour delta (`memory_recent_at`) and the pure window-edge
// baseline selection (`select_last_hour_baseline`) so the placeholder cannot
// silently return.
#[cfg(test)]
mod tests_memory_recent_last_hour;
// Issue #2922: the dashboard goal-board READ is live — `dashboard_live_goal_board`
// unions the `goal-board:snapshot` base with a live `CognitiveMemoryGoalStore`
// overlay (deduped by slug, board wins), so a promoted creative-idea Proposed
// goal appears immediately instead of lagging a ~5-min snapshot cycle; the read
// is fail-closed (no silent fallback to the stale snapshot).
#[cfg(test)]
mod tests_live_goal_board;
#[cfg(test)]
mod tests_ooda_cycles_history;
// Issue #26: the Logs tab's "Cycle Reports" card (#cycle-reports) must show the
// live cycle index (unioned across both persisted dirs, newest-first), the real
// per-cycle tree status, full OODA detail in the shared Thinking-tab shape, and
// collapse identical no-progress cycles — agreeing with the Thinking view via
// one shared reader.
#[cfg(test)]
mod tests_cycle_reports_activity;
#[cfg(test)]
mod tests_routes_a;
#[cfg(test)]
mod tests_routes_b;

// Issue #2491 / measurement issue #2494 (G1 hybrid measurement, Step 7): the
// read-only GET /api/cognition/recall-precision correlation endpoint — the
// hybrid join of the fixed-corpus benchmark score and the live trend, its
// clamped params, verdict truth table, and fail-closed auth/leak contract.
#[cfg(test)]
mod tests_recall_precision_correlation;

// Issue #2942 (Step 7): the read-only GET /api/enrichment endpoint that surfaces
// whether recall is reaching decisions — attach-rate and average injected
// facts/procedures/preamble-bytes per decision, read from the live
// metrics_snapshot.json. Pins its clamped params, degrade-safe/missing-snapshot
// contract, populated-schema surface, and fail-closed auth + no-leak posture.
#[cfg(test)]
mod tests_enrichment_endpoint;

// Issue #2798 — Layer B: pins that the dashboard resolver (`resolve_state_root`)
// equals the daemon resolver (`simard_state_root` / `default_state_root`) for
// every `SIMARD_STATE_ROOT` input class, so reader tier-0 shares the daemon's
// live writer. The empty/relative env cases fail RED on the pre-fix divergent
// resolver.
#[cfg(test)]
mod tests_state_root_parity;

use std::net::SocketAddr;
use std::path::Path;

use crate::error::SimardResult;
use crate::goal_curation::{
    BoardPlacement, GoalBoard, load_goal_board, record_as_active_goal, save_goal_board,
    save_goal_board_with_removals,
};
use crate::goals::{goal_slug, list_via_ops};
use crate::memory_ipc::{launch_writer_client, open_reader_client};

/// Read the cognitive-memory `goal-board:snapshot` for the dashboard.
///
/// Used by every dashboard handler that previously read the legacy
/// on-disk goal-records file from `<state_root>` (issue #1590). Routes
/// through [`open_reader_client`] so the daemon's IPC writer can serve
/// the read when running embedded; otherwise opens the on-disk DB
/// read-only.
pub(crate) fn dashboard_goal_board_snapshot(state_root: &Path) -> SimardResult<GoalBoard> {
    let reader = open_reader_client(state_root)?;
    load_goal_board(reader.ops())
}

/// Persist a `GoalBoard` from a dashboard write handler.
///
/// Routes through [`launch_writer_client`] which prefers the daemon's IPC
/// socket (avoiding lock contention when the daemon is running) and falls
/// back to a direct on-disk open otherwise (issue #1590).
pub(crate) fn dashboard_save_goal_board(state_root: &Path, board: &GoalBoard) -> SimardResult<()> {
    let writer = launch_writer_client(state_root)?;
    save_goal_board(board, writer.ops())
}

/// Persist a `GoalBoard` from a dashboard write handler while force-removing
/// `force_remove_ids` from the merged snapshot.
///
/// Plain [`dashboard_save_goal_board`] uses merge-on-write semantics that
/// re-add any goal *absent* from the in-flight board, so a concurrent writer's
/// goals are never lost (#1915). That same merge resurrects a goal an operator
/// explicitly removed: its id is absent from the in-flight board, so
/// [`merge_boards`](crate::goal_curation) keeps the persisted copy. Routing an
/// explicit removal through [`save_goal_board_with_removals`] filters those ids
/// out of the merged result so the removal actually persists — matching the CLI
/// `simard goal remove` path (#1923 / #1925 / #1926).
pub(crate) fn dashboard_save_goal_board_with_removals(
    state_root: &Path,
    board: &GoalBoard,
    force_remove_ids: &[String],
) -> SimardResult<()> {
    let writer = launch_writer_client(state_root)?;
    save_goal_board_with_removals(board, force_remove_ids, writer.ops())
}

/// Build the dashboard's **live** goal board (issue #2922): the authoritative
/// `goal-board:snapshot` board unioned with a LIVE `CognitiveMemoryGoalStore`
/// overlay, so a freshly-persisted `goal-store:record` (a promoted creative-idea
/// Proposed goal, a meeting goal, an unblocked goal, …) appears immediately
/// instead of lagging up to a ~5-min snapshot cycle.
///
/// Two live sources — both read off a SINGLE shared reader (issue #2922
/// perf) and deduped by slug with the **base winning**:
/// - **Base** — [`load_goal_board`], the operator read-your-writes snapshot
///   board (identical to [`dashboard_goal_board_snapshot`]) carrying the
///   daemon-OODA active/backlog goals with their rich fields (`current_activity`,
///   `wip_refs`, `repo`, …).
/// - **Overlay** — [`list_via_ops`] over the same reader, the shared live
///   goal-store the creative-ideas / meeting / runtime / seed writers `put`
///   into. A record whose slug is already on the base board is dropped (the base
///   carries the richer, authoritative fields); only records *absent* from the
///   base are mapped in via [`record_as_active_goal`].
///
/// **Fail-closed** (issues #2922 / #2896): either leg failing — the snapshot
/// base read or the overlay `list()` — propagates as `Err`. There is no fallback
/// to the stale snapshot and no coercion of a transport fault into a phantom
/// empty board; the caller surfaces the error rather than serving silently-stale
/// or partial data.
pub(crate) fn dashboard_live_goal_board(state_root: &Path) -> SimardResult<GoalBoard> {
    // Issue #2922 perf: open the reader ONCE and serve BOTH live legs
    // (the snapshot base and the goal-store overlay) from the same handle. Both
    // legs already resolve to the same `state_root` reader, so a single open is
    // behaviorally identical — but in the standalone-dashboard tier each open is
    // a fresh Unix-socket connect plus a synchronous Ping/Pong handshake, so
    // collapsing two opens into one removes a full round-trip from every Goals /
    // Memory poll. Fail-closed: a reader-open or transport fault on either leg
    // still propagates as `Err`, never a stale or partial board.
    let reader = open_reader_client(state_root)?;
    let ops = reader.ops();

    // Base: authoritative snapshot board. Fail-closed — never unwrap_or_default.
    let mut board = load_goal_board(ops)?;

    // Index the base by slug so the overlay dedup is O(overlay). Slugs are
    // derived with `goal_slug` (the SAME function the forward adapter uses),
    // because an `ActiveGoal.id` is not guaranteed to already be a slug —
    // comparing raw ids would let a slugged overlay record slip past dedup and
    // double-render.
    let mut seen: std::collections::HashSet<String> = board
        .active
        .iter()
        .map(|g| goal_slug(&g.id))
        .chain(board.backlog.iter().map(|b| goal_slug(&b.id)))
        .collect();

    // Overlay: LIVE goal-store records off the SAME reader handle. The read is
    // fail-closed (#2896); a `search_facts` transport fault propagates as `Err`.
    let overlay = list_via_ops(ops)?;
    for record in overlay {
        if seen.contains(&record.slug) {
            continue;
        }
        match record_as_active_goal(&record) {
            BoardPlacement::Active(goal) => {
                seen.insert(record.slug);
                board.active.push(goal);
            }
            BoardPlacement::Backlog(item) => {
                seen.insert(record.slug);
                board.backlog.push(item);
            }
            // Terminal (`Completed`) records are not surfaced on the live board.
            BoardPlacement::Skip => {}
        }
    }

    Ok(board)
}

/// Initialize dashboard auth and print the login code to stderr.
/// Must be called before serving traffic (both standalone and embedded modes).
pub fn init_auth() -> (String, bool) {
    let (code, loaded) = auth::init_login_code();
    assert!(
        auth::is_auth_initialized(),
        "BUG: dashboard auth not initialized after init_login_code()"
    );
    (code, loaded)
}

/// Spawn the dashboard as a tokio background task on the given runtime.
///
/// Returns a `JoinHandle` so the caller can detect if the server exits
/// unexpectedly. The dashboard is cancelled automatically when the runtime
/// shuts down, which is the desired behavior for daemon integration.
pub fn spawn_dashboard_task(
    rt: &tokio::runtime::Handle,
    port: u16,
) -> tokio::task::JoinHandle<Result<(), String>> {
    rt.spawn(async move {
        let app = routes::build_router();
        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("[simard] Dashboard listening on http://{addr}");

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("dashboard bind failed on port {port}: {e}"))?;
        axum::serve(listener, app)
            .await
            .map_err(|e| format!("dashboard serve error: {e}"))
    })
}

/// Serve the dashboard as a standalone process (creates its own tokio runtime).
pub fn serve(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let (code, loaded) = init_auth();

    eprintln!("\n  🌲 Simard Dashboard");
    if loaded {
        eprintln!("  Login code: {code} (loaded from ~/.simard/.dashkey)");
    } else {
        eprintln!("  Login code: {code} (saved to ~/.simard/.dashkey)");
    }
    eprintln!("  Open http://localhost:{port} and enter the code\n");

    // Standalone `dashboard serve` does NOT open or register its own
    // cognitive-memory handle. Every dashboard read/write goes through the
    // launcher resolution ladder (`open_reader_client` / `launch_writer_client`),
    // consulted per request: it routes to a running daemon's IPC socket (tier-1)
    // when one is serving this `state_root`, and otherwise to the tier-2
    // shared-store cache (#2334), which already gives this process a single
    // handle per `state_root` with read-after-write consistency. Eagerly
    // registering a dashboard-owned tier-0 handle here would shadow the tier-1
    // socket for the whole process lifetime, turning the dashboard into a second
    // concurrent cross-process writer that silently drops goals whenever a daemon
    // runs on the same `state_root` (issue #2366).

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let app = routes::build_router();

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        eprintln!("Simard dashboard listening on http://{addr}");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
}
