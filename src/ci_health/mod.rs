//! Governed-fleet CI-health sweep — a precise, reproducible check of whether
//! every **active** default-branch workflow across the amplihack ecosystem is
//! green.
//!
//! ## Why this exists
//! The CI-health stewardship goal was previously executed by hand-rolling
//! `gh run list` each cycle, which cannot distinguish an *actionable active-CI
//! failure* from a *disabled/cancelled non-failure*. That ambiguity produced
//! imprecise, un-evidenced claims (e.g. "every workflow's latest run is
//! `success`" when a governed repo's latest runs were actually `failure` — on
//! workflows that had been manually **disabled**). This module codifies the
//! sweep so its verdict is reproducible and its evidence is explicit.
//!
//! ## Pipeline
//! 1. [`gh::collect_fleet`] reads, per repo: default branch, workflow states,
//!    and the latest default-branch run per workflow.
//! 2. [`classify::build_report`] classifies each workflow into
//!    green / actionable-failure / ignored(reason).
//! 3. The fleet is green iff there are zero actionable failures.
//!
//! See `docs/reference/ci-health-sweep.md`.

pub mod classify;
pub mod gh;
pub mod report;
pub mod types;

#[cfg(test)]
mod tests;

pub use classify::{
    ActionableFailure, FleetReport, WorkflowVerdict, build_report, classify_workflow,
};
pub use gh::{
    GhWorkflowClient, RealGhWorkflowClient, build_repo_snapshot, collect_fleet,
    snapshot_from_fixture,
};
pub use report::render_human;
pub use types::{
    FleetSnapshot, RepoSnapshot, RunConclusion, WorkflowRun, WorkflowSnapshot, WorkflowState,
};

use crate::error::{SimardError, SimardResult};

/// The amplihack ecosystem fleet: Simard plus its governed sibling repos, by
/// GitHub `owner/repo` slug. Source of truth: the ecosystem table in
/// `prompt_assets/simard/engineer_system.md` (note `amplihack` → `amplihack-rs`
/// on GitHub).
pub const GOVERNED_REPOS: &[&str] = &[
    "rysweet/Simard",
    "rysweet/RustyClawd",
    "rysweet/amplihack-rs",
    "rysweet/azlin",
    "rysweet/amplihack-memory-lib",
    "rysweet/amplihack-agent-eval",
    "rysweet/agent-kgpacks",
    "rysweet/amplihack-recipe-runner",
    "rysweet/amplihack-xpia-defender",
    "rysweet/gadugi-agentic-test",
];

/// Run a live sweep of [`GOVERNED_REPOS`] and classify it into a report.
pub fn sweep_live(gh: &dyn GhWorkflowClient) -> SimardResult<FleetReport> {
    let snapshot = collect_fleet(gh, GOVERNED_REPOS)?;
    Ok(build_report(&snapshot))
}

/// Classify an offline fixture snapshot into a report (`--from-json`).
pub fn sweep_fixture(json: &[u8]) -> SimardResult<FleetReport> {
    let snapshot = snapshot_from_fixture(json)?;
    Ok(build_report(&snapshot))
}

/// Serialize a report as pretty JSON.
pub fn report_to_json(report: &FleetReport) -> SimardResult<String> {
    serde_json::to_string_pretty(report).map_err(|e| SimardError::CiHealthGhCommandFailed {
        reason: format!("failed to serialize CI-health report: {e}"),
    })
}
