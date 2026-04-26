//! GitHub issue number extraction helpers.

use crate::error::{SimardError, SimardResult};

use crate::engineer_loop::types::{
    AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind, GitCommitRequest,
    OpenIssueRequest, RepoInspection, SelectedEngineerAction, ShellCommandRequest,
    StructuredEditRequest, extract_command_from_objective, extract_file_path_from_objective,
    is_prose_fragment, parse_structured_edit_request, validate_repo_relative_path,
};

use crate::engineer_loop::SHELL_COMMAND_ALLOWLIST;

const MAX_ISSUE_DIGITS: usize = 20;

/// Scan an objective string for a reference to an existing GitHub issue
/// number. Returns the earliest non-zero issue number found across two
/// patterns:
///
/// - Pattern A: `#<digits>` — but rejected when the `#` is preceded by `&`
///   (HTML numeric character reference like `&#915;`) or when the digit run
///   is followed by an ASCII alphanumeric character.
/// - Pattern B: `issue` (case-insensitive) followed by ASCII whitespace,
///   then optionally `#`, `number `, or `id `, then `<digits>` with the
///   same word-boundary guard at the end.
///
/// The scanner is single-pass linear — no regex, no backtracking. Digit
/// runs are capped at `MAX_ISSUE_DIGITS` to bound input cost. Zero is
/// rejected because GitHub issue numbers start at 1.
pub fn extract_existing_issue_number(objective: &str) -> Option<u64> {
    let bytes = objective.as_bytes();
    let mut earliest: Option<(usize, u64)> = None;

    let mut consider = |start: usize, n: u64| {
        if earliest.map(|(s, _)| start < s).unwrap_or(true) {
            earliest = Some((start, n));
        }
    };

    // Pattern A: `#<digits>` with HTML-entity guard and trailing word-boundary.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            // HTML numeric character reference guard: reject `&#...`.
            let preceded_by_amp = i > 0 && bytes[i - 1] == b'&';
            if !preceded_by_amp
                && let Some((n, end)) = parse_digits(bytes, i + 1)
                && !is_alnum_byte(bytes.get(end).copied())
                && n != 0
            {
                consider(i, n);
            }
        }
        i += 1;
    }

    // Pattern B: `issue` (case-insensitive) + ws + optional `#` / `number ` / `id ` + digits.
    let lower = objective.to_ascii_lowercase();
    let lower_bytes = lower.as_bytes();
    let needle = b"issue";
    let mut search_from = 0;
    while let Some(rel) = find_subslice(&lower_bytes[search_from..], needle) {
        let start = search_from + rel;
        let end_word = start + needle.len();
        // Require a leading word-boundary: previous byte must not be alnum.
        let leading_ok = start == 0 || !is_alnum_byte(Some(bytes[start - 1]));
        // Require ASCII whitespace immediately after `issue`.
        let trailing_ok = bytes
            .get(end_word)
            .map(|&b| b.is_ascii_whitespace())
            .unwrap_or(false);
        if leading_ok && trailing_ok {
            // Skip the whitespace run.
            let mut p = end_word;
            while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                p += 1;
            }
            // Optional `#`, `number `, or `id ` qualifier.
            if p < bytes.len() && bytes[p] == b'#' {
                p += 1;
            } else if try_consume_keyword(lower_bytes, p, b"number") {
                p += b"number".len();
                while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                    p += 1;
                }
            } else if try_consume_keyword(lower_bytes, p, b"id") {
                p += b"id".len();
                while p < bytes.len() && bytes[p].is_ascii_whitespace() {
                    p += 1;
                }
            }
            if let Some((n, dend)) = parse_digits(bytes, p)
                && !is_alnum_byte(bytes.get(dend).copied())
                && n != 0
            {
                consider(start, n);
            }
        }
        search_from = start + needle.len();
    }

    earliest.map(|(_, n)| n)
}

fn is_alnum_byte(b: Option<u8>) -> bool {
    matches!(b, Some(c) if c.is_ascii_alphanumeric())
}

fn parse_digits(bytes: &[u8], start: usize) -> Option<(u64, usize)> {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() && (end - start) < MAX_ISSUE_DIGITS {
        end += 1;
    }
    if end == start {
        return None;
    }
    // If the digit run is bounded by MAX_ISSUE_DIGITS but more digits follow,
    // treat as overflow and reject.
    if end < bytes.len() && bytes[end].is_ascii_digit() {
        return None;
    }
    let s = std::str::from_utf8(&bytes[start..end]).ok()?;
    let n: u64 = s.parse().ok()?;
    Some((n, end))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// True when `lower_bytes[at..]` begins with `kw` AND the byte after `kw`
/// is ASCII whitespace (so `number` matches but `numbered` does not).
fn try_consume_keyword(lower_bytes: &[u8], at: usize, kw: &[u8]) -> bool {
    if at + kw.len() > lower_bytes.len() {
        return false;
    }
    if &lower_bytes[at..at + kw.len()] != kw {
        return false;
    }
    lower_bytes
        .get(at + kw.len())
        .map(|&b| b.is_ascii_whitespace())
        .unwrap_or(false)
}
