//! Output-text sanitization helpers.

use super::MAX_ISSUE_TITLE_LEN;

/// Strip characters that are dangerous in shell arguments or would produce
/// malformed CLI commands when embedded in `--title` / `-m` values.
pub fn strip_shell_unsafe(input: &str) -> String {
    input
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '`' | '$'
                    | '\\'
                    | '"'
                    | '\''
                    | '|'
                    | ';'
                    | '&'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '{'
                    | '}'
                    | '!'
                    | '\0'
            )
        })
        .collect()
}

/// Sanitize an objective string for use as a GitHub issue title.
///
/// Strips newlines, removes shell-unsafe characters, collapses whitespace,
/// and truncates to a reasonable length.
pub fn sanitize_issue_title(raw: &str) -> String {
    let cleaned = strip_shell_unsafe(raw);
    let single_line: String = cleaned
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if single_line.len() <= MAX_ISSUE_TITLE_LEN {
        single_line
    } else {
        let truncated = &single_line[..MAX_ISSUE_TITLE_LEN];
        // Cut at the last word boundary to avoid mid-word truncation.
        match truncated.rfind(' ') {
            Some(pos) if pos > MAX_ISSUE_TITLE_LEN / 2 => format!("{}…", &truncated[..pos]),
            _ => format!("{truncated}…"),
        }
    }
}

/// Sanitize a commit message: strip newlines, remove shell-unsafe characters,
/// collapse whitespace, and truncate.
pub const MAX_COMMIT_MESSAGE_LEN: usize = 256;

pub fn sanitize_commit_message(raw: &str) -> String {
    let cleaned = strip_shell_unsafe(raw);
    let single_line: String = cleaned
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if single_line.len() <= MAX_COMMIT_MESSAGE_LEN {
        single_line
    } else {
        let truncated = &single_line[..MAX_COMMIT_MESSAGE_LEN];
        match truncated.rfind(' ') {
            Some(pos) if pos > MAX_COMMIT_MESSAGE_LEN / 2 => format!("{}…", &truncated[..pos]),
            _ => format!("{truncated}…"),
        }
    }
}
