mod activity;
mod agent_log;
mod auth;
mod chat;
mod current_work;
mod distributed;
mod goals;
mod hosts;
mod index_html;
mod logs;
mod memory;
mod metrics;
mod monitoring;
mod registry;
pub(crate) mod routes;
mod subagent;
mod tmux;
mod workboard;

#[cfg(test)]
mod tests_attach;
#[cfg(test)]
mod tests_goal_records_migration;
#[cfg(test)]
mod tests_routes_a;
#[cfg(test)]
mod tests_routes_b;

use std::net::SocketAddr;
use std::path::Path;

use crate::error::SimardResult;
use crate::goal_curation::{GoalBoard, load_goal_board, save_goal_board};
use crate::memory_ipc::{launch_writer_bridge, open_reader_bridge};

/// Read the cognitive-memory `goal-board:snapshot` for the dashboard.
///
/// Used by every dashboard handler that previously read
/// `<state_root>/goal_records.json` from disk (issue #1590). Routes
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
