mod auth;
mod monitoring;
mod goals;
mod memory;
mod activity;
mod workboard;
mod current_work;
mod distributed;
mod tmux;
mod hosts;
mod chat;
mod logs;
mod registry;
mod metrics;
mod agent_log;
pub(crate) mod routes;

#[cfg(test)]
mod tests_attach;
#[cfg(test)]
mod tests_routes_a;
#[cfg(test)]
mod tests_routes_b;

use std::net::SocketAddr;

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
