mod display;

#[cfg(test)]
mod tests_infra;
#[cfg(test)]
mod tests_variants;

use std::error::Error;
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
    BudgetExceeded {
        period: String,
        spent: String,
        limit: String,
    },
    ReviewUnavailable {
        reason: String,
    },
    ReviewBlocked {
        summary: String,
    },
    GitCommandFailed {
        command: String,
        reason: String,
    },
    GymHistoryDb {
        action: String,
        reason: String,
    },
    RuntimeInitFailed {
        component: String,
        reason: String,
    },
    MemoryIntegrityError {
        path: PathBuf,
        reason: String,
    },
    PromptNotFound {
        name: String,
    },
    /// Stewardship: source-module → repo routing has no matching keyword. Fail-loud — no default repo.
    StewardshipRoutingAmbiguous {
        source: String,
    },
    /// Stewardship: a `gh` subprocess invocation failed (non-zero exit, missing binary, malformed JSON).
    StewardshipGhCommandFailed {
        reason: String,
    },
    /// Stewardship: an `OrchestratorRunSummary` had an empty required field.
    StewardshipInvalidRunSummary {
        field: &'static str,
    },
}

pub type SimardResult<T> = Result<T, SimardError>;

impl Error for SimardError {}
