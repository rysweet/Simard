use crate::operator_commands_dashboard;

use super::args::next_required;

pub fn dispatch_dashboard_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = next_required(&mut args, "dashboard subcommand (serve)")?;
    match subcommand.as_str() {
        "serve" => {
            let mut port: u16 = 8080;
            for arg in args {
                if let Some(p) = arg.strip_prefix("--port=") {
                    port = p
                        .parse()
                        .map_err(|_| format!("invalid --port value: {p}"))?;
                } else {
                    return Err(format!("unexpected argument: {arg}").into());
                }
            }
            operator_commands_dashboard::serve(port)
        }
        other => Err(format!("unsupported command 'dashboard {other}'").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_subcommand_returns_error() {
        let args = Vec::<String>::new().into_iter();
        let result = dispatch_dashboard_command(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected"));
    }

    #[test]
    fn unsupported_subcommand_returns_error() {
        let args = vec!["unknown".to_string()].into_iter();
        let result = dispatch_dashboard_command(args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported command")
        );
    }

    #[test]
    fn serve_invalid_port_returns_error() {
        let args = vec!["serve".to_string(), "--port=abc".to_string()].into_iter();
        let result = dispatch_dashboard_command(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid --port"));
    }

    #[test]
    fn serve_unexpected_arg_returns_error() {
        let args = vec!["serve".to_string(), "--foo".to_string()].into_iter();
        let result = dispatch_dashboard_command(args);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unexpected argument")
        );
    }
}
