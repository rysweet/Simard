//! M4 — the [`Auditor`] adapter: run crusty-old-engineer-gated quality audits on
//! demand and on a recurring cadence.
//!
//! Reuse (design doc §capability table): `self_quality_audit::run_self_quality_audit`
//! (`src/self_quality_audit.rs:260`) drives the shipped
//! `monthly-self-quality-audit.yaml` recipe (SEEK→VALIDATE→FIX waves, each merge
//! crusty-old-engineer-gated); `self_quality_audit::{read_last_run, write_last_run}`
//! provide the durable cadence marker (no new schema).
//!
//! The recipe subprocess is behind an injectable [`AuditRunner`] seam so scope
//! routing and cadence are unit-tested with a fake — no subprocess, no network.

use std::path::PathBuf;

use crate::overseer::capabilities::{AuditReport, AuditScope, Auditor, OverseerError};

/// The outcome of one quality-audit run, projected from
/// `self_quality_audit::SelfQualityAuditReport`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualityAuditOutcome {
    pub scope: AuditScope,
    pub waves_completed: u32,
    pub prs_opened: Vec<String>,
    pub prs_merged: Vec<String>,
    /// PRs the bounded crusty-old-engineer loop could not resolve — surfaced for
    /// human follow-up (a non-empty list means the audit did NOT fully pass).
    pub crusty_unresolved: Vec<String>,
    pub summary: String,
}

/// Runs a quality audit for a scope. Injectable so cadence + routing are tested
/// without spawning the recipe. Production uses [`SelfQualityAuditRunner`].
pub trait AuditRunner {
    fn run(&self, scope: &AuditScope) -> Result<QualityAuditOutcome, OverseerError>;
}

/// Real runner over the shipped self-quality-audit recipe.
pub struct SelfQualityAuditRunner {
    pub repo_root: PathBuf,
    pub state_root: PathBuf,
}

impl AuditRunner for SelfQualityAuditRunner {
    fn run(&self, scope: &AuditScope) -> Result<QualityAuditOutcome, OverseerError> {
        let report = crate::self_quality_audit::run_self_quality_audit(
            &self.repo_root,
            &self.state_root,
            None,
        )
        .map_err(|e| OverseerError::Capability {
            what: "self_quality_audit",
            detail: e.to_string(),
        })?;
        Ok(QualityAuditOutcome {
            scope: scope.clone(),
            waves_completed: report.waves_completed,
            prs_opened: report.prs_opened.clone(),
            prs_merged: report.prs_merged.clone(),
            crusty_unresolved: report.crusty_unresolved.clone(),
            summary: report.summary(),
        })
    }
}

/// The [`Auditor`]. Routes a scope through the runner and reports pass/fail
/// (fail iff the crusty loop left PRs unresolved). Also drives a recurring
/// self-audit via a durable last-run marker.
pub struct SelfQualityAuditor {
    runner: Box<dyn AuditRunner>,
    marker_path: PathBuf,
    interval_secs: u64,
}

impl SelfQualityAuditor {
    pub fn new(runner: Box<dyn AuditRunner>, marker_path: PathBuf, interval_secs: u64) -> Self {
        Self {
            runner,
            marker_path,
            interval_secs,
        }
    }

    /// Production auditor: real recipe runner, a marker under the state root, and
    /// a monthly cadence (matching `monthly-self-quality-audit.yaml`).
    pub fn from_env(repo_root: PathBuf, state_root: PathBuf) -> Self {
        let marker = state_root.join("overseer/self_quality_audit.last_run");
        Self::new(
            Box::new(SelfQualityAuditRunner {
                repo_root,
                state_root,
            }),
            marker,
            MONTHLY_SECS,
        )
    }

    /// True iff a recurring self-audit is due at `now_epoch` (never run, or the
    /// interval has elapsed since the last run).
    pub fn recurring_due(&self, now_epoch: u64) -> bool {
        match crate::self_quality_audit::read_last_run(&self.marker_path) {
            None => true,
            Some(last) => now_epoch.saturating_sub(last) >= self.interval_secs,
        }
    }

    /// Run the recurring self-health audit if due, stamping the marker. Returns
    /// `None` when not due (no work, no side effect).
    pub fn run_recurring(&self, now_epoch: u64) -> Option<Result<AuditReport, OverseerError>> {
        if !self.recurring_due(now_epoch) {
            return None;
        }
        let result = self.run_audit(&AuditScope::SelfHealth);
        // Stamp the marker even on failure so a red audit does not hot-loop; the
        // findings surface the crusty-unresolved PRs for human follow-up.
        let _ = crate::self_quality_audit::write_last_run(&self.marker_path, now_epoch);
        Some(result)
    }
}

/// A recurring self-audit cadence of ~30 days (monthly recipe).
pub const MONTHLY_SECS: u64 = 30 * 24 * 60 * 60;

impl Auditor for SelfQualityAuditor {
    fn run_audit(&self, scope: &AuditScope) -> Result<AuditReport, OverseerError> {
        let outcome = self.runner.run(scope)?;
        // A clean audit is one the crusty-old-engineer gate fully resolved.
        let passed = outcome.crusty_unresolved.is_empty();
        let mut findings = Vec::new();
        findings.push(outcome.summary.clone());
        for pr in &outcome.crusty_unresolved {
            findings.push(format!("crusty-unresolved (needs human): {pr}"));
        }
        Ok(AuditReport {
            scope: outcome.scope,
            passed,
            findings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeRunner {
        seen: Mutex<Vec<AuditScope>>,
        unresolved: Vec<String>,
    }
    impl FakeRunner {
        fn new(unresolved: Vec<String>) -> Self {
            Self {
                seen: Mutex::new(vec![]),
                unresolved,
            }
        }
    }
    impl AuditRunner for std::sync::Arc<FakeRunner> {
        fn run(&self, scope: &AuditScope) -> Result<QualityAuditOutcome, OverseerError> {
            self.seen.lock().unwrap().push(scope.clone());
            Ok(QualityAuditOutcome {
                scope: scope.clone(),
                waves_completed: 3,
                prs_opened: vec!["pr1".to_string()],
                prs_merged: vec!["pr1".to_string()],
                crusty_unresolved: self.unresolved.clone(),
                summary: "audit done".to_string(),
            })
        }
    }

    fn auditor(
        runner: std::sync::Arc<FakeRunner>,
        marker: PathBuf,
        interval: u64,
    ) -> SelfQualityAuditor {
        SelfQualityAuditor::new(Box::new(runner), marker, interval)
    }

    #[test]
    fn audit_routes_each_scope_to_the_runner() {
        let runner = std::sync::Arc::new(FakeRunner::new(vec![]));
        let tmp = tempfile::tempdir().unwrap();
        let a = auditor(runner.clone(), tmp.path().join("m"), 100);
        for scope in [
            AuditScope::SelfHealth,
            AuditScope::Repo {
                slug: "rysweet/amplihack".to_string(),
            },
            AuditScope::CrossCutting,
        ] {
            let report = a.run_audit(&scope).unwrap();
            assert_eq!(report.scope, scope);
        }
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen.len(), 3, "every scope routed to the runner");
        assert!(seen.contains(&AuditScope::SelfHealth));
        assert!(seen.contains(&AuditScope::CrossCutting));
    }

    #[test]
    fn audit_passes_only_when_crusty_resolved_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let clean = auditor(
            std::sync::Arc::new(FakeRunner::new(vec![])),
            tmp.path().join("a"),
            100,
        );
        assert!(clean.run_audit(&AuditScope::SelfHealth).unwrap().passed);

        let dirty = auditor(
            std::sync::Arc::new(FakeRunner::new(vec!["pr-stuck".to_string()])),
            tmp.path().join("b"),
            100,
        );
        let report = dirty.run_audit(&AuditScope::SelfHealth).unwrap();
        assert!(!report.passed, "unresolved crusty PRs fail the audit");
        assert!(report.findings.iter().any(|f| f.contains("pr-stuck")));
    }

    #[test]
    fn recurring_is_due_when_never_run_then_respects_interval() {
        let runner = std::sync::Arc::new(FakeRunner::new(vec![]));
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("last_run");
        let a = auditor(runner.clone(), marker.clone(), 1000);

        // Never run → due.
        assert!(a.recurring_due(5_000));
        // Run it → stamps the marker.
        assert!(a.run_recurring(5_000).is_some());
        // Immediately after → not due.
        assert!(!a.recurring_due(5_500));
        // After the interval → due again.
        assert!(a.recurring_due(6_000));
        // Not-due window runs nothing.
        assert!(a.run_recurring(5_500).is_none());
        assert_eq!(
            runner.seen.lock().unwrap().len(),
            1,
            "only the due run executed"
        );
    }
}
