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
}
