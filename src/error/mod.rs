mod display;

#[cfg(test)]
mod tests_infra;
#[cfg(test)]
mod tests_variants;
#[cfg(test)]
mod tests_variants_extra;

use std::error::Error;
use std::path::PathBuf;

use crate::base_types::BaseTypeCapability;
use crate::cognitive_memory::creative_idea::IdeaStatus;
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
    /// Creative-ideas subsystem (#2419): an illegal `IdeaStatus` transition,
    /// from `try_transition` or an illegal synthesis `next_status`. Mirrors
    /// `InvalidRuntimeTransition` / `InvalidSessionTransition`.
    InvalidIdeaTransition {
        from: IdeaStatus,
        to: IdeaStatus,
    },
    /// Creative-ideas subsystem (#2419): a malformed `CreativeIdea` record —
    /// a serde (de)serialize failure, an unknown enum string, or a too-new
    /// `payload_version`. Mirrors `InvalidGoalRecord` / `InvalidImprovementRecord`.
    InvalidCreativeIdeaRecord {
        field: String,
        reason: String,
    },
    /// A stored journal entry (issue #2606) could not be parsed — a
    /// `journal:`-keyed cognitive-memory fact whose JSON content is corrupt.
    InvalidJournalRecord {
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
    /// A known engineer claim's worktree directory is absent (reaped, swept, or
    /// never allocated). Distinct from [`SimardError::NotARepo`]: the engineer
    /// is not "not a repo", its worktree simply does not exist on disk. The
    /// reaper treats this as a genuinely-missing worktree, NOT as a healthy
    /// engineer producing nothing, so it never triggers a false-stale reap of a
    /// live-but-idle engineer (issue #4744).
    MissingWorktree {
        claim_key: String,
        expected_path: PathBuf,
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
    RpcSpawnFailed {
        endpoint: String,
        reason: String,
    },
    RpcTransportError {
        endpoint: String,
        reason: String,
    },
    RpcProtocolError {
        endpoint: String,
        reason: String,
    },
    RpcCallFailed {
        endpoint: String,
        method: String,
        reason: String,
    },
    RpcCircuitOpen {
        endpoint: String,
    },
    RpcError(String),
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
    /// Stewardship: source-module → repo routing had no matching keyword.
    /// Retained for API/`Display` stability; **no longer produced by
    /// `route_failure`**, which now falls back to the default repo instead.
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
    /// CI-health sweep: a `gh` subprocess invocation failed (non-zero exit,
    /// missing binary, malformed JSON) or a report could not be serialized.
    CiHealthGhCommandFailed {
        reason: String,
    },
    /// Merge authority: a `gh pr` subprocess invocation failed.
    MergeAuthorityGhCommandFailed {
        reason: String,
    },
    /// Merge authority: a refusal that the operator must investigate
    /// (e.g. malformed `gh` output that prevented evaluation). This is
    /// distinct from the structured `MergeOutcome::Refused` returned on a
    /// well-understood block.
    MergeAuthorityEvaluationFailed {
        reason: String,
    },
    /// Pre-mutation guard: the working tree has uncommitted changes and the
    /// requested objective implies a mutating action. Per spec line 256 the
    /// mutating path requires a clean repo.
    DirtyWorktree {
        changed_files: Vec<String>,
    },
    /// The on-disk schema version is newer than this binary supports.
    /// Callers should refuse to load rather than silently corrupt data.
    SchemaTooNew {
        store: String,
        found_version: u32,
        max_supported: u32,
        path: PathBuf,
    },
    /// Failed to parse an identity.toml or watches.toml file.
    IdentityTomlParseError {
        path: PathBuf,
        reason: String,
    },
    /// The identity directory is not under the configured prompt root.
    IdentityPathNotUnderPromptRoot {
        identity_path: PathBuf,
        prompt_root: PathBuf,
    },
    /// Supply-chain steward (#2741): `cargo audit --json` output could not be
    /// parsed into advisories (malformed / unexpected JSON shape).
    SupplyChainAuditParseFailed {
        reason: String,
    },
    /// Supply-chain steward (#2741): a remediation step (issue filing, cargo
    /// update, PR open, or ignore-list write) failed; `reason` names the step
    /// and carries the underlying diagnostic.
    SupplyChainRemediationFailed {
        reason: String,
    },
    /// Supply-chain steward (#2741) HARD-RAIL guard: an advisory ignore write
    /// was attempted with no tracking-issue URL. Unreachable through
    /// `decide()` (a fixable advisory never yields `JustifiedIgnore`); this
    /// guards a future execution-path bug so the reasoner can never silently
    /// suppress an advisory without an open tracker.
    SupplyChainSuppressionWithoutTracker {
        advisory_id: String,
    },
}

pub type SimardResult<T> = Result<T, SimardError>;

impl Error for SimardError {}
