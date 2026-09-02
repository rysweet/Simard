//! Root-cause classification of a stalled OODA goal (issue #16).
//!
//! # Why this exists
//!
//! The no-progress breaker (`super::no_progress_breaker`) historically parked a
//! stalled goal with a **bare** `GoalProgress::Blocked` reason — "…needs human
//! review" — that stated *what* (three idle cycles) but never *why*. The
//! production incident: seven `kgpacks-rs` goals were parked as "no progress"
//! when the work was **already done** (referenced issues `CLOSED`, workstream PRs
//! `MERGED`); the brain kept returning `NO ACTION` because there was nothing left
//! to do, but nothing marked the goals `Completed`, so the safeguard misread
//! *done* as *stuck*.
//!
//! This module names the small, stable vocabulary of **root causes**
//! ([`NoProgressClass`]) and the structured [`Evidence`] gathered for one, so the
//! breaker can route each stall down a self-resolving ladder (see
//! [`super::no_progress_breaker::resolution_for_why`]) and, when a human block is
//! genuinely unavoidable, author a reason that carries the concrete WHY +
//! evidence rather than a bare sentinel.
//!
//! The types here are **pure** (no I/O). The investigation that produces a
//! [`NoProgressWhy`] is injected as a [`NoProgressWhyReasoner`]; its
//! side-effecting production implementation and the resolution driver live in the
//! curate-phase adapter `crate::ooda_loop::no_progress`.
//!
//! See `docs/concepts/no-progress-root-cause-resolution.md`.

use crate::error::SimardResult;

use super::types::ActiveGoal;

/// Stable classification tokens. These travel verbatim into block reasons,
/// tracking issues, logs, and metrics — like the goal-edge type strings they are
/// a durable cross-system contract and must never drift.
pub const CLASS_ALREADY_COMPLETE: &str = "ALREADY-COMPLETE";
/// Token for [`NoProgressClass::Obsolete`].
pub const CLASS_OBSOLETE: &str = "OBSOLETE";
/// Token for [`NoProgressClass::MissingPrecondition`].
pub const CLASS_MISSING_PRECONDITION: &str = "MISSING-PRECONDITION";
/// Token for [`NoProgressClass::UpstreamDependency`].
pub const CLASS_UPSTREAM_DEPENDENCY: &str = "UPSTREAM-DEPENDENCY";
/// Token for [`NoProgressClass::UnclearCriteria`].
pub const CLASS_UNCLEAR_CRITERIA: &str = "UNCLEAR-CRITERIA";
/// Token for [`NoProgressClass::GenuinelyStuck`].
pub const CLASS_GENUINELY_STUCK: &str = "GENUINELY-STUCK";

/// The root-cause classification of *why* a goal reached the breaker threshold.
///
/// Each variant maps to exactly one rung of the resolution ladder in
/// [`super::no_progress_breaker::resolution_for_why`]; only the last two ever
/// reach a human, and then only after a bounded guided-engineer retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoProgressClass {
    /// Live artifacts satisfy the done-criteria (referenced issues `CLOSED` /
    /// PRs `MERGED` / self-change deployed). Routes to auto-complete — the direct
    /// fix for the `kgpacks-rs` "already done" incident.
    AlreadyComplete,
    /// The goal's work is tracked elsewhere / out of scope. Routes to drop.
    Obsolete,
    /// A machine-establishable precondition is absent (e.g. a governed repo was
    /// never cloned). Routes to self-heal (clone) + retry.
    MissingPrecondition,
    /// Blocked on a specific upstream goal / PR / issue that has not landed.
    /// Routes to defer (`Paused`) with the blocking ref recorded; auto-clears.
    UpstreamDependency,
    /// The done-criteria are not expressed as anything the done-gate can check,
    /// so it can never certify. Routes to a guided engineer, then a human.
    UnclearCriteria,
    /// No machine-resolvable cause found. Routes to a guided engineer, then a
    /// human — always with the WHY + evidence attached.
    GenuinelyStuck,
}

impl NoProgressClass {
    /// Every classification, in ladder order. Lets callers/tests enumerate the
    /// stable set without hard-coding it.
    pub const ALL: [NoProgressClass; 6] = [
        NoProgressClass::AlreadyComplete,
        NoProgressClass::Obsolete,
        NoProgressClass::MissingPrecondition,
        NoProgressClass::UpstreamDependency,
        NoProgressClass::UnclearCriteria,
        NoProgressClass::GenuinelyStuck,
    ];

    /// The stable screaming-kebab token for this class.
    pub fn token(&self) -> &'static str {
        match self {
            NoProgressClass::AlreadyComplete => CLASS_ALREADY_COMPLETE,
            NoProgressClass::Obsolete => CLASS_OBSOLETE,
            NoProgressClass::MissingPrecondition => CLASS_MISSING_PRECONDITION,
            NoProgressClass::UpstreamDependency => CLASS_UPSTREAM_DEPENDENCY,
            NoProgressClass::UnclearCriteria => CLASS_UNCLEAR_CRITERIA,
            NoProgressClass::GenuinelyStuck => CLASS_GENUINELY_STUCK,
        }
    }
}

/// One piece of structured evidence supporting a classification — a live
/// artifact reference and its observed state, e.g. `issue #16 (CLOSED)` or
/// `pr #33 (MERGED)` or `repo kgpacks-rs (absent)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    /// The evidence kind, e.g. `"issue"`, `"pr"`, `"repo"`, `"dependency"`.
    pub kind: String,
    /// The artifact reference, e.g. `"#16"`, `"kgpacks-rs"`, an upstream goal id.
    pub reference: String,
    /// The observed state, e.g. `"CLOSED"`, `"MERGED"`, `"absent"`, `"OPEN"`.
    pub state: String,
}

impl Evidence {
    /// Construct one piece of evidence.
    pub fn new(
        kind: impl Into<String>,
        reference: impl Into<String>,
        state: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            reference: reference.into(),
            state: state.into(),
        }
    }

    /// A human-readable reference, e.g. `issue #16 (CLOSED)`.
    pub fn render(&self) -> String {
        format!("{} {} ({})", self.kind, self.reference, self.state)
    }
}

/// A classified WHY plus the evidence behind it. Produced by a
/// [`NoProgressWhyReasoner`]; consumed by
/// [`super::no_progress_breaker::resolution_for_why`] to pick the ladder rung and
/// by [`super::no_progress_breaker::no_progress_blocked_reason_with_why`] to
/// author a WHY-bearing block reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoProgressWhy {
    /// The classified root cause.
    pub class: NoProgressClass,
    /// The structured evidence gathered for the classification.
    pub evidence: Vec<Evidence>,
}

impl NoProgressWhy {
    /// Build a WHY from a class and its evidence.
    pub fn new(class: NoProgressClass, evidence: Vec<Evidence>) -> Self {
        Self { class, evidence }
    }

    /// Render the evidence as a comma-joined list, or the explicit `(none)`
    /// sentinel when empty — never an empty string, so an escalation reason
    /// always reads coherently.
    pub fn render_evidence(&self) -> String {
        if self.evidence.is_empty() {
            "(none)".to_string()
        } else {
            self.evidence
                .iter()
                .map(Evidence::render)
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    /// The single most-specific blocking reference for an
    /// [`NoProgressClass::UpstreamDependency`] defer: the first evidence's
    /// `reference`, falling back to its full render, then to a sentinel.
    pub fn blocking_ref(&self) -> String {
        match self.evidence.first() {
            Some(e) if !e.reference.trim().is_empty() => e.reference.clone(),
            Some(e) => e.render(),
            None => "unknown-upstream".to_string(),
        }
    }
}

/// The investigation seam: given a stalled goal, classify *why* it made no
/// shippable progress and gather the evidence.
///
/// Injected so the breaker's investigation is exercised hermetically (tests
/// inject a fake) and so the production classifier stays decoupled from the
/// curate-phase adapter. On `Err` the caller **fails closed** — it takes no
/// terminal action (neither blocks nor completes), surfaces the error, and lets
/// the goal retry next cycle — never silently swallowing an unknown root cause
/// into a spurious block or completion.
pub trait NoProgressWhyReasoner: Send + Sync {
    /// Investigate `goal` and return its classified WHY + evidence.
    fn investigate(&self, goal: &ActiveGoal) -> SimardResult<NoProgressWhy>;
}
