mod artifact;
mod context;
mod types;

#[cfg(test)]
mod tests;

// Re-export all public items so `crate::terminal_engineer_bridge::X` still works.
pub use artifact::{
    compatibility_handoff_path, load_runtime_handoff_snapshot, persist_handoff_artifacts,
    scoped_handoff_path, select_handoff_artifact_for_read, select_optional_handoff_artifact,
};
pub use types::{
    ScopedHandoffMode, SelectedHandoffArtifact, TerminalBridgeContext,
    COMPATIBILITY_HANDOFF_FILE_NAME, ENGINEER_HANDOFF_FILE_NAME, ENGINEER_MODE_BOUNDARY,
    SHARED_DEFAULT_STATE_ROOT_SOURCE, SHARED_EXPLICIT_STATE_ROOT_SOURCE,
    TERMINAL_HANDOFF_FILE_NAME, TERMINAL_MODE_BOUNDARY,
};
