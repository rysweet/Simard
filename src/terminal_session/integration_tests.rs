//! Integration tests for terminal recipe parsing and step validation.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::terminal_session::types::{TerminalStep, TerminalTurnSpec};

    // ── Parsing: simple recipes ──────────────────────────────────────

    #[test]
    fn parse_simple_echo_command() {
        let raw = "command: echo hello world";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.steps, vec![TerminalStep::Input("echo hello world".into())]);
        assert_eq!(spec.shell, "/usr/bin/bash");
        assert_eq!(spec.wait_timeout, Duration::from_secs(5));
        assert!(spec.working_directory.is_none());
    }

    #[test]
    fn parse_multi_step_recipe_with_wait() {
        let raw = "\
            command: echo start\n\
            wait-for: start\n\
            command: echo done\n\
            expect: done\n\
        ";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.input_count(), 2);
        assert_eq!(spec.wait_count(), 2);
        assert_eq!(
            spec.steps,
            vec![
                TerminalStep::Input("echo start".into()),
                TerminalStep::WaitFor("start".into()),
                TerminalStep::Input("echo done".into()),
                TerminalStep::WaitFor("done".into()),
            ]
        );
    }

    #[test]
    fn parse_recipe_with_all_metadata() {
        let raw = "\
            shell: /usr/bin/bash\n\
            cwd: /home/user/project\n\
            wait-timeout-seconds: 30\n\
            command: ls -la\n\
        ";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.shell, "/usr/bin/bash");
        assert_eq!(spec.working_directory, Some(PathBuf::from("/home/user/project")));
        assert_eq!(spec.wait_timeout, Duration::from_secs(30));
        assert_eq!(spec.steps, vec![TerminalStep::Input("ls -la".into())]);
    }

    // ── Label aliases ────────────────────────────────────────────────

    #[test]
    fn parse_recognizes_all_label_aliases() {
        let raw = "\
            working_directory: /a\n\
            wait_timeout_seconds: 10\n\
            input: cmd1\n\
            wait_for: ready\n\
        ";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.working_directory, Some(PathBuf::from("/a")));
        assert_eq!(spec.wait_timeout, Duration::from_secs(10));
        assert_eq!(spec.steps[0], TerminalStep::Input("cmd1".into()));
        assert_eq!(spec.steps[1], TerminalStep::WaitFor("ready".into()));
    }

    // ── Bare lines become Input steps ────────────────────────────────

    #[test]
    fn parse_bare_lines_become_input_steps() {
        let raw = "echo bare line without label";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.steps, vec![TerminalStep::Input("echo bare line without label".into())]);
    }

    #[test]
    fn parse_unknown_label_becomes_input() {
        let raw = "unknown-label: some value\ncommand: real";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.steps[0], TerminalStep::Input("unknown-label: some value".into()));
        assert_eq!(spec.steps[1], TerminalStep::Input("real".into()));
    }

    // ── Blank lines are ignored ──────────────────────────────────────

    #[test]
    fn parse_ignores_blank_lines() {
        let raw = "\n\ncommand: echo ok\n\n\n";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.steps, vec![TerminalStep::Input("echo ok".into())]);
    }

    // ── Error handling ───────────────────────────────────────────────

    #[test]
    fn parse_empty_input_is_error() {
        let err = TerminalTurnSpec::parse("", "test").unwrap_err();
        assert!(err.to_string().contains("at least one input"), "got: {err}");
    }

    #[test]
    fn parse_only_wait_steps_is_error() {
        let err = TerminalTurnSpec::parse("wait-for: something", "test").unwrap_err();
        assert!(err.to_string().contains("at least one input"), "got: {err}");
    }

    #[test]
    fn parse_empty_value_after_label_is_skipped() {
        // "shell:" with no value should not set shell; default used instead.
        let raw = "shell:\ncommand: echo ok";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();
        assert_eq!(spec.shell, "/usr/bin/bash");
    }

    #[test]
    fn parse_invalid_timeout_out_of_range() {
        let raw = "wait-timeout-seconds: 999\ncommand: echo ok";
        let err = TerminalTurnSpec::parse(raw, "test").unwrap_err();
        assert!(err.to_string().contains("between 1 and 60"), "got: {err}");
    }

    #[test]
    fn parse_invalid_timeout_zero() {
        let raw = "wait-timeout-seconds: 0\ncommand: echo ok";
        let err = TerminalTurnSpec::parse(raw, "test").unwrap_err();
        assert!(err.to_string().contains("between 1 and 60"), "got: {err}");
    }

    #[test]
    fn parse_invalid_timeout_not_a_number() {
        let raw = "wait-timeout-seconds: abc\ncommand: echo ok";
        let err = TerminalTurnSpec::parse(raw, "test").unwrap_err();
        assert!(err.to_string().contains("invalid"), "got: {err}");
    }

    #[test]
    fn parse_invalid_shell_relative_path() {
        let raw = "shell: bash\ncommand: echo ok";
        let err = TerminalTurnSpec::parse(raw, "test").unwrap_err();
        assert!(
            err.to_string().contains("safe path characters"),
            "got: {err}"
        );
    }

    // ── Simulated multi-step bash recipe (echo pipeline) ─────────────

    #[test]
    fn parse_realistic_echo_pipeline() {
        let raw = "\
            working-directory: /home/azureuser\n\
            wait-timeout: 10\n\
            command: echo step-1-init\n\
            wait-for: step-1-init\n\
            command: echo step-2-build\n\
            wait-for: step-2-build\n\
            command: echo step-3-verify\n\
            wait-for: step-3-verify\n\
            command: exit 0\n\
        ";
        let spec = TerminalTurnSpec::parse(raw, "test").unwrap();

        assert_eq!(spec.input_count(), 4);
        assert_eq!(spec.wait_count(), 3);
        assert_eq!(spec.working_directory, Some(PathBuf::from("/home/azureuser")));
        assert_eq!(spec.wait_timeout, Duration::from_secs(10));
    }
}
