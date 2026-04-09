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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_handoff_mode_terminal_file_name() {
        assert_eq!(
            ScopedHandoffMode::Terminal.scoped_file_name(),
            TERMINAL_HANDOFF_FILE_NAME
        );
    }

    #[test]
    fn scoped_handoff_mode_engineer_file_name() {
        assert_eq!(
            ScopedHandoffMode::Engineer.scoped_file_name(),
            ENGINEER_HANDOFF_FILE_NAME
        );
    }

    #[test]
    fn constants_are_distinct() {
        assert_ne!(TERMINAL_HANDOFF_FILE_NAME, ENGINEER_HANDOFF_FILE_NAME);
        assert_ne!(TERMINAL_MODE_BOUNDARY, ENGINEER_MODE_BOUNDARY);
    }

    #[test]
    fn selected_handoff_artifact_construction() {
        let a = SelectedHandoffArtifact {
            path: PathBuf::from("/state/handoff.json"),
            file_name: COMPATIBILITY_HANDOFF_FILE_NAME,
        };
        assert_eq!(a.file_name, "latest_handoff.json");
    }

    #[test]
    fn terminal_bridge_context_construction() {
        let ctx = TerminalBridgeContext {
            continuity_source: "src".to_string(),
            handoff_file_name: "f.json".to_string(),
            working_directory: "/home".to_string(),
            command_count: "5".to_string(),
            wait_count: "2".to_string(),
            last_output_line: Some("done".to_string()),
        };
        assert_eq!(ctx.command_count, "5");
        assert_eq!(ctx.last_output_line, Some("done".to_string()));
    }
}
