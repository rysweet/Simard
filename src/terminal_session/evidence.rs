use crate::sanitization::sanitize_terminal_text;

use super::types::TerminalStep;

pub(crate) fn transcript_preview(transcript: &str) -> String {
    let sanitized = sanitize_terminal_text(transcript);
    let mut normalized = transcript_content_lines(&sanitized).join(" | ");

    if normalized.len() > 512 {
        normalized.truncate(512);
        normalized.push_str("...");
    }

    normalized
}

pub(crate) fn terminal_step_evidence(steps: &[TerminalStep]) -> Vec<String> {
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            format!(
                "terminal-step-{}={}",
                index + 1,
                compact_terminal_evidence_value(&render_terminal_step(step), 160)
            )
        })
        .collect()
}

pub(crate) fn terminal_checkpoint_evidence(steps: &[TerminalStep]) -> Vec<String> {
    steps
        .iter()
        .filter_map(|step| match step {
            TerminalStep::WaitFor(expected) => Some(expected.as_str()),
            TerminalStep::Input(_) => None,
        })
        .enumerate()
        .map(|(index, expected)| {
            format!(
                "terminal-checkpoint-{}={}",
                index + 1,
                compact_terminal_evidence_value(expected, 160)
            )
        })
        .collect()
}

pub(crate) fn terminal_last_output_line(
    transcript: &str,
    steps: &[TerminalStep],
) -> Option<String> {
    let input_commands = steps
        .iter()
        .filter_map(|step| match step {
            TerminalStep::Input(command) => Some(sanitize_terminal_text(command)),
            TerminalStep::WaitFor(_) => None,
        })
        .collect::<Vec<_>>();
    transcript_content_lines(transcript)
        .into_iter()
        .rev()
        .map(sanitize_terminal_text)
        .find(|line| is_meaningful_terminal_output(line, &input_commands))
        .map(|line| compact_terminal_evidence_value(&line, 160))
}

fn is_meaningful_terminal_output(line: &str, input_commands: &[String]) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed == "exit"
        || trimmed.ends_with("$ exit")
        || trimmed.ends_with("# exit")
    {
        return false;
    }

    !input_commands.iter().any(|command| {
        trimmed == command
            || trimmed.ends_with(&format!("$ {command}"))
            || trimmed.ends_with(&format!("# {command}"))
    })
}

pub(crate) fn transcript_content_lines_iter(transcript: &str) -> impl Iterator<Item = &str> + '_ {
    transcript.lines().map(str::trim).filter(|line| {
        !line.is_empty()
            && !line.starts_with("Script started on ")
            && !line.starts_with("Script done on ")
    })
}

pub(crate) fn transcript_content_lines(transcript: &str) -> Vec<&str> {
    transcript_content_lines_iter(transcript).collect()
}

pub(crate) fn render_terminal_step(step: &TerminalStep) -> String {
    match step {
        TerminalStep::Input(command) => format!("input: {command}"),
        TerminalStep::WaitFor(expected) => format!("wait-for: {expected}"),
    }
}

pub(crate) fn compact_terminal_evidence_value(raw: &str, limit: usize) -> String {
    let mut normalized = sanitize_terminal_text(raw)
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    if normalized.len() > limit {
        normalized.truncate(limit);
        normalized.push_str("...");
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_preview_redacts_secret_like_lines() {
        let preview = transcript_preview(
            "Script started on 2026-03-30\nAuthorization: Bearer top-secret\nplain output\ntoken=abc123\nScript done on 2026-03-30",
        );

        assert_eq!(
            preview,
            "Authorization: [REDACTED] | plain output | token=[REDACTED]"
        );
    }

    #[test]
    fn terminal_step_and_checkpoint_evidence_preserve_operator_visible_flow() {
        let steps = vec![
            TerminalStep::Input("printf \"ready\\n\"".to_string()),
            TerminalStep::WaitFor("ready".to_string()),
            TerminalStep::Input("/status".to_string()),
        ];

        assert_eq!(
            terminal_step_evidence(&steps),
            vec![
                "terminal-step-1=input: printf \"ready\\n\"".to_string(),
                "terminal-step-2=wait-for: ready".to_string(),
                "terminal-step-3=input: /status".to_string(),
            ]
        );
        assert_eq!(
            terminal_checkpoint_evidence(&steps),
            vec!["terminal-checkpoint-1=ready".to_string()]
        );
    }

    #[test]
    fn terminal_last_output_line_ignores_script_preamble_and_sanitizes_control_text() {
        let transcript = "Script started on 2025-03-29 12:00:00+00:00 [COMMAND=\"/usr/bin/bash --noprofile --norc -i\" <not executed on terminal>]\nterminal-ready\n\u{1b}[32mterminal-ok\u{1b}[0m\nScript done on 2025-03-29 12:00:01+00:00 [COMMAND_EXIT_CODE=\"0\"]";
        assert_eq!(
            terminal_last_output_line(transcript, &[]),
            Some("terminal-ok".to_string())
        );
    }

    #[test]
    fn terminal_last_output_line_ignores_prompt_wrapped_inputs_and_exit() {
        let transcript = "pwd\nprintf \"terminal-foundation-ok\\n\"\nbash-5.2$ pwd\n/home/azureuser/src/Simard\nbash-5.2$ printf \"terminal-foundation-ok\\n\"\nterminal-foundation-ok\nbash-5.2$ exit";
        let steps = vec![
            TerminalStep::Input("pwd".to_string()),
            TerminalStep::Input("printf \"terminal-foundation-ok\\n\"".to_string()),
        ];
        assert_eq!(
            terminal_last_output_line(transcript, &steps),
            Some("terminal-foundation-ok".to_string())
        );
    }

    #[test]
    fn compact_terminal_evidence_value_replaces_newlines_and_truncates() {
        let raw = "line1\nline2\tline3";
        assert_eq!(compact_terminal_evidence_value(raw, 12), "line1\\nline2...");
    }

    // ---------------------------------------------------------------------
    // Issue #1590 follow-up — UTF-8 truncation panic regression tests.
    //
    // `String::truncate(N)` panics if `N` is not a UTF-8 char boundary.
    // The transcript_preview / compact_terminal_evidence_value sites call
    // `normalized.truncate(N)` with `N` as a byte budget, so any input
    // where a multi-byte sequence (em-dash, CJK, emoji) crosses byte `N`
    // panics the runtime worker. Confirmed in production journal:
    //
    //   thread 'tokio-rt-worker' panicked at src/terminal_session/evidence.rs:10:20
    //
    // These tests assert the helpers do not panic on multi-byte input at
    // the budget boundary. Until the implementation switches to
    // `crate::util::string_truncate::truncate_to_char_boundary`, they fail
    // by panicking.
    // ---------------------------------------------------------------------

    /// Build a string whose total byte length exceeds `budget` and where a
    /// multi-byte char straddles `budget`. Returns a string built from
    /// `prefix_ascii_bytes` ASCII bytes, then the multi-byte char `mb`,
    /// then enough trailing ASCII to push past `budget`. The caller picks
    /// `prefix_ascii_bytes` so that the multi-byte char crosses the
    /// boundary they want to test.
    fn straddle(prefix_ascii_bytes: usize, mb: char, trailing: usize) -> String {
        let mut s = String::with_capacity(prefix_ascii_bytes + 4 + trailing);
        s.push_str(&"a".repeat(prefix_ascii_bytes));
        s.push(mb);
        s.push_str(&"b".repeat(trailing));
        s
    }

    #[test]
    fn transcript_preview_does_not_panic_on_em_dash_at_byte_512_boundary() {
        // `transcript_preview` uses the literal budget `512`. We need the
        // joined preview line to be longer than 512 bytes with an em-dash
        // straddling byte 512. The join inserts " | " between non-script
        // lines; a single content line with an em-dash at byte 511 is
        // simplest.
        let body = straddle(511, '—', 100);
        let transcript = format!("Script started on 2026-04-01\n{body}\nScript done on 2026-04-01");

        // Sanity: the body's em-dash straddles byte 512.
        assert!(
            !body.is_char_boundary(512),
            "test fixture invariant: em-dash should straddle byte 512"
        );

        // Today: panics with `assertion failed: self.is_char_boundary(new_len)`.
        // After fix: returns a sanitized, valid-UTF-8 preview.
        let preview = transcript_preview(&transcript);

        assert!(
            std::str::from_utf8(preview.as_bytes()).is_ok(),
            "preview must remain valid UTF-8"
        );
        assert!(
            preview.ends_with("..."),
            "preview must still indicate truncation; got {preview:?}"
        );
    }

    #[test]
    fn compact_terminal_evidence_value_does_not_panic_on_em_dash_at_boundary() {
        // limit = 100; em-dash straddles byte 100.
        let raw = straddle(99, '—', 50);
        let compacted = compact_terminal_evidence_value(&raw, 100);
        assert!(
            std::str::from_utf8(compacted.as_bytes()).is_ok(),
            "compacted value must remain valid UTF-8"
        );
        assert!(compacted.ends_with("..."));
    }

    #[test]
    fn compact_terminal_evidence_value_does_not_panic_on_emoji_at_boundary() {
        // 4-byte emoji 🎉 straddles byte 50; limit = 50.
        let raw = straddle(48, '🎉', 30);
        let compacted = compact_terminal_evidence_value(&raw, 50);
        assert!(
            std::str::from_utf8(compacted.as_bytes()).is_ok(),
            "compacted value must remain valid UTF-8"
        );
        assert!(compacted.ends_with("..."));
    }

    #[test]
    fn compact_terminal_evidence_value_does_not_panic_on_cjk_at_boundary() {
        // 3-byte CJK '日' straddles byte 64; limit = 64.
        let raw = straddle(63, '日', 30);
        let compacted = compact_terminal_evidence_value(&raw, 64);
        assert!(
            std::str::from_utf8(compacted.as_bytes()).is_ok(),
            "compacted value must remain valid UTF-8"
        );
        assert!(compacted.ends_with("..."));
    }
}
