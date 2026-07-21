//! Objective merge-judge — the opt-in, last-resort JUDGMENT tier (P1 / #4389).
//!
//! The default [`super::merge_judge::RefusingMergeJudge`] always returns
//! `NotReady` whenever no LLM/recipe provider is wired, which stalls every
//! delivery-ready PR and re-escalates the same ones each overseer tick. This
//! module provides an OBJECTIVE alternative the operator can opt into
//! (`SIMARD_MERGE_OBJECTIVE_FALLBACK=1`): it issues a `Ready` verdict for a PR
//! authored by an explicitly-TRUSTED GitHub login, and `NotReady` for anyone
//! else.
//!
//! It replaces ONLY the judgment half. The objective gates (CI-green,
//! `MERGEABLE`, base-branch + repo allowlists) still run in
//! [`super::merge_authority`] downstream and are never bypassed.
//!
//! ## Security invariants (mirrored by `tests_objective_merge_judge.rs`)
//! * Trust is keyed on the AUTHENTICATED `author.login` (exact, case-insensitive
//!   equality) — never on a spoofable body/title/trailer.
//! * The overseer bot identity is ALWAYS excluded from a `Ready` verdict, even
//!   if it is somehow present in the allowlist (no self-merge loop).
//! * An empty/unknown author (absent `author` object) can never be trusted.
//! * An empty allowlist trusts no one (fail-closed).

use crate::error::SimardResult;

use super::merge_authority::PrSnapshot;
use super::merge_judge::{Blocker, JudgeOutcome, MergeJudge, MergeJudgeKind, Verdict};

/// Objective merge judge: passes a green PR iff its authenticated author is on
/// the configured trusted-author allowlist (and is not the overseer bot).
pub struct ObjectiveMergeJudge {
    trusted_authors: Vec<String>,
    bot_login: String,
}

impl ObjectiveMergeJudge {
    /// Construct with the trusted-author allowlist and the overseer bot login
    /// to exclude from any `Ready` verdict.
    pub fn new(trusted_authors: Vec<String>, bot_login: String) -> Self {
        Self {
            trusted_authors,
            bot_login,
        }
    }

    /// Whether `author` is a trusted, non-bot, non-empty login. Case-insensitive
    /// but EXACT: a padded (`" rysweet"`) or look-alike (`rysweet-bot`) login
    /// never matches.
    fn is_trusted(&self, author: &str) -> bool {
        if author.is_empty() {
            return false;
        }
        if author.eq_ignore_ascii_case(&self.bot_login) {
            return false;
        }
        self.trusted_authors
            .iter()
            .any(|t| t.eq_ignore_ascii_case(author))
    }
}

impl MergeJudge for ObjectiveMergeJudge {
    fn judge(
        &self,
        _pr_number: u32,
        _repo: &str,
        snapshot: &PrSnapshot,
    ) -> SimardResult<JudgeOutcome> {
        let author = snapshot.author_login.as_str();
        if self.is_trusted(author) {
            Ok(JudgeOutcome {
                verdict: Verdict::Ready,
                rationale: format!(
                    "objective merge-judge: PR authored by trusted author {author:?}; \
                     objective gates enforced separately"
                ),
                blockers: vec![],
            })
        } else {
            Ok(JudgeOutcome {
                verdict: Verdict::NotReady,
                rationale: format!(
                    "objective merge-judge: author {author:?} is not on the trusted-author \
                     allowlist (or is the overseer bot / empty)"
                ),
                blockers: vec![Blocker {
                    section: "trusted-author".to_string(),
                    severity: "high".to_string(),
                    observation: format!(
                        "author {author:?} is not a configured trusted author for the \
                         objective merge-judge fallback"
                    ),
                    fix: "Add the author to SIMARD_MERGE_TRUSTED_AUTHORS, or land the PR via a \
                          configured LLM/recipe merge-judge or a manual merge-ready review."
                        .to_string(),
                }],
            })
        }
    }

    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Objective
    }
}
