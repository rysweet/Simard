//! Incremental sanitizer for the meeting agent proxy's streamed stdout.
//!
//! Copilot/Claude CLI stdout wraps the substantive reply in usage stats,
//! bootstrap banners and progress markers. Historically these were stripped in
//! one pass over the *completed* output (`strip_copilot_noise`). True
//! incremental streaming (the #2586 follow-up) needs the SAME cleaning applied
//! line-by-line as bytes arrive, so the fragments the operator sees stream
//! match — by construction — the text that is finally persisted.
//!
//! [`StreamingSanitizer`] is that single source of truth: feed it raw stdout
//! lines with [`StreamingSanitizer::push_line`], forward each returned delta to
//! the live stream, and call [`StreamingSanitizer::finish`] for the assembled
//! clean reply. Feeding every line of a completed output then calling `finish`
//! yields exactly what the old whole-output pass produced — the free function
//! [`strip_copilot_noise`] is that one-shot form, kept so existing callers and
//! tests are unchanged.

/// Incremental line filter mirroring the legacy `strip_copilot_noise` pass.
///
/// Stateful across lines: it suppresses leading blank lines until the first
/// substantive line and, once a usage/stats trailer appears, drops every
/// subsequent line (the trailer marks the end of the conversational reply).
#[derive(Debug, Default)]
pub struct StreamingSanitizer {
    /// True once a kept line has been emitted. Mirrors the old pass's
    /// `result.is_empty()` leading-blank suppression.
    started: bool,
    /// Once a usage/stats trailer line is seen every later line is dropped.
    skip_rest: bool,
    /// Assembled clean reply (kept lines, each with its trailing newline).
    cleaned: String,
}

impl StreamingSanitizer {
    /// A fresh sanitizer with no lines consumed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw stdout line. Returns `Some(delta)` — the exact fragment to
    /// stream (the kept line plus its newline) — or `None` when the line is
    /// noise and produces no output.
    pub fn push_line(&mut self, line: &str) -> Option<String> {
        let trimmed = line.trim();

        // Suppress leading blank lines until the first substantive line.
        if !self.started && trimmed.is_empty() {
            return None;
        }
        // Usage/stats trailer: drop this and every subsequent line.
        if trimmed.starts_with("Total usage est:")
            || trimmed.starts_with("API time spent:")
            || trimmed.starts_with("Total session time:")
            || trimmed.starts_with("Changes ")
            || trimmed.starts_with("Requests ")
            || trimmed.starts_with("Tokens ")
        {
            self.skip_rest = true;
            return None;
        }
        if self.skip_rest {
            return None;
        }
        if trimmed.contains("Staged") && trimmed.contains("hook") {
            return None;
        }
        if trimmed.contains("XPIA") || trimmed.starts_with("Script started on") {
            return None;
        }
        if trimmed.starts_with("Warning:") {
            return None;
        }
        if trimmed.len() <= 2 && !trimmed.is_empty() {
            return None;
        }
        if trimmed.starts_with('●') {
            return None;
        }

        self.started = true;
        let mut delta = String::with_capacity(line.len() + 1);
        delta.push_str(line);
        delta.push('\n');
        self.cleaned.push_str(&delta);
        Some(delta)
    }

    /// The assembled clean reply, trimmed — identical to running the legacy
    /// whole-output `strip_copilot_noise` over the same lines.
    pub fn finish(&self) -> String {
        self.cleaned.trim().to_string()
    }
}

/// One-shot form mirroring the legacy `strip_copilot_noise`: feed every line of
/// `raw` through a fresh [`StreamingSanitizer`] and return
/// [`StreamingSanitizer::finish`].
pub fn strip_copilot_noise(raw: &str) -> String {
    let mut sanitizer = StreamingSanitizer::new();
    for line in raw.lines() {
        let _ = sanitizer.push_line(line);
    }
    sanitizer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_removes_usage_stats() {
        let input = "Here is the answer.\nTotal usage est: 1234 tokens\nAPI time spent: 2.3s";
        assert_eq!(strip_copilot_noise(input), "Here is the answer.");
    }

    #[test]
    fn one_shot_removes_bootstrap() {
        let input = "Staged 3 hook files\nXPIA defender loaded\nActual response here.";
        assert_eq!(strip_copilot_noise(input), "Actual response here.");
    }

    #[test]
    fn one_shot_passes_clean_text() {
        let input = "Normal response.\nWith multiple lines.";
        assert_eq!(
            strip_copilot_noise(input),
            "Normal response.\nWith multiple lines."
        );
    }

    /// The core streaming invariant: concatenating the incremental deltas and
    /// trimming equals the one-shot whole-output pass — streamed == persisted
    /// by construction.
    #[test]
    fn incremental_deltas_concat_equals_one_shot() {
        let raw =
            "\n\nTotal session time: 9s\nHello there.\n●\nx\nSecond line.\nTotal usage est: 5";
        let mut sanitizer = StreamingSanitizer::new();
        let mut streamed = String::new();
        for line in raw.lines() {
            if let Some(delta) = sanitizer.push_line(line) {
                streamed.push_str(&delta);
            }
        }
        assert_eq!(streamed.trim(), sanitizer.finish());
        assert_eq!(sanitizer.finish(), strip_copilot_noise(raw));
    }

    /// A leading usage trailer must suppress every later line, and short/marker
    /// lines are dropped, incrementally.
    #[test]
    fn incremental_drops_noise_lines() {
        let mut s = StreamingSanitizer::new();
        assert_eq!(s.push_line(""), None, "leading blank suppressed");
        assert_eq!(s.push_line("●●● progress"), None, "bullet marker dropped");
        assert_eq!(s.push_line("ok"), None, "<=2 char line dropped");
        assert_eq!(
            s.push_line("Real content line."),
            Some("Real content line.\n".to_string())
        );
        assert_eq!(
            s.push_line("Total usage est: 9"),
            None,
            "trailer starts skip"
        );
        assert_eq!(s.push_line("trailing noise"), None, "post-trailer dropped");
        assert_eq!(s.finish(), "Real content line.");
    }
}
