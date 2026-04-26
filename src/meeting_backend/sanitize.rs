//! Sanitization helpers for meeting backend outputs.

use crate::base_types::BaseTypeOutcome;

/// Extract the conversational response text from a `BaseTypeOutcome`,
/// stripping agentic tool-call log noise that garbles terminal display.
pub fn extract_response(outcome: &BaseTypeOutcome) -> String {
    sanitize_agent_output(outcome.execution_summary.trim())
}

/// Remove agentic tool-call log lines, ANSI escape codes, and infrastructure
/// noise from LLM output so the dashboard displays only conversational content.
pub fn sanitize_agent_output(raw: &str) -> String {
    let raw = strip_ansi_escapes(raw);

    let mut result = String::with_capacity(raw.len());
    let mut in_tool_block = false;
    let mut consecutive_blank = 0u8;

    for line in raw.lines() {
        let trimmed = line.trim();

        if is_tool_block_open(trimmed) {
            in_tool_block = true;
            continue;
        }

        if in_tool_block {
            if is_tool_block_close(trimmed) {
                in_tool_block = false;
            }
            continue;
        }

        if is_tool_call_line(trimmed) {
            continue;
        }

        if is_agent_noise_line(trimmed) {
            continue;
        }

        if trimmed.is_empty() {
            consecutive_blank += 1;
            if consecutive_blank <= 2 {
                result.push('\n');
            }
            continue;
        }
        consecutive_blank = 0;

        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

/// Strip ANSI escape sequences (CSI sequences: ESC [ params final-byte).
pub fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some(next) = chars.next()
                && next == '['
            {
                for ch in chars.by_ref() {
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Detect lines that are agent infrastructure noise, not conversational content.
fn is_agent_noise_line(trimmed: &str) -> bool {
    if trimmed.len() > 20
        && trimmed.starts_with("202")
        && let Some('-') = trimmed.chars().nth(4)
    {
        return true;
    }
    if trimmed.contains("newer version of amplihack") || trimmed.contains("amplihack update") {
        return true;
    }
    if trimmed.contains("NODE_OPTIONS=") {
        return true;
    }
    if trimmed.starts_with("ACTION:")
        || trimmed.starts_with("EXPLANATION:")
        || trimmed.starts_with("CONFIDENCE:")
    {
        return true;
    }
    if trimmed.starts_with("Changes ") && trimmed.contains("Requests") {
        return true;
    }
    if trimmed.starts_with("Tokens ")
        && (trimmed.contains('\u{2191}') || trimmed.contains('\u{2193}'))
    {
        return true;
    }
    if trimmed.contains("launching copilot") && trimmed.contains("binary=") {
        return true;
    }
    if trimmed.starts_with("\u{2139} ") || trimmed.starts_with("\u{2713} ") {
        return true;
    }
    if trimmed.contains(" INFO ") && (trimmed.contains("simard") || trimmed.contains("rustyclawd"))
    {
        return true;
    }
    false
}

fn is_tool_block_open(trimmed: &str) -> bool {
    for tag in &[
        "<tool_call",
        "<tool_result",
        "<function_call",
        "<invoke",
        "<function",
    ] {
        if trimmed.starts_with(tag) {
            return true;
        }
    }
    false
}

fn is_tool_block_close(trimmed: &str) -> bool {
    for tag in &[
        "</tool_call",
        "</tool_result",
        "</function_call",
        "</invoke",
        "</function",
    ] {
        if trimmed.contains(tag) {
            return true;
        }
    }
    false
}

fn is_tool_call_line(trimmed: &str) -> bool {
    if trimmed.starts_with("[tool_call:") || trimmed.starts_with("[tool_result:") {
        return true;
    }
    if trimmed.starts_with("[Tool ") && trimmed.contains("executed") {
        return true;
    }
    if trimmed.starts_with("Running tool:") || trimmed.starts_with("Tool output:") {
        return true;
    }
    false
}
