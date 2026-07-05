//! Production external-service adapter for the journal's PR seam (issue #2606).
//!
//! The journal's plain-language code-change-proposal table is fed through the
//! [`PrListSource`](crate::journal::providers::PrListSource) seam. In tests a
//! canned list is injected; in production this module wraps the existing
//! PR-readiness / merge-authority view — the same `gh pr list` external service
//! the dashboard's Merge Readiness panel uses (#1880) — behind that seam.
//!
//! [`GhPrListSource`] performs the (network) `gh pr list` fetch through the
//! stewardship [`PrGhClient`] and maps each [`OpenPrSummary`] into a
//! layperson-readable [`PrSummary`]:
//!
//! * `plain_summary` — the PR title with its Conventional-Commits prefix
//!   (`feat(scope): `, `fix!: `, ...) stripped and engineering jargon scrubbed
//!   ([`plainify_pr_title`]), so it reads as "what changed & why it matters".
//! * `outcome` — a plain-language readiness phrase derived from the *same*
//!   objective gates the merge authority evaluates ([`pr_readiness_outcome`]).
//!
//! A `gh` failure degrades **honestly**: the day's proposal table falls back to
//! empty (logged), so the narrative is still written rather than the whole tick
//! failing on a transient network blip.

use chrono::NaiveDate;

use crate::error::SimardResult;
use crate::journal::jargon::scrub_jargon;
use crate::journal::providers::PrListSource;
use crate::journal::types::PrSummary;
use crate::stewardship::merge_authority::{OpenPrSummary, PrGhClient, evaluate_objective_gates};

/// `gh pr list` page size for the journal's PR table. Matches the dashboard's
/// Merge Readiness panel (#1880); 50 covers the active repo without paginating.
pub const JOURNAL_PR_LIMIT: u32 = 50;

/// Conventional-Commits types recognised when stripping a title prefix. A
/// prefix is only removed when the token before the first colon is one of
/// these, so an ordinary sentence that merely contains a colon is preserved.
const CONVENTIONAL_TYPES: &[&str] = &[
    "feat", "fix", "chore", "docs", "test", "tests", "refactor", "perf", "build", "ci", "style",
    "revert",
];

/// CI check states that mean "still running" — the change is neither ready nor
/// a hard failure. Kept in step with the dashboard panel's pending mapping.
fn is_pending_state(state: &str) -> bool {
    matches!(state, "PENDING" | "QUEUED" | "IN_PROGRESS")
}

/// Strip a leading Conventional-Commits `type(scope)!:` prefix if present,
/// returning the human-meaningful remainder. Only a recognised
/// [`CONVENTIONAL_TYPES`] token (optionally followed by a `(scope)` and/or `!`)
/// immediately before the first `:` is stripped; anything else is left as-is.
fn strip_conventional_prefix(title: &str) -> &str {
    let Some(colon) = title.find(':') else {
        return title;
    };
    let (head, rest) = title.split_at(colon);
    let head = head.trim();
    // Drop a breaking-change `!` marker, then an optional `(scope)`.
    let head = head.strip_suffix('!').unwrap_or(head);
    let type_token = match head.find('(') {
        Some(paren) if head.ends_with(')') => head[..paren].trim(),
        _ => head,
    };
    if CONVENTIONAL_TYPES
        .iter()
        .any(|t| t.eq_ignore_ascii_case(type_token))
    {
        // Skip the ':' (one ASCII byte) and any following whitespace.
        rest[1..].trim_start()
    } else {
        title
    }
}

/// Rewrite a raw PR title into a layperson-readable "what changed & why it
/// matters" phrase: drop a Conventional-Commits prefix that means nothing to a
/// non-engineer, then scrub engineering jargon. Falls back to a neutral phrase
/// when nothing readable remains.
#[must_use]
pub fn plainify_pr_title(title: &str) -> String {
    let without_prefix = strip_conventional_prefix(title.trim());
    let scrubbed = scrub_jargon(without_prefix).trim().to_string();
    if scrubbed.is_empty() {
        "A code change.".to_string()
    } else {
        scrubbed
    }
}

/// A layperson-readable outcome/readiness phrase for one open code-change
/// proposal, derived from the same objective gates the merge authority uses
/// ([`evaluate_objective_gates`]): base-branch allow-list, `mergeable`, and CI
/// rollup. The phrase is jargon-free by construction.
#[must_use]
pub fn pr_readiness_outcome(pr: &OpenPrSummary, base_allowlist: &[String]) -> String {
    match evaluate_objective_gates(&pr.to_snapshot(), base_allowlist) {
        Ok(()) => "still open — ready to combine into the main code".to_string(),
        Err(_) if pr.checks.iter().any(|c| is_pending_state(&c.state)) => {
            "still open — automated checks still running".to_string()
        }
        Err(_) => "still open — not ready yet".to_string(),
    }
}

/// Map one [`OpenPrSummary`] from the `gh pr list` view into a layperson-ready
/// journal [`PrSummary`].
#[must_use]
pub fn open_pr_to_summary(pr: &OpenPrSummary, base_allowlist: &[String]) -> PrSummary {
    PrSummary {
        number: u64::from(pr.number),
        plain_summary: plainify_pr_title(&pr.title),
        outcome: pr_readiness_outcome(pr, base_allowlist),
    }
}

/// Production [`PrListSource`] over the `gh pr list` PR-readiness service.
///
/// Wraps a stewardship [`PrGhClient`] (in production the `gh`-shelling
/// [`RealPrGhClient`](crate::stewardship::RealPrGhClient)) and the base-branch
/// allow-list used to judge readiness. Because it touches the network it is
/// driven off the hot OODA path (a spawned journal tick), never inline.
pub struct GhPrListSource<'a> {
    gh: &'a (dyn PrGhClient + Send + Sync),
    repo: &'a str,
    base_allowlist: Vec<String>,
    limit: u32,
}

impl<'a> GhPrListSource<'a> {
    /// Wrap `gh` for `repo`, judging readiness against `base_allowlist`.
    pub fn new(
        gh: &'a (dyn PrGhClient + Send + Sync),
        repo: &'a str,
        base_allowlist: Vec<String>,
    ) -> Self {
        Self {
            gh,
            repo,
            base_allowlist,
            limit: JOURNAL_PR_LIMIT,
        }
    }

    /// Override the `gh pr list` page size (default [`JOURNAL_PR_LIMIT`]).
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }
}

impl PrListSource for GhPrListSource<'_> {
    fn prs_for_date(&self, _date: NaiveDate) -> SimardResult<Vec<PrSummary>> {
        // Honest degradation: a `gh` blip yields an empty proposal table (logged)
        // rather than failing the whole tick, so the narrative is still written.
        match self.gh.list_open_prs(self.repo, self.limit) {
            Ok(open) => Ok(open
                .iter()
                .map(|pr| open_pr_to_summary(pr, &self.base_allowlist))
                .collect()),
            Err(e) => {
                tracing::warn!(
                    target: "simard::journal",
                    error = %e,
                    repo = self.repo,
                    "journal PR fetch failed; the day's proposal table degrades to empty"
                );
                Ok(Vec::new())
            }
        }
    }
}
