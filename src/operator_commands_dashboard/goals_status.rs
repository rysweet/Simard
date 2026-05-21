//! Plain-English Status + Detail rendering for the dashboard Goals tab
//! (issue #1684).
//!
//! The OODA cycle populates `ActiveGoal::current_activity` with a raw
//! brain-log string such as:
//!
//! ```text
//! advance-goal: brain: continue_skipping (brain-error fallback: base type
//! 'ooda-brain' failed during invocation: no JSON object found in…
//! ```
//!
//! Rendering that verbatim in the operator dashboard leaks private vocabulary
//! and the mid-sentence "…" truncation makes it unreadable. This module
//! converts the raw string into:
//!
//! * a small `status_chip` ("Working" / "Skipped" / "Failed" /
//!   "Spawned engineer" / "Waiting"), and
//! * a stripped one-line `detail` (hard-capped at 140 chars).
//!
//! The unredacted original is preserved separately as `detail_full` so the
//! frontend can offer click-to-expand without losing information.
//!
//! Pure function — no IO, no allocations beyond the returned strings — so it
//! is exhaustively unit-tested below.

/// Status classification derived from the raw brain-log activity string.
///
/// String form matches the chip labels the dashboard renders verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusChip {
    Working,
    Skipped,
    Failed,
    SpawnedEngineer,
    Waiting,
}

impl StatusChip {
    /// Operator-facing label rendered inside the chip element.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Skipped => "Skipped",
            Self::Failed => "Failed",
            Self::SpawnedEngineer => "Spawned engineer",
            Self::Waiting => "Waiting",
        }
    }
}

/// Hard cap on the `detail` field (acceptance criterion #1).
pub(crate) const DETAIL_MAX_CHARS: usize = 140;

/// Tokens / phrases stripped from `detail` because they are internal brain
/// vocabulary or leaked launcher noise (acceptance criterion #1). Order
/// matters: longer phrases first so we do not leave dangling fragments
/// behind when a shorter substring is contained in a longer one.
const NOISE_PHRASES: &[&str] = &[
    "brain-error fallback",
    "goal-action parse failed",
    "spawn_engineer dispatched",
    "continue_skipping",
    "ooda-brain",
    "brain:",
];

/// Substrings that mark a leaked launcher-noise line. Any line containing
/// one of these is dropped wholesale before further cleanup.
const LAUNCHER_NOISE_MARKERS: &[&str] = &[
    "Warning: Could not read Copilot config.json",
    "Could not read Copilot config",
];

/// Convert a raw `current_activity` string into the public dashboard fields
/// `(status_chip, detail, detail_full)`.
///
/// * `status_chip` — coarse status derived from the action verb / outcome.
/// * `detail` — plain-English summary with brain vocabulary and launcher
///   noise stripped, hard-capped at [`DETAIL_MAX_CHARS`] chars.
/// * `detail_full` — the unmodified original string (empty when `raw` is
///   `None`) for click-to-expand UI.
///
/// Pure: same input always yields the same output. See unit tests below.
pub(crate) fn render_status_and_detail(raw: Option<&str>) -> (StatusChip, String, String) {
    let raw_owned = raw.unwrap_or("").to_string();

    // Waiting branch — no activity recorded yet.
    if raw.is_none() || raw_owned.trim().is_empty() {
        return (StatusChip::Waiting, String::new(), raw_owned);
    }

    let chip = classify(&raw_owned);
    let detail = clean_detail(&raw_owned);
    let detail = cap_chars(&detail, DETAIL_MAX_CHARS);

    (chip, detail, raw_owned)
}

/// Pick the status chip from the raw string.
///
/// Precedence (first match wins):
/// 1. Outcome marker " (failed)" → `Failed`.
/// 2. "spawn_engineer dispatched" → `SpawnedEngineer`.
/// 3. "continue_skipping" → `Skipped`.
/// 4. Default → `Working` (any other successful action / advance-goal).
fn classify(raw: &str) -> StatusChip {
    if raw.contains(" (failed)") || raw.contains("(failed):") {
        StatusChip::Failed
    } else if raw.contains("spawn_engineer dispatched") {
        StatusChip::SpawnedEngineer
    } else if raw.contains("continue_skipping") {
        StatusChip::Skipped
    } else {
        StatusChip::Working
    }
}

/// Strip brain vocabulary, launcher noise and structural punctuation from
/// the raw string, then normalise whitespace.
fn clean_detail(raw: &str) -> String {
    // 1. Process per line so launcher-noise stripping has a hard right
    //    boundary at end-of-line (no risk of swallowing the next prose line).
    let stripped_lines: Vec<String> = raw
        .split('\n')
        .map(|line| {
            let mut line = line.to_string();
            for marker in LAUNCHER_NOISE_MARKERS {
                line = strip_phrase_until_terminator(&line, marker);
            }
            line
        })
        .collect();
    let mut s = stripped_lines.join(" ");

    // 2. Drop any parenthetical group whose body contains a noise marker
    //    (brain vocabulary or launcher warning). Examples: `(brain-error
    //    fallback: ...)`, `(spawn_engineer dispatched ...)`. The grouping
    //    is conservative — only top-level pairs are dropped, never nested.
    s = drop_noisy_parens(&s);

    // 3. Strip the brain-vocabulary phrases (simple substring replacement —
    //    every phrase in `NOISE_PHRASES` is a fixed, unambiguous token).
    for phrase in NOISE_PHRASES {
        s = s.replace(phrase, " ");
    }

    // 4. Drop the redundant `(failed)` / `(failed):` markers: the chip
    //    already conveys "Failed", repeating it in the detail wastes pixels.
    s = s.replace("(failed):", " ").replace("(failed)", " ");

    // 5. Collapse leftover orphan punctuation (stray `:`, `()`, `''`).
    s = collapse_punctuation(&s);

    // 6. Collapse runs of whitespace into single spaces and trim.
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Remove top-level `(...)` groups whose body contains any noise marker.
/// Nested parens inside a noisy group are removed with the parent.
fn drop_noisy_parens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '(' {
            // Find matching `)` — track nesting depth.
            let start = i;
            let mut depth = 1;
            let mut end = None;
            for (j, cj) in s[i + 1..].char_indices() {
                match cj {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i + 1 + j);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(end_idx) = end {
                let body = &s[start + 1..end_idx];
                let is_noisy = NOISE_PHRASES.iter().any(|p| body.contains(p))
                    || LAUNCHER_NOISE_MARKERS.iter().any(|m| body.contains(m));
                if is_noisy {
                    // Skip the entire `(…)` group, including the closing `)`.
                    while let Some(&(k, _)) = chars.peek() {
                        if k > end_idx {
                            break;
                        }
                        chars.next();
                    }
                    out.push(' ');
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Remove `marker` and the text after it up to the next `.`, `,`, `;`, `:`
/// not directly part of a URL, or end-of-string. Leaves surrounding prose
/// intact so legitimate text before the launcher warning survives.
fn strip_phrase_until_terminator(s: &str, marker: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0;
    while let Some(rel) = s[cursor..].find(marker) {
        let start = cursor + rel;
        out.push_str(&s[cursor..start]);
        // Find the next terminator after the marker.
        let after = start + marker.len();
        let end = s[after..]
            .find(['.', '\n'])
            .map(|t| after + t + 1)
            .unwrap_or(s.len());
        cursor = end;
    }
    out.push_str(&s[cursor..]);
    out
}

/// Remove orphan punctuation left behind by phrase stripping:
/// * lone `:` / `(` / `)` characters with no surrounding content,
/// * empty quoted strings `''`, `'  '`, `""`, `"  "`,
/// * trailing structural punctuation.
fn collapse_punctuation(s: &str) -> String {
    // Drop empty single/double-quoted strings (with optional inner spaces)
    // e.g. "base type '   '" after `ooda-brain` is stripped from `'ooda-brain'`.
    let mut s = s.to_string();
    for _ in 0..3 {
        // Multiple passes in case stripping reveals more empties.
        let before = s.clone();
        s = drop_empty_quoted(&s, '\'');
        s = drop_empty_quoted(&s, '"');
        if s == before {
            break;
        }
    }

    // Iterate char-by-char, dropping orphan structural punctuation.
    let mut out = String::with_capacity(s.len());
    let mut prev: Option<char> = None;
    for c in s.chars() {
        let drop_if_lone = matches!(c, ':' | '(' | ')');
        if drop_if_lone {
            let last_non_space = out.chars().rev().find(|c| !c.is_whitespace());
            if last_non_space.is_none()
                || matches!(last_non_space, Some(':' | '(' | ')' | ',' | ';'))
                || prev.map(|p| p.is_whitespace()).unwrap_or(true)
            {
                prev = Some(c);
                continue;
            }
        }
        out.push(c);
        prev = Some(c);
    }

    // Trim trailing structural punctuation that no longer leads anywhere.
    out.trim_end_matches([':', ',', ';', '(', ')', ' '])
        .to_string()
}

/// Single-pass removal of empty quoted strings (`'   '` / `"  "`) using
/// `quote` as the delimiter. Conservative — only drops pairs whose inner
/// content is whitespace-only, so prose strings stay intact.
fn drop_empty_quoted(s: &str, quote: char) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == quote {
            // Look ahead for a closing quote on the same "segment".
            if let Some(rel_end) = s[i + 1..].find(quote) {
                let end = i + 1 + rel_end;
                let body = &s[i + 1..end];
                if body.chars().all(|ch| ch.is_whitespace()) {
                    // Skip the empty quoted pair, replacing with a single space.
                    while let Some(&(k, _)) = chars.peek() {
                        if k > end {
                            break;
                        }
                        chars.next();
                    }
                    out.push(' ');
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Truncate `s` to at most `max_chars` Unicode scalar values, appending an
/// ellipsis if truncation occurred. Unlike the brain log's mid-word "…"
/// truncation, this only triggers when the string actually exceeds the cap.
fn cap_chars(s: &str, max_chars: usize) -> String {
    let mut iter = s.char_indices();
    match iter.nth(max_chars) {
        None => s.to_string(),
        Some((byte_pos, _)) => {
            // Try to back up to a word boundary so we don't slice inside a
            // word. Falls back to byte position if no nearby space.
            let slice = &s[..byte_pos];
            let cut = slice.rfind(|c: char| c.is_whitespace()).unwrap_or(byte_pos);
            let trimmed = slice[..cut].trim_end();
            // If backing up to a word boundary cost us more than 20 chars,
            // prefer the hard cut — better to surface more text than less.
            if cut + 20 < byte_pos {
                format!("{}…", slice.trim_end())
            } else {
                format!("{trimmed}…")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- StatusChip basics ------------------------------------------------

    #[test]
    fn chip_labels_match_acceptance_spec() {
        assert_eq!(StatusChip::Working.as_str(), "Working");
        assert_eq!(StatusChip::Skipped.as_str(), "Skipped");
        assert_eq!(StatusChip::Failed.as_str(), "Failed");
        assert_eq!(StatusChip::SpawnedEngineer.as_str(), "Spawned engineer");
        assert_eq!(StatusChip::Waiting.as_str(), "Waiting");
    }

    // ---- Waiting branch ---------------------------------------------------

    #[test]
    fn none_input_yields_waiting() {
        let (chip, detail, full) = render_status_and_detail(None);
        assert_eq!(chip, StatusChip::Waiting);
        assert!(detail.is_empty());
        assert!(full.is_empty());
    }

    #[test]
    fn empty_string_yields_waiting() {
        let (chip, detail, full) = render_status_and_detail(Some(""));
        assert_eq!(chip, StatusChip::Waiting);
        assert!(detail.is_empty());
        assert_eq!(full, "");
    }

    #[test]
    fn whitespace_only_yields_waiting() {
        let (chip, detail, _) = render_status_and_detail(Some("   \n  "));
        assert_eq!(chip, StatusChip::Waiting);
        assert!(detail.is_empty());
    }

    // ---- Skipped branch ---------------------------------------------------

    #[test]
    fn continue_skipping_yields_skipped_chip_and_strips_brain_vocab() {
        let raw = "advance-goal: brain: continue_skipping (brain-error fallback: \
                   base type 'ooda-brain' failed during invocation: no JSON object found in…";
        let (chip, detail, full) = render_status_and_detail(Some(raw));
        assert_eq!(chip, StatusChip::Skipped);
        assert!(
            !detail.contains("brain:"),
            "detail still leaks 'brain:': {detail}"
        );
        assert!(
            !detail.contains("continue_skipping"),
            "detail still leaks 'continue_skipping': {detail}"
        );
        assert!(
            !detail.contains("ooda-brain"),
            "detail still leaks 'ooda-brain': {detail}"
        );
        assert!(
            !detail.contains("brain-error fallback"),
            "detail still leaks 'brain-error fallback': {detail}"
        );
        assert!(detail.starts_with("advance-goal"));
        assert_eq!(full, raw, "detail_full must preserve the original verbatim");
    }

    // ---- Failed branch ----------------------------------------------------

    #[test]
    fn advance_goal_failed_yields_failed_chip() {
        let raw = "advance-goal (failed): goal-action parse failed: missing 'choice' field";
        let (chip, detail, full) = render_status_and_detail(Some(raw));
        assert_eq!(chip, StatusChip::Failed);
        assert!(
            !detail.contains("goal-action parse failed"),
            "detail leaks parse-failed phrase: {detail}"
        );
        assert!(detail.contains("missing 'choice' field"));
        assert_eq!(full, raw);
    }

    #[test]
    fn run_improvement_failed_also_yields_failed_chip() {
        let raw = "run-improvement (failed): patch did not apply cleanly";
        let (chip, _detail, _) = render_status_and_detail(Some(raw));
        assert_eq!(chip, StatusChip::Failed);
    }

    // ---- Spawned-engineer branch -----------------------------------------

    #[test]
    fn spawn_engineer_dispatched_yields_spawned_chip() {
        let raw = "advance-goal: spawn_engineer dispatched: agent='dashboard-1684', \
                   task='Fix unreadable Current Activity column' (goal 'dashboard-1684', pid=12345)";
        let (chip, detail, full) = render_status_and_detail(Some(raw));
        assert_eq!(chip, StatusChip::SpawnedEngineer);
        assert!(
            !detail.contains("spawn_engineer dispatched"),
            "detail still leaks 'spawn_engineer dispatched': {detail}"
        );
        assert!(detail.contains("agent='dashboard-1684'"));
        assert_eq!(full, raw);
    }

    // ---- Working branch ---------------------------------------------------

    #[test]
    fn successful_advance_goal_yields_working_chip() {
        let raw = "advance-goal: opened PR #1685 to fix Current Activity column";
        let (chip, detail, full) = render_status_and_detail(Some(raw));
        assert_eq!(chip, StatusChip::Working);
        assert!(detail.contains("opened PR #1685"));
        assert_eq!(full, raw);
    }

    #[test]
    fn other_action_kinds_default_to_working() {
        let raw = "consolidate-memory: 42 facts merged into long-term store";
        let (chip, _detail, _) = render_status_and_detail(Some(raw));
        assert_eq!(chip, StatusChip::Working);
    }

    // ---- Launcher-noise stripping ----------------------------------------

    #[test]
    fn leaked_copilot_launcher_warning_is_stripped() {
        let raw = "advance-goal: Warning: Could not read Copilot config.json: \
                   No such file. brain: continue_skipping";
        let (chip, detail, full) = render_status_and_detail(Some(raw));
        assert_eq!(chip, StatusChip::Skipped);
        assert!(
            !detail.contains("Could not read Copilot config"),
            "detail leaks launcher warning: {detail}"
        );
        assert!(
            !detail.contains("Warning:"),
            "detail leaks 'Warning:': {detail}"
        );
        assert!(!detail.contains("brain:"));
        assert_eq!(full, raw, "detail_full preserves launcher noise verbatim");
    }

    #[test]
    fn launcher_noise_on_its_own_line_is_dropped() {
        let raw = "advance-goal: dispatching\nWarning: Could not read Copilot config.json: ENOENT\nfinished cleanly";
        let (_chip, detail, _) = render_status_and_detail(Some(raw));
        assert!(detail.contains("dispatching"));
        assert!(detail.contains("finished cleanly"));
        assert!(!detail.contains("Could not read Copilot config"));
    }

    // ---- Truncation behaviour --------------------------------------------

    #[test]
    fn detail_is_hard_capped_at_140_chars() {
        let long_tail = "x".repeat(500);
        let raw = format!("advance-goal: {long_tail}");
        let (_chip, detail, full) = render_status_and_detail(Some(&raw));
        assert!(
            detail.chars().count() <= DETAIL_MAX_CHARS + 1, // +1 for trailing '…'
            "detail length {} exceeds cap {DETAIL_MAX_CHARS}",
            detail.chars().count()
        );
        assert!(
            detail.ends_with('…'),
            "detail must end with '…' when truncated"
        );
        assert_eq!(full, raw, "detail_full carries the untruncated original");
    }

    #[test]
    fn detail_under_cap_is_not_modified_with_ellipsis() {
        let raw = "advance-goal: short and sweet";
        let (_chip, detail, _) = render_status_and_detail(Some(raw));
        assert!(!detail.ends_with('…'));
    }

    #[test]
    fn detail_full_preserves_existing_mid_sentence_ellipsis() {
        // The brain-log itself emits "…" at byte 120; detail_full must keep
        // that character verbatim so click-to-expand surfaces exactly what
        // the daemon recorded.
        let raw = "advance-goal: brain: continue_skipping no JSON object found in…";
        let (_chip, _detail, full) = render_status_and_detail(Some(raw));
        assert!(full.ends_with('…'));
    }

    // ---- Determinism / purity --------------------------------------------

    #[test]
    fn function_is_deterministic() {
        let raw = "advance-goal: brain: continue_skipping (brain-error fallback)";
        let a = render_status_and_detail(Some(raw));
        let b = render_status_and_detail(Some(raw));
        assert_eq!(a, b);
    }

    // ---- Unicode safety ---------------------------------------------------

    #[test]
    fn unicode_input_does_not_panic_or_slice_inside_char() {
        let raw = "advance-goal: 日本語のテスト brain: continue_skipping ".repeat(20);
        let (_chip, detail, full) = render_status_and_detail(Some(&raw));
        // Should not panic and should be valid UTF-8 (trivially true for &str).
        assert!(detail.chars().count() <= DETAIL_MAX_CHARS + 1);
        assert_eq!(full, raw);
    }
}
