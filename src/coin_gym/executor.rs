//! Harness executor (research doc Part 3.3, component 3).
//!
//! The executor is the **objective oracle**: it decides whether a submitted
//! input actually reaches the target line. In production this delegates to
//! `coin evaluate` (Docker + instrumented replay) and is **never
//! re-implemented**. Real Docker wiring is gated behind Phase 3 (a provisioned
//! VM). For offline development, tests, and CI, a [`MockHarnessExecutor`] returns
//! deterministic verdicts from a ground-truth lookup table — a test double, not
//! a reachability engine.

use std::collections::{HashMap, HashSet};

use super::types::{CoinGymError, CoinGymResult, OutcomeCode, Target};

/// The oracle's verdict for a single submitted input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GradeResult {
    /// The input drove execution to the target line.
    Reached,
    /// The input was valid but did not reach the target line.
    WrongInput,
    /// Grading exceeded the per-target budget.
    TimedOut,
    /// Grading errored (harness crash, build failure, etc.).
    Error,
}

impl GradeResult {
    /// Map a grade of a *submitted* input to its outcome code.
    #[must_use]
    pub fn to_outcome_code(self) -> OutcomeCode {
        match self {
            Self::Reached => OutcomeCode::Reached,
            Self::WrongInput => OutcomeCode::WrongInput,
            Self::TimedOut => OutcomeCode::TimedOut,
            Self::Error => OutcomeCode::Error,
        }
    }
}

/// Grades a submitted input against a target. Implementors are the objective
/// oracle; they must not be spoofed by the agent under test.
pub trait HarnessExecutor {
    /// Grade `input` against `target`.
    ///
    /// # Errors
    /// Returns [`CoinGymError::Executor`] when grading cannot be performed at all
    /// (e.g. `coin evaluate` is unavailable because the Phase-3 VM is not
    /// provisioned). A *reached/not-reached* verdict is returned as
    /// `Ok(GradeResult)`, not an error.
    fn grade(&self, target: &Target, input: &str) -> CoinGymResult<GradeResult>;

    /// Whether this executor is an offline scaffold (mock) rather than the real
    /// `coin evaluate` oracle. Recorded on the [`crate::coin_gym::types::RunReport`]
    /// so offline runs are never mistaken for graded results.
    fn is_offline_scaffold(&self) -> bool {
        false
    }
}

// ── Real executor: delegates to `coin evaluate` (Phase 3 gated) ───────────────

/// Configuration for the real `coin evaluate` delegate.
#[derive(Clone, Debug)]
pub struct CoinEvaluateConfig {
    /// Path/name of the `coin` binary (default `coin`).
    pub binary: String,
    /// Snapshot dataset reference (e.g. `you/coin@v1`).
    pub dataset: String,
}

impl CoinEvaluateConfig {
    /// Build a config for a snapshot using the default `coin` binary.
    #[must_use]
    pub fn new(dataset: impl Into<String>) -> Self {
        Self {
            binary: "coin".to_string(),
            dataset: dataset.into(),
        }
    }
}

/// Delegates grading to the COIN maintainer harness via `coin evaluate`.
///
/// This is the production oracle path. It is intentionally a thin shell around
/// the external tool — the reachability judgement lives entirely in
/// `coin evaluate`'s instrumented replay, never here. Actually invoking it
/// requires Docker + a pulled snapshot (Phase 3); until then [`Self::grade`]
/// surfaces a clear Phase-3 gate error while [`Self::build_argv`] (the exact
/// delegation contract) stays unit-testable offline.
#[derive(Clone, Debug)]
pub struct CoinEvaluateExecutor {
    config: CoinEvaluateConfig,
}

impl CoinEvaluateExecutor {
    /// Create the delegate from a config.
    #[must_use]
    pub fn new(config: CoinEvaluateConfig) -> Self {
        Self { config }
    }

    /// The argv this executor would pass to the `coin` binary to grade a target,
    /// with the candidate input already staged at `input_path`.
    ///
    /// Exposed (and unit-tested) so the delegation contract is verified without a
    /// Docker host. The reachability oracle remains `coin evaluate` itself.
    #[must_use]
    pub fn build_argv(&self, target: &Target, input_path: &str) -> Vec<String> {
        vec![
            self.config.binary.clone(),
            "evaluate".to_string(),
            "--dataset".to_string(),
            self.config.dataset.clone(),
            "--target".to_string(),
            target.id.clone(),
            "--input".to_string(),
            input_path.to_string(),
        ]
    }
}

impl HarnessExecutor for CoinEvaluateExecutor {
    fn grade(&self, _target: &Target, _input: &str) -> CoinGymResult<GradeResult> {
        // Phase 3 gate: real grading needs `coin evaluate` on a Docker host with
        // a pulled snapshot. Surfacing an explicit error (rather than a silent
        // fake verdict) keeps the harness honest — an offline run must use
        // MockHarnessExecutor and be flagged `offline_scaffold`.
        Err(CoinGymError::Executor(format!(
            "`{} evaluate` requires a Docker host + pulled snapshot (Phase 3, azlin VM); \
             use the offline MockHarnessExecutor for local scaffold runs",
            self.config.binary
        )))
    }
}

// ── Mock executor: deterministic test double ─────────────────────────────────

/// A deterministic oracle test double backed by a ground-truth lookup table.
///
/// It does **not** compute reachability — it simply checks the submitted input
/// against a known reaching input per target (and optional injected
/// timeout/error sets). This lets the full pipeline run offline without a VM
/// while keeping the real oracle (`coin evaluate`) the only thing that ever
/// judges reachability for real.
#[derive(Clone, Debug, Default)]
pub struct MockHarnessExecutor {
    reaching_input: HashMap<String, String>,
    timeout_ids: HashSet<String>,
    error_ids: HashSet<String>,
}

impl MockHarnessExecutor {
    /// An empty mock (every submission grades as `WrongInput`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a mock from a `target_id -> reaching_input` ground-truth map.
    #[must_use]
    pub fn from_oracle(reaching_input: HashMap<String, String>) -> Self {
        Self {
            reaching_input,
            ..Self::default()
        }
    }

    /// Register a reaching input for a target.
    #[must_use]
    pub fn with_reaching_input(
        mut self,
        target_id: impl Into<String>,
        input: impl Into<String>,
    ) -> Self {
        self.reaching_input.insert(target_id.into(), input.into());
        self
    }

    /// Force a target to grade as `TimedOut` regardless of input.
    #[must_use]
    pub fn with_timeout(mut self, target_id: impl Into<String>) -> Self {
        self.timeout_ids.insert(target_id.into());
        self
    }

    /// Force a target to grade as `Error` regardless of input.
    #[must_use]
    pub fn with_error(mut self, target_id: impl Into<String>) -> Self {
        self.error_ids.insert(target_id.into());
        self
    }
}

impl HarnessExecutor for MockHarnessExecutor {
    fn grade(&self, target: &Target, input: &str) -> CoinGymResult<GradeResult> {
        if self.error_ids.contains(&target.id) {
            return Ok(GradeResult::Error);
        }
        if self.timeout_ids.contains(&target.id) {
            return Ok(GradeResult::TimedOut);
        }
        match self.reaching_input.get(&target.id) {
            Some(expected) if expected == input => Ok(GradeResult::Reached),
            _ => Ok(GradeResult::WrongInput),
        }
    }

    fn is_offline_scaffold(&self) -> bool {
        true
    }
}
