use std::path::PathBuf;

use super::*;

// --- Display: MissingRequiredConfig ---

#[test]
fn display_missing_required_config() {
    let err = SimardError::MissingRequiredConfig {
        key: "API_KEY".to_string(),
        help: "set ANTHROPIC_API_KEY".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("API_KEY"), "should contain key: {msg}");
    assert!(
        msg.contains("set ANTHROPIC_API_KEY"),
        "should contain help: {msg}"
    );
}

// --- Display: NonUnicodeConfigValue ---

#[test]
fn display_non_unicode_config_value() {
    let err = SimardError::NonUnicodeConfigValue {
        key: "PATH".to_string(),
    };
    assert!(err.to_string().contains("PATH"));
    assert!(err.to_string().contains("UTF-8"));
}

// --- Display: InvalidConfigValue ---

#[test]
fn display_invalid_config_value() {
    let err = SimardError::InvalidConfigValue {
        key: "TOPOLOGY".to_string(),
        value: "bad".to_string(),
        help: "expected single-process".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("TOPOLOGY"), "should contain key: {msg}");
    assert!(
        msg.contains("expected single-process"),
        "should contain help: {msg}"
    );
}

// --- Display: UnknownIdentity ---

#[test]
fn display_unknown_identity() {
    let err = SimardError::UnknownIdentity {
        requested: "nonexistent".to_string(),
    };
    assert!(err.to_string().contains("nonexistent"));
    assert!(err.to_string().contains("not registered"));
}

// --- Display: InvalidIdentityComposition ---

#[test]
fn display_invalid_identity_composition() {
    let err = SimardError::InvalidIdentityComposition {
        identity: "bad-identity".to_string(),
        reason: "missing field".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("bad-identity"), "{msg}");
    assert!(msg.contains("invalid composition"), "{msg}");
    assert!(msg.contains("missing field"), "{msg}");
}

// --- Display: InvalidManifestContract ---

#[test]
fn display_invalid_manifest_contract() {
    let err = SimardError::InvalidManifestContract {
        field: "version".to_string(),
        reason: "must be semver".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("manifest contract"), "{msg}");
    assert!(msg.contains("version"), "{msg}");
    assert!(msg.contains("must be semver"), "{msg}");
}

// --- Display: InvalidGoalRecord ---

#[test]
fn display_invalid_goal_record() {
    let err = SimardError::InvalidGoalRecord {
        field: "title".to_string(),
        reason: "empty".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("goal record"), "{msg}");
    assert!(msg.contains("title"), "{msg}");
}

// --- Display: InvalidResearchRecord ---

#[test]
fn display_invalid_research_record() {
    let err = SimardError::InvalidResearchRecord {
        field: "url".to_string(),
        reason: "invalid".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("research record"), "{msg}");
}

// --- Display: InvalidMeetingRecord ---

#[test]
fn display_invalid_meeting_record() {
    let err = SimardError::InvalidMeetingRecord {
        field: "agenda".to_string(),
        reason: "missing".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("meeting record"), "{msg}");
    assert!(msg.contains("agenda"), "{msg}");
}

// --- Display: InvalidImprovementRecord ---

#[test]
fn display_invalid_improvement_record() {
    let err = SimardError::InvalidImprovementRecord {
        field: "status".to_string(),
        reason: "unknown".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("improvement record"), "{msg}");
}

// --- Display: InvalidSessionId ---

#[test]
fn display_invalid_session_id() {
    let err = SimardError::InvalidSessionId {
        value: "bad-id".to_string(),
        reason: "not a uuid".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("bad-id"), "{msg}");
    assert!(msg.contains("not a uuid"), "{msg}");
}

// --- Display: PromptAssetMissing ---

#[test]
fn display_prompt_asset_missing() {
    let err = SimardError::PromptAssetMissing {
        asset_id: "system-prompt".to_string(),
        path: PathBuf::from("/prompts/system.md"),
    };
    let msg = err.to_string();
    assert!(msg.contains("system-prompt"), "{msg}");
    assert!(msg.contains("prompt root"), "{msg}");
}

// --- Display: PromptAssetRead ---

#[test]
fn display_prompt_asset_read() {
    let err = SimardError::PromptAssetRead {
        path: PathBuf::from("/prompts/system.md"),
        reason: "permission denied".to_string(),
    };
    assert!(err.to_string().contains("permission denied"));
}

// --- Display: InvalidPromptAssetPath ---

#[test]
fn display_invalid_prompt_asset_path() {
    let err = SimardError::InvalidPromptAssetPath {
        asset_id: "meeting".to_string(),
        path: PathBuf::from("/bad/path"),
        reason: "traversal".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("meeting"), "{msg}");
    assert!(msg.contains("traversal"), "{msg}");
}

// --- Display: UnsupportedMemoryPolicy ---

#[test]
fn display_unsupported_memory_policy() {
    let err = SimardError::UnsupportedMemoryPolicy {
        field: "retention".to_string(),
        reason: "not implemented".to_string(),
    };
    assert!(err.to_string().contains("retention"));
}

// --- Display: UnsupportedBaseType ---

#[test]
fn display_unsupported_base_type() {
    let err = SimardError::UnsupportedBaseType {
        identity: "simard-engineer".to_string(),
        base_type: "unsupported-type".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("simard-engineer"), "{msg}");
    assert!(msg.contains("unsupported-type"), "{msg}");
}

// --- Display: AdapterNotRegistered ---

#[test]
fn display_adapter_not_registered() {
    let err = SimardError::AdapterNotRegistered {
        base_type: "custom-adapter".to_string(),
    };
    assert!(err.to_string().contains("custom-adapter"));
    assert!(err.to_string().contains("no adapter"));
}

// --- Display: AdapterInvocationFailed ---

#[test]
fn display_adapter_invocation_failed() {
    let err = SimardError::AdapterInvocationFailed {
        base_type: "terminal-shell".to_string(),
        reason: "timeout".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("terminal-shell"), "{msg}");
    assert!(msg.contains("timeout"), "{msg}");
}

// --- Display: BaseTypeSessionCleanupFailed ---

#[test]
fn display_base_type_session_cleanup_failed() {
    let err = SimardError::BaseTypeSessionCleanupFailed {
        base_type: "terminal-shell".to_string(),
        action: "close".to_string(),
        reason: "io error".to_string(),
        cleanup_reason: "lock held".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("terminal-shell"), "{msg}");
    assert!(msg.contains("close"), "{msg}");
    assert!(msg.contains("io error"), "{msg}");
    assert!(msg.contains("lock held"), "{msg}");
}

// --- Display: InvalidBaseTypeSessionState ---

#[test]
fn display_invalid_base_type_session_state() {
    let err = SimardError::InvalidBaseTypeSessionState {
        base_type: "terminal-shell".to_string(),
        action: "execute".to_string(),
        reason: "session closed".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("terminal-shell"), "{msg}");
    assert!(msg.contains("execute"), "{msg}");
}

// --- Display: MissingCapability ---

#[test]
fn display_missing_capability() {
    let err = SimardError::MissingCapability {
        base_type: "local-harness".to_string(),
        capability: BaseTypeCapability::TerminalSession,
    };
    let msg = err.to_string();
    assert!(msg.contains("local-harness"), "{msg}");
    assert!(msg.contains("terminal-session"), "{msg}");
}

// --- Display: UnsupportedTopology ---

#[test]
fn display_unsupported_topology() {
    let err = SimardError::UnsupportedTopology {
        base_type: "local-harness".to_string(),
        topology: RuntimeTopology::Distributed,
    };
    let msg = err.to_string();
    assert!(msg.contains("local-harness"), "{msg}");
    assert!(msg.contains("distributed"), "{msg}");
}

// --- Display: UnsupportedRuntimeTopology ---

#[test]
fn display_unsupported_runtime_topology() {
    let err = SimardError::UnsupportedRuntimeTopology {
        topology: RuntimeTopology::MultiProcess,
        driver: "basic-driver".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("basic-driver"), "{msg}");
    assert!(msg.contains("multi-process"), "{msg}");
}

// --- Display: InvalidRuntimeTransition ---

#[test]
fn display_invalid_runtime_transition() {
    let err = SimardError::InvalidRuntimeTransition {
        from: RuntimeState::Stopped,
        to: RuntimeState::Active,
    };
    let msg = err.to_string();
    assert!(msg.contains("stopped"), "{msg}");
    assert!(msg.contains("active"), "{msg}");
}

// --- Display: RuntimeStopped ---

#[test]
fn display_runtime_stopped() {
    let err = SimardError::RuntimeStopped {
        action: "execute".to_string(),
    };
    assert!(err.to_string().contains("execute"));
    assert!(err.to_string().contains("stopped"));
}

// --- Display: RuntimeFailed ---

#[test]
fn display_runtime_failed() {
    let err = SimardError::RuntimeFailed {
        action: "persist".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("persist"), "{msg}");
    assert!(msg.contains("failed"), "{msg}");
}

// --- Display: InvalidSessionTransition ---

#[test]
fn display_invalid_session_transition() {
    let err = SimardError::InvalidSessionTransition {
        from: SessionPhase::Complete,
        to: SessionPhase::Execution,
    };
    let msg = err.to_string();
    assert!(msg.contains("complete"), "{msg}");
    assert!(msg.contains("execution"), "{msg}");
}

// --- Display: InvalidHandoffSnapshot ---

#[test]
fn display_invalid_handoff_snapshot() {
    let err = SimardError::InvalidHandoffSnapshot {
        field: "session".to_string(),
        reason: "missing".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("session"), "{msg}");
    assert!(msg.contains("missing"), "{msg}");
}

// --- Display: NotARepo ---

#[test]
fn display_not_a_repo() {
    let err = SimardError::NotARepo {
        path: PathBuf::from("/home/user/project"),
        reason: "no .git directory".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("NOT_A_REPO"), "{msg}");
    assert!(msg.contains("/home/user/project"), "{msg}");
    assert!(msg.contains("no .git directory"), "{msg}");
}
