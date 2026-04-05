use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

use crate::base_types::BaseTypeCapability;
use crate::runtime::{RuntimeState, RuntimeTopology};
use crate::session::SessionPhase;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SimardError {
    MissingRequiredConfig {
        key: String,
        help: String,
    },
    NonUnicodeConfigValue {
        key: String,
    },
    InvalidConfigValue {
        key: String,
        value: String,
        help: String,
    },
    UnknownIdentity {
        requested: String,
    },
    InvalidIdentityComposition {
        identity: String,
        reason: String,
    },
    InvalidManifestContract {
        field: String,
        reason: String,
    },
    InvalidGoalRecord {
        field: String,
        reason: String,
    },
    InvalidResearchRecord {
        field: String,
        reason: String,
    },
    InvalidMeetingRecord {
        field: String,
        reason: String,
    },
    InvalidImprovementRecord {
        field: String,
        reason: String,
    },
    InvalidSessionId {
        value: String,
        reason: String,
    },
    PromptAssetMissing {
        asset_id: String,
        path: PathBuf,
    },
    PromptAssetRead {
        path: PathBuf,
        reason: String,
    },
    InvalidPromptAssetPath {
        asset_id: String,
        path: PathBuf,
        reason: String,
    },
    UnsupportedMemoryPolicy {
        field: String,
        reason: String,
    },
    UnsupportedBaseType {
        identity: String,
        base_type: String,
    },
    AdapterNotRegistered {
        base_type: String,
    },
    AdapterInvocationFailed {
        base_type: String,
        reason: String,
    },
    BaseTypeSessionCleanupFailed {
        base_type: String,
        action: String,
        reason: String,
        cleanup_reason: String,
    },
    InvalidBaseTypeSessionState {
        base_type: String,
        action: String,
        reason: String,
    },
    MissingCapability {
        base_type: String,
        capability: BaseTypeCapability,
    },
    UnsupportedTopology {
        base_type: String,
        topology: RuntimeTopology,
    },
    UnsupportedRuntimeTopology {
        topology: RuntimeTopology,
        driver: String,
    },
    InvalidRuntimeTransition {
        from: RuntimeState,
        to: RuntimeState,
    },
    RuntimeStopped {
        action: String,
    },
    RuntimeFailed {
        action: String,
    },
    InvalidSessionTransition {
        from: SessionPhase,
        to: SessionPhase,
    },
    InvalidHandoffSnapshot {
        field: String,
        reason: String,
    },
    NotARepo {
        path: PathBuf,
        reason: String,
    },
    UnsupportedEngineerAction {
        reason: String,
    },
    ActionExecutionFailed {
        action: String,
        reason: String,
    },
    CommandTimeout {
        action: String,
        timeout_secs: u64,
    },
    VerificationFailed {
        reason: String,
    },
    InvalidStateRoot {
        path: PathBuf,
        reason: String,
    },
    PersistentStoreIo {
        store: String,
        action: String,
        path: PathBuf,
        reason: String,
    },
    BenchmarkScenarioNotFound {
        scenario_id: String,
    },
    BenchmarkSuiteNotFound {
        suite_id: String,
    },
    BenchmarkComparisonUnavailable {
        scenario_id: String,
        reason: String,
    },
    ArtifactIo {
        path: PathBuf,
        reason: String,
    },
    StoragePoisoned {
        store: String,
    },
    ClockBeforeUnixEpoch {
        reason: String,
    },
    BridgeSpawnFailed {
        bridge: String,
        reason: String,
    },
    BridgeTransportError {
        bridge: String,
        reason: String,
    },
    BridgeProtocolError {
        bridge: String,
        reason: String,
    },
    BridgeCallFailed {
        bridge: String,
        method: String,
        reason: String,
    },
    BridgeCircuitOpen {
        bridge: String,
    },
    BridgeError(String),
    PlanningUnavailable {
        reason: String,
    },
    ReviewUnavailable {
        reason: String,
    },
    ReviewBlocked {
        summary: String,
    },
}

pub type SimardResult<T> = Result<T, SimardError>;

impl Display for SimardError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredConfig { key, help } => {
                write!(f, "missing required configuration '{key}': {help}")
            }
            Self::NonUnicodeConfigValue { key } => {
                write!(f, "configuration '{key}' must be valid UTF-8")
            }
            Self::InvalidConfigValue { key, help, .. } => {
                write!(f, "invalid value for configuration '{key}': {help}")
            }
            Self::UnknownIdentity { requested } => {
                write!(f, "identity '{requested}' is not registered")
            }
            Self::InvalidIdentityComposition { identity, reason }
            | Self::InvalidManifestContract {
                field: identity,
                reason,
            }
            | Self::InvalidGoalRecord {
                field: identity,
                reason,
            }
            | Self::InvalidResearchRecord {
                field: identity,
                reason,
            }
            | Self::InvalidMeetingRecord {
                field: identity,
                reason,
            }
            | Self::InvalidImprovementRecord {
                field: identity,
                reason,
            } => fmt_field_reason(f, self, identity, reason),
            Self::InvalidSessionId { value, reason } => {
                write!(f, "invalid session id '{value}': {reason}")
            }
            Self::PromptAssetMissing { asset_id, .. } => {
                write!(
                    f,
                    "prompt asset '{asset_id}' was not found under the configured prompt root"
                )
            }
            Self::PromptAssetRead { reason, .. } => {
                write!(f, "failed to read configured prompt asset: {reason}")
            }
            Self::InvalidPromptAssetPath {
                asset_id, reason, ..
            } => {
                write!(f, "invalid prompt asset path for '{asset_id}': {reason}")
            }
            Self::UnsupportedMemoryPolicy { field, reason } => {
                write!(f, "unsupported memory policy '{field}': {reason}")
            }
            Self::UnsupportedBaseType {
                identity,
                base_type,
            } => {
                write!(
                    f,
                    "identity '{identity}' does not allow base type '{base_type}'"
                )
            }
            Self::AdapterNotRegistered { base_type } => {
                write!(f, "no adapter is registered for base type '{base_type}'")
            }
            Self::AdapterInvocationFailed { base_type, reason } => {
                write!(
                    f,
                    "base type '{base_type}' failed during invocation: {reason}"
                )
            }
            Self::BaseTypeSessionCleanupFailed {
                base_type,
                action,
                reason,
                cleanup_reason,
            } => {
                write!(
                    f,
                    "base type session '{base_type}' failed during '{action}': {reason}; cleanup failed: {cleanup_reason}"
                )
            }
            Self::InvalidBaseTypeSessionState {
                base_type,
                action,
                reason,
            } => {
                write!(
                    f,
                    "base type session '{base_type}' cannot '{action}': {reason}"
                )
            }
            Self::MissingCapability {
                base_type,
                capability,
            } => {
                write!(
                    f,
                    "base type '{base_type}' does not provide required capability '{capability}'"
                )
            }
            Self::UnsupportedTopology {
                base_type,
                topology,
            } => {
                write!(
                    f,
                    "base type '{base_type}' does not support topology '{topology}'"
                )
            }
            Self::UnsupportedRuntimeTopology { topology, driver } => {
                write!(
                    f,
                    "runtime topology driver '{driver}' does not support topology '{topology}'"
                )
            }
            Self::InvalidRuntimeTransition { from, to } => {
                write!(f, "invalid runtime transition from '{from}' to '{to}'")
            }
            Self::RuntimeStopped { action } => {
                write!(f, "runtime is stopped and cannot '{action}'")
            }
            Self::RuntimeFailed { action } => {
                write!(
                    f,
                    "runtime is failed and cannot '{action}' until it is stopped"
                )
            }
            Self::InvalidSessionTransition { from, to } => {
                write!(f, "invalid session transition from '{from}' to '{to}'")
            }
            Self::InvalidHandoffSnapshot { field, reason } => {
                write!(f, "invalid handoff snapshot field '{field}': {reason}")
            }
            Self::NotARepo { path, reason } => {
                write!(
                    f,
                    "NOT_A_REPO: '{}' is not inside a valid git worktree: {reason}",
                    path.display()
                )
            }
            Self::UnsupportedEngineerAction { reason } => {
                write!(
                    f,
                    "no supported local engineer action is available: {reason}"
                )
            }
            Self::ActionExecutionFailed { action, reason } => {
                write!(f, "engineer action '{action}' failed: {reason}")
            }
            Self::CommandTimeout {
                action,
                timeout_secs,
            } => {
                write!(
                    f,
                    "engineer action '{action}' timed out after {timeout_secs}s"
                )
            }
            Self::VerificationFailed { reason } => {
                write!(f, "engineer loop verification failed: {reason}")
            }
            Self::InvalidStateRoot { path, reason } => {
                write!(f, "invalid state root '{}': {reason}", path.display())
            }
            Self::PersistentStoreIo {
                store,
                action,
                reason,
                ..
            } => {
                write!(
                    f,
                    "persistent store '{store}' failed during '{action}': {reason}"
                )
            }
            Self::BenchmarkScenarioNotFound { scenario_id } => {
                write!(f, "benchmark scenario '{scenario_id}' is not registered")
            }
            Self::BenchmarkSuiteNotFound { suite_id } => {
                write!(f, "benchmark suite '{suite_id}' is not registered")
            }
            Self::BenchmarkComparisonUnavailable {
                scenario_id,
                reason,
            } => {
                write!(
                    f,
                    "benchmark comparison for scenario '{scenario_id}' is unavailable: {reason}"
                )
            }
            Self::ArtifactIo { reason, .. } => {
                write!(f, "failed to read or write benchmark artifact: {reason}")
            }
            Self::StoragePoisoned { store } => {
                write!(f, "storage lock for '{store}' is poisoned")
            }
            Self::ClockBeforeUnixEpoch { reason } => {
                write!(f, "system clock is before UNIX epoch: {reason}")
            }
            Self::BridgeSpawnFailed { bridge, reason } => {
                write!(f, "bridge '{bridge}' failed to spawn: {reason}")
            }
            Self::BridgeTransportError { bridge, reason } => {
                write!(f, "bridge '{bridge}' transport error: {reason}")
            }
            Self::BridgeProtocolError { bridge, reason } => {
                write!(f, "bridge '{bridge}' protocol error: {reason}")
            }
            Self::BridgeCallFailed {
                bridge,
                method,
                reason,
            } => {
                write!(f, "bridge '{bridge}' call to '{method}' failed: {reason}")
            }
            Self::BridgeCircuitOpen { bridge } => {
                write!(
                    f,
                    "bridge '{bridge}' circuit is open — calls are rejected until the bridge recovers"
                )
            }
            Self::BridgeError(msg) => {
                write!(f, "bridge error: {msg}")
            }
            Self::PlanningUnavailable { reason } => {
                write!(f, "LLM-based planning is unavailable: {reason}")
            }
            Self::ReviewUnavailable { reason } => {
                write!(f, "LLM-based review is unavailable: {reason}")
            }
            Self::ReviewBlocked { summary } => {
                write!(f, "review blocked commit: {summary}")
            }
        }
    }
}

fn fmt_field_reason(
    f: &mut Formatter<'_>,
    variant: &SimardError,
    field: &str,
    reason: &str,
) -> fmt::Result {
    let prefix = match variant {
        SimardError::InvalidIdentityComposition { .. } => "identity",
        SimardError::InvalidManifestContract { .. } => "invalid manifest contract field",
        SimardError::InvalidGoalRecord { .. } => "invalid goal record field",
        SimardError::InvalidResearchRecord { .. } => "invalid research record field",
        SimardError::InvalidMeetingRecord { .. } => "invalid meeting record field",
        SimardError::InvalidImprovementRecord { .. } => "invalid improvement record field",
        _ => unreachable!(),
    };
    if matches!(variant, SimardError::InvalidIdentityComposition { .. }) {
        write!(f, "{prefix} '{field}' has invalid composition: {reason}")
    } else {
        write!(f, "{prefix} '{field}': {reason}")
    }
}

impl Error for SimardError {}

#[cfg(test)]
mod tests {
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

    // --- Display: UnsupportedEngineerAction ---

    #[test]
    fn display_unsupported_engineer_action() {
        let err = SimardError::UnsupportedEngineerAction {
            reason: "no tooling".to_string(),
        };
        assert!(err.to_string().contains("no tooling"));
    }

    // --- Display: ActionExecutionFailed ---

    #[test]
    fn display_action_execution_failed() {
        let err = SimardError::ActionExecutionFailed {
            action: "cargo test".to_string(),
            reason: "exit code 1".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("cargo test"), "{msg}");
        assert!(msg.contains("exit code 1"), "{msg}");
    }

    // --- Display: CommandTimeout ---

    #[test]
    fn display_command_timeout() {
        let err = SimardError::CommandTimeout {
            action: "build".to_string(),
            timeout_secs: 120,
        };
        let msg = err.to_string();
        assert!(msg.contains("build"), "{msg}");
        assert!(msg.contains("120"), "{msg}");
    }

    // --- Display: VerificationFailed ---

    #[test]
    fn display_verification_failed() {
        let err = SimardError::VerificationFailed {
            reason: "tests did not pass".to_string(),
        };
        assert!(err.to_string().contains("tests did not pass"));
    }

    // --- Display: InvalidStateRoot ---

    #[test]
    fn display_invalid_state_root() {
        let err = SimardError::InvalidStateRoot {
            path: PathBuf::from("/bad/root"),
            reason: "not writable".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("/bad/root"), "{msg}");
        assert!(msg.contains("not writable"), "{msg}");
    }

    // --- Display: PersistentStoreIo ---

    #[test]
    fn display_persistent_store_io() {
        let err = SimardError::PersistentStoreIo {
            store: "memory".to_string(),
            action: "write".to_string(),
            path: PathBuf::from("/store/memory.json"),
            reason: "disk full".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("memory"), "{msg}");
        assert!(msg.contains("write"), "{msg}");
        assert!(msg.contains("disk full"), "{msg}");
    }

    // --- Display: BenchmarkScenarioNotFound ---

    #[test]
    fn display_benchmark_scenario_not_found() {
        let err = SimardError::BenchmarkScenarioNotFound {
            scenario_id: "missing-scenario".to_string(),
        };
        assert!(err.to_string().contains("missing-scenario"));
    }

    // --- Display: BenchmarkSuiteNotFound ---

    #[test]
    fn display_benchmark_suite_not_found() {
        let err = SimardError::BenchmarkSuiteNotFound {
            suite_id: "missing-suite".to_string(),
        };
        assert!(err.to_string().contains("missing-suite"));
    }

    // --- Display: BenchmarkComparisonUnavailable ---

    #[test]
    fn display_benchmark_comparison_unavailable() {
        let err = SimardError::BenchmarkComparisonUnavailable {
            scenario_id: "test-scenario".to_string(),
            reason: "not enough runs".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("test-scenario"), "{msg}");
        assert!(msg.contains("not enough runs"), "{msg}");
    }

    // --- Display: ArtifactIo ---

    #[test]
    fn display_artifact_io() {
        let err = SimardError::ArtifactIo {
            path: PathBuf::from("/artifacts/report.json"),
            reason: "corrupt json".to_string(),
        };
        assert!(err.to_string().contains("corrupt json"));
    }

    // --- Display: StoragePoisoned ---

    #[test]
    fn display_storage_poisoned() {
        let err = SimardError::StoragePoisoned {
            store: "evidence".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("evidence"), "{msg}");
        assert!(msg.contains("poisoned"), "{msg}");
    }

    // --- Display: ClockBeforeUnixEpoch ---

    #[test]
    fn display_clock_before_unix_epoch() {
        let err = SimardError::ClockBeforeUnixEpoch {
            reason: "system time error".to_string(),
        };
        assert!(err.to_string().contains("UNIX epoch"));
    }

    // --- Display: Bridge errors ---

    #[test]
    fn display_bridge_spawn_failed() {
        let err = SimardError::BridgeSpawnFailed {
            bridge: "memory".to_string(),
            reason: "python not found".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("memory"), "{msg}");
        assert!(msg.contains("python not found"), "{msg}");
    }

    #[test]
    fn display_bridge_transport_error() {
        let err = SimardError::BridgeTransportError {
            bridge: "knowledge".to_string(),
            reason: "connection refused".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("knowledge"), "{msg}");
        assert!(msg.contains("transport error"), "{msg}");
    }

    #[test]
    fn display_bridge_protocol_error() {
        let err = SimardError::BridgeProtocolError {
            bridge: "gym".to_string(),
            reason: "invalid json".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("gym"), "{msg}");
        assert!(msg.contains("protocol error"), "{msg}");
    }

    #[test]
    fn display_bridge_call_failed() {
        let err = SimardError::BridgeCallFailed {
            bridge: "memory".to_string(),
            method: "store_episode".to_string(),
            reason: "timeout".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("memory"), "{msg}");
        assert!(msg.contains("store_episode"), "{msg}");
        assert!(msg.contains("timeout"), "{msg}");
    }

    #[test]
    fn display_bridge_circuit_open() {
        let err = SimardError::BridgeCircuitOpen {
            bridge: "memory".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("memory"), "{msg}");
        assert!(msg.contains("circuit"), "{msg}");
    }

    // --- Display: Planning / Review ---

    #[test]
    fn display_planning_unavailable() {
        let err = SimardError::PlanningUnavailable {
            reason: "no API key".to_string(),
        };
        assert!(err.to_string().contains("no API key"));
    }

    #[test]
    fn display_review_unavailable() {
        let err = SimardError::ReviewUnavailable {
            reason: "session closed".to_string(),
        };
        assert!(err.to_string().contains("session closed"));
    }

    #[test]
    fn display_review_blocked() {
        let err = SimardError::ReviewBlocked {
            summary: "security issue found".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("review blocked"), "{msg}");
        assert!(msg.contains("security issue found"), "{msg}");
    }

    // --- Error trait ---

    #[test]
    fn simard_error_implements_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(SimardError::RuntimeStopped {
            action: "test".to_string(),
        });
        assert!(err.source().is_none());
    }

    // --- Clone / PartialEq ---

    #[test]
    fn simard_error_clone_eq() {
        let err = SimardError::UnknownIdentity {
            requested: "test".to_string(),
        };
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn simard_error_debug_format() {
        let err = SimardError::StoragePoisoned {
            store: "memory".to_string(),
        };
        let debug = format!("{err:?}");
        assert!(debug.contains("StoragePoisoned"), "{debug}");
    }

    // --- fmt_field_reason branch coverage ---

    #[test]
    fn fmt_field_reason_identity_composition() {
        let err = SimardError::InvalidIdentityComposition {
            identity: "test-id".to_string(),
            reason: "bad".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("identity 'test-id' has invalid composition: bad"),
            "{msg}"
        );
    }

    #[test]
    fn fmt_field_reason_manifest_contract() {
        let err = SimardError::InvalidManifestContract {
            field: "name".to_string(),
            reason: "empty".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("invalid manifest contract field 'name': empty"),
            "{msg}"
        );
    }

    #[test]
    fn fmt_field_reason_goal_record() {
        let err = SimardError::InvalidGoalRecord {
            field: "priority".to_string(),
            reason: "out of range".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("invalid goal record field 'priority': out of range"),
            "{msg}"
        );
    }

    #[test]
    fn fmt_field_reason_research_record() {
        let err = SimardError::InvalidResearchRecord {
            field: "source".to_string(),
            reason: "blank".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("invalid research record field 'source': blank"),
            "{msg}"
        );
    }

    #[test]
    fn fmt_field_reason_meeting_record() {
        let err = SimardError::InvalidMeetingRecord {
            field: "decisions".to_string(),
            reason: "malformed".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("invalid meeting record field 'decisions': malformed"),
            "{msg}"
        );
    }

    #[test]
    fn fmt_field_reason_improvement_record() {
        let err = SimardError::InvalidImprovementRecord {
            field: "plan".to_string(),
            reason: "incomplete".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("invalid improvement record field 'plan': incomplete"),
            "{msg}"
        );
    }
}
