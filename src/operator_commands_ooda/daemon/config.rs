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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize tests that mutate `SIMARD_DASHBOARD_PORT`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_values_without_env() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
        let cfg = DaemonDashboardConfig::default();
        assert!(cfg.enabled, "dashboard enabled by default");
        assert_eq!(cfg.port, 8080, "default port must be 8080");
    }

    #[test]
    fn env_override_sets_port() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "4444") };
        let cfg = DaemonDashboardConfig::default();
        assert_eq!(cfg.port, 4444);
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
    }

    #[test]
    fn invalid_env_falls_back_to_default_port() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "garbage") };
        let cfg = DaemonDashboardConfig::default();
        assert_eq!(cfg.port, 8080, "unparseable env must fall back to 8080");
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
    }

    #[test]
    fn empty_env_falls_back_to_default_port() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "") };
        let cfg = DaemonDashboardConfig::default();
        assert_eq!(cfg.port, 8080);
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
    }

    #[test]
    fn struct_construction_direct() {
        let cfg = DaemonDashboardConfig {
            enabled: false,
            port: 9999,
        };
        assert!(!cfg.enabled);
        assert_eq!(cfg.port, 9999);
    }

    #[test]
    fn zero_port_is_valid() {
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SIMARD_DASHBOARD_PORT", "0") };
        let cfg = DaemonDashboardConfig::default();
        assert_eq!(cfg.port, 0, "port 0 lets the OS pick an ephemeral port");
        unsafe { std::env::remove_var("SIMARD_DASHBOARD_PORT") };
    }
}
