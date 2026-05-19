use chrono::Local;

use crate::operator_commands::{run_meeting_probe, run_meeting_read_probe};
use crate::operator_commands_meeting::run_meeting_repl_command;

use super::args::{next_optional_path, next_required, reject_extra_args};

pub(super) const MEETING_HELP: &str = "\
Simard meeting subcommand

Usage: simard meeting <command> [args]

Commands:
  run <base-type> <topology> <objective> [state-root]
                            Run an automated meeting probe and exit.
  read <base-type> <topology> [state-root]
                            Read the latest meeting transcript and exit.
  repl [topic]              Start an interactive meeting REPL on stdin.
  begin [topic]             Alias for `repl`.
  start [topic]             Alias for `repl`.
  help, -h, --help          Show this help message and exit.

If no command is given, an interactive REPL is started with a timestamp topic.

Examples:
  simard meeting --help
  simard meeting repl \"weekly sync\"
  simard meeting run local-harness ring \"design review\"
  simard meeting read local-harness ring
";

pub(super) fn dispatch_meeting_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = args.next().unwrap_or_else(|| "repl".to_string());
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{MEETING_HELP}");
            Ok(())
        }
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
        // Reject unknown flag-shaped tokens visibly instead of silently
        // treating them as a meeting topic and blocking the REPL on stdin.
        // See issue #1746 (Pillar 11: honest degradation beats hidden silence).
        flag if flag.starts_with('-') => Err(format!(
            "unknown flag '{flag}' for `simard meeting` (try `simard meeting --help`)"
        )
        .into()),
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

    // ── issue #1746: --help must short-circuit before the REPL ──

    #[test]
    fn test_meeting_double_dash_help_exits_ok() {
        // Regression for issue #1746: `simard meeting --help` previously
        // treated `--help` as a topic name and entered an interactive REPL
        // that blocked on stdin forever. It MUST now exit Ok without doing
        // any I/O on stdin.
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "--help".to_string()]);
        assert!(
            result.is_ok(),
            "meeting --help must exit Ok, got: {result:?}"
        );
    }

    #[test]
    fn test_meeting_short_dash_help_exits_ok() {
        // Regression for issue #1746: `simard meeting -h` must short-circuit
        // the same way `--help` does.
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "-h".to_string()]);
        assert!(result.is_ok(), "meeting -h must exit Ok, got: {result:?}");
    }

    #[test]
    fn test_meeting_help_word_exits_ok() {
        // `simard meeting help` must also be honoured as a help request,
        // not as a meeting topic literally named "help".
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "help".to_string()]);
        assert!(result.is_ok(), "meeting help must exit Ok, got: {result:?}");
    }

    #[test]
    fn test_meeting_help_text_lists_subcommands() {
        // The help text constant must enumerate the supported subcommands
        // so users can discover the meeting surface.
        for keyword in &["run", "read", "repl", "begin", "start"] {
            assert!(
                super::MEETING_HELP.contains(keyword),
                "MEETING_HELP must mention '{keyword}'"
            );
        }
    }

    #[test]
    fn test_meeting_unknown_long_flag_errors() {
        // Unknown long flags must fail visibly to stderr (Pillar 11) instead
        // of being silently accepted as a meeting topic name.
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "--bogus".to_string()]);
        assert!(result.is_err(), "unknown --flag must error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unknown flag") && msg.contains("--bogus"),
            "error must name the offending flag, got: {msg}"
        );
    }

    #[test]
    fn test_meeting_unknown_short_flag_errors() {
        // Unknown short flags must also error rather than start a REPL.
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "-x".to_string()]);
        assert!(result.is_err(), "unknown -x flag must error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("unknown flag") && msg.contains("-x"),
            "error must name the offending flag, got: {msg}"
        );
    }
}
