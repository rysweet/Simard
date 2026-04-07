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
