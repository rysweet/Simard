//! Dedup / novelty, portfolio balancing, and budget helpers (design spike #2419).
//!
//! These are the safety "make it more interesting, yet safe" hooks:
//! - [`is_near_duplicate`] / [`reject_duplicates`] keep the pool novel;
//! - [`select_balanced`] keeps a bounded, balanced portfolio;
//! - [`within_budget`] gates an expensive generation tick.
//!
//! The similarity metric is a deterministic word-set Jaccard so tests are
//! reproducible with no network. FUTURE (M6): richer shingle/embedding novelty
//! scoring and risk/novelty-bucketed portfolio selection.
#![allow(dead_code)]

use std::collections::BTreeSet;

use crate::cognitive_memory::creative_idea::CreativeIdea;
use crate::cognitive_threads::threads::creative_ideas::RawIdea;

/// Default Jaccard threshold at or above which two ideas are "near-duplicates".
pub const DEFAULT_DEDUP_THRESHOLD: f64 = 0.6;

/// Whether `candidate` is a near-duplicate of `prior` at `threshold`
/// (word-set Jaccard similarity `>= threshold`).
#[must_use]
pub fn is_near_duplicate(candidate: &str, prior: &str, threshold: f64) -> bool {
    jaccard(candidate, prior) >= threshold
}

/// Coarse word-set similarity in `[0.0, 1.0]` between two idea texts. This is
/// the v1 primitive (deterministic, no network) the semantic dedup gate reuses
/// **only** as a cheap pre-filter (Stage-1 shortlist ranking) and as the
/// fail-closed backstop — never as the semantic authority (issue #2925). If a
/// store-layer embedding similarity is ever added it swaps this scorer without
/// changing the gate contract.
#[must_use]
pub(crate) fn similarity(a: &str, b: &str) -> f64 {
    jaccard(a, b)
}

/// Keep only candidates that are **not** a near-duplicate of any previous idea.
#[must_use]
pub fn reject_duplicates(
    candidates: Vec<RawIdea>,
    previous: &[CreativeIdea],
    threshold: f64,
) -> Vec<RawIdea> {
    candidates
        .into_iter()
        .filter(|c| {
            !previous
                .iter()
                .any(|p| is_near_duplicate(&c.idea, &p.idea, threshold))
        })
        .collect()
}

/// Select a bounded portfolio of at most `budget` candidates.
///
/// FUTURE (M6): spread across risk/novelty buckets. For the spike this simply
/// truncates to the budget while preserving order.
#[must_use]
pub fn select_balanced(mut candidates: Vec<RawIdea>, budget: usize) -> Vec<RawIdea> {
    candidates.truncate(budget);
    candidates
}

/// Whether an expensive generation tick is within the daily budget.
#[must_use]
pub fn within_budget(spent_usd: f64, limit_usd: f64) -> bool {
    spent_usd < limit_usd
}

fn tokenize(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn jaccard(a: &str, b: &str) -> f64 {
    let sa = tokenize(a);
    let sb = tokenize(b);
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let intersection = sa.intersection(&sb).count();
    let union = sa.union(&sb).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}
