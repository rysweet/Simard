//! Core data model for the LOCAL COIN Gym harness (Phase 4).
//!
//! COIN (COde → INput) grades *reachability by execution*: an agent produces a
//! concrete input that must drive a maintainer harness to a chosen target line,
//! verified by re-running the harness on a coverage-instrumented build. See
//! `docs/research/coin-benchmark-and-skwaq-study.md` for the full design this
//! module scaffolds.
//!
//! These types are deliberately serde-friendly, self-contained, and free of any
//! Docker/VM/LLM dependency so the whole pipeline can be exercised offline
//! against a mock oracle (see [`crate::coin_gym::executor::MockHarnessExecutor`]).

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// The two COIN target families sampled from the signal algebra (research doc
/// Part 1.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetFamily {
    /// `(B ∪ C) \ (G ∪ F ∪ L)` — provably reachable by static analysis but never
    /// reached by any fuzzer or cheap baseline. Hardest, most
    /// contamination-resistant class.
    Frontier,
    /// `G \ (F ∪ L)` — reached by long-running fuzzing but missed by fresh
    /// fuzzers and goal-blind LLM seeds.
    NonTrivialReachable,
}

impl TargetFamily {
    /// Stable kebab-case label used in reports and CLI output.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Frontier => "frontier",
            Self::NonTrivialReachable => "non-trivial-reachable",
        }
    }
}

impl Display for TargetFamily {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A single COIN task: a target line `ℓ` in a real project at a pinned commit,
/// reached through one maintainer-written harness (research doc Part 1.2).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Stable identifier for the target within a snapshot.
    pub id: String,
    /// Upstream project name (e.g. `libraw`).
    pub project: String,
    /// Pinned commit the target line refers to.
    pub commit: String,
    /// Maintainer-written harness the agent must drive.
    pub harness: String,
    /// Source file containing the target line.
    pub file: String,
    /// 1-based target line number `ℓ`.
    pub line: u32,
    /// Target family (frontier vs. non-trivial reachable).
    pub family: TargetFamily,
}

impl Target {
    /// `project:file:line` — a human-readable locator for logs and reports.
    #[must_use]
    pub fn locator(&self) -> String {
        format!("{}:{}:{}", self.project, self.file, self.line)
    }
}

/// The strategy under test — a single-model baseline vs. a multi-agent team
/// (research doc Part 3.3, component 2).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    /// One model reads the harness + source and emits candidate input bytes.
    Baseline,
    /// A skwaq-style debate: a *reacher* proposes an input, a *skeptic* challenges
    /// over-claims, and a *synthesizer* submits-or-abstains via a
    /// `threshold_hint`-style gate.
    Team,
}

impl Strategy {
    /// Stable kebab-case label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Team => "team",
        }
    }

    /// Parse a CLI `--strategy` value.
    ///
    /// # Errors
    /// Returns an error string for unrecognised values.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "baseline" => Ok(Self::Baseline),
            "team" => Ok(Self::Team),
            other => Err(format!(
                "unknown strategy '{other}' (expected 'baseline' or 'team')"
            )),
        }
    }
}

impl Display for Strategy {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Per-target outcome code from the results matrix (research doc Part 1.4).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum OutcomeCode {
    /// `R` — Reached ✓ (verified by harness replay on the instrumented build).
    Reached,
    /// `W` — Submitted a wrong input (did not reach the line).
    WrongInput,
    /// `A` — Abstained (deliberately declined to submit).
    Abstained,
    /// `T` — Timed out.
    TimedOut,
    /// `N` — No submission (the agent produced nothing).
    NoSubmission,
    /// `E` — Error during evaluation.
    Error,
}

impl OutcomeCode {
    /// Single-letter code (`R`/`W`/`A`/`T`/`N`/`E`).
    #[must_use]
    pub fn letter(self) -> char {
        match self {
            Self::Reached => 'R',
            Self::WrongInput => 'W',
            Self::Abstained => 'A',
            Self::TimedOut => 'T',
            Self::NoSubmission => 'N',
            Self::Error => 'E',
        }
    }

    /// Whether this outcome reached the target line.
    #[must_use]
    pub fn reached(self) -> bool {
        matches!(self, Self::Reached)
    }

    /// Whether the agent *submitted* an input (counts toward precision's
    /// denominator). Abstain / no-submission do not.
    #[must_use]
    pub fn submitted(self) -> bool {
        matches!(self, Self::Reached | Self::WrongInput | Self::TimedOut)
    }
}

impl Display for OutcomeCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.letter())
    }
}

/// The result of grading one target: the objective oracle's verdict for a single
/// (target, submission) pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// The target this outcome is for.
    pub target_id: String,
    /// Family of the target (denormalised so scoring needs only outcomes).
    pub family: TargetFamily,
    /// The outcome code.
    pub code: OutcomeCode,
    /// Estimated USD cost of producing/grading this target (0.0 when unknown).
    pub cost_usd: f64,
}

impl Outcome {
    /// Whether the target line was reached.
    #[must_use]
    pub fn reached(&self) -> bool {
        self.code.reached()
    }

    /// Whether an input was submitted (precision denominator membership).
    #[must_use]
    pub fn submitted(&self) -> bool {
        self.code.submitted()
    }
}

/// A complete evaluation pass over a target set for one (model, strategy).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunReport {
    /// Unique run identifier (`<model>-<strategy>-<unix_ms>`).
    pub run_id: String,
    /// Model under test (LiteLLM model id, e.g. `claude-opus-4.6`).
    pub model: String,
    /// Strategy under test.
    pub strategy: Strategy,
    /// Snapshot the targets were drawn from (e.g. `you/coin@v1`).
    pub snapshot: String,
    /// Wall-clock start time (unix epoch milliseconds).
    pub started_at_unix_ms: u128,
    /// Per-target outcomes, one per evaluated target.
    pub outcomes: Vec<Outcome>,
    /// `true` when the outcomes came from a mock oracle (offline scaffold run)
    /// rather than a real `coin evaluate` grade. Real grading is gated behind
    /// Phase 3 (VM + Docker).
    pub offline_scaffold: bool,
}

impl RunReport {
    /// Number of targets in the run.
    #[must_use]
    pub fn target_count(&self) -> usize {
        self.outcomes.len()
    }
}

/// Errors surfaced by the COIN Gym harness. Kept small and `Display`-able so it
/// composes cleanly into the CLI's `Box<dyn std::error::Error>`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CoinGymError {
    /// A snapshot / report / fixture failed to parse.
    Parse(String),
    /// A filesystem operation failed.
    Io(String),
    /// A referenced entity (run, profile, target) was not found.
    NotFound(String),
    /// The harness executor (or its gated `coin evaluate` delegate) failed.
    Executor(String),
    /// A CLI argument was invalid.
    Usage(String),
}

impl Display for CoinGymError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(m) => write!(f, "parse error: {m}"),
            Self::Io(m) => write!(f, "io error: {m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::Executor(m) => write!(f, "executor error: {m}"),
            Self::Usage(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CoinGymError {}

/// Convenience result alias for the module.
pub type CoinGymResult<T> = Result<T, CoinGymError>;
