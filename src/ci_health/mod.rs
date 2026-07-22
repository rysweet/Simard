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

pub mod cache;
pub mod classify;
pub mod diagnose;
pub mod gh;
pub mod report;
pub mod steward;
pub mod types;

#[cfg(test)]
mod tests;

pub use cache::GreenShaCache;
pub use classify::{
    ActionableFailure, FleetReport, WorkflowVerdict, build_report, classify_workflow,
    repo_cacheable, update_cache_from_report,
};
pub use diagnose::{
    FailedJob, RealGhRunDiagnostics, RunDiagnosis, RunDiagnostics, parse_failure_annotations,
    parse_run_diagnosis,
};
pub use gh::{
    GhWorkflowClient, RealGhWorkflowClient, build_repo_snapshot, collect_fleet,
    snapshot_from_fixture,
};
pub use report::render_human;
pub use steward::{
    CiIssueResolver, IssueFilingReport, IssueResolutionReport, RealCiIssueResolver,
    ResolutionOutcome, UnauthorizedSkip, ci_failure_signature, ci_signature_for,
    file_issues_for_report, resolve_issues_for_report,
};
pub use types::{
    FleetSnapshot, RepoSnapshot, RunConclusion, WorkflowRun, WorkflowSnapshot, WorkflowState,
};

use crate::error::{SimardError, SimardResult};
use tracing::warn;

/// The amplihack ecosystem fleet — Simard plus her governed sibling repos, by
/// GitHub `owner/repo` slug — resolved from Simard's **single source of truth**:
/// her identity-scoped, mutable, deploy-DURABLE curated roster (see
/// [`crate::identity_curated_state`] and
/// [`crate::overseer::ecosystem_observe::resolve_governed_roster`]). The Overseer's
/// `ecosystem-observe` sweep and merge-queue reasoner resolve the SAME durable
/// dataset, so a repo Simard adds to (or removes from) her roster is swept on the
/// next build with no second list to silently drift out of sync (a drift that
/// would let a newly-governed repo's red CI go unswept and the fleet be reported
/// green). `amplihack` is `amplihack-rs` on GitHub.
///
/// Fail-loud: an empty or all-invalid roster is an `Err`, never a silently empty
/// sweep — an empty repo list would classify as zero actionable failures and
/// report the fleet **green**, the exact false-green this module exists to
/// prevent. The error is surfaced rather than `unwrap`-panicked so a steward
/// never aborts mid-sweep.
pub fn governed_repos() -> SimardResult<Vec<String>> {
    governed_repos_from(&crate::identity_curated_state::CuratedDataStore::resolve())
}

/// Resolve the governed roster from an explicit curated-state store — the
/// testable seam behind [`governed_repos`]. Resolves Simard's default-identity
/// `governed_repos` dataset, seeding it from her default roster on first use.
pub(crate) fn governed_repos_from(
    store: &crate::identity_curated_state::CuratedDataStore,
) -> SimardResult<Vec<String>> {
    use crate::overseer::ecosystem_observe::{
        DEFAULT_ROSTER_IDENTITY, default_simard_roster_seed, resolve_governed_roster,
    };
    resolve_governed_roster(
        store,
        DEFAULT_ROSTER_IDENTITY,
        &default_simard_roster_seed(),
    )
    .map_err(|error| SimardError::CiHealthGhCommandFailed {
        reason: format!("failed to resolve governed ecosystem roster: {error}"),
    })
}

/// Run a live sweep of the governed fleet ([`governed_repos`]), using and
/// updating the persistent last-known-green head-SHA cache so an unchanged-green
/// fleet is a cheap no-op.
///
/// The cache is loaded from [`GreenShaCache::default_path`], consulted by
/// [`collect_fleet`] to skip unchanged-green repos, reconciled against the fresh
/// report by [`update_cache_from_report`], and saved back. A save failure is
/// non-fatal (the verdict is already computed) and only warns.
pub fn sweep_live(gh: &dyn GhWorkflowClient) -> SimardResult<FleetReport> {
    sweep_live_with_options(gh, true)
}

/// Like [`sweep_live`] but `use_cache = false` forces a full re-collection of
/// every repo (no skips) while still refreshing the persisted cache from the
/// fresh report — the `--no-cache` / `--refresh` path.
pub fn sweep_live_with_options(
    gh: &dyn GhWorkflowClient,
    use_cache: bool,
) -> SimardResult<FleetReport> {
    let roster = governed_repos()?;
    let repos: Vec<&str> = roster.iter().map(String::as_str).collect();
    let path = GreenShaCache::default_path();
    let mut cache = if use_cache {
        GreenShaCache::load(&path)
    } else {
        GreenShaCache::empty()
    };
    let report = run_sweep(gh, &repos, &mut cache)?;
    if let Err(e) = cache.save(&path) {
        warn!(
            path = %path.display(),
            error = %e,
            "failed to persist ci-health green-SHA cache; next sweep will re-audit"
        );
    }
    Ok(report)
}

/// Collect → classify → reconcile-cache, without any disk I/O. This is the
/// testable core shared by the live paths: it skips cached-green repos, builds
/// the report, and updates `cache` in place.
pub fn run_sweep(
    gh: &dyn GhWorkflowClient,
    repos: &[&str],
    cache: &mut GreenShaCache,
) -> SimardResult<FleetReport> {
    let snapshot = collect_fleet(gh, repos, cache)?;
    let report = build_report(&snapshot);
    update_cache_from_report(cache, &snapshot, &report);
    Ok(report)
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
