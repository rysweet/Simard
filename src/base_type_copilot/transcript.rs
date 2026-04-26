//! Transcript parsing helpers for the copilot adapter.

/// Parse the raw transcript text to isolate the copilot's response.
///
/// The transcript from `script` contains (in order):
///   Script started on ...
///   <bash prompt> <command echo>
///   <copilot bootstrap lines — hooks, XPIA defender, etc.>
///   <actual LLM response>
///   Total usage est: ...
///   API time spent: ...
///   bash-5.2$ exit
///   Script done on ...
///
/// We find the end of bootstrap noise and the start of usage stats to
/// isolate the actual LLM response in between.
pub fn extract_response_from_transcript(transcript: &str) -> String {
    let lines: Vec<&str> = transcript.lines().collect();
    // Also try pipe-delimited (preview format) if no newlines found.
    let lines = if lines.len() <= 1 && transcript.contains(" | ") {
        transcript.split(" | ").collect::<Vec<_>>()
    } else {
        lines
    };

    let mut response_start = 0;
    let mut response_end = lines.len();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Bootstrap output: hook staging, XPIA defender, prompt file creation
        if trimmed.contains("Staged") && trimmed.contains("hook")
            || trimmed.contains("XPIA")
            || trimmed.contains("SIMARD_PROMPT_FILE")
            || trimmed.contains("amplihack copilot")
            || trimmed.starts_with("Script started on")
            || trimmed.starts_with("bash-") && trimmed.contains("$") && trimmed.contains("cat ")
            || is_transcript_noise_line(trimmed)
        {
            response_start = i + 1;
        }
        // Usage stats / session footer
        if is_copilot_footer_line(trimmed) && response_end == lines.len() {
            response_end = i;
        }
        // Shell exit / script done
        if (trimmed == "exit"
            || trimmed.ends_with("$ exit")
            || trimmed.starts_with("Script done on"))
            && response_end == lines.len()
        {
            response_end = i;
        }
    }

    if response_start >= response_end {
        // Delimiters not found — strip known noise lines from PTY output
        let stripped: String = lines
            .iter()
            .filter(|l| {
                let t = l.trim();
                !(t.is_empty()
                    || t.starts_with("Script ")
                    || t.contains("amplihack copilot")
                    || t.contains("SIMARD_PROMPT_FILE")
                    || t == "exit"
                    || t.ends_with("$ exit")
                    || is_copilot_footer_line(t)
                    || is_transcript_noise_line(t))
            })
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        return stripped;
    }

    let body: String = lines[response_start..response_end]
        .iter()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !is_copilot_footer_line(t) && !is_transcript_noise_line(t)
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    body
}

/// Recognize Copilot CLI session-footer / telemetry lines that must never
/// appear in the extracted assistant response.
///
/// Includes both the legacy `Total usage est:` / `API time spent:` /
/// `Total session time:` markers and the newer billing-summary footer
/// emitted by Copilot CLI ≥1.x:
///
/// ```text
/// Changes   +0 -0
/// Requests  7.5 Premium (10s)
/// ```
///
/// Without this guard the chat dashboard echoes the telemetry line back
/// to the user as if it were the assistant's reply (issue #1062).
pub fn is_copilot_footer_line(trimmed: &str) -> bool {
    if trimmed.starts_with("Total usage est:")
        || trimmed.starts_with("API time spent:")
        || trimmed.starts_with("Total session time:")
    {
        return true;
    }
    // Newer Copilot CLI billing summary lines.
    if trimmed.starts_with("Changes") && (trimmed.contains(" +") || trimmed.contains(" -")) {
        return true;
    }
    if trimmed.starts_with("Requests")
        && (trimmed.contains("Premium") || trimmed.contains("Free") || trimmed.contains('('))
    {
        return true;
    }
    false
}

/// Detect transcript lines that are infrastructure artefacts rather than
/// conversational LLM output.
///
/// Filters:
///   * Shell `time` builtin output: `real 0m1.234s`, `user 0m0.123s`, `sys ...`
///   * Hook telemetry: `Staged ... hook`, `Loaded hook`, `Hook fired:`
///   * File-system artefacts emitted by tool plumbing: `Created file ...`,
///     `Modified file ...`, `Deleted file ...`, `Wrote file ...`
fn is_transcript_noise_line(trimmed: &str) -> bool {
    // Shell `time` builtin output (POSIX format: "real\t0m1.234s")
    for prefix in ["real\t", "real ", "user\t", "user ", "sys\t", "sys "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            // Looks like `0m1.234s` or `1.234s` — digit-led, ends with 's'
            let rest = rest.trim_start();
            if rest.ends_with('s')
                && rest
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
            {
                return true;
            }
        }
    }
    // Hook telemetry lines
    if (trimmed.contains("hook") || trimmed.contains("Hook"))
        && (trimmed.starts_with("Staged")
            || trimmed.starts_with("Loaded")
            || trimmed.starts_with("Unloaded")
            || trimmed.starts_with("Hook fired")
            || trimmed.starts_with("Hook:")
            || trimmed.starts_with("[hook]"))
    {
        return true;
    }
    // File-system artefacts from tool plumbing
    for prefix in [
        "Created file ",
        "Created file:",
        "Modified file ",
        "Modified file:",
        "Deleted file ",
        "Deleted file:",
        "Wrote file ",
        "Wrote file:",
    ] {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }
    // Amplihack CLI startup banners and version-update nags. These are
    // wrapped in ANSI color codes by the CLI, so test against the
    // ANSI-stripped form. The leading `ℹ` glyph amplihack prints is a
    // multi-byte UTF-8 character, so we match on the substring after it.
    let stripped = strip_ansi(trimmed);
    let stripped_trim = stripped.trim();
    let without_info_glyph = stripped_trim
        .trim_start_matches(|c: char| !c.is_ascii() || c.is_whitespace())
        .trim_start();
    if stripped_trim.contains("amplihack is available")
        || stripped_trim.starts_with("Run 'amplihack update'")
        || without_info_glyph.starts_with("NODE_OPTIONS=")
        || without_info_glyph.starts_with("amplihack ")
        || stripped_trim.starts_with("amplihack: ")
    {
        return true;
    }
    false
}

/// Remove ANSI escape sequences (CSI `\x1b[…m` color codes) from a line
/// so noise-detection can match on the visible text alone.
pub fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // CSI: \x1b[ … final-byte-in-0x40..=0x7e
            let mut j = i + 2;
            while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                j += 1;
            }
            i = j.saturating_add(1);
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}
