use chrono::Local;

use crate::operator_commands::{run_meeting_probe, run_meeting_read_probe};
use crate::operator_commands_meeting::run_meeting_repl_command;

use super::args::{next_optional_path, next_required, reject_extra_args};

pub(super) fn dispatch_meeting_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = args.next().unwrap_or_else(|| "repl".to_string());
    match subcommand.as_str() {
        "run" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let objective = next_required(&mut args, "objective")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_probe(&base_type, &topology, &objective, state_root)
        }
        "read" => {
            let base_type = next_required(&mut args, "base type")?;
            let topology = next_required(&mut args, "topology")?;
            let state_root = next_optional_path(&mut args);
            reject_extra_args(args)?;
            run_meeting_read_probe(&base_type, &topology, state_root)
        }
        "repl" | "begin" | "start" => {
            let topic = args
                .next()
                .unwrap_or_else(|| Local::now().format("%Y-%m-%d:%H:%M").to_string());
            reject_extra_args(args)?;
            run_meeting_repl_command(&topic)
        }
        // Any other word is treated as a topic for a meeting repl
        topic => {
            let rest: Vec<String> = args.collect();
            let full_topic = if rest.is_empty() {
                topic.to_string()
            } else {
                format!("{topic} {}", rest.join(" "))
            };
            run_meeting_repl_command(&full_topic)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::operator_cli::dispatch_operator_cli;

    #[test]
    fn test_meeting_run_missing_base_type() {
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "run".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected base type")
        );
    }

    #[test]
    fn test_meeting_read_missing_base_type() {
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "read".to_string()]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected base type")
        );
    }

    #[test]
    fn test_meeting_run_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "meeting".to_string(),
            "run".to_string(),
            "base-type".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }

    #[test]
    fn test_meeting_run_missing_objective() {
        let result = dispatch_operator_cli(vec![
            "meeting".to_string(),
            "run".to_string(),
            "base-type".to_string(),
            "topology".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected objective")
        );
    }

    #[test]
    fn test_meeting_read_missing_topology() {
        let result = dispatch_operator_cli(vec![
            "meeting".to_string(),
            "read".to_string(),
            "base-type".to_string(),
        ]);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("expected topology")
        );
    }
}
