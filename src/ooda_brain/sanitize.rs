//! Sanitize context-variable strings before passing them as `-c key=value`
//! arguments to `recipe-runner-rs`.
//!
//! **Problem**: Log tails, goal descriptions, and other user-authored strings
//! can contain newlines, carriage returns, and excessive whitespace. When
//! passed as `-c` context vars, these break YAML template interpolation in
//! recipe-runner-rs (issue #2127 — 1341 failures in 24 hours).
//!
//! **Solution**: `sanitize_context_var` replaces `\n`/`\r` with spaces,
//! collapses consecutive whitespace, and truncates on a char boundary.

/// Sanitize a string for use as a recipe-runner-rs `-c` context variable.
///
/// Steps:
/// 1. Replace `\n` and `\r` with a single space.
/// 2. Collapse consecutive whitespace (`split_whitespace().join(" ")`).
/// 3. Truncate to `max_len` characters on a char boundary, appending `…`
///    if truncation occurred.
///
/// Returns an owned `String` that is safe to embed in `-c key=value` args.
pub(super) fn sanitize_context_var(s: &str, max_len: usize) -> String {
    // Step 1+2: split_whitespace handles \n, \r, \t, and consecutive spaces
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");

    // Step 3: truncate on char boundary
    if collapsed.chars().count() <= max_len {
        return collapsed;
    }
    match collapsed.char_indices().nth(max_len) {
        Some((byte_offset, _)) => format!("{}…", &collapsed[..byte_offset]),
        None => collapsed,
    }
}

// ---------------------------------------------------------------------------
// Tests — TDD: specify the contract, verify behavior.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // =======================================================================
    // Core behavior: newline replacement
    // =======================================================================

    #[test]
    fn newlines_replaced_with_space() {
        let input = "line one\nline two\nline three";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "line one line two line three");
        assert!(!result.contains('\n'), "must not contain newlines");
    }

    #[test]
    fn carriage_returns_replaced_with_space() {
        let input = "line one\rline two\rline three";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "line one line two line three");
        assert!(!result.contains('\r'), "must not contain carriage returns");
    }

    #[test]
    fn crlf_replaced_with_single_space() {
        let input = "line one\r\nline two\r\nline three";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "line one line two line three");
    }

    #[test]
    fn mixed_newlines_and_carriage_returns() {
        let input = "a\nb\rc\r\nd";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "a b c d");
    }

    // =======================================================================
    // Core behavior: whitespace collapse
    // =======================================================================

    #[test]
    fn consecutive_spaces_collapsed() {
        let input = "word1   word2     word3";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "word1 word2 word3");
    }

    #[test]
    fn tabs_collapsed_to_space() {
        let input = "word1\t\tword2\tword3";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "word1 word2 word3");
    }

    #[test]
    fn mixed_whitespace_collapsed() {
        let input = "word1 \t \n \r word2";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "word1 word2");
    }

    #[test]
    fn leading_and_trailing_whitespace_stripped() {
        let input = "  \n  hello world  \n  ";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "hello world");
    }

    // =======================================================================
    // Core behavior: truncation
    // =======================================================================

    #[test]
    fn truncation_at_max_len() {
        let input = "a".repeat(1000);
        let result = sanitize_context_var(&input, 100);
        // Should be 100 chars + "…"
        assert_eq!(
            result.chars().count(),
            101,
            "truncated output must be max_len chars + ellipsis"
        );
        assert!(result.ends_with('…'), "truncated output must end with …");
    }

    #[test]
    fn no_truncation_when_within_limit() {
        let input = "short string";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "short string");
        assert!(!result.ends_with('…'));
    }

    #[test]
    fn exact_length_not_truncated() {
        let input = "abcde";
        let result = sanitize_context_var(input, 5);
        assert_eq!(result, "abcde");
        assert!(!result.ends_with('…'));
    }

    #[test]
    fn one_over_limit_is_truncated() {
        let input = "abcdef";
        let result = sanitize_context_var(input, 5);
        assert_eq!(result, "abcde…");
    }

    #[test]
    fn truncation_respects_char_boundary_multibyte() {
        // Each emoji is multiple bytes but one char
        let input = "🔥🔥🔥🔥🔥🔥";
        let result = sanitize_context_var(input, 3);
        assert_eq!(result, "🔥🔥🔥…");
        assert_eq!(result.chars().count(), 4); // 3 emoji + ellipsis
    }

    #[test]
    fn truncation_after_whitespace_collapse() {
        // After collapsing "a  b  c  d" → "a b c d" (7 chars)
        let input = "a  b  c  d";
        let result = sanitize_context_var(input, 5);
        assert_eq!(result, "a b c…");
    }

    // =======================================================================
    // Edge cases
    // =======================================================================

    #[test]
    fn empty_input() {
        let result = sanitize_context_var("", 500);
        assert_eq!(result, "");
    }

    #[test]
    fn whitespace_only_input() {
        let result = sanitize_context_var("   \n\t\r  ", 500);
        assert_eq!(result, "");
    }

    #[test]
    fn max_len_zero() {
        let result = sanitize_context_var("hello", 0);
        assert_eq!(result, "…");
    }

    #[test]
    fn max_len_one_with_content() {
        let result = sanitize_context_var("hello world", 1);
        assert_eq!(result, "h…");
    }

    #[test]
    fn single_char_input() {
        let result = sanitize_context_var("x", 500);
        assert_eq!(result, "x");
    }

    // =======================================================================
    // Realistic inputs — the actual bug scenario
    // =======================================================================

    #[test]
    fn realistic_log_tail_with_newlines() {
        let input = "2024-01-15T10:30:00Z INFO starting engineer\n\
                     2024-01-15T10:30:01Z DEBUG checking worktree\n\
                     2024-01-15T10:30:02Z ERROR panicked at 'index out of bounds'\n\
                     stack backtrace:\n\
                       0: std::panicking::begin_panic\n\
                       1: simard::engineer::run";
        let result = sanitize_context_var(input, 2000);
        assert!(!result.contains('\n'));
        assert!(!result.contains('\r'));
        assert!(result.contains("panicked at"));
        assert!(result.chars().count() <= 2000);
    }

    #[test]
    fn realistic_log_tail_truncated() {
        // Simulate a very long log tail (>2000 chars after collapse)
        let line = "2024-01-15T10:30:00Z INFO processing goal advance-feature-x step 42\n";
        let input = line.repeat(50); // ~3500 chars
        let result = sanitize_context_var(&input, 2000);
        assert!(
            result.chars().count() <= 2001,
            "must truncate to max_len + ellipsis; got {} chars",
            result.chars().count()
        );
        assert!(result.ends_with('…'));
    }

    #[test]
    fn goal_description_with_special_chars() {
        let input = "Fix the\n\"broken\" parser's\thyperlink & <tags>";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, "Fix the \"broken\" parser's hyperlink & <tags>");
        assert!(!result.contains('\n'));
        assert!(!result.contains('\t'));
    }

    #[test]
    fn worktree_path_passthrough() {
        let input = "/home/user/src/Simard/worktrees/feat/my-feature";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, input, "clean paths must pass through unchanged");
    }

    // =======================================================================
    // Security: YAML injection prevention
    // =======================================================================

    #[test]
    fn yaml_injection_via_newline_neutralized() {
        // An attacker tries to inject a new YAML key via a newline
        let input = "normal value\nmalicious_key: injected_value";
        let result = sanitize_context_var(input, 500);
        assert!(!result.contains('\n'));
        assert_eq!(result, "normal value malicious_key: injected_value");
    }

    #[test]
    fn multiline_yaml_block_scalar_neutralized() {
        let input = "value\n  - injected_list_item\n  - another_item";
        let result = sanitize_context_var(input, 500);
        assert!(!result.contains('\n'));
        assert_eq!(result, "value - injected_list_item - another_item");
    }

    // =======================================================================
    // Contract: idempotence
    // =======================================================================

    #[test]
    fn already_clean_input_unchanged() {
        let input = "this is already clean text";
        let result = sanitize_context_var(input, 500);
        assert_eq!(result, input);
    }

    #[test]
    fn double_sanitize_is_idempotent() {
        let input = "line\none\n  two\t\tthree";
        let first = sanitize_context_var(input, 500);
        let second = sanitize_context_var(&first, 500);
        assert_eq!(first, second, "sanitize must be idempotent");
    }
}
