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
/// lines collapse to ordinary launcher noise. Non-matching lines are returned
/// unchanged on the borrowed path.
fn strip_recipe_agent_prefix(line: &str) -> Cow<'_, str> {
    let leading = line.len() - line.trim_start().len();
    let t = &line[leading..];
    let b = t.as_bytes();
    if b.len() < "[00:00:00] [amplihack:x:0] ".len() {
        return Cow::Borrowed(line);
    }
    if b.first() != Some(&b'[') {
        return Cow::Borrowed(line);
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
        return Cow::Borrowed(line);
    }
    let rest = &t[11..];
    let Some(after_agent) = rest.strip_prefix("[amplihack:") else {
        return Cow::Borrowed(line);
    };
    let Some(end) = after_agent.find("] ") else {
        return Cow::Borrowed(line);
    };
    Cow::Owned(after_agent[end + 2..].to_string())
}

/// Strip ANSI escapes **and** drop whole tracing/env_logger log lines,
/// recipe-runner summary-banner lines, and Copilot CLI launch-log preamble
/// lines (all via [`is_noise_line`]).
///
/// Returns [`Cow::Borrowed`] unchanged on the clean path (no `ESC` byte and
/// no droppable line), preserving today's behaviour and allocations.
pub fn strip_recipe_noise(raw: &str) -> Cow<'_, str> {
    let de_ansi = strip_ansi(raw);
    if !de_ansi
        .lines()
        .any(|line| is_noise_line(line) || matches!(strip_recipe_agent_prefix(line), Cow::Owned(_)))
    {
        // No droppable lines: pass through the ANSI-strip result as-is
        // (`Borrowed` stays `Borrowed` on the fully-clean path).
        return de_ansi;
    }
    let kept: Vec<String> = de_ansi
        .lines()
        .filter_map(|line| {
            let stripped = strip_recipe_agent_prefix(line);
            let payload = stripped.as_ref();
            if is_noise_line(payload) {
                None
            } else {
                Some(payload.to_string())
            }
        })
        .collect();
    Cow::Owned(kept.join("\n"))
}

/// Scan `bytes` starting at `start` (which must index a `{`) for the matching
/// closing brace, honouring JSON string literals so braces inside `"…"` do
/// not affect the depth count. Returns the byte index of the matching `}`.
fn scan_balanced(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Return every balanced `{…}` span in `s`, in source order, by trying each
/// `{` opener in turn.
///
/// String-literal aware (braces inside JSON strings are ignored). A `{` that
/// never closes — an unmatched opener in leading prose, e.g. a code fragment
/// like `fn f() {` — is skipped so a genuinely balanced object *after* it is
/// still found, rather than being demoted to a nested span and lost (relied on
/// by distillation, issue #2508). Used by callers that need to try each
/// candidate against a typed envelope — e.g. distillation parses each span as a
/// `{ "facts": [...] }` object and keeps the first that deserialises.
pub fn balanced_objects(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end) = scan_balanced(bytes, i)
        {
            spans.push(&s[i..=end]);
            i = end + 1;
            continue;
        }
        i += 1;
    }
    spans
}

/// Return the **last** balanced top-level `{…}` span in `s`, or `None`.
///
/// Returning the last span means a leading "thinking"/banner object cannot
/// shadow the real answer object that follows it.
pub fn last_balanced_object(s: &str) -> Option<&str> {
    balanced_objects(s).pop()
}

/// Strip JSON **trailing commas** — a `,` immediately preceding a closing `}`
/// or `]` (ignoring intervening ASCII whitespace) — as a last-resort recovery
/// view for otherwise-well-formed LLM JSON (issue #2658).
///
/// A trailing comma before `}`/`]` is the single most common real-world LLM
/// JSON defect and is **never valid JSON**, so this stripper is a *provable
/// no-op on valid input*: it returns [`Cow::Borrowed`] byte-for-byte unchanged
/// whenever no trailing comma is present (the zero-allocation clean path).
/// A caller therefore retries a strict-parse failure on this view without any
/// risk of altering behaviour on well-formed output.
///
/// String-literal aware: a comma inside a JSON string (respecting `\"`
/// escapes) is never touched, so a comma in a fact's `content` is preserved
/// verbatim. Only the offending comma bytes are removed and every removed byte
/// is ASCII, so the result is always valid UTF-8.
///
/// Note this targets *only* the single-trailing-comma shape. A genuinely
/// malformed object (e.g. an elided element `[1,,2]`, an unquoted key, a
/// missing value) is left still-malformed so the caller's strict parse still
/// rejects it — leniency never widens to accept broken JSON.
pub fn strip_json_trailing_commas(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    // Cheap detection pass first so clean JSON borrows unchanged (zero-alloc).
    if !has_trailing_comma(bytes) {
        return Cow::Borrowed(s);
    }
    // Rebuild, dropping only the trailing commas. Only ASCII comma bytes are
    // ever skipped, so the surviving bytes remain valid UTF-8.
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else if c == b'"' {
            in_string = true;
            out.push(c);
        } else if c == b',' && next_nonspace_is_close(bytes, i + 1) {
            // Trailing comma — drop it (do not copy).
        } else {
            out.push(c);
        }
        i += 1;
    }
    match String::from_utf8(out) {
        Ok(stripped) => Cow::Owned(stripped),
        // Unreachable in practice (only ASCII commas are dropped), but never
        // panic on recovery input — fall back to the original text.
        Err(_) => Cow::Borrowed(s),
    }
}

/// Does `bytes` contain at least one string-aware trailing comma?
///
/// Mirrors the scan in [`strip_json_trailing_commas`] but only detects, so the
/// common clean case can borrow the input without allocating.
fn has_trailing_comma(bytes: &[u8]) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else if c == b'"' {
            in_string = true;
        } else if c == b',' && next_nonspace_is_close(bytes, i + 1) {
            return true;
        }
        i += 1;
    }
    false
}

/// Is the first non-ASCII-whitespace byte at or after `i` a closing `}`/`]`?
///
/// Only JSON insignificant whitespace (space, tab, LF, CR, form-feed) is
/// skipped; any other byte (including `"` opening the next key/element) means
/// the comma is a legitimate separator and must be kept.
fn next_nonspace_is_close(bytes: &[u8], mut i: usize) -> bool {
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0c => i += 1,
            b'}' | b']' => return true,
            _ => return false,
        }
    }
    false
}

/// Extract a JSON object from noisy recipe stdout as an owned `String`.
///
/// Tries two cleaned views and returns the last balanced `{…}` from the first
/// that yields one:
///
/// 1. [`strip_recipe_noise`] — drops whole log/banner lines, which recovers a
///    payload whose pretty-printed body has an interleaved tracing line.
/// 2. [`strip_ansi`] — line-preserving, which recovers a payload that sits on
///    the *same physical line* as a leading timestamp/log prefix (that line
///    would otherwise be dropped wholesale by view 1).
///
/// Returns `None` when neither view contains a balanced object.
pub fn extract_json_payload(raw: &str) -> Option<String> {
    for cleaned in [strip_recipe_noise(raw), strip_ansi(raw)] {
        if let Some(obj) = last_balanced_object(cleaned.as_ref()) {
            return Some(obj.to_string());
        }
    }
    None
}

/// A verdict-keyword match against cleaned recipe output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictMatch<'k> {
    /// The keyword (borrowed from the caller's precedence list) that matched.
    pub keyword: &'k str,
    /// The full ANSI/log/banner-stripped, trimmed recipe text — the source
    /// the caller turns into a rationale string.
    pub rationale: String,
}

/// Scan recipe stdout for the first verdict keyword present, in the caller's
/// precedence order, after stripping ANSI/log/banner noise.
///
/// Matching is case-insensitive substring containment. The first keyword in
/// `keywords` that appears wins, so callers encode precedence by ordering
/// (e.g. `["not_ready", "not ready", "unclear", "ready"]` so `not_ready`
/// beats the `ready` it contains). Returns `None` when the cleaned text is
/// empty or contains none of the keywords.
pub fn extract_verdict<'k>(raw: &str, keywords: &[&'k str]) -> Option<VerdictMatch<'k>> {
    let cleaned = strip_recipe_noise(raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    for &kw in keywords {
        if lower.contains(&kw.to_ascii_lowercase()) {
            return Some(VerdictMatch {
                keyword: kw,
                rationale: trimmed.to_string(),
            });
        }
    }
    None
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

    // ---- balanced_objects / last_balanced_object -------------------------

    #[test]
    fn balanced_objects_finds_each_top_level_object() {
        let s = "{\"a\":1} junk {\"b\":2}";
        assert_eq!(balanced_objects(s), vec!["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn balanced_objects_ignores_braces_inside_strings() {
        let s = "{\"text\":\"a } b { c\"}";
        assert_eq!(balanced_objects(s), vec!["{\"text\":\"a } b { c\"}"]);
    }

    #[test]
    fn balanced_objects_handles_escaped_quote_in_string() {
        let s = r#"{"q":"he said \"}\" loudly"}"#;
        assert_eq!(balanced_objects(s), vec![s]);
    }

    #[test]
    fn balanced_objects_skips_unmatched_leading_brace() {
        // An unmatched, never-closing `{` in leading prose (e.g. a code fragment
        // such as `fn f() {`) must not anchor the scan and swallow the genuinely
        // balanced object that follows it — the candidate restart recovers it
        // (relied on by episode distillation, issue #2508).
        let s = r#"prefix fn f() { then {"facts":[]}"#;
        assert_eq!(balanced_objects(s), vec![r#"{"facts":[]}"#]);
    }

    #[test]
    fn last_balanced_object_returns_trailing_answer() {
        let s = "{\"thinking\":true} then {\"answer\":42}";
        assert_eq!(last_balanced_object(s), Some("{\"answer\":42}"));
    }

    #[test]
    fn last_balanced_object_none_when_no_object() {
        assert_eq!(last_balanced_object("no braces"), None);
    }

    // ---- extract_json_payload --------------------------------------------

    #[test]
    fn extract_json_payload_recovers_from_ansi_log_noise() {
        // Raw fails (contains ESC); cleaned succeeds — the #2479-style
        // raw-vs-stripped recovery proof, generalised.
        let raw = "\x1b[2m2026-06-28T08:08:58.151133Z\x1b[0m  INFO simard: run\n\
                   {\"facts\":[{\"concept\":\"lesson-learned\"}]}";
        assert!(
            serde_json::from_str::<serde_json::Value>(raw).is_err(),
            "raw span with ESC byte must not parse as JSON"
        );
        let payload = extract_json_payload(raw).expect("payload recovered");
        assert!(serde_json::from_str::<serde_json::Value>(&payload).is_ok());
        assert_eq!(payload, "{\"facts\":[{\"concept\":\"lesson-learned\"}]}");
    }

    #[test]
    fn extract_json_payload_none_for_pure_noise() {
        let raw = "2026-06-28T08:08:58.151133Z  INFO no payload at all";
        assert_eq!(extract_json_payload(raw), None);
    }

    #[test]
    fn extract_json_payload_recovers_same_line_log_prefix() {
        // Payload appended to a log line on the SAME physical line: the
        // line-dropped view would discard it, so the ANSI-only fallback view
        // must recover it.
        let raw = "\x1b[2m2026-06-28T08:08:58.151133Z\x1b[0m  INFO done {\"facts\":[]}";
        assert_eq!(
            extract_json_payload(raw),
            Some("{\"facts\":[]}".to_string())
        );
    }

    #[test]
    fn extract_json_payload_recovers_interleaved_log_line() {
        // A tracing line interleaved inside a pretty-printed body: the
        // line-dropped view removes it so the braces balance again.
        let raw = "{\n  \"facts\": [\n\
                   2026-06-28T08:08:58.151133Z  INFO progress\n\
                   ]\n}";
        assert_eq!(
            extract_json_payload(raw),
            Some("{\n  \"facts\": [\n]\n}".to_string())
        );
    }

    // ---- extract_verdict -------------------------------------------------

    const MERGE_KEYWORDS: &[&str] = &["not_ready", "not ready", "unclear", "ready"];

    #[test]
    fn extract_verdict_precedence_not_ready_beats_ready() {
        let m = extract_verdict("The PR is not_ready yet.", MERGE_KEYWORDS).unwrap();
        assert_eq!(m.keyword, "not_ready");
    }

    #[test]
    fn extract_verdict_plain_ready() {
        let m = extract_verdict("Looks ready to merge.", MERGE_KEYWORDS).unwrap();
        assert_eq!(m.keyword, "ready");
    }

    #[test]
    fn extract_verdict_case_insensitive() {
        let m = extract_verdict("VERDICT: READY", MERGE_KEYWORDS).unwrap();
        assert_eq!(m.keyword, "ready");
    }

    #[test]
    fn extract_verdict_none_when_absent() {
        assert!(extract_verdict("cannot decide", MERGE_KEYWORDS).is_none());
    }

    #[test]
    fn extract_verdict_none_when_empty_after_stripping() {
        let raw = "2026-06-28T08:08:58.151133Z  INFO only a log line";
        assert!(extract_verdict(raw, MERGE_KEYWORDS).is_none());
    }

    #[test]
    fn extract_verdict_ignores_keyword_substring_inside_dropped_log_line() {
        // "already" contains "ready"; the log line must be dropped so it does
        // not produce a false Ready verdict. The real verdict follows.
        let raw = "2026-06-28T08:08:58.000000Z  INFO already running batch\n\
                   Verdict: not_ready — missing tests.";
        let m = extract_verdict(raw, MERGE_KEYWORDS).unwrap();
        assert_eq!(m.keyword, "not_ready");
        assert!(!m.rationale.contains("already running"));
    }

    #[test]
    fn extract_verdict_rationale_is_cleaned_full_text() {
        let raw = "After review I accept the claim.";
        let m = extract_verdict(raw, &["reject", "accept"]).unwrap();
        assert_eq!(m.keyword, "accept");
        assert_eq!(m.rationale, "After review I accept the claim.");
    }

    // ---- strip_json_trailing_commas (issue #2658) ------------------------

    #[test]
    fn strip_trailing_commas_valid_json_is_borrowed_zero_copy() {
        // The clean path must not allocate and must be byte-identical: a
        // provable no-op on valid JSON, so a strict-parse retry on this view
        // can never change behaviour on well-formed output.
        let s = r#"{"facts":[{"concept":"pr-pattern","content":"a"}],"procedures":[]}"#;
        let out = strip_json_trailing_commas(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "valid JSON must borrow unchanged (zero-alloc)"
        );
        assert_eq!(out, s);
    }

    #[test]
    fn strip_trailing_commas_before_brace_and_bracket() {
        // Trailing comma before `}` and before `]` are both removed, yielding
        // parseable JSON.
        let malformed = r#"{"facts":[{"concept":"pr-pattern","content":"a",},],}"#;
        let fixed = strip_json_trailing_commas(malformed);
        assert!(matches!(fixed, Cow::Owned(_)), "a change must allocate");
        assert!(
            serde_json::from_str::<serde_json::Value>(&fixed).is_ok(),
            "stripped output must be valid JSON: {fixed}"
        );
        assert_eq!(
            fixed,
            r#"{"facts":[{"concept":"pr-pattern","content":"a"}]}"#
        );
    }

    #[test]
    fn strip_trailing_commas_tolerates_whitespace_before_close() {
        // Whitespace (incl. newlines) between the comma and the closer is
        // skipped — the pretty-printed shape an LLM actually emits.
        let malformed = "{\n  \"facts\": [\n    {\"concept\": \"a\"},\n  ],\n}";
        let fixed = strip_json_trailing_commas(malformed);
        assert!(
            serde_json::from_str::<serde_json::Value>(&fixed).is_ok(),
            "pretty-printed trailing commas must be recovered: {fixed}"
        );
    }

    #[test]
    fn strip_trailing_commas_never_corrupts_comma_in_string_content() {
        // A `,}` or `,]` sequence INSIDE a JSON string is content, not a
        // trailing comma, and must survive verbatim (including when the string
        // ends immediately after it).
        let s = r#"{"content":"first, second,","tag":"x"}"#;
        let out = strip_json_trailing_commas(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "string-internal commas are not trailing commas — must borrow"
        );
        assert_eq!(out, s);

        // Even a literal `, ]` inside string content is preserved.
        let s2 = r#"{"content":"a, ] b, } c"}"#;
        assert_eq!(strip_json_trailing_commas(s2), s2);
    }

    #[test]
    fn strip_trailing_commas_respects_escaped_quote_in_string() {
        // An escaped quote must not prematurely end the string, so a `,]`
        // after it (still inside the string) is not stripped.
        let s = r#"{"content":"he said \"go,\" then left,","k":1}"#;
        let out = strip_json_trailing_commas(s);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, s);
    }

    #[test]
    fn strip_trailing_commas_leaves_genuinely_malformed_still_malformed() {
        // An elided element `,,` is NOT a single trailing comma: leniency must
        // not repair it into valid JSON.
        let malformed = r#"{"facts":[1,,2]}"#;
        let out = strip_json_trailing_commas(malformed);
        assert!(
            serde_json::from_str::<serde_json::Value>(&out).is_err(),
            "a doubly-malformed array must remain malformed: {out}"
        );
    }

    #[test]
    fn strip_trailing_commas_preserves_multibyte_utf8() {
        // Non-ASCII content around a stripped comma must round-trip intact.
        let malformed = r#"{"content":"café — δοκιμή","k":1,}"#;
        let fixed = strip_json_trailing_commas(malformed);
        let v: serde_json::Value =
            serde_json::from_str(&fixed).expect("multibyte content must stay valid after strip");
        assert_eq!(v["content"], "café — δοκιμή");
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
        assert_eq!(
            extract_json_payload(&raw),
            Some("{\"facts\":[]}".to_string())
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
