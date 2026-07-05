//! Jargon scrubbing for layperson-readable journal prose (issue #2606).
//!
//! Journal generation is two-pass: a draft is assembled from episodics and the
//! day's data, then a **mandatory** review pass rewrites it for a layperson.
//! This module provides the default reviewer — a deterministic, whole-word
//! glossary substitution that removes or explains the engineering jargon a
//! non-engineer would trip over (`PR`, `CI`, `episodic`, `deploy`, `merge`,
//! `daemon`, `OODA`, ...).
//!
//! The default reviewer is deterministic and offline so the whole pipeline is
//! testable without a network or an LLM; an LLM reasoner reviewer can be
//! swapped in behind the [`JournalReviewer`](crate::journal::generate::JournalReviewer)
//! trait when available.

use std::sync::LazyLock;

/// The journal jargon glossary: `(term, replacement)`.
///
/// Matching is **whole-word** and case-insensitive (see [`scrub_jargon`]).
/// Two flavours of replacement:
///
/// * **Explain-on-use** — the plain phrase keeps the term in parentheses so a
///   curious reader still learns the word, e.g. `PR` ->
///   `"code-change proposal (PR)"`.
/// * **Remove** — insider terms with no reader value are replaced outright,
///   e.g. `OODA` -> `"decision cycle"`.
///
/// Ordering is **longest phrase first** so multi-word terms and plurals win
/// over their shorter prefixes (the whole-word boundary check already prevents
/// `episode` from matching inside `episodes`, but longest-first keeps intent
/// obvious).
pub const JOURNAL_GLOSSARY: &[(&str, &str)] = &[
    ("pull requests", "code-change proposals"),
    ("pull request", "code-change proposal"),
    (
        "episodic memories",
        "moment-by-moment memories (episodic memory)",
    ),
    (
        "episodic memory",
        "moment-by-moment memory (episodic memory)",
    ),
    ("episodic", "moment-by-moment"),
    ("episodes", "remembered moments"),
    ("episode", "remembered moment"),
    ("deployments", "updates to the live system"),
    ("deployment", "update to the live system"),
    ("deploys", "updates to the live system"),
    ("deployed", "shipped to the live system"),
    ("deploy", "ship to the live system"),
    ("idempotent", "safe to repeat"),
    ("merged", "combined into the main code"),
    ("merges", "combines into the main code"),
    ("merge", "combine into the main code"),
    ("daemon", "always-on background service"),
    ("OODA loop", "decision cycle"),
    ("OODA", "decision cycle"),
    ("LadybugDB", "the memory database"),
    ("CI", "the automated checks (CI)"),
    ("PRs", "code-change proposals (PRs)"),
    ("PR", "code-change proposal (PR)"),
];

/// The glossary terms lowercased into `Vec<char>` keys once, paired with their
/// replacement text. The glossary is a `const`, so folding each term to
/// lowercase chars is the same work on every call — [`scrub_jargon`] runs once
/// per narrative and once per code-change-proposal title, so this precompute is
/// cached here rather than rebuilt each time.
static LOWERED_TERMS: LazyLock<Vec<(Vec<char>, &'static str)>> = LazyLock::new(|| {
    JOURNAL_GLOSSARY
        .iter()
        .map(|(term, replacement)| {
            (
                term.chars().map(|c| c.to_ascii_lowercase()).collect(),
                *replacement,
            )
        })
        .collect()
});

/// Rewrite `input`, replacing every whole-word occurrence of a
/// [`JOURNAL_GLOSSARY`] term with its plain-language replacement.
///
/// Matching is case-insensitive and **word-boundary aware**: a term only
/// matches when the characters immediately before and after it are not
/// alphanumeric. This is what keeps `PR` from corrupting `approve` and `CI`
/// from corrupting `social`. Replacement text is emitted to the output and is
/// never re-scanned, so a replacement that legitimately contains its own term
/// (e.g. the `"(PR)"` in `"code-change proposal (PR)"`) does not loop.
///
/// The pass is a pure function of `input` — deterministic and side-effect free.
#[must_use]
pub fn scrub_jargon(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    // ASCII-lowercase per char so indices stay aligned with `chars` (unlike
    // `str::to_lowercase`, which can change length). Glossary terms are ASCII,
    // so ASCII folding is sufficient for matching.
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    // The lowercased glossary keys are precomputed once (see `LOWERED_TERMS`).
    let terms = &*LOWERED_TERMS;

    let n = chars.len();
    let mut out = String::with_capacity(input.len() + 64);
    let mut i = 0;
    while i < n {
        let boundary_before = i == 0 || !chars[i - 1].is_alphanumeric();
        let mut matched = false;
        if boundary_before {
            for (term, replacement) in terms {
                let tlen = term.len();
                if i + tlen > n {
                    continue;
                }
                if lower[i..i + tlen] != term[..] {
                    continue;
                }
                let boundary_after = i + tlen == n || !chars[i + tlen].is_alphanumeric();
                if boundary_after {
                    out.push_str(replacement);
                    i += tlen;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}
