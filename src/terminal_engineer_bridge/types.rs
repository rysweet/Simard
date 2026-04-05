use std::path::PathBuf;

pub const COMPATIBILITY_HANDOFF_FILE_NAME: &str = "latest_handoff.json";
pub const TERMINAL_HANDOFF_FILE_NAME: &str = "latest_terminal_handoff.json";
pub const ENGINEER_HANDOFF_FILE_NAME: &str = "latest_engineer_handoff.json";
pub const TERMINAL_MODE_BOUNDARY: &str = "terminal is a bounded local terminal session surface";
pub const ENGINEER_MODE_BOUNDARY: &str = "engineer is a separate repo-grounded bounded loop";
pub const SHARED_EXPLICIT_STATE_ROOT_SOURCE: &str = "shared explicit state-root";
pub const SHARED_DEFAULT_STATE_ROOT_SOURCE: &str = "shared default state-root";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopedHandoffMode {
    Terminal,
    Engineer,
}

impl ScopedHandoffMode {
    pub fn scoped_file_name(self) -> &'static str {
        match self {
            Self::Terminal => TERMINAL_HANDOFF_FILE_NAME,
            Self::Engineer => ENGINEER_HANDOFF_FILE_NAME,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedHandoffArtifact {
    pub path: PathBuf,
    pub file_name: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalBridgeContext {
    pub continuity_source: String,
    pub handoff_file_name: String,
    pub working_directory: String,
    pub command_count: String,
    pub wait_count: String,
    pub last_output_line: Option<String>,
}
