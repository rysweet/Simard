mod activity;
mod agent_log;
mod auth;
mod brain_failures;
mod chat;
mod chat_store;
mod current_work;
mod cycle_source;
mod distributed;
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
mod tests_goal_records_migration;
#[cfg(test)]
mod tests_goals_crud;
#[cfg(test)]
mod tests_routes_a;
#[cfg(test)]
mod tests_routes_b;

use std::net::SocketAddr;
use std::path::Path;

use crate::error::SimardResult;
use crate::goal_curation::{
    GoalBoard, load_goal_board, save_goal_board, save_goal_board_with_removals,
};
use crate::memory_ipc::{launch_writer_bridge, open_reader_bridge};

/// Read the cognitive-memory `goal-board:snapshot` for the dashboard.
///
/// Used by every dashboard handler that previously read the legacy
/// on-disk goal-records file from `<state_root>` (issue #1590). Routes
/// through [`open_reader_bridge`] so the daemon's IPC writer can serve
/// the read when running embedded; otherwise opens the on-disk DB
/// read-only.
pub(crate) fn dashboard_goal_board_snapshot(state_root: &Path) -> SimardResult<GoalBoard> {
    let reader = open_reader_bridge(state_root)?;
    load_goal_board(reader.ops())
}

/// Persist a `GoalBoard` from a dashboard write handler.
///
/// Routes through [`launch_writer_bridge`] which prefers the daemon's IPC
/// socket (avoiding lock contention when the daemon is running) and falls
/// back to a direct on-disk open otherwise (issue #1590).
pub(crate) fn dashboard_save_goal_board(state_root: &Path, board: &GoalBoard) -> SimardResult<()> {
    let writer = launch_writer_bridge(state_root)?;
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
    let writer = launch_writer_bridge(state_root)?;
    save_goal_board_with_removals(board, force_remove_ids, writer.ops())
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
    // launcher resolution ladder (`open_reader_bridge` / `launch_writer_bridge`),
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
