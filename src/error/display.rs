use std::fmt::{self, Display, Formatter};

use super::SimardError;

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
            Self::BudgetExceeded {
                period,
                spent,
                limit,
            } => {
                write!(
                    f,
                    "{period} budget exceeded: {spent} spent of {limit} limit"
                )
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
            Self::GitCommandFailed { command, reason } => {
                write!(f, "git command '{command}' failed: {reason}")
            }
            Self::GymHistoryDb { action, reason } => {
                write!(f, "gym history database '{action}' failed: {reason}")
            }
            Self::RuntimeInitFailed { component, reason } => {
                write!(
                    f,
                    "runtime component '{component}' failed to initialize: {reason}"
                )
            }
            Self::MemoryIntegrityError { path, reason } => {
                write!(
                    f,
                    "memory integrity check failed for '{}': {reason}",
                    path.display()
                )
            }
            Self::PromptNotFound { name } => {
                write!(f, "required prompt asset not found: {name}")
            }
            Self::StewardshipRoutingAmbiguous { source } => {
                write!(
                    f,
                    "stewardship: cannot route source-module '{source}' to a target repo (no matching keyword)"
                )
            }
            Self::StewardshipGhCommandFailed { reason } => {
                write!(f, "stewardship: gh command failed: {reason}")
            }
            Self::StewardshipInvalidRunSummary { field } => {
                write!(
                    f,
                    "stewardship: orchestrator run summary missing required field '{field}'"
                )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gym_history_db_display() {
        let err = SimardError::GymHistoryDb {
            action: "open".into(),
            reason: "file not found".into(),
        };
        assert_eq!(
            err.to_string(),
            "gym history database 'open' failed: file not found"
        );
    }

    #[test]
    fn runtime_init_failed_display() {
        let err = SimardError::RuntimeInitFailed {
            component: "goal_store".into(),
            reason: "allocation failed".into(),
        };
        assert_eq!(
            err.to_string(),
            "runtime component 'goal_store' failed to initialize: allocation failed"
        );
    }

    #[test]
    fn git_command_failed_display() {
        let err = SimardError::GitCommandFailed {
            command: "git diff".into(),
            reason: "not a repository".into(),
        };
        assert_eq!(
            err.to_string(),
            "git command 'git diff' failed: not a repository"
        );
    }

    #[test]
    fn missing_config_display() {
        let err = SimardError::MissingRequiredConfig {
            key: "API_KEY".into(),
            help: "set via environment variable".into(),
        };
        assert!(err.to_string().contains("API_KEY"));
        assert!(err.to_string().contains("missing required configuration"));
    }

    #[test]
    fn bridge_transport_error_display() {
        let err = SimardError::BridgeTransportError {
            bridge: "subprocess".into(),
            reason: "child missing".into(),
        };
        assert!(err.to_string().contains("subprocess"));
        assert!(err.to_string().contains("child missing"));
    }
}
