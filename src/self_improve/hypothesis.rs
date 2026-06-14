//! Improvement hypothesis formation from benchmark and session failure signals.
//!
//! Per `Specs/ProductArchitecture.md` lines 679–681 the self-improvement loop
//! must "capture failures, retries, weak explanations, or unnecessary actions"
//! and "form improvement hypotheses". This module is the seam that turns
//! observed failure signals into structured [`ImprovementHypothesis`] records
//! that downstream tooling can promote (or hand to an operator for review).
//!
//! The implementation is intentionally deterministic and side-effect-free so
//! it can run inside the cycle, inside reflection, or inside a probe. Every
//! emitted hypothesis carries a `source_evidence: Vec<EvidenceRef>` chain so
//! the spec's traceability requirement (line 696) is preserved even when a
//! hypothesis is later promoted into a durable [`crate::goals::GoalUpdate`].

use serde::{Deserialize, Serialize};

use crate::gym::{BenchmarkCheckResult, BenchmarkRunReport};
use crate::gym_history::{GymSignal, ScenarioSignal};
use crate::improvements::EvidenceRef;
use crate::review::ReviewArtifact;

use super::types::{ProposedChange, WeakDimension};

/// A structured improvement hypothesis derived from one or more failure
/// signals. Each hypothesis carries the typed evidence that justified it so
/// promotion can preserve the link per spec line 696.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImprovementHypothesis {
    /// Stable id (deterministic from the underlying signal so duplicate
    /// signals from the same scenario produce the same hypothesis id).
    pub id: String,
    /// Short title suitable for use as a goal title after promotion.
    pub title: String,
    /// Coarse classification (e.g. `benchmark-failure`, `weak-dimension`,
    /// `review-finding`, `session-failure`).
    pub category: String,
    /// Why the hypothesis was emitted. Plain text.
    pub rationale: String,
    /// The concrete change being hypothesised.
    pub suggested_change: String,
    /// Typed evidence references that motivated the hypothesis.
    pub source_evidence: Vec<EvidenceRef>,
}

impl ImprovementHypothesis {
    /// Convert this hypothesis into a [`ProposedChange`] suitable for the
    /// [`super::cycle::run_improvement_cycle`] entry point.
    ///
    /// `expected_impact` is rendered from the hypothesis category and
    /// evidence count so the `ImprovementCycle` log stays grounded.
    pub fn into_proposed_change(self, file_path: impl Into<String>) -> ProposedChange {
        let evidence_label = if self.source_evidence.is_empty() {
            "no-evidence".to_string()
        } else {
            self.source_evidence
                .iter()
                .map(EvidenceRef::to_persisted_string)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        ProposedChange {
            file_path: file_path.into(),
            description: self.suggested_change,
            expected_impact: format!(
                "{} hypothesis: {} (evidence: {})",
                self.category, self.title, evidence_label
            ),
        }
    }
}

/// Form hypotheses by clustering failed benchmark scenarios in `reports`.
///
/// One hypothesis is emitted per failed scenario, with `source_evidence`
/// containing:
/// - a [`EvidenceRef::BenchmarkScenario`] for the scenario,
/// - a [`EvidenceRef::BenchmarkRunReport`] for the specific run, and
/// - one [`EvidenceRef::BenchmarkCheckFailure`] per failed correctness check.
///
/// Skipped or passed scenarios are ignored. The output preserves the input
/// order so callers can correlate by index.
pub fn form_hypotheses_from_benchmark_reports(
    reports: &[BenchmarkRunReport],
) -> Vec<ImprovementHypothesis> {
    reports
        .iter()
        .filter(|report| !report.passed)
        .map(hypothesis_from_benchmark_report)
        .collect()
}

/// Form hypotheses for the most recent regression signal of each scenario.
///
/// Only [`GymSignal::Regression`] entries produce hypotheses; promotion,
/// improvement, and stable signals are skipped (they do not require
/// improvement action). The `suite_id` argument is required because
/// [`ScenarioSignal`] only carries the scenario id.
pub fn form_hypotheses_from_signals(
    suite_id: &str,
    signals: &[ScenarioSignal],
) -> Vec<ImprovementHypothesis> {
    signals
        .iter()
        .filter_map(|signal| match &signal.signal {
            GymSignal::Regression { delta } => Some(hypothesis_from_regression(
                suite_id,
                &signal.scenario_id,
                *delta,
            )),
            _ => None,
        })
        .collect()
}

/// Form hypotheses for each weak scoring dimension. Each hypothesis links
/// the [`WeakDimension`] via an [`EvidenceRef::WeakDimension`] reference.
pub fn form_hypotheses_from_weak_dimensions(weak: &[WeakDimension]) -> Vec<ImprovementHypothesis> {
    weak.iter().map(hypothesis_from_weak_dimension).collect()
}

/// Form hypotheses from a review artifact. Each [`crate::review::ImprovementProposal`]
/// in the review becomes one hypothesis with the review id and any
/// proposal-attached evidence preserved.
pub fn form_hypotheses_from_review(review: &ReviewArtifact) -> Vec<ImprovementHypothesis> {
    review
        .proposals
        .iter()
        .map(|proposal| {
            let mut source_evidence: Vec<EvidenceRef> = proposal
                .evidence
                .iter()
                .map(|item| EvidenceRef::parse_str(item))
                .collect();
            source_evidence.push(EvidenceRef::Review {
                review_id: review.review_id.clone(),
                target_label: if review.target_label.is_empty() {
                    None
                } else {
                    Some(review.target_label.clone())
                },
            });
            ImprovementHypothesis {
                id: format!("review-{}::{}", review.review_id, slugify(&proposal.title)),
                title: proposal.title.clone(),
                category: format!("review-{}", proposal.category),
                rationale: proposal.rationale.clone(),
                suggested_change: proposal.suggested_change.clone(),
                source_evidence,
            }
        })
        .collect()
}

/// Form hypotheses from each failed signal recorded in
/// [`crate::review::ReviewEvidenceSummary::failed_signals`].
///
/// Useful when the orchestrator already discarded the per-proposal evidence
/// but kept session-level failure indicators.
pub fn form_hypotheses_from_session_failures(
    session_id: &str,
    failed_signals: &[String],
) -> Vec<ImprovementHypothesis> {
    failed_signals
        .iter()
        .map(|signal_id| ImprovementHypothesis {
            id: format!("session-{session_id}::{}", slugify(signal_id)),
            title: format!("Investigate failed signal '{signal_id}'"),
            category: "session-failure".to_string(),
            rationale: format!(
                "Session '{session_id}' reported a failed signal '{signal_id}' during reflection. \
                Repeated occurrences indicate a systemic gap per spec line 689."
            ),
            suggested_change: format!(
                "Capture the failed signal '{signal_id}' as a structured failure mode and add a \
                 benchmark or session reproducer to drive the gap to closure."
            ),
            source_evidence: vec![EvidenceRef::SessionFailure {
                session_id: session_id.to_string(),
                signal_id: signal_id.clone(),
                detail: None,
            }],
        })
        .collect()
}

/// Aggregate hypotheses from all four inputs (benchmark failure reports,
/// regression signals, weak dimensions, and reviews) into a single ordered
/// vector. Order is `reports → signals → weak → reviews → session_failures`.
pub fn aggregate_hypotheses(
    suite_id: &str,
    benchmark_reports: &[BenchmarkRunReport],
    signals: &[ScenarioSignal],
    weak: &[WeakDimension],
    reviews: &[ReviewArtifact],
    session_failures: &[(String, Vec<String>)],
) -> Vec<ImprovementHypothesis> {
    let mut out = Vec::new();
    out.extend(form_hypotheses_from_benchmark_reports(benchmark_reports));
    out.extend(form_hypotheses_from_signals(suite_id, signals));
    out.extend(form_hypotheses_from_weak_dimensions(weak));
    for review in reviews {
        out.extend(form_hypotheses_from_review(review));
    }
    for (session_id, failed_signals) in session_failures {
        out.extend(form_hypotheses_from_session_failures(
            session_id,
            failed_signals,
        ));
    }
    out
}

// ── Internal helpers ─────────────────────────────────────────────────────

fn hypothesis_from_benchmark_report(report: &BenchmarkRunReport) -> ImprovementHypothesis {
    let suite_id = report.suite_id.clone();
    let scenario_id = report.scenario.id.to_string();

    let failed_checks: Vec<&BenchmarkCheckResult> =
        report.checks.iter().filter(|check| !check.passed).collect();
    let failed_check_count = failed_checks.len();
    let unnecessary_actions = report.scorecard.unnecessary_action_count.unwrap_or(0);
    let retry_count = report.scorecard.retry_count.unwrap_or(0);

    let mut source_evidence = Vec::with_capacity(2 + failed_checks.len());
    source_evidence.push(EvidenceRef::BenchmarkScenario {
        suite_id: suite_id.clone(),
        scenario_id: scenario_id.clone(),
        session_id: Some(report.session_id.clone()),
    });
    source_evidence.push(EvidenceRef::BenchmarkRunReport {
        suite_id: suite_id.clone(),
        scenario_id: scenario_id.clone(),
        session_id: report.session_id.clone(),
        run_started_at_unix_ms: u64::try_from(report.run_started_at_unix_ms).unwrap_or(u64::MAX),
    });
    for check in &failed_checks {
        source_evidence.push(EvidenceRef::BenchmarkCheckFailure {
            suite_id: suite_id.clone(),
            scenario_id: scenario_id.clone(),
            check_id: check.id.clone(),
            detail: check.detail.clone(),
        });
    }

    let rationale = format!(
        "Benchmark '{}/{}' failed with {failed_check_count} failed correctness check(s), \
         {unnecessary_actions} unnecessary action(s), {retry_count} retry/retries. \
         Per spec line 680, repeated failure modes must drive an improvement hypothesis.",
        suite_id, scenario_id,
    );

    let suggested_change = if failed_check_count > 0 {
        format!(
            "Investigate the {failed_check_count} failed correctness check(s) on '{}/{}' and \
             update the prompt, policy, or orchestration logic that produced the failing output.",
            suite_id, scenario_id,
        )
    } else {
        format!(
            "Investigate the scorecard signals on '{}/{}' (unnecessary actions / retries) and \
             tighten the behaviour responsible for the wasted work.",
            suite_id, scenario_id,
        )
    };

    ImprovementHypothesis {
        id: format!("benchmark-failure::{suite_id}::{scenario_id}"),
        title: format!("Fix benchmark failure: {}/{}", suite_id, scenario_id),
        category: "benchmark-failure".to_string(),
        rationale,
        suggested_change,
        source_evidence,
    }
}

fn hypothesis_from_regression(
    suite_id: &str,
    scenario_id: &str,
    delta: f64,
) -> ImprovementHypothesis {
    let abs_delta_pct = (delta.abs() * 100.0).round() / 100.0;
    ImprovementHypothesis {
        id: format!("regression::{suite_id}::{scenario_id}"),
        title: format!(
            "Address regression in {}/{} (Δ {:.2})",
            suite_id, scenario_id, delta
        ),
        category: "benchmark-regression".to_string(),
        rationale: format!(
            "Scenario '{}/{}' regressed by {abs_delta_pct} versus the previous recorded score. \
             Per spec line 684, the loop must only promote measurable improvements — a regression \
             needs a hypothesis to drive the next cycle.",
            suite_id, scenario_id,
        ),
        suggested_change: format!(
            "Bisect the change between the previous and current benchmark runs on '{}/{}' and \
             revert or remediate the contributing prompt/policy change.",
            suite_id, scenario_id,
        ),
        source_evidence: vec![EvidenceRef::BenchmarkScenario {
            suite_id: suite_id.to_string(),
            scenario_id: scenario_id.to_string(),
            session_id: None,
        }],
    }
}

fn hypothesis_from_weak_dimension(weak: &WeakDimension) -> ImprovementHypothesis {
    ImprovementHypothesis {
        id: format!("weak-dimension::{}", weak.name),
        title: format!(
            "Strengthen weak dimension '{}' (deficit {:.2})",
            weak.name, weak.deficit
        ),
        category: "weak-dimension".to_string(),
        rationale: format!(
            "Dimension '{}' scored {:.2} below the configured weak threshold. Per spec line 681 \
             this is a captured failure signal that should drive an improvement hypothesis.",
            weak.name, weak.deficit
        ),
        suggested_change: format!(
            "Target the '{}' dimension with a focused improvement cycle (prompt update, policy \
             change, or memory heuristic) and re-run the suite to confirm the deficit shrinks.",
            weak.name
        ),
        source_evidence: vec![EvidenceRef::WeakDimension {
            dimension: weak.name.clone(),
            deficit: weak.deficit,
        }],
    }
}

fn slugify(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}
