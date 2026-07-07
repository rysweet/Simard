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
/// 0. Strip ANSI escape sequences (ESC + CSI …) and non-whitespace C0/DEL
///    control bytes. `\t`, `\n`, `\r` are preserved here so step 1 folds them
///    into single spaces rather than deleting them outright — they are
///    semantically whitespace, unlike the raw control bytes (BEL, NUL, VT, FF,
///    BS) that a journald / log-tail live-signal detail can smuggle in.
/// 1. Replace `\n` and `\r` with a single space.
/// 2. Collapse consecutive whitespace (`split_whitespace().join(" ")`).
/// 3. Truncate to `max_len` characters on a char boundary, appending `…`
///    if truncation occurred.
///
/// The ANSI/control strip (issue #2751) prevents a `LiveSignal.detail` from
/// corrupting terminal-rendered logs or smuggling escape sequences into the
/// reasoner prompt. It is the sanitization boundary the closed-loop
/// outcome-verification step relies on.
///
/// Returns an owned `String` that is safe to embed in `-c key=value` args.
pub fn sanitize_context_var(s: &str, max_len: usize) -> String {
    // Step 0: strip ANSI escape sequences + non-whitespace control bytes.
    let filtered = strip_ansi_and_control(s);

    // Step 1+2: split_whitespace handles \n, \r, \t, and consecutive spaces.
    // Push directly into a pre-sized String — avoids intermediate Vec<&str>.
    let mut collapsed = String::with_capacity(filtered.len());
    for word in filtered.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }

    // Step 3: truncate on char boundary.
    // Single char_indices().nth() pass replaces separate .count() + .nth().
    if let Some((byte_offset, _)) = collapsed.char_indices().nth(max_len) {
        collapsed.truncate(byte_offset);
        collapsed.push('…');
    }
    collapsed
}

/// Remove ANSI escape sequences and non-whitespace C0/DEL control characters.
///
/// - `ESC [ … <final>` CSI sequences (colour, cursor movement, clear-screen)
///   are consumed whole: `ESC`, `[`, any parameter/intermediate bytes
///   (`0x20..=0x3F`), and a single final byte (`0x40..=0x7E`).
/// - A lone `ESC` (not starting a CSI) is dropped.
/// - `\t`, `\n`, `\r` are preserved (folded to spaces downstream).
/// - Every other C0 control (`0x00..=0x1F`) and `DEL` (`0x7F`) is dropped.
fn strip_ansi_and_control(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // ANSI escape. Consume a CSI sequence if one follows; else the
                // lone ESC is simply dropped.
                if chars.peek() == Some(&'[') {
                    chars.next(); // consume '['
                    // Parameter + intermediate bytes: 0x20..=0x3F.
                    while matches!(chars.peek(), Some(&p) if ('\u{20}'..='\u{3f}').contains(&p)) {
                        chars.next();
                    }
                    // Final byte: 0x40..=0x7E — consume one if present.
                    if matches!(chars.peek(), Some(&f) if ('\u{40}'..='\u{7e}').contains(&f)) {
                        chars.next();
                    }
                }
            }
            // Preserve tab/newline/CR for the whitespace-collapse step.
            '\t' | '\n' | '\r' => out.push(c),
            // Drop other C0 controls (0x00-0x1F) and DEL (0x7F).
            c if (c as u32) < 0x20 || c == '\u{7f}' => {}
            c => out.push(c),
        }
    }
    out
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

    // =======================================================================
    // T-sec1 (issue #2751) — ANSI escape + control-character neutralization.
    //
    // TDD: these specify the UPGRADED contract needed by the closed-loop
    // outcome-verification step. `LiveSignal` details come from journald / log
    // tails that can embed ANSI colour codes and raw control bytes. When those
    // strings become recipe `-c` context vars they must be stripped so they can
    // neither corrupt terminal-rendered logs nor smuggle escape sequences into
    // the reasoner prompt. The current implementation collapses whitespace but
    // does NOT strip ESC (0x1b) or other C0 control bytes, so these FAIL until
    // the sanitizer is extended (design: "add ANSI strip + length caps").
    // =======================================================================

    #[test]
    fn ansi_color_codes_stripped() {
        // SGR colour: ESC [ 3 1 m ... ESC [ 0 m
        let input = "\u{1b}[31mALERT\u{1b}[0m disk full";
        let result = sanitize_context_var(input, 500);
        assert!(
            !result.contains('\u{1b}'),
            "ESC (0x1b) must be stripped; got {result:?}"
        );
        assert_eq!(
            result, "ALERT disk full",
            "ANSI SGR sequences must be removed, leaving only the visible text"
        );
    }

    #[test]
    fn ansi_cursor_movement_stripped() {
        // ESC [ 2 J (clear screen) + ESC [ H (cursor home)
        let input = "before\u{1b}[2J\u{1b}[Hafter";
        let result = sanitize_context_var(input, 500);
        assert!(!result.contains('\u{1b}'), "ESC must be stripped");
        assert!(
            !result.contains("[2J"),
            "CSI payload must not leak: {result:?}"
        );
        assert_eq!(result, "beforeafter");
    }

    #[test]
    fn c0_control_bytes_stripped() {
        // Bell (0x07), NUL (0x00), vertical tab (0x0b), form feed (0x0c),
        // and a stray backspace (0x08) — none are whitespace, so the current
        // split_whitespace pass preserves them.
        let input = "a\u{07}b\u{00}c\u{0b}d\u{0c}e\u{08}f";
        let result = sanitize_context_var(input, 500);
        for ch in ['\u{07}', '\u{00}', '\u{0b}', '\u{0c}', '\u{08}'] {
            assert!(
                !result.contains(ch),
                "control char {:?} must be stripped; got {result:?}",
                ch
            );
        }
        assert_eq!(result, "abcdef");
    }

    #[test]
    fn ansi_and_newline_injection_combined_neutralized() {
        // A live-signal detail that tries BOTH a colour code and a newline-based
        // YAML/context injection must be fully neutralised.
        let input = "\u{1b}[1;32mOK\u{1b}[0m\nmalicious_key: injected";
        let result = sanitize_context_var(input, 500);
        assert!(!result.contains('\u{1b}'), "ESC stripped");
        assert!(!result.contains('\n'), "newline neutralised");
        assert_eq!(result, "OK malicious_key: injected");
    }

    #[test]
    fn tab_newline_still_treated_as_whitespace_after_ansi_upgrade() {
        // Regression guard: the ANSI/control upgrade must not change the
        // existing whitespace handling for \t, \n, \r (which are C0 controls
        // but are semantically whitespace and must become a single space, not
        // be deleted outright).
        let input = "a\tb\nc\rd";
        let result = sanitize_context_var(input, 500);
        assert_eq!(
            result, "a b c d",
            "\\t \\n \\r must remain whitespace separators, not be deleted"
        );
    }
}
