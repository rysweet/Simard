//! Self-improvement scaffold (research doc Part 3.3, component 6).
//!
//! This is the **Phase-4** slice of the loop: the pure, offline, fully-testable
//! pieces — a **failure-analyst** that turns unreached targets into *general*
//! reachability tactics, and the **overfitting-reviewer GATE** that rejects any
//! tactic which memorises a specific input or keys off a specific target /
//! project. Both mirror skwaq's `failure-analyst` + `overfitting-reviewer`.
//!
//! The **live** part of the loop — apply an accepted tactic, re-run on held-out
//! *fresh* targets, and keep it iff reach improves without a precision
//! regression (else roll back), plus durable tactic memory — is **Phase 5**
//! (issue #2825) and lives in [`super::improve_loop`]. It composes the
//! analyst + gate defined here.

use serde::{Deserialize, Serialize};

use super::target_loader::TargetSet;
use super::types::{OutcomeCode, RunReport, Target, TargetFamily};

/// A general reachability tactic proposed from a failing target.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TacticProposal {
    /// Stable proposal id.
    pub id: String,
    /// The target whose failure motivated the proposal (diagnosis context).
    pub target_id: String,
    /// The proposed **general** tactic (must generalise across projects/harnesses).
    pub tactic: String,
    /// Evidence for the diagnosis (may be target-specific; the *tactic* may not).
    pub evidence: String,
}

/// The overfitting-reviewer's verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewVerdict {
    /// The tactic generalises — keep it.
    Accept,
    /// The tactic memorises a specific input / keys off a specific target — drop it.
    Reject,
}

/// A proposal after review.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReviewedProposal {
    /// The reviewed proposal.
    pub proposal: TacticProposal,
    /// Accept/Reject.
    pub verdict: ReviewVerdict,
    /// Why the reviewer decided this way.
    pub reason: String,
}

impl ReviewedProposal {
    /// Uppercase label for the verdict (`ACCEPT` / `REJECT`).
    #[must_use]
    pub fn verdict_label(&self) -> &'static str {
        match self.verdict {
            ReviewVerdict::Accept => "ACCEPT",
            ReviewVerdict::Reject => "REJECT",
        }
    }
}

/// The result of one offline analysis cycle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImproveReport {
    /// The run analysed.
    pub run_id: String,
    /// Number of unreached targets analysed.
    pub analyzed: usize,
    /// Reviewed proposals (accepted + rejected).
    pub proposals: Vec<ReviewedProposal>,
    /// Count accepted by the gate.
    pub accepted: usize,
    /// Count rejected by the gate.
    pub rejected: usize,
    /// Phase boundary note (live verify/rollback is Phase 5).
    pub note: String,
}

/// Broad heuristic class a target falls into, used to pick a *general* tactic.
///
/// The class is a **general** target family (format-gated decoder,
/// cryptographic state machine, or a generic guard) — never a specific project
/// or target id — so tactics keyed to it stay generalisable and durable-memory
/// entries are shared across projects. Exposed to the live self-improvement loop
/// ([`super::improve_loop`]) which keys accepted tactics by this category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TacticCategory {
    FormatGatedDecoder,
    CryptoStateMachine,
    Generic,
}

impl TacticCategory {
    /// Stable kebab-case label used in reports, memory keys, and CLI output.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FormatGatedDecoder => "format-gated-decoder",
            Self::CryptoStateMachine => "crypto-state-machine",
            Self::Generic => "generic",
        }
    }

    /// Parse a [`Self::label`] back into a category (used to rehydrate durable
    /// tactic memory). Returns `None` for an unrecognised label.
    pub(crate) fn from_label(label: &str) -> Option<Self> {
        match label {
            "format-gated-decoder" => Some(Self::FormatGatedDecoder),
            "crypto-state-machine" => Some(Self::CryptoStateMachine),
            "generic" => Some(Self::Generic),
            _ => None,
        }
    }
}

pub(crate) fn categorize(target: &Target) -> TacticCategory {
    let hay = format!(
        "{} {} {}",
        target.project.to_ascii_lowercase(),
        target.harness.to_ascii_lowercase(),
        target.file.to_ascii_lowercase()
    );
    let is_decoder = [
        "raw",
        "png",
        "jpeg",
        "zstd",
        "decompress",
        "decode",
        "image",
        "font",
        "shape",
        "harfbuzz",
        "metadata",
    ]
    .iter()
    .any(|k| hay.contains(k));
    let is_crypto = ["oqs", "kem", "asn1", "ssl", "crypto", "cipher", "tls"]
        .iter()
        .any(|k| hay.contains(k));
    if is_crypto {
        TacticCategory::CryptoStateMachine
    } else if is_decoder {
        TacticCategory::FormatGatedDecoder
    } else {
        TacticCategory::Generic
    }
}

pub(crate) fn tactic_text(category: TacticCategory) -> &'static str {
    match category {
        TacticCategory::FormatGatedDecoder => {
            "For format-gated decoders, first satisfy the container's magic-byte / header \
             validator, then construct the minimal well-formed structure that routes control \
             flow into the guarded deep branch."
        }
        TacticCategory::CryptoStateMachine => {
            "For cryptographic state machines, drive the protocol into the specific state that \
             unlocks the target branch (e.g. a decapsulation or parse-error path) rather than \
             fuzzing random bytes."
        }
        TacticCategory::Generic => {
            "Work backward from the target line's guarding predicate to the input constraints \
             that satisfy it, prioritising branch conditions over surface-level fuzzing."
        }
    }
}

/// Failure-analyst: turn each **unreached** target (`W`/`T`/`N`) into a general
/// reachability tactic proposal. Abstentions (`A`) are deliberate no-claims and
/// are not treated as failures. Reached (`R`) targets need no tactic.
#[must_use]
pub fn analyze_failures(report: &RunReport, targets: &TargetSet) -> Vec<TacticProposal> {
    let all: Vec<&Target> = targets
        .pinned
        .iter()
        .chain(targets.held_out_fresh.iter())
        .collect();
    let mut proposals = Vec::new();
    for outcome in &report.outcomes {
        let is_failure = matches!(
            outcome.code,
            OutcomeCode::WrongInput | OutcomeCode::TimedOut | OutcomeCode::NoSubmission
        );
        if !is_failure {
            continue;
        }
        let Some(target) = all.iter().find(|t| t.id == outcome.target_id) else {
            continue;
        };
        let category = categorize(target);
        proposals.push(TacticProposal {
            id: format!("tactic-{}", outcome.target_id),
            target_id: outcome.target_id.clone(),
            tactic: tactic_text(category).to_string(),
            evidence: format!(
                "target {} ({}) ended {}; the guarding predicate at line {} was not satisfied",
                target.locator(),
                family_label(target.family),
                outcome.code.letter(),
                target.line
            ),
        });
    }
    proposals
}

fn family_label(family: TargetFamily) -> &'static str {
    family.label()
}

/// Overfitting-reviewer GATE. Rejects a tactic that memorises a specific input
/// or keys off a specific target / project / locator; accepts tactics that
/// plausibly generalise. Mirrors skwaq rejecting benchmark-specific naming.
///
/// The `targets` set supplies the concrete ids / project names / locators the
/// tactic must **not** encode.
#[must_use]
pub fn review_proposal(proposal: &TacticProposal, targets: &TargetSet) -> ReviewedProposal {
    let tactic_lc = proposal.tactic.to_ascii_lowercase();

    // 1. Explicit memorisation language.
    const OVERFIT_PHRASES: &[&str] = &[
        "memorize",
        "memorise",
        "hardcode",
        "hard-code",
        "hard code",
        "specific input",
        "exact bytes",
        "exact input",
        "this input",
        "known input",
        "cached input",
    ];
    if let Some(phrase) = OVERFIT_PHRASES.iter().find(|p| tactic_lc.contains(**p)) {
        return reject(
            proposal,
            format!("tactic encodes a memorised input (\"{phrase}\")"),
        );
    }

    // 2. Keys off a specific target id or locator.
    for target in targets.pinned.iter().chain(targets.held_out_fresh.iter()) {
        if tactic_lc.contains(&target.id.to_ascii_lowercase()) {
            return reject(
                proposal,
                format!("tactic names a specific target id '{}'", target.id),
            );
        }
        let locator = target.locator().to_ascii_lowercase();
        if tactic_lc.contains(&locator) {
            return reject(
                proposal,
                format!(
                    "tactic names a specific target locator '{}'",
                    target.locator()
                ),
            );
        }
        // Keying off a concrete project name is target-specific.
        let project = target.project.to_ascii_lowercase();
        if project.len() >= 4 && tactic_lc.contains(&project) {
            return reject(
                proposal,
                format!("tactic keys off a specific project '{}'", target.project),
            );
        }
    }

    ReviewedProposal {
        proposal: proposal.clone(),
        verdict: ReviewVerdict::Accept,
        reason: "tactic generalises across projects/harnesses; no memorised input or \
                 target-specific key detected"
            .to_string(),
    }
}

fn reject(proposal: &TacticProposal, reason: String) -> ReviewedProposal {
    ReviewedProposal {
        proposal: proposal.clone(),
        verdict: ReviewVerdict::Reject,
        reason,
    }
}

/// Run one **offline** analysis cycle: failure-analysis → overfitting-review.
/// This does NOT apply, verify, or roll back tactics — that live loop lives in
/// [`super::improve_loop`] (`improve --holdout fresh`, Phase 5, issue #2825),
/// which composes this analyst + gate with held-out verification.
#[must_use]
pub fn analyze_and_review(report: &RunReport, targets: &TargetSet) -> ImproveReport {
    let reviewed: Vec<ReviewedProposal> = analyze_failures(report, targets)
        .iter()
        .map(|p| review_proposal(p, targets))
        .collect();
    let accepted = reviewed
        .iter()
        .filter(|r| r.verdict == ReviewVerdict::Accept)
        .count();
    let rejected = reviewed.len() - accepted;
    ImproveReport {
        run_id: report.run_id.clone(),
        analyzed: reviewed.len(),
        proposals: reviewed,
        accepted,
        rejected,
        note: "offline analysis only — applying an accepted tactic, re-running on held-out \
               fresh targets, and keeping it iff reach improves without a precision regression \
               (else roll back) is the live loop `improve --holdout fresh` (Phase 5, issue \
               #2825); real `coin evaluate` grading still needs the Phase-3 VM (issue #2823)"
            .to_string(),
    }
}
