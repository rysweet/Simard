//! Copilot output filtering utilities for meeting conversation turns.

/// Strip copilot bootstrap noise and usage stats from stdout output.
#[allow(dead_code)]
pub(crate) fn strip_copilot_noise(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let mut skip_rest = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        // Skip empty leading lines
        if result.is_empty() && trimmed.is_empty() {
            continue;
        }

        // Stop at usage stats footer
        if trimmed.starts_with("Total usage est:")
            || trimmed.starts_with("API time spent:")
            || trimmed.starts_with("Total session time:")
            || trimmed.starts_with("Changes ")
            || trimmed.starts_with("Requests ")
            || trimmed.starts_with("Tokens ")
        {
            skip_rest = true;
            continue;
        }

        if skip_rest {
            continue;
        }

        // Skip copilot bootstrap noise
        if trimmed.contains("Staged") && trimmed.contains("hook") {
            continue;
        }
        if trimmed.contains("XPIA") || trimmed.starts_with("Script started on") {
            continue;
        }
        // Skip Warning lines from copilot config validation
        if trimmed.starts_with("Warning:") {
            continue;
        }
        // Skip single-character progress indicator lines (● C\n  o\n  n\n...) but
        // only while still in the leading-noise region.  Once real content has been
        // emitted a short line like "OK" or "No" is valid response text.
        if result.is_empty() && trimmed.len() <= 2 && !trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('●') {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }

    // Truncate trailing whitespace in-place instead of allocating a new String.
    let len = result.trim_end().len();
    result.truncate(len);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_copilot_noise_removes_usage_stats() {
        let input = "Here is the answer.\nTotal usage est: 1234 tokens\nAPI time spent: 2.3s";
        let result = strip_copilot_noise(input);
        assert_eq!(result, "Here is the answer.");
    }

    #[test]
    fn strip_copilot_noise_removes_bootstrap() {
        let input = "Staged 3 hook files\nXPIA defender loaded\nActual response here.";
        let result = strip_copilot_noise(input);
        assert_eq!(result, "Actual response here.");
    }

    #[test]
    fn strip_copilot_noise_passes_clean_text() {
        let input = "Normal response.\nWith multiple lines.";
        let result = strip_copilot_noise(input);
        assert_eq!(result, "Normal response.\nWith multiple lines.");
    }

    #[test]
    fn strip_copilot_noise_handles_empty() {
        assert_eq!(strip_copilot_noise(""), "");
        assert_eq!(strip_copilot_noise("   \n  \n"), "");
    }

    // ── additional strip_copilot_noise contract tests ─────────────────────────

    /// Warning: lines from Copilot config validation must be stripped.
    #[test]
    fn strip_copilot_noise_removes_warning_lines() {
        let input = "Warning: Could not enable MCP server\nActual response.";
        let result = strip_copilot_noise(input);
        assert_eq!(
            result, "Actual response.",
            "Warning: prefix lines must be stripped"
        );
    }

    /// Bullet (●) progress-indicator lines must be stripped.
    #[test]
    fn strip_copilot_noise_removes_bullet_progress_lines() {
        let input = "● Connecting to agent\nActual response.";
        let result = strip_copilot_noise(input);
        assert_eq!(
            result, "Actual response.",
            "Lines starting with ● must be stripped"
        );
    }

    /// Lines that are 1 or 2 characters (progress spinners) must be stripped.
    #[test]
    fn strip_copilot_noise_removes_one_and_two_char_lines() {
        let input = "a\nbc\nActual response.";
        let result = strip_copilot_noise(input);
        assert_eq!(
            result, "Actual response.",
            "1-2 char lines must be treated as noise and stripped"
        );
    }

    /// All recognised footer marker prefixes must trigger the stop-reading gate.
    #[test]
    fn strip_copilot_noise_removes_all_footer_marker_variants() {
        let markers = [
            "Total usage est: 1234 tokens",
            "API time spent: 2.3s",
            "Total session time: 10s",
            "Changes 5",
            "Requests 3",
            "Tokens 100",
        ];
        for marker in &markers {
            let input = format!("Response text.\n{marker}\nmore stuff");
            let result = strip_copilot_noise(&input);
            assert_eq!(
                result, "Response text.",
                "Footer marker '{marker}' should stop output"
            );
        }
    }

    /// Mixed noise + real content: all noise categories before and after
    /// the real content must be stripped; footer must truncate.
    #[test]
    fn strip_copilot_noise_mixed_noise_and_content() {
        let input = concat!(
            "● Setting up\n",
            "Warning: something minor\n",
            "a\n",
            "b\n",
            "Here is the actual answer.\n",
            "It continues here.\n",
            "Total usage est: 500 tokens\n",
            "API time spent: 1.2s\n"
        );
        let result = strip_copilot_noise(input);
        assert_eq!(result, "Here is the actual answer.\nIt continues here.");
    }

    /// Short lines that appear MID-response (after real content) must NOT be
    /// stripped — "OK", "No", "Go" are valid LLM responses.
    #[test]
    fn strip_copilot_noise_preserves_short_lines_after_content() {
        let input = "Here is my answer.\nOK\nMore details follow.";
        let result = strip_copilot_noise(input);
        assert!(
            result.contains("OK"),
            "short line after real content must be preserved: got {result:?}"
        );
        assert_eq!(result, "Here is my answer.\nOK\nMore details follow.");
    }

    /// A three-character line must NOT be stripped (only <=2 are noise).
    #[test]
    fn strip_copilot_noise_preserves_three_char_lines() {
        let input = "yes\nActual response.";
        let result = strip_copilot_noise(input);
        assert!(
            result.contains("yes"),
            "3-char lines must not be stripped: got {result:?}"
        );
    }

    /// Meaningful multi-line responses must pass through unchanged.
    #[test]
    fn strip_copilot_noise_preserves_meaningful_multiline_response() {
        let input = "Line one of the response.\nLine two of the response.\nLine three.";
        let result = strip_copilot_noise(input);
        assert_eq!(result, input.trim());
    }
}
