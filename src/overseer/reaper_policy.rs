//! Pure, fail-closed, **tighten-only** PR-reaper policy (issue #4).
//!
//! ROOT CAUSE this layer addresses: a large stale / `CONFLICTING` / near-duplicate
//! open-PR backlog accumulates with no OODA reaper cleaning it up. The fix adds a
//! PURE, deterministic post-parse layer that sits between the agentic reviewer's
//! per-PR [`PrDisposition`](crate::overseer::capabilities::PrDisposition) proposal
//! and the [`AutonomyGate`](crate::overseer::guardrails::AutonomyGate). It can only
//! ever **tighten** a proposal — never escalate one:
//!
//! * a `Stale` proposal becomes at most a non-destructive [`ReaperDecision::Flag`],
//! * a `Duplicate` proposal becomes a [`ReaperDecision::CloseDuplicate`] **only**
//!   when there is real changed-file overlap, the candidate is the later
//!   (higher-numbered) PR of the pair, its mergeable state is known-good, AND the
//!   destructive gate (`allow_verify_merge`) is explicitly open,
//! * everything else collapses to [`ReaperDecision::NoAction`].
//!
//! It performs **no I/O** and issues no `gh` command; the destructive close still
//! flows through the existing MergeAuthority-gated
//! [`Intervention::CloseDuplicatePr`](crate::overseer::intervention::Intervention)
//! whose argv is built by
//! [`close_duplicate_pr_argv`](crate::overseer::intervention::close_duplicate_pr_argv)
//! (positional, never `--admin`/`--no-verify`). Survivor selection is
//! griefing-resistant: the **lowest-numbered** (earliest) PR always survives, so an
//! attacker who opens a near-duplicate *after* a legitimate PR can never close the
//! legitimate one.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::overseer::capabilities::PrDisposition;

/// The merge state of an open PR, as observed. `Unknown` is fail-closed: it can
/// never be the basis for a destructive close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeableState {
    /// GitHub reports the PR is cleanly mergeable.
    Mergeable,
    /// GitHub reports the PR conflicts with its base.
    Conflicting,
    /// The mergeable state could not be determined (still computing, error, or
    /// absent). Treated conservatively — never closable.
    Unknown,
}

/// The deterministic facts about one open PR the reaper reasons over. Carries no
/// behaviour; built by the observe stage and handed to [`evaluate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrFacts {
    /// `owner/name` slug.
    pub repo: String,
    /// PR number (the griefing-resistant survivor tiebreak: lowest survives).
    pub number: u32,
    /// Last update timestamp. `None` is fail-closed — no age ⇒ no stale flag.
    pub updated_at: Option<DateTime<Utc>>,
    /// Observed mergeable state.
    pub mergeable: MergeableState,
    /// The normalized (lowercased, stopwords-dropped) title used for similarity.
    pub normalized_title: String,
    /// The set of changed file paths. Real overlap here is REQUIRED before any
    /// duplicate close — title similarity alone is never sufficient.
    pub changed_files: BTreeSet<String>,
    /// The PR this one is proposed to duplicate (advisory; survivor is still
    /// re-derived independently by lowest number).
    pub duplicate_of: Option<u32>,
}

/// Conservative, operator-tunable thresholds. Resolved (with clamps) by
/// [`reaper_thresholds_from`](crate::overseer::config::reaper_thresholds_from).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReaperThresholds {
    /// Days with no update before a PR is eligible for a `StaleNoUpdate` flag.
    pub stale_days: i64,
    /// Days a PR may be `CONFLICTING` before a `LongConflicting` flag.
    pub conflicting_days: i64,
    /// Minimum normalized-title similarity (`0.0..=1.0`) for a duplicate close.
    pub similarity: f64,
}

/// Why a PR was flagged (non-destructive). Every variant is notify-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagStaleReason {
    /// Untouched past the stale window.
    StaleNoUpdate,
    /// `CONFLICTING` past the conflicting window (but still inside the stale one).
    LongConflicting,
    /// A genuine duplicate whose destructive gate is closed — downgraded from a
    /// close to a flag so nothing is closed unattended.
    DuplicateNotClosable,
}

/// Why a duplicate close was proposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuplicateReason {
    /// Both a title-similarity match AND a changed-file overlap were present.
    TitleAndFileOverlap,
}

/// The reaper's tightened disposition for one PR. Only [`Self::CloseDuplicate`]
/// is destructive, and it is emitted solely when the destructive gate is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaperDecision {
    /// Do nothing (the safe default for anything that fails a rail).
    NoAction,
    /// Post a non-destructive flag/comment for the given reason.
    Flag(FlagStaleReason),
    /// Propose closing `number` as a duplicate of the surviving `survivor`.
    CloseDuplicate {
        /// The (later, higher-numbered) PR to close.
        number: u32,
        /// The (earlier, lowest-numbered) PR that survives.
        survivor: u32,
        /// The evidence that justified the close.
        reason: DuplicateReason,
    },
}

/// Evaluate one agentic PR proposal into a tightened, fail-closed reaper decision.
///
/// * `proposed` — the reviewer's per-PR disposition. Only `Stale`/`Duplicate` are
///   reaper dispositions; `ReadyForMerge`/`NeedsWork` collapse to `NoAction`.
/// * `facts` — the candidate PR under evaluation.
/// * `peers` — other open PRs (used to find a duplicate cluster + survivor).
/// * `thresholds` — the conservative, clamped windows/similarity.
/// * `now` — the evaluation instant (injected; no clock read here).
/// * `destructive_allowed` — the `allow_verify_merge` gate. When `false` (the
///   default dry-run/notify-only posture) no `CloseDuplicate` is ever emitted.
///
/// Pure and deterministic: no I/O, no `gh`, no global state.
pub fn evaluate(
    proposed: PrDisposition,
    facts: &PrFacts,
    peers: &[PrFacts],
    thresholds: &ReaperThresholds,
    now: DateTime<Utc>,
    destructive_allowed: bool,
) -> ReaperDecision {
    match proposed {
        // Non-reaper dispositions never produce a reaper action.
        PrDisposition::ReadyForMerge | PrDisposition::NeedsWork => ReaperDecision::NoAction,
        PrDisposition::Stale => evaluate_stale(facts, thresholds, now),
        PrDisposition::Duplicate => {
            evaluate_duplicate(facts, peers, thresholds, destructive_allowed)
        }
    }
}

/// Stale/long-conflicting flagging. Fail-closed on a missing timestamp (no age ⇒
/// no flag). `StaleNoUpdate` (past the wider stale window) takes precedence over
/// `LongConflicting` (past the narrower conflicting window).
fn evaluate_stale(
    facts: &PrFacts,
    thresholds: &ReaperThresholds,
    now: DateTime<Utc>,
) -> ReaperDecision {
    let Some(updated_at) = facts.updated_at else {
        // Fail-closed: without an age we cannot assert staleness.
        return ReaperDecision::NoAction;
    };
    let age_days = (now - updated_at).num_days();
    if age_days > thresholds.stale_days {
        ReaperDecision::Flag(FlagStaleReason::StaleNoUpdate)
    } else if facts.mergeable == MergeableState::Conflicting
        && age_days > thresholds.conflicting_days
    {
        ReaperDecision::Flag(FlagStaleReason::LongConflicting)
    } else {
        ReaperDecision::NoAction
    }
}

/// Duplicate handling. Requires BOTH title similarity ≥ threshold AND real
/// changed-file overlap to form a cluster; the survivor is the lowest-numbered
/// PR in that cluster. Only the later candidate is ever closed, only when its
/// mergeable state is known-good, and only when the destructive gate is open —
/// otherwise it downgrades to a `DuplicateNotClosable` flag.
fn evaluate_duplicate(
    facts: &PrFacts,
    peers: &[PrFacts],
    thresholds: &ReaperThresholds,
    destructive_allowed: bool,
) -> ReaperDecision {
    // Build the near-duplicate cluster: peers with sufficient title similarity AND
    // a non-empty changed-file overlap. Title similarity alone is never evidence.
    //
    // The candidate's title token set is invariant across peers, so tokenize it
    // ONCE here rather than re-splitting `facts.normalized_title` on every peer.
    // The changed-file overlap check (`is_disjoint`) is cheap and highly
    // selective, so it is evaluated first and short-circuits the similarity work
    // (a token-set Jaccard) for the common non-overlapping-peer case.
    let facts_tokens = title_tokens(&facts.normalized_title);
    let mut cluster_min = facts.number;
    let mut has_duplicate_peer = false;
    for peer in peers {
        if peer.number == facts.number {
            continue;
        }
        let overlaps = !facts.changed_files.is_disjoint(&peer.changed_files);
        if overlaps
            && title_similarity(&facts_tokens, &peer.normalized_title) >= thresholds.similarity
        {
            has_duplicate_peer = true;
            cluster_min = cluster_min.min(peer.number);
        }
    }

    // No real duplicate evidence ⇒ never touch the PR.
    if !has_duplicate_peer {
        return ReaperDecision::NoAction;
    }

    // Griefing resistance: the lowest-numbered (earliest) PR always survives. If
    // THIS PR is the survivor, it is never the close candidate.
    if facts.number == cluster_min {
        return ReaperDecision::NoAction;
    }

    // Fail-closed: only a known-good mergeable candidate can ever be closed.
    if facts.mergeable != MergeableState::Mergeable {
        return ReaperDecision::NoAction;
    }

    // Destructive gate: closed (default dry-run) ⇒ downgrade to a non-destructive
    // flag that names the real reason; nothing is ever closed unattended.
    if !destructive_allowed {
        return ReaperDecision::Flag(FlagStaleReason::DuplicateNotClosable);
    }

    ReaperDecision::CloseDuplicate {
        number: facts.number,
        survivor: cluster_min,
        reason: DuplicateReason::TitleAndFileOverlap,
    }
}

/// Tokenize a normalized title into its whitespace-split word set. Hoisted out of
/// [`title_similarity`] so the candidate's tokens are computed once per evaluation
/// rather than once per peer.
fn title_tokens(s: &str) -> BTreeSet<&str> {
    s.split_whitespace().collect()
}

/// Token-set Jaccard similarity between an already-tokenized title `ta` and the
/// raw normalized title `b`, in `[0.0, 1.0]`. Two empty titles are treated as
/// fully dissimilar (`0.0`) so blank titles never form a duplicate cluster.
/// Identical non-empty titles yield `1.0`.
fn title_similarity(ta: &BTreeSet<&str>, b: &str) -> f64 {
    let tb = title_tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let intersection = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    intersection / union
}
