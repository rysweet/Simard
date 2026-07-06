//! Jargon scrubbing for layperson-readable journal prose (issue #2606).
//!
//! Journal generation is two-pass: a draft is assembled from episodics and the
//! day's data, then a **mandatory** review pass rewrites it for a layperson.
//! This module provides the default (offline) reviewer — a deterministic,
//! whole-word glossary substitution that **expands** the engineering jargon a
//! non-engineer would trip over: acronyms are spelled out (`PR` -> `pull
//! request`, `CI` -> `the automated checks`), raw code identifiers are removed
//! (`temporal_index` -> `timestamp`), and insider terms are plainly explained
//! (`episodic` -> `moment-by-moment`, `OODA` -> `decision cycle`, `daemon` ->
//! `always-on background service`). It also carries [`scrub_secrets`], an
//! unconditional redaction pass the generator applies over the final narrative.
//!
//! The default reviewer is deterministic and offline so the whole pipeline is
//! testable without a network or an LLM; a language-model reviewer (the
//! preferred production path) can be swapped in behind the
//! [`JournalReviewer`](crate::journal::generate::JournalReviewer) trait when
//! available — and [`scrub_secrets`] still runs after it, so a secret survives
//! neither reviewer.

use std::sync::LazyLock;

/// The journal jargon glossary: `(term, replacement)`.
///
/// Matching is **whole-word** and case-insensitive (see [`scrub_jargon`]).
/// Replacements are plain-language and **never re-introduce a banned token**
/// (e.g. an `episodic …` replacement must not itself contain "episodic"):
///
/// * **Expand** — acronyms are spelled out in full, never left bare or merely
///   parenthesised, e.g. `PR` -> `"pull request"`, `CI` -> `"the automated
///   checks"`.
/// * **Explain / remove** — raw identifiers and insider terms with no reader
///   value are replaced outright, e.g. `temporal_index` -> `"timestamp"`,
///   `OODA` -> `"decision cycle"`, `dear diary` -> `""`.
///
/// Ordering is **longest phrase first** so multi-word terms and plurals win
/// over their shorter prefixes (the whole-word boundary check already prevents
/// `episode` from matching inside `episodes`, but longest-first keeps intent
/// obvious).
pub const JOURNAL_GLOSSARY: &[(&str, &str)] = &[
    // Diary phrasing has no place in a professional report — strip it outright
    // wherever it leaks in from raw memory content.
    ("dear diary", ""),
    // Insider domain terms, longest-first so multi-word phrases and plurals win
    // over their shorter prefixes. Replacements must never re-introduce a banned
    // token (e.g. the plain phrase must not contain the word "episodic").
    ("episodic memories", "moment-by-moment memories"),
    ("episodic memory", "moment-by-moment memory"),
    // Raw code identifiers a non-engineer would trip over — removed/explained.
    ("temporal_index", "timestamp"),
    ("working_memory", "short-term memory"),
    ("deployments", "updates to the live system"),
    ("deployment", "update to the live system"),
    ("idempotent", "safe to repeat"),
    ("OODA loop", "decision cycle"),
    ("LadybugDB", "the memory database"),
    ("episodic", "moment-by-moment"),
    ("episodes", "remembered moments"),
    ("deployed", "shipped to the live system"),
    ("deploys", "updates to the live system"),
    ("episode", "remembered moment"),
    ("deploy", "ship to the live system"),
    ("merged", "combined into the main code"),
    ("merges", "combines into the main code"),
    ("daemon", "always-on background service"),
    ("merge", "combine into the main code"),
    ("OODA", "decision cycle"),
    // Acronyms are **expanded** into plain words, not merely parenthesised, so a
    // non-engineer never meets a bare initialism (issue #2606).
    ("PRs", "pull requests"),
    ("TUI", "text-based dashboard"),
    ("LLM", "the language model"),
    ("PR", "pull request"),
    ("CI", "the automated checks"),
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
/// never re-scanned, so a replacement that legitimately contains a glossary
/// term does not loop.
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

/// Placeholder substituted for any redacted secret.
const SECRET_PLACEHOLDER: &str = "[redacted secret]";

/// Token prefixes that unambiguously introduce a credential (GitHub tokens and
/// fine-grained personal access tokens). Matched literally, then followed by a
/// run of token characters.
const TOKEN_PREFIXES: &[&str] = &["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];

/// Minimum token-body length (characters after the prefix) before a match is
/// treated as a real credential. Keeps a passing mention like `ghp_` in prose
/// from being redacted while catching real 36+ character tokens.
const MIN_TOKEN_BODY: usize = 20;

/// Redact credential-shaped substrings (GitHub tokens and PEM key blocks) from
/// `input`, leaving ordinary prose untouched (issue #2606).
///
/// This is an **unconditional** safety pass the generator applies over the
/// final narrative *after* the reviewer, so a secret that a language-model
/// reviewer failed to strip on its own still never reaches the durable entry.
/// It is deliberately conservative — it only fires on unmistakable secret
/// shapes — so a word like "key" in normal prose is never disturbed.
#[must_use]
pub fn scrub_secrets(input: &str) -> String {
    redact_tokens(&redact_pem_blocks(input))
}

/// Redact whole `-----BEGIN … -----END …-----` PEM blocks.
fn redact_pem_blocks(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(begin) = rest.find("-----BEGIN") {
        out.push_str(&rest[..begin]);
        let after_begin = &rest[begin..];
        // A well-formed block closes with `-----END …-----`; redact through the
        // closing dashes. A `BEGIN` with no `END` is malformed — redact the
        // remainder rather than risk leaking a key body (secret-safety first).
        if let Some(end) = after_begin.find("-----END") {
            let after_end = &after_begin[end + "-----END".len()..];
            if let Some(close) = after_end.find("-----") {
                let block_len = end + "-----END".len() + close + "-----".len();
                out.push_str(SECRET_PLACEHOLDER);
                rest = &after_begin[block_len..];
                continue;
            }
        }
        out.push_str(SECRET_PLACEHOLDER);
        return out;
    }
    out.push_str(rest);
    out
}

/// Redact credential tokens introduced by a [`TOKEN_PREFIXES`] prefix.
fn redact_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        let rest = &input[i..];
        let mut matched = false;
        for prefix in TOKEN_PREFIXES {
            if let Some(body) = rest.strip_prefix(prefix) {
                let body_len = body
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    .count();
                if body_len >= MIN_TOKEN_BODY {
                    out.push_str(SECRET_PLACEHOLDER);
                    i += prefix.len() + body_len;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            // Advance by one full char so a multi-byte codepoint is never split.
            let ch = rest.chars().next().expect("non-empty remainder");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}
