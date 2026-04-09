use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

pub(crate) const DEFAULT_SHELL: &str = "/usr/bin/bash";
pub(crate) const PTY_LAUNCHER: &str = "script";
pub(crate) const WAIT_STEP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TerminalStep {
    Input(String),
    WaitFor(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalTurnSpec {
    pub shell: String,
    pub working_directory: Option<PathBuf>,
    pub wait_timeout: Duration,
    pub steps: Vec<TerminalStep>,
}

#[derive(Clone, Debug)]
pub(crate) enum TerminalWaitStatus {
    Satisfied,
    ExitedEarly(ExitStatus),
    TimedOut,
}

#[derive(Debug)]
pub(crate) struct TerminalSessionCapture {
    pub transcript: String,
    pub exit_status: ExitStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_constant() {
        assert_eq!(DEFAULT_SHELL, "/usr/bin/bash");
    }

    #[test]
    fn pty_launcher_constant() {
        assert_eq!(PTY_LAUNCHER, "script");
    }

    #[test]
    fn wait_step_timeout_is_five_seconds() {
        assert_eq!(WAIT_STEP_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn terminal_step_input_equality() {
        let a = TerminalStep::Input("ls".to_string());
        let b = TerminalStep::Input("ls".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn terminal_step_wait_for_equality() {
        let a = TerminalStep::WaitFor("$".to_string());
        let b = TerminalStep::WaitFor("$".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn terminal_turn_spec_construction() {
        let spec = TerminalTurnSpec {
            shell: DEFAULT_SHELL.to_string(),
            working_directory: Some(PathBuf::from("/home")),
            wait_timeout: WAIT_STEP_TIMEOUT,
            steps: vec![
                TerminalStep::Input("echo hello".to_string()),
                TerminalStep::WaitFor("hello".to_string()),
            ],
        };
        assert_eq!(spec.steps.len(), 2);
        assert_eq!(spec.wait_timeout, Duration::from_secs(5));
    }
}
