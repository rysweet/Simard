mod activity;
mod agent_log;
mod auth;
mod brain_failures;
mod chat;
mod current_work;
mod cycle_source;
mod distributed;
mod goals;
mod goals_status;
mod hosts;
mod index_html;
mod logs;
mod memory;
mod merge_judge;
mod merge_readiness;
mod metrics;
mod monitoring;
mod ooda_cycles;
mod pr_readiness;
mod registry;
pub(crate) mod routes;
mod subagent;
mod tmux;
mod workboard;

#[cfg(test)]
mod tests_activity;
#[cfg(test)]
mod tests_attach;
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
use std::sync::Arc;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::SimardResult;
use crate::goal_curation::{GoalBoard, load_goal_board, save_goal_board};
use crate::memory_ipc::{launch_writer_bridge, open_reader_bridge, register_in_process_writer};

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

    // Open the cognitive-memory store ONCE and register it as the shared tier-0
    // in-process writer, mirroring the OODA daemon and bootstrap assembly (which
    // standalone `dashboard serve` does not go through). The launcher's tier-2
    // store cache (#2334, closing the #2320 goal-board read-after-write race)
    // already shares one handle per `state_root`, so this registration is
    // defense-in-depth and architectural consistency: the dashboard owns its
    // handle on the same tier-0 path the daemon uses instead of relying solely on
    // the tier-2 fallback. The strong Arc is held for the lifetime of `serve`
    // (i.e. the whole process) so the registry's `Weak` stays upgradeable.
    let state_root = routes::resolve_state_root();
    let _shared_writer = register_dashboard_shared_writer(&state_root);

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

/// Open the dashboard's cognitive-memory store and register it as the shared
/// in-process writer, returning the strong `Arc` the caller must keep alive for
/// as long as the dashboard serves requests.
///
/// Mirrors the OODA daemon and bootstrap assembly, which open the library-backed
/// store once and register one shared handle. Registering the dashboard's handle
/// on the same tier-0 in-process path keeps dashboard reads and writes on one
/// store. Same-process read-after-write consistency is also guaranteed by the
/// launcher's tier-2 store cache (added in #2334 to close the #2320 race), so
/// this tier-0 registration is defense-in-depth: it aligns the dashboard with
/// the daemon/bootstrap rather than relying solely on the tier-2 fallback.
///
/// Returns `None` (after logging) when the store cannot be opened, leaving
/// handlers on their graceful tier-1/tier-2 fallback rather than failing to serve.
pub(crate) fn register_dashboard_shared_writer(
    state_root: &Path,
) -> Option<Arc<dyn CognitiveMemoryOps>> {
    match LibraryCognitiveMemory::open(state_root) {
        Ok(memory) => {
            let writer: Arc<dyn CognitiveMemoryOps> = Arc::new(memory);
            register_in_process_writer(state_root.to_path_buf(), Arc::clone(&writer));
            Some(writer)
        }
        Err(error) => {
            eprintln!(
                "[simard] dashboard: shared cognitive-memory handle not registered at {} \
                 ({error}); handlers will use the IPC/tier-2 fallback",
                state_root.display()
            );
            None
        }
    }
}
