//! Agent-under-test runner (research doc Part 3.3, component 2).
//!
//! Two interchangeable strategies sit behind one [`AgentStrategy`] interface:
//!
//! - [`BaselineStrategy`] — a single model reads the harness + source and submits
//!   its candidate input directly.
//! - [`TeamStrategy`] — a skwaq-style debate: a *reacher* proposes an input, a
//!   *skeptic* challenges whether it truly reaches `ℓ`, and a *synthesizer*
//!   submits-or-abstains via a `threshold_hint`-style gate. Because COIN
//!   **precision** punishes over-claiming, abstaining on a low-confidence input
//!   is often better than submitting a wrong one.
//!
//! The actual reasoning is abstracted behind [`Reasoner`] so a live LLM (via
//! LiteLLM, Phase 3+) and an offline scripted stand-in ([`FixtureReasoner`]) are
//! interchangeable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::executor::HarnessExecutor;
use super::types::{CoinGymResult, Outcome, OutcomeCode, RunReport, Strategy, Target};

/// Default `threshold_hint` for the team synthesizer's submit/abstain gate.
pub const DEFAULT_THRESHOLD_HINT: f64 = 0.6;

/// Process-local monotonic counter appended to run ids so two runs of the same
/// model+strategy in the same millisecond never collide.
static RUN_SEQ: AtomicU64 = AtomicU64::new(0);

/// A candidate input proposed by the agent under test.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// The candidate input (a placeholder UTF-8 string in the scaffold; real
    /// COIN inputs are raw bytes staged for `coin evaluate`).
    pub input: String,
    /// The agent's self-reported confidence in `0.0..=1.0`.
    pub confidence: f64,
    /// Free-text rationale (why this input should reach the line).
    pub rationale: String,
}

/// Produces candidate inputs for targets. A live implementation calls a model
/// through LiteLLM; [`FixtureReasoner`] replays scripted candidates offline.
pub trait Reasoner {
    /// Propose a candidate input for `target`, or `None` if the agent produced
    /// nothing (yields a `NoSubmission` outcome).
    fn propose(&self, target: &Target) -> Option<Candidate>;

    /// Skeptic assessment of how likely `candidate` truly reaches the line, in
    /// `0.0..=1.0`. Defaults to the candidate's own confidence; a real skeptic
    /// agent lowers this when it spots over-claiming.
    fn assess(&self, _target: &Target, candidate: &Candidate) -> f64 {
        candidate.confidence
    }
}

/// A reasoner that replays scripted candidates keyed by target id. Targets
/// without a script entry yield `None` (→ `NoSubmission`).
#[derive(Clone, Debug, Default)]
pub struct FixtureReasoner {
    script: HashMap<String, Candidate>,
}

impl FixtureReasoner {
    /// Build from a `target_id -> Candidate` script map.
    #[must_use]
    pub fn new(script: HashMap<String, Candidate>) -> Self {
        Self { script }
    }
}

impl Reasoner for FixtureReasoner {
    fn propose(&self, target: &Target) -> Option<Candidate> {
        self.script.get(&target.id).cloned()
    }
}

/// The synthesizer's decision for one target.
#[derive(Clone, Debug, PartialEq)]
pub enum SubmissionDecision {
    /// Submit `input` for grading.
    Submit {
        /// The input to grade.
        input: String,
        /// Confidence attached to the submission.
        confidence: f64,
    },
    /// Deliberately decline to submit (precision-preserving).
    Abstain {
        /// Why the agent abstained.
        reason: String,
    },
    /// The agent produced no candidate at all.
    NoSubmission,
}

/// The agent's submission for one target, with the rationale trail.
#[derive(Clone, Debug, PartialEq)]
pub struct Submission {
    /// Target this submission is for.
    pub target_id: String,
    /// The submit/abstain/no-submission decision.
    pub decision: SubmissionDecision,
    /// Human-readable rationale.
    pub rationale: String,
}

/// A strategy for turning a target into a submission.
pub trait AgentStrategy {
    /// Which strategy this is (recorded in the run report).
    fn kind(&self) -> Strategy;

    /// Decide what to submit (or abstain) for `target`.
    fn evaluate(&self, target: &Target) -> Submission;
}

/// Single-model baseline: submit the proposed candidate as-is.
#[derive(Clone, Debug)]
pub struct BaselineStrategy<R: Reasoner> {
    reasoner: R,
}

impl<R: Reasoner> BaselineStrategy<R> {
    /// Create a baseline strategy over `reasoner`.
    #[must_use]
    pub fn new(reasoner: R) -> Self {
        Self { reasoner }
    }
}

impl<R: Reasoner> AgentStrategy for BaselineStrategy<R> {
    fn kind(&self) -> Strategy {
        Strategy::Baseline
    }

    fn evaluate(&self, target: &Target) -> Submission {
        match self.reasoner.propose(target) {
            Some(candidate) => Submission {
                target_id: target.id.clone(),
                decision: SubmissionDecision::Submit {
                    input: candidate.input,
                    confidence: candidate.confidence,
                },
                rationale: candidate.rationale,
            },
            None => Submission {
                target_id: target.id.clone(),
                decision: SubmissionDecision::NoSubmission,
                rationale: "reasoner produced no candidate".to_string(),
            },
        }
    }
}

/// Multi-agent team: reacher → skeptic → synthesizer submit/abstain gate.
#[derive(Clone, Debug)]
pub struct TeamStrategy<R: Reasoner> {
    reasoner: R,
    threshold_hint: f64,
}

impl<R: Reasoner> TeamStrategy<R> {
    /// Create a team strategy with the default `threshold_hint`.
    #[must_use]
    pub fn new(reasoner: R) -> Self {
        Self {
            reasoner,
            threshold_hint: DEFAULT_THRESHOLD_HINT,
        }
    }

    /// Create a team strategy with an explicit `threshold_hint` (clamped to
    /// `0.0..=1.0`).
    #[must_use]
    pub fn with_threshold(reasoner: R, threshold_hint: f64) -> Self {
        Self {
            reasoner,
            threshold_hint: threshold_hint.clamp(0.0, 1.0),
        }
    }

    /// The submit/abstain threshold in effect.
    #[must_use]
    pub fn threshold_hint(&self) -> f64 {
        self.threshold_hint
    }
}

impl<R: Reasoner> AgentStrategy for TeamStrategy<R> {
    fn kind(&self) -> Strategy {
        Strategy::Team
    }

    fn evaluate(&self, target: &Target) -> Submission {
        let Some(candidate) = self.reasoner.propose(target) else {
            return Submission {
                target_id: target.id.clone(),
                decision: SubmissionDecision::NoSubmission,
                rationale: "reacher produced no candidate".to_string(),
            };
        };
        // Skeptic challenges the reacher's over-claim; synthesizer gates on it.
        let skeptic_score = self.reasoner.assess(target, &candidate);
        if skeptic_score >= self.threshold_hint {
            Submission {
                target_id: target.id.clone(),
                decision: SubmissionDecision::Submit {
                    input: candidate.input,
                    confidence: skeptic_score,
                },
                rationale: format!(
                    "skeptic score {skeptic_score:.2} >= threshold {:.2}; {}",
                    self.threshold_hint, candidate.rationale
                ),
            }
        } else {
            Submission {
                target_id: target.id.clone(),
                decision: SubmissionDecision::Abstain {
                    reason: format!(
                        "skeptic score {skeptic_score:.2} < threshold {:.2} (precision-preserving abstention)",
                        self.threshold_hint
                    ),
                },
                rationale: candidate.rationale,
            }
        }
    }
}

/// Drives a strategy over a target set and grades each submission through the
/// harness executor to produce a [`RunReport`].
pub struct AgentRunner<'a, S: AgentStrategy, E: HarnessExecutor> {
    strategy: &'a S,
    executor: &'a E,
    model: String,
    snapshot: String,
}

impl<'a, S: AgentStrategy, E: HarnessExecutor> AgentRunner<'a, S, E> {
    /// Create a runner for `model` on `snapshot`.
    #[must_use]
    pub fn new(
        strategy: &'a S,
        executor: &'a E,
        model: impl Into<String>,
        snapshot: impl Into<String>,
    ) -> Self {
        Self {
            strategy,
            executor,
            model: model.into(),
            snapshot: snapshot.into(),
        }
    }

    /// Evaluate every target, grading submitted inputs through the executor.
    ///
    /// # Errors
    /// Propagates [`crate::coin_gym::types::CoinGymError::Executor`] if the
    /// executor cannot grade at all (e.g. the real `coin evaluate` delegate on a
    /// host without Docker). Legitimate *timeout*/*error* grade verdicts are
    /// recorded as outcomes, not surfaced as errors.
    pub fn run(&self, targets: &[Target]) -> CoinGymResult<RunReport> {
        let started_at_unix_ms = now_unix_ms();
        let mut outcomes = Vec::with_capacity(targets.len());
        for target in targets {
            let submission = self.strategy.evaluate(target);
            let code = match submission.decision {
                SubmissionDecision::Submit { input, .. } => {
                    self.executor.grade(target, &input)?.to_outcome_code()
                }
                SubmissionDecision::Abstain { .. } => OutcomeCode::Abstained,
                SubmissionDecision::NoSubmission => OutcomeCode::NoSubmission,
            };
            outcomes.push(Outcome {
                target_id: target.id.clone(),
                family: target.family,
                code,
                cost_usd: 0.0,
            });
        }
        let run_id = format!(
            "{}-{}-{}-{}",
            sanitize(&self.model),
            self.strategy.kind().label(),
            started_at_unix_ms,
            RUN_SEQ.fetch_add(1, Ordering::Relaxed)
        );
        Ok(RunReport {
            run_id,
            model: self.model.clone(),
            strategy: self.strategy.kind(),
            snapshot: self.snapshot.clone(),
            started_at_unix_ms,
            outcomes,
            offline_scaffold: self.executor.is_offline_scaffold(),
        })
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn sanitize(model: &str) -> String {
    model
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
