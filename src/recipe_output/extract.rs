//! Hardened extraction primitives for `recipe-runner-rs` stdout (issue #2484).
//!
//! `recipe-runner-rs` stdout is routinely contaminated with four kinds of
//! non-payload noise that break the per-phase extractors that read it:
//!
//! 1. **ANSI SGR/CSI/OSC colour codes** emitted by `tracing`/`env_logger`
//!    (e.g. the leading `\x1b[2m` "dim" before a timestamp). The raw `ESC`
//!    (`0x1b`) byte is invalid inside a JSON document, so `serde_json`
//!    rejects any span that contains it.
//! 2. **Timestamped log lines** (`2026-06-28T08:08:58.151133Z INFO …`)
//!    interleaved with the agent answer. These add stray `{`/`}`, decimal
//!    numbers, and verdict-substring false positives (e.g. "al*ready*").
//! 3. The runner's **human summary banner** (`Recipe: … SUCCESS (36.0s)`,
//!    `Steps: …`, `  [completed] …`).
//! 4. The GitHub Copilot CLI **launch-log preamble** (issue #2496): launcher
//!    lines the agent binary prints to stdout *before* its real answer — the
//!    `ℹ … NODE_OPTIONS=… (saved preference)` info marker, a
//!    `Run 'copilot update' …` nag, a `… launching copilot binary=…
//!    version="GitHub Copilot CLI …"` line, and bare `INFO`/`WARN` launcher
//!    lines. These carry **no** ISO-8601 timestamp, so category 2 above does
//!    not catch them; left in place, the first token of the cleaned text
//!    became `ℹ`/`Run`/the version string `1.0.66-2` instead of a decide
//!    action keyword or an orient urgency decimal, so *every* decide/orient
//!    parse missed and the goal deadlocked.
//!
//! Before this module each phase scanned that raw text with a bespoke,
//! fragile extractor and fell back to a permissive default on a miss
//! (`continue_skipping`, "no verdict keyword → accept", distill batch
//! deferral). This is the single shared, well-tested path that strips the
//! noise once, then either returns the last balanced `{…}` object or scans
//! for a verdict keyword.
//!
//! ## Clean-path guarantee
//!
//! [`strip_ansi`] and [`strip_recipe_noise`] return [`Cow::Borrowed`] when
//! the input has no `ESC` byte and (for `strip_recipe_noise`) no droppable
//! log/banner line. The common case — clean recipe stdout — therefore
//! allocates nothing and yields byte-for-byte identical text, so adopting
//! the helper does not change any phase's behaviour on clean output.

use std::borrow::Cow;

/// Strip ANSI escape sequences (CSI, OSC, and bare two-character escapes).
///
/// Returns [`Cow::Borrowed`] unchanged when the input contains no `ESC`
/// (`0x1b`) byte — the zero-allocation clean path.
///
/// Handled forms:
/// - **CSI**: `ESC [ <params> <final 0x40..=0x7E>` (e.g. `\x1b[2m`, `\x1b[0m`)
/// - **OSC**: `ESC ] <text> (BEL | ESC \\)`
/// - **Two-char**: `ESC <byte>` (e.g. `ESC c` reset) — both bytes dropped.
pub fn strip_ansi(s: &str) -> Cow<'_, str> {
    if !s.as_bytes().contains(&0x1b) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: consume '[' then params until a final byte 0x40..=0x7E.
            Some('[') => {
                chars.next();
                for ch in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        break;
                    }
                }
            }
            // OSC: consume ']' then text until BEL or the ST terminator ESC \.
            Some(']') => {
                chars.next();
                while let Some(ch) = chars.next() {
                    if ch == '\u{7}' {
                        break;
                    }
                    if ch == '\x1b' {
                        if let Some('\\') = chars.peek() {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // Bare two-character escape: drop the following byte too.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    Cow::Owned(out)
}

/// `true` when `s` begins with an ISO-8601 date stamp (`YYYY-MM-DDT…`), the
/// signature of a `tracing`/`env_logger` log line that bled into stdout.
fn starts_with_iso_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 11
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[10] == b'T'
}

/// `true` when (after trimming leading whitespace) `line` is a GitHub Copilot
/// CLI **launch-log preamble** line — the non-payload banner the agent binary
/// prints to stdout *before* its real answer (observed with Copilot CLI
/// `1.0.66-2`, issue #2496). These lines carry **no** ISO-8601 timestamp, so
/// [`starts_with_iso_timestamp`] does not catch them, yet they would otherwise
/// become the first token a decide/orient first-word/first-float parser reads
/// (`ℹ` / `Run` / the version string `1.0.66-2`), defaulting every parse and
/// deadlocking the goal.
///
/// Deliberately conservative — it matches **only** these anchored launcher
/// shapes and never a line that could be the answer:
///
/// - the `ℹ … NODE_OPTIONS=… (saved preference)` info-marker line,
/// - a `Run 'copilot update' …` update nag,
/// - a `… launching copilot binary=… version="GitHub Copilot CLI …"` line,
/// - a leading `INFO`/`WARN` launcher line that carries no ISO-8601 timestamp.
///
/// A line beginning with a JSON structural token (`{`, `"`, `[`) — a JSON
/// payload, a pretty-printed object member (`"key": …`), or an array element —
/// an action keyword, a bare decimal, or a verdict keyword is **never**
/// classified as launcher noise, so dropping a launcher line can never discard a
/// decision token or a JSON answer. The structural-token guard is explicit
/// (issue #2570): without it the `contains`-based `launching copilot binary=` /
/// `version="GitHub Copilot CLI` arms would drop a pretty-printed fact
/// `"content"` line that legitimately quotes one of those launcher substrings.
/// ANSI escapes are stripped before this runs (see [`strip_recipe_noise`]), so a
/// colour-coded launcher line still matches and a coloured payload line still
/// survives. This is correctness-as-safety: the predicate consumes untrusted
/// agent stdout, so it errs toward keeping a line rather than eating a payload.
fn is_copilot_launcher_line(line: &str) -> bool {
    let t = line.trim_start();

    // A line that begins with a JSON structural token (`{`, `"`, `[`) is a JSON
    // payload line — a whole object, a pretty-printed object member (`"key": …`),
    // or an array element — never a launcher-log preamble line. Every real
    // launcher shape begins with `\u{2139}` (info marker), `Run`, or an `INFO`/
    // `WARN` level token, so guarding here loses no launcher line while honouring
    // the module's documented contract that "a `{`-leading JSON payload line is
    // never launcher noise". This closes the lossy edge (issue #2570) where the
    // `contains`-based `launching copilot binary=` / `version="GitHub Copilot CLI`
    // arms below would otherwise drop a pretty-printed fact `"content"` line that
    // legitimately quotes a launcher substring, silently emptying that fact.
    if matches!(t.as_bytes().first(), Some(b'{') | Some(b'"') | Some(b'[')) {
        return false;
    }

    // An ISO-timestamped line is a tracing line owned by the timestamp arm of
    // `is_noise_line`; never treat it as a launcher line here (keeps this
    // predicate safe to call standalone and from the timestamp-first chokepoint).
    if starts_with_iso_timestamp(t) {
        return false;
    }

    // `Run 'copilot update' …` update nag.
    if t.starts_with("Run 'copilot update'") {
        return true;
    }

    // `… launching copilot binary=… version="GitHub Copilot CLI …"` — anchored
    // on launcher-only substrings that no decision/verdict/JSON line contains.
    if t.contains("launching copilot binary=") || t.contains("version=\"GitHub Copilot CLI") {
        return true;
    }

    // `ℹ … NODE_OPTIONS=… (saved preference)` info marker. Require BOTH the
    // env-var token AND the saved-preference marker so an agent answer that
    // merely mentions NODE_OPTIONS in prose is never eaten.
    if t.starts_with('\u{2139}') && t.contains("NODE_OPTIONS=") && t.contains("(saved preference)")
    {
        return true;
    }

    // Bare `INFO`/`WARN` launcher line with no ISO-8601 timestamp. A decide
    // action keyword, an orient decimal, a verdict keyword, and a `{`-leading
    // JSON payload never begin with these level tokens, so this is safe.
    t.starts_with("INFO ") || t.starts_with("WARN ")
}

/// `true` when `line` is an **unambiguous** Copilot CLI launcher-preamble line —
/// the narrow subset of [`is_copilot_launcher_line`] whose signature no
/// human-authored *goal title* could plausibly carry. This is the predicate
/// [`crate::goals::goal_slug`] uses to strip launcher noise before slugifying a
/// captured title (#4376): a raw-stdout preamble must never leak env-var tokens
/// or the host config path into a goal slug or `engineer/<slug>` branch, yet a
/// legitimate title that merely *begins with* a bare `INFO`/`WARN` word, or that
/// mentions `copilot update`, must survive intact.
///
/// Matches only the two prose-proof launcher shapes:
///
/// - the `ℹ … NODE_OPTIONS=… (saved preference)` saved-preference marker
///   (leading U+2139 info marker **and** both anchor substrings), and
/// - a `… launching copilot binary=… version="GitHub Copilot CLI …"` line
///   (anchored on substrings no goal title contains).
///
/// It deliberately **excludes** the bare `INFO `/`WARN ` and
/// `Run 'copilot update'` arms of [`is_copilot_launcher_line`]. Those arms are
/// correct when classifying untrusted *stdout*, but on the title surface they
/// false-positive on ordinary prose — collapsing a title such as
/// `"INFO redesign the dashboard"` to an empty slug and colliding otherwise
/// distinct goals. A `{`/`"`/`[`-leading JSON line is never a preamble line.
pub(crate) fn is_copilot_launcher_preamble_signature(line: &str) -> bool {
    let t = line.trim_start();

    // A `{`/`"`/`[`-leading JSON payload line is never a launcher preamble (see
    // the same guard in `is_copilot_launcher_line`).
    if matches!(t.as_bytes().first(), Some(b'{') | Some(b'"') | Some(b'[')) {
        return false;
    }

    // `… launching copilot binary=… version="GitHub Copilot CLI …"` — anchored
    // on launcher-only substrings no goal title prose contains.
    if t.contains("launching copilot binary=") || t.contains("version=\"GitHub Copilot CLI") {
        return true;
    }

    // `ℹ … NODE_OPTIONS=… (saved preference)` — the #4376 saved-preference
    // preamble. Require the leading info marker AND both anchor substrings so a
    // title that merely mentions `NODE_OPTIONS` in prose is never stripped.
    t.starts_with('\u{2139}') && t.contains("NODE_OPTIONS=") && t.contains("(saved preference)")
}

/// `true` when (after trimming) `line` is non-payload recipe-runner noise:
/// a tracing/env_logger timestamped log line, a runner summary-banner line, or
/// a Copilot CLI launch-log preamble line ([`is_copilot_launcher_line`]). JSON
/// payloads start with `{` and agent prose does not match these prefixes, so
/// dropping such lines never discards the answer. This is the single shared
/// chokepoint: extending it re-hardens every consumer — decide, orient,
/// engineer-lifecycle, merge-judge, progress checker, distill — at once.
fn is_noise_line(line: &str) -> bool {
    let t = line.trim_start();
    if starts_with_iso_timestamp(t) {
        return true;
    }
    if is_copilot_launcher_line(t) {
        return true;
    }
    // recipe-runner-rs text-mode summary banner.
    t.starts_with("Recipe:")
        || t.starts_with("Steps:")
        || t.starts_with("[completed]")
        || t.starts_with("[failed]")
        || t.starts_with("[skipped]")
        || t.starts_with("[running]")
}

/// Strip recipe-runner's per-agent line prefix:
/// `  [08:25:09] [amplihack:copilot:123] <payload>`.
///
/// The prefix is logging metadata, not payload. Removing it lets downstream
/// extractors see a real verdict when the agent answered, and lets launcher-only
/// lines collapse to ordinary launcher noise.
///
/// Returns [`Some`] with the payload substring (a **borrowed** slice of `line`,
/// never a fresh allocation) when the prefix is present, or [`None`] when `line`
/// carries no such prefix. Callers treat `None` as "use the line unchanged".
fn strip_recipe_agent_prefix(line: &str) -> Option<&str> {
    let leading = line.len() - line.trim_start().len();
    let t = &line[leading..];
    let b = t.as_bytes();
    if b.len() < "[00:00:00] [amplihack:x:0] ".len() {
        return None;
    }
    if b.first() != Some(&b'[') {
        return None;
    }
    let time_ok = b.get(1..9).is_some_and(|time| {
        time[0].is_ascii_digit()
            && time[1].is_ascii_digit()
            && time[2] == b':'
            && time[3].is_ascii_digit()
            && time[4].is_ascii_digit()
            && time[5] == b':'
            && time[6].is_ascii_digit()
            && time[7].is_ascii_digit()
    });
    if !time_ok || b.get(9) != Some(&b']') || b.get(10) != Some(&b' ') {
        return None;
    }
    let rest = &t[11..];
    let after_agent = rest.strip_prefix("[amplihack:")?;
    let end = after_agent.find("] ")?;
    Some(&after_agent[end + 2..])
}

/// Strip ANSI escapes **and** drop whole tracing/env_logger log lines,
/// recipe-runner summary-banner lines, and Copilot CLI launch-log preamble
/// lines (all via [`is_noise_line`]).
///
/// Returns [`Cow::Borrowed`] unchanged on the clean path (no `ESC` byte and
/// no droppable line), preserving today's behaviour and allocations.
pub fn strip_recipe_noise(raw: &str) -> Cow<'_, str> {
    let de_ansi = strip_ansi(raw);
    // Clean-path detection is allocation-free: `strip_recipe_agent_prefix` now
    // borrows (returning `Some` payload / `None`) and `is_noise_line` only
    // scans, so a fully-clean input never allocates and returns the ANSI-strip
    // result as-is (`Borrowed` stays `Borrowed`). A line is droppable if it
    // carries an agent prefix we'd rewrite (`Some`) or is noise on its own.
    let has_droppable = de_ansi
        .lines()
        .any(|line| match strip_recipe_agent_prefix(line) {
            Some(_) => true,
            None => is_noise_line(line),
        });
    if !has_droppable {
        return de_ansi;
    }
    // Rebuild once. Each kept line is a borrowed slice (prefix-stripped payload
    // or the original line), so the only allocation on the dirty path is the
    // final joined `String` — no per-line `String` churn.
    let kept: Vec<&str> = de_ansi
        .lines()
        .filter_map(|line| {
            let payload = strip_recipe_agent_prefix(line).unwrap_or(line);
            if is_noise_line(payload) {
                None
            } else {
                Some(payload)
            }
        })
        .collect();
    Cow::Owned(kept.join("\n"))
}
#[cfg(test)]
mod tests {
    use super::*;

    // ---- strip_ansi -------------------------------------------------------

    #[test]
    fn strip_ansi_clean_input_is_borrowed_zero_copy() {
        let s = "no escapes here";
        let out = strip_ansi(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "clean path must not allocate"
        );
        assert_eq!(out, "no escapes here");
    }

    #[test]
    fn strip_ansi_removes_sgr_colour_codes() {
        let input = "\x1b[33mWarning\x1b[0m: something \x1b[2mhappened\x1b[0m";
        assert_eq!(strip_ansi(input), "Warning: something happened");
    }

    #[test]
    fn strip_ansi_removes_dim_prefix_before_timestamp() {
        // The exact #2484 live-evidence signature: SGR "dim" before an
        // ISO-8601 timestamp.
        let input = "\x1b[2m2026-06-28T08:08:58.151133Z\x1b[0m INFO distill";
        assert_eq!(
            strip_ansi(input),
            "2026-06-28T08:08:58.151133Z INFO distill"
        );
    }

    #[test]
    fn strip_ansi_handles_osc_sequence() {
        // OSC 0 ; title BEL  → fully removed.
        let input = "before\x1b]0;my title\x07after";
        assert_eq!(strip_ansi(input), "beforeafter");
    }

    #[test]
    fn strip_ansi_matches_legacy_dedup_behaviour() {
        // Mirrors stewardship::dedup::normalize's pass-1 expectation.
        assert_eq!(
            strip_ansi("\x1b[31mERR\x1b[0m   trailing"),
            "ERR   trailing"
        );
    }

    #[test]
    fn strip_ansi_leaves_json_escaped_literal_untouched() {
        // A JSON-escaped `\u001b` in fact content is the six-char ASCII run
        // `\`,`u`,`0`,`0`,`1`,`b` (no raw `ESC` byte), so it must pass through
        // unchanged and borrowed — legitimate content is never mangled. This is
        // the guarantee inherited from the former distill-private stripper.
        let s = r"literal \u001b stays";
        let out = strip_ansi(s);
        assert!(matches!(out, Cow::Borrowed(_)), "no raw ESC ⇒ zero-copy");
        assert_eq!(out, r"literal \u001b stays");
    }

    // ---- strip_recipe_noise ----------------------------------------------

    #[test]
    fn strip_recipe_noise_clean_input_is_borrowed_zero_copy() {
        let s = "{\"facts\":[]}";
        let out = strip_recipe_noise(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "clean path must not allocate"
        );
        assert_eq!(out, s);
    }

    #[test]
    fn strip_recipe_noise_drops_tracing_log_lines() {
        let raw = "2026-06-28T08:08:58.151133Z  INFO simard::distill: starting\n\
                   {\"facts\":[]}\n\
                   2026-06-28T08:08:59.000000Z  INFO simard::distill: done";
        assert_eq!(strip_recipe_noise(raw), "{\"facts\":[]}");
    }

    #[test]
    fn strip_recipe_noise_drops_runner_banner_lines() {
        let raw = "Recipe: distill-episodes SUCCESS (36.0s)\n\
                   Steps: 1/1 completed\n\
                   [completed] distill (36.0s)\n\
                   {\"facts\":[]}";
        assert_eq!(strip_recipe_noise(raw), "{\"facts\":[]}");
    }

    #[test]
    fn strip_recipe_noise_strips_ansi_and_logs_together() {
        // ANSI-coloured tracing line + clean JSON line.
        let raw = "\x1b[2m2026-06-28T08:08:58.151133Z\x1b[0m \x1b[32m INFO\x1b[0m run\n\
                   {\"facts\":[{\"concept\":\"bug-pattern\"}]}";
        assert_eq!(
            strip_recipe_noise(raw),
            "{\"facts\":[{\"concept\":\"bug-pattern\"}]}"
        );
    }

    #[test]
    fn strip_recipe_noise_keeps_agent_prose() {
        let raw = "Sure, here is the result:\n{\"facts\":[]}\nThat's all.";
        assert_eq!(
            strip_recipe_noise(raw),
            "Sure, here is the result:\n{\"facts\":[]}\nThat's all."
        );
    }

    #[test]
    fn strip_recipe_noise_removes_prefixed_launcher_line() {
        let raw = "Recipe: progress-assessment (v1.0.0)\n\
                   Steps: 1\n\
                     [08:26:10] [amplihack:copilot:460198] ℹ NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: /home/u/.amplihack/config\n\
                   Recipe 'progress-assessment': SUCCESS (6.0s)\n\
                     [completed] assess-progress (6.0s)";
        assert_eq!(
            strip_recipe_noise(raw),
            "Recipe 'progress-assessment': SUCCESS (6.0s)"
        );
    }

    #[test]
    fn strip_recipe_noise_preserves_prefixed_payload() {
        let raw = "Recipe: progress-assessment (v1.0.0)\n\
                   Steps: 1\n\
                     [08:25:09] [amplihack:copilot:1635817] {\"verdict\":\"accept\",\"rationale\":\"evidence-backed\"}\n\
                   Recipe 'progress-assessment': SUCCESS (14.0s)\n\
                     [completed] assess-progress (14.0s)";
        let cleaned = strip_recipe_noise(raw);
        assert!(
            cleaned.contains("{\"verdict\":\"accept\""),
            "payload must survive: {cleaned}"
        );
        assert!(
            !cleaned.contains("amplihack:copilot"),
            "agent log prefix must be removed: {cleaned}"
        );
    }
}

/// Issue #2496: the Copilot CLI launch-log preamble must be dropped at this
/// shared chokepoint so the decide/orient first-word/first-float parsers read
/// the agent's real answer, never the launcher banner. The cardinal safety
/// property is that no decision token, JSON payload, decimal, or verdict
/// keyword is ever eaten.
#[cfg(test)]
mod issue_2496_launcher_tests {
    use super::*;

    // The exact live preamble (Copilot CLI 1.0.66-2), ANSI stripped.
    const INFO_MARKER: &str = "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). \
         To change: /home/azureuser/.amplihack/config";
    const LAUNCHING: &str = "INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot \
         version=\"GitHub Copilot CLI 1.0.66-2.\"";
    const UPDATE_NAG: &str = "Run 'copilot update' to check for updates.";

    // ---- each launcher shape is dropped ----------------------------------

    #[test]
    fn drops_node_options_info_marker_line() {
        assert!(is_copilot_launcher_line(INFO_MARKER));
        assert!(is_noise_line(INFO_MARKER));
    }

    #[test]
    fn drops_copilot_update_nag_line() {
        assert!(is_copilot_launcher_line(UPDATE_NAG));
        assert!(is_noise_line(UPDATE_NAG));
    }

    #[test]
    fn drops_launching_binary_version_line() {
        assert!(is_copilot_launcher_line(LAUNCHING));
        assert!(is_noise_line(LAUNCHING));
    }

    #[test]
    fn drops_bare_info_and_warn_launcher_lines() {
        assert!(is_copilot_launcher_line("INFO using cached login"));
        assert!(is_copilot_launcher_line("WARN extension not pinned"));
        assert!(is_noise_line("INFO using cached login"));
        assert!(is_noise_line("WARN extension not pinned"));
    }

    // ---- payload recovery: the preamble must NOT shadow the answer -------

    #[test]
    fn recovers_decide_action_keyword_behind_launcher_preamble() {
        let raw = format!(
            "\x1b[2m{INFO_MARKER}\x1b[0m\n\
             \x1b[34m{LAUNCHING}\x1b[0m\n\
             {UPDATE_NAG}\n\
             advance_goal The next PR is ready to open."
        );
        let cleaned = strip_recipe_noise(&raw);
        assert_eq!(
            cleaned.split_whitespace().next(),
            Some("advance_goal"),
            "first word must be the action keyword, not launcher noise"
        );
    }

    #[test]
    fn recovers_orient_urgency_decimal_behind_launcher_preamble() {
        // The version string 1.0.66-2 must be gone so it cannot be mined as the
        // urgency decimal ahead of the model's real first float.
        let raw = format!("{INFO_MARKER}\n{LAUNCHING}\n{UPDATE_NAG}\n0.42");
        let cleaned = strip_recipe_noise(&raw);
        assert_eq!(cleaned.trim(), "0.42");
        assert!(
            !cleaned.contains("1.0.66"),
            "version string must not survive to be scraped as urgency"
        );
    }

    #[test]
    fn recovers_json_payload_behind_launcher_preamble() {
        let raw = format!("{INFO_MARKER}\n{LAUNCHING}\n{{\"facts\":[]}}");
        let cleaned = strip_recipe_noise(&raw);
        assert_eq!(
            cleaned.trim(),
            "{\"facts\":[]}",
            "the JSON payload must survive with the launcher preamble stripped"
        );
    }

    // ---- negative / safety: a payload line is NEVER classified as noise --

    #[test]
    fn never_drops_json_object_line() {
        assert!(!is_copilot_launcher_line("{\"facts\":[]}"));
        assert!(!is_noise_line("{\"facts\":[]}"));
    }

    #[test]
    fn never_drops_action_keyword_line() {
        assert!(!is_copilot_launcher_line("advance_goal proceeding now"));
        assert!(!is_noise_line("advance_goal proceeding now"));
    }

    #[test]
    fn never_drops_bare_decimal_line() {
        assert!(!is_copilot_launcher_line("0.42"));
        assert!(!is_noise_line("0.42"));
    }

    #[test]
    fn never_drops_verdict_keyword_line() {
        assert!(!is_copilot_launcher_line("ready to merge"));
        assert!(!is_copilot_launcher_line("not_ready missing tests"));
        assert!(!is_noise_line("ready to merge"));
    }

    #[test]
    fn never_drops_prose_that_merely_mentions_node_options() {
        // Mentions NODE_OPTIONS but is not the saved-preference info marker.
        let line = "We should raise NODE_OPTIONS for the next run.";
        assert!(!is_copilot_launcher_line(line));
        assert!(!is_noise_line(line));
    }

    #[test]
    fn iso_timestamped_info_line_is_not_a_launcher_line() {
        // A real tracing line is owned by the timestamp arm, not the launcher
        // arm — keeps the two causes distinct.
        let line = "2026-06-29T12:26:24.512Z  INFO launching copilot binary=x";
        assert!(!is_copilot_launcher_line(line));
        assert!(is_noise_line(line), "still dropped, but as a tracing line");
    }

    #[test]
    fn clean_output_is_borrowed_zero_copy() {
        // Adopting the stricter predicate changes nothing for noise-free output.
        let s = "advance_goal proceed with the implementation";
        let out = strip_recipe_noise(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "clean path must not allocate"
        );
        assert_eq!(out, s);
    }
}

/// Issue #2570: a line beginning with a JSON structural token (`{`, `"`, `[`) is
/// a JSON payload line and must NEVER be classified as launcher noise, even when
/// it literally quotes a launcher substring (`launching copilot binary=` /
/// `version="GitHub Copilot CLI`). This is the shared-chokepoint half of the fix
/// for the distillation fact-yield edge — a pretty-printed fact `"content"` line
/// that quotes such a substring was being line-dropped, silently emptying the
/// fact. These tests pin BOTH halves of the contract at the shared predicate the
/// six consumers (decide, orient, engineer-lifecycle, merge-judge,
/// progress-checker, distill) all route through: real launcher lines are still
/// stripped, and structural-token payload lines are preserved.
#[cfg(test)]
mod issue_2570_structural_token_guard_tests {
    use super::*;

    // The real Copilot CLI 1.0.66-2 launcher shapes (ANSI already stripped), the
    // lines the consumers RELY ON being dropped.
    const INFO_MARKER: &str = "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 \
         (saved preference). To change: /home/azureuser/.amplihack/config";
    const LAUNCHING: &str = "INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot \
         version=\"GitHub Copilot CLI 1.0.66-2.\"";
    const UPDATE_NAG: &str = "Run 'copilot update' to check for updates.";

    #[test]
    fn structural_token_leading_lines_quoting_launcher_substring_are_never_noise() {
        // A pretty-printed fact `"content"` member line quoting the substring.
        let content_line =
            "\"content\": \"the agent logged launching copilot binary=/x before answering\",";
        // A whole compact object line quoting the substring.
        let object_line = "{\"content\":\"see launching copilot binary=/x\"}";
        // An array-element line quoting the launcher substring. Uses the raw
        // needle (not a JSON-escaped one) so that WITHOUT the guard the
        // `contains("launching copilot binary=")` arm would match and drop it —
        // this input actually exercises the `[` arm of the structural-token guard.
        let array_line = "[\"the agent logged launching copilot binary=/x here\"]";
        // Pretty (indented) content line — `trim_start` must apply before the guard.
        let indented_content = "      \"content\": \"launching copilot binary=/x\"";
        for line in [content_line, object_line, array_line, indented_content] {
            assert!(
                !is_copilot_launcher_line(line),
                "a JSON structural-token line is never launcher noise: {line:?}"
            );
            assert!(
                !is_noise_line(line),
                "a JSON structural-token line is never noise: {line:?}"
            );
        }
    }

    #[test]
    fn real_launcher_shapes_are_still_classified_after_the_guard() {
        // The structural-token guard must not stop any real launcher shape from
        // being dropped — the property every text consumer depends on.
        for line in [
            INFO_MARKER,
            LAUNCHING,
            UPDATE_NAG,
            "INFO using cached login",
            "WARN extension not pinned",
        ] {
            assert!(
                is_copilot_launcher_line(line),
                "real launcher line must still be classified: {line:?}"
            );
            assert!(
                is_noise_line(line),
                "real launcher line must still be dropped: {line:?}"
            );
        }
    }

    #[test]
    fn strip_recipe_noise_drops_launcher_lines_but_keeps_pretty_content_line() {
        // The end-to-end shared-chokepoint behaviour: real launcher preamble
        // lines are dropped, while the `"`-leading fact content line that quotes
        // the launcher substring survives.
        let content_line =
            "\"content\": \"the agent logged launching copilot binary=/x before answering\"";
        let raw = format!("{LAUNCHING}\n{UPDATE_NAG}\n{INFO_MARKER}\n{content_line}");
        let cleaned = strip_recipe_noise(&raw);
        assert_eq!(
            cleaned.trim(),
            content_line,
            "launcher lines dropped; the quoted-substring content line preserved"
        );
    }

    #[test]
    fn pretty_json_object_quoting_launcher_substring_survives_unchanged() {
        // A full pretty-printed object whose content quotes the substring passes
        // through the cleaner unchanged (no line is noise) and remains valid JSON.
        let pretty = "{\n  \"facts\": [\n    {\n      \
             \"content\": \"see launching copilot binary=/x\"\n    }\n  ]\n}";
        let cleaned = strip_recipe_noise(pretty);
        assert_eq!(
            cleaned.as_ref(),
            pretty,
            "no line of a pretty JSON object is launcher noise"
        );
        serde_json::from_str::<serde_json::Value>(cleaned.as_ref())
            .expect("the cleaned view must still be valid JSON");
    }
}
