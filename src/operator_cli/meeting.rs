use chrono::Local;

use crate::meeting_facilitator::{default_handoff_dir, load_session_wip, remove_session_wip};
use crate::operator_commands::{run_meeting_probe, run_meeting_read_probe};
use crate::operator_commands_meeting::run_meeting_repl_command;

use super::args::{next_optional_path, next_required, reject_extra_args};

pub(super) const MEETING_HELP: &str = "\
Simard meeting subcommand

Usage: simard meeting <command> [args]

Commands:
  run <base-type> <topology> <objective> [state-root]
                            Run an automated meeting probe and exit.
  read <base-type> <topology> <state-root>
                            Read the latest meeting transcript and exit.
  repl [topic]              Start an interactive meeting REPL on stdin.
  begin [topic]             Alias for `repl`.
  start [topic]             Alias for `repl`.
  resume                    Resume an interrupted meeting from the last WIP checkpoint.
  resume --discard          Discard the saved WIP checkpoint without resuming.
  help, -h, --help          Show this help message and exit.

If no command is given, an interactive REPL is started with a timestamp topic.

Examples:
  simard meeting --help
  simard meeting repl \"weekly sync\"
  simard meeting resume
  simard meeting resume --discard
  simard meeting run local-harness single-process \"design review\"
  simard meeting read local-harness single-process /path/to/state-root
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
        "resume" => {
            let first_arg = args.next();
            match first_arg.as_deref() {
                Some("--discard") => {
                    reject_extra_args(args)?;
                    dispatch_resume_discard()
                }
                None => {
                    dispatch_resume()
                }
                Some(other) => Err(format!(
                    "unknown flag '{other}' for `simard meeting resume` (expected --discard or no args)"
                ).into()),
            }
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

/// Resume a meeting from the last WIP checkpoint. Loads the saved session,
/// prints a summary of recovered state, and re-enters the REPL with the
/// original topic. Issue #1984.
fn dispatch_resume() -> Result<(), Box<dyn std::error::Error>> {
    let dir = default_handoff_dir();
    let session =
        load_session_wip(&dir).map_err(|e| format!("failed to read WIP checkpoint: {e}"))?;

    let Some(session) = session else {
        return Err("no WIP checkpoint found — nothing to resume (start a new meeting with `simard meeting repl`)".into());
    };

    let n_decisions = session.decisions.len();
    let n_actions = session.action_items.len();
    let n_questions = session.explicit_questions.len();
    eprintln!(
        "Resuming meeting \"{}\" (started {}, {} decision(s), {} action(s), {} question(s))",
        session.topic, session.started_at, n_decisions, n_actions, n_questions
    );

    // Remove the WIP file before re-entering the REPL — the REPL's own
    // checkpoint_wip calls will recreate it on the next slash command.
    if let Err(e) = remove_session_wip(&dir) {
        tracing::warn!(error = %e, "failed to remove stale WIP file before resume");
    }

    run_meeting_repl_command(&session.topic)
}

/// Discard the saved WIP checkpoint without resuming the meeting.
/// Issue #1984.
fn dispatch_resume_discard() -> Result<(), Box<dyn std::error::Error>> {
    let dir = default_handoff_dir();

    // Check if a WIP file exists at all.
    let session =
        load_session_wip(&dir).map_err(|e| format!("failed to read WIP checkpoint: {e}"))?;

    if session.is_none() {
        println!("No WIP checkpoint found — nothing to discard.");
        return Ok(());
    }

    remove_session_wip(&dir).map_err(|e| format!("failed to remove WIP checkpoint: {e}"))?;

    println!("WIP checkpoint discarded.");
    Ok(())
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
    fn test_meeting_help_uses_registered_base_type_and_topology() {
        // Regression for issue #1907: `simard meeting --help` previously advertised
        // example commands using `gpt-5` as the base type and `ring` as the topology.
        // Neither value is registered with the runtime base-type/topology registry
        // (see Specs/ProductArchitecture.md §"Agent Base Type"), so copy-pasting the
        // example into adjacent subcommands such as `simard goal-curation read`
        // produced "no adapter is registered for base type 'gpt-5'" and
        // "invalid value for configuration 'SIMARD_RUNTIME_TOPOLOGY'" errors.
        //
        // The help text MUST advertise only registry-valid identifiers so the
        // examples remain runnable across every operator subcommand. We assert on
        // `local-harness` / `single-process` because they are the canonical
        // registered builtin pair already used throughout docs/reference/simard-cli.md.
        //
        // The forbidden-substring checks use space-padded " ring " specifically to
        // avoid false positives on legitimate substrings like "string" or "differing".
        let help = super::MEETING_HELP;
        assert!(
            !help.contains("gpt-5"),
            "MEETING_HELP must not advertise unregistered base type 'gpt-5' (issue #1907); \
             use a registry-valid identifier such as 'local-harness'"
        );
        assert!(
            !help.contains(" ring "),
            "MEETING_HELP must not advertise unregistered topology 'ring' (issue #1907); \
             use a registry-valid topology such as 'single-process'"
        );
        assert!(
            help.contains("local-harness"),
            "MEETING_HELP must advertise the registered builtin base type 'local-harness' \
             so the example is runnable across operator subcommands (issue #1907)"
        );
        assert!(
            help.contains("single-process"),
            "MEETING_HELP must advertise the registered topology 'single-process' \
             so the example is runnable across operator subcommands (issue #1907)"
        );
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

    // ── issue #1984: resume subcommand ──

    #[test]
    fn test_meeting_help_mentions_resume() {
        let help = super::MEETING_HELP;
        assert!(
            help.contains("resume"),
            "MEETING_HELP must mention the resume subcommand (issue #1984)"
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_meeting_resume_no_wip_errors() {
        let dir = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };
        let result = dispatch_operator_cli(vec!["meeting".to_string(), "resume".to_string()]);
        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
        assert!(result.is_err(), "resume with no WIP file should error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("no WIP checkpoint found"),
            "error should mention no WIP found, got: {msg}"
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_meeting_resume_discard_no_wip_ok() {
        let dir = tempfile::tempdir().expect("temp dir");
        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };
        let result = dispatch_operator_cli(vec![
            "meeting".to_string(),
            "resume".to_string(),
            "--discard".to_string(),
        ]);
        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
        assert!(
            result.is_ok(),
            "resume --discard with no WIP file should succeed, got: {result:?}"
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_meeting_resume_discard_removes_wip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let wip_path = dir.path().join("meeting_session_wip.json");
        let session = crate::meeting_facilitator::MeetingSession {
            topic: "test-discard".to_string(),
            decisions: Vec::new(),
            action_items: Vec::new(),
            notes: Vec::new(),
            status: crate::meeting_facilitator::MeetingSessionStatus::Open,
            started_at: "2025-01-01T00:00:00Z".to_string(),
            participants: vec!["operator".to_string()],
            explicit_questions: Vec::new(),
            themes: Vec::new(),
            next_owner: None,
            goal: None,
        };
        crate::meeting_facilitator::save_session_wip(dir.path(), &session).expect("save WIP");
        assert!(wip_path.is_file(), "WIP file should exist before discard");

        unsafe { std::env::set_var("SIMARD_HANDOFF_DIR", dir.path()) };
        let result = dispatch_operator_cli(vec![
            "meeting".to_string(),
            "resume".to_string(),
            "--discard".to_string(),
        ]);
        unsafe { std::env::remove_var("SIMARD_HANDOFF_DIR") };
        assert!(
            result.is_ok(),
            "resume --discard should succeed, got: {result:?}"
        );
        assert!(
            !wip_path.is_file(),
            "WIP file should be removed after discard"
        );
    }

    #[test]
    fn test_meeting_resume_unknown_flag_errors() {
        let result = dispatch_operator_cli(vec![
            "meeting".to_string(),
            "resume".to_string(),
            "--bogus".to_string(),
        ]);
        assert!(result.is_err(), "resume --bogus should error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("--bogus"),
            "error should name the offending flag, got: {msg}"
        );
    }
}
