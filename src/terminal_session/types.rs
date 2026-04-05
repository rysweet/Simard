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
