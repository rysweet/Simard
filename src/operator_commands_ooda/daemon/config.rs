/// Configuration for the embedded dashboard that runs inside the OODA daemon.
pub struct DaemonDashboardConfig {
    /// Whether to spawn the dashboard as a background task.
    pub enabled: bool,
    /// TCP port for the dashboard (default: 8080, overridable via
    /// `SIMARD_DASHBOARD_PORT` env var or `--dashboard-port=` CLI flag).
    pub port: u16,
}

impl Default for DaemonDashboardConfig {
    fn default() -> Self {
        let port = std::env::var("SIMARD_DASHBOARD_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8080);
        Self {
            enabled: true,
            port,
        }
    }
}
