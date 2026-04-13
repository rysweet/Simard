use std::path::PathBuf;

use crate::operator_commands_ooda::{DaemonDashboardConfig, run_ooda_daemon};

use super::args::next_required;

pub(super) fn dispatch_ooda_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "ooda command")?;
    match subcommand.as_str() {
        "run" => {
            let mut max_cycles: u32 = 0; // 0 = infinite
            let mut state_root: Option<PathBuf> = None;
            let mut auto_reload = true;
            let mut dashboard = DaemonDashboardConfig::default();

            for arg in args {
                if let Some(n) = arg.strip_prefix("--cycles=") {
                    max_cycles = n
                        .parse()
                        .map_err(|_| format!("invalid --cycles value: {n}"))?;
                } else if arg == "--no-auto-reload" {
                    auto_reload = false;
                } else if arg == "--no-dashboard" {
                    dashboard.enabled = false;
                } else if let Some(p) = arg.strip_prefix("--dashboard-port=") {
                    dashboard.port = p
                        .parse()
                        .map_err(|_| format!("invalid --dashboard-port value: {p}"))?;
                } else if state_root.is_none() {
                    state_root = Some(PathBuf::from(arg));
                } else {
                    return Err(format!("unexpected argument: {arg}").into());
                }
            }

            run_ooda_daemon(max_cycles, state_root, auto_reload, dashboard)
        }
        other => Err(format!("unsupported command 'ooda {other}'").into()),
    }
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn test_ooda_missing_subcommand() {
        let result = dispatch_operator_cli(vec!["ooda".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected ooda command")
        );
    }

    #[test]
    fn test_ooda_unknown_subcommand() {
        let result = dispatch_operator_cli(vec!["ooda".to_string(), "xyz".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command 'ooda xyz'")
        );
    }

    #[test]
    fn test_ooda_run_invalid_cycles() {
        let result = dispatch_operator_cli(vec![
            "ooda".to_string(),
            "run".to_string(),
            "--cycles=abc".to_string(),
        ]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid --cycles"));
    }

    #[test]
    fn test_ooda_run_extra_positional_after_state_root() {
        let result = dispatch_operator_cli(vec![
            "ooda".to_string(),
            "run".to_string(),
            "/state".to_string(),
            "extra".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected argument")
        );
    }
}
