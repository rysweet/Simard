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

/// Strip ANSI escapes **and** drop whole tracing/env_logger log lines,
/// recipe-runner summary-banner lines, and Copilot CLI launch-log preamble
/// lines (all via [`is_noise_line`]).
///
/// Returns [`Cow::Borrowed`] unchanged on the clean path (no `ESC` byte and
/// no droppable line), preserving today's behaviour and allocations.
pub fn strip_recipe_noise(raw: &str) -> Cow<'_, str> {
    let de_ansi = strip_ansi(raw);
    if !de_ansi.lines().any(is_noise_line) {
        // No droppable lines: pass through the ANSI-strip result as-is
        // (`Borrowed` stays `Borrowed` on the fully-clean path).
        return de_ansi;
    }
    let kept: Vec<&str> = de_ansi.lines().filter(|l| !is_noise_line(l)).collect();
    Cow::Owned(kept.join("\n"))
}

/// Strip JSON **trailing commas** — a `,` immediately preceding a closing `}`
/// or `]` (ignoring intervening ASCII whitespace) — as a last-resort recovery
/// view for otherwise-well-formed LLM JSON (issues #2658/#2678).
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

/// Issue #2678: distill parse-fail rate 100% — the distiller intermittently
/// emits an otherwise well-formed `{ "facts": [...] }` envelope with a JSON
/// **trailing comma** (a `,` immediately before a `}` or `]`). Strict
/// `serde_json` rejects the whole object, so every batch deferred forever and
/// the Overseer reported a recurring `anomaly:distill parse-fail rate 100%`.
///
/// [`strip_json_trailing_commas`] is the string-literal-aware, structural-only
/// primitive that removes exactly those commas so a fallback retry can recover
/// the batch. These tests are the executable contract for that primitive
/// (written first, red until it exists):
///
/// * **Removal** — a `,` whose next non-whitespace byte is `}`/`]` is dropped,
///   including several *non-adjacent* such commas in a single pass.
/// * **Clean-path no-op** — input with no *removable* structural comma is
///   returned [`Cow::Borrowed`] (zero-copy, byte-identical), so adopting the
///   helper never perturbs the overwhelmingly-common well-formed output.
/// * **String-literal safety** — a comma inside a `"..."` literal (honouring
///   `\` escapes) is never touched, so fact content is never corrupted.
/// * **Fail-closed / removal-only** — output bytes are a subset of input bytes;
///   genuinely malformed input (e.g. an *adjacent* double comma) is not
///   "repaired" into validity, so the caller still fails closed.
/// * **Robustness** — total over arbitrary byte sequences: never panics and
///   never produces invalid UTF-8.
#[cfg(test)]
mod issue_2678_trailing_comma_tests {
    use super::*;

    /// A parse-round-trip helper: the stripped view of a trailing-comma document
    /// must be accepted by strict `serde_json`.
    fn parses_after_strip(input: &str) -> serde_json::Value {
        let stripped = strip_json_trailing_commas(input);
        serde_json::from_str::<serde_json::Value>(stripped.as_ref()).unwrap_or_else(|e| {
            panic!("stripped view of {input:?} must be valid JSON, got error: {e}")
        })
    }

    // ---- clean-path no-op (detection-first, zero-copy) --------------------

    #[test]
    fn clean_object_with_no_trailing_comma_is_borrowed_zero_copy() {
        let s = r#"{"facts":[{"concept":"pr-pattern","content":"x"}]}"#;
        let out = strip_json_trailing_commas(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "well-formed input must not allocate"
        );
        assert_eq!(out, s, "well-formed input must be byte-identical");
    }

    #[test]
    fn internal_commas_without_a_trailing_one_are_borrowed() {
        // Commas that separate members/elements are structural but NOT trailing
        // (their next non-whitespace byte is not `}`/`]`), so nothing is removed
        // and the clean path stays zero-copy.
        let s = r#"{"a":1,"b":[1,2,3],"c":"x,y,z"}"#;
        let out = strip_json_trailing_commas(s);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "no removable comma ⇒ borrowed"
        );
        assert_eq!(out, s);
    }

    #[test]
    fn empty_input_is_borrowed_and_unchanged() {
        let out = strip_json_trailing_commas("");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "");
    }

    // ---- removal of a structural trailing comma --------------------------

    #[test]
    fn removes_trailing_comma_before_closing_brace() {
        let out = strip_json_trailing_commas(r#"{"facts":[],}"#);
        assert_eq!(out.as_ref(), r#"{"facts":[]}"#);
        parses_after_strip(r#"{"facts":[],}"#);
    }

    #[test]
    fn removes_trailing_comma_before_closing_bracket() {
        let out = strip_json_trailing_commas(r#"{"a":[1,2,]}"#);
        assert_eq!(out.as_ref(), r#"{"a":[1,2]}"#);
        let v = parses_after_strip(r#"{"a":[1,2,]}"#);
        assert_eq!(v["a"][0], 1);
        assert_eq!(v["a"][1], 2);
    }

    #[test]
    fn removes_trailing_comma_separated_from_closer_by_whitespace() {
        // The real pretty-printed shape: `,\n}` — the comma's next
        // *non-whitespace* byte is `}`, so it is trailing and must be removed.
        let input = "{\n  \"a\": 1,\n}";
        let out = strip_json_trailing_commas(input);
        assert_eq!(out.as_ref(), "{\n  \"a\": 1\n}");
        let v = parses_after_strip(input);
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn removes_multiple_non_adjacent_structural_commas_in_one_pass() {
        // The dominant real-world defect: a comma after the last array element
        // AND a comma after the last object member. The two commas are
        // non-adjacent (separated by `]` and whitespace), so a single pass
        // removes both and the result is valid JSON.
        let input = r#"{ "facts": [ {"x":1}, ], }"#;
        let out = strip_json_trailing_commas(input);
        assert_eq!(out.as_ref(), r#"{ "facts": [ {"x":1} ] }"#);
        let v = parses_after_strip(input);
        assert_eq!(v["facts"][0]["x"], 1);
    }

    // ---- string-literal safety (never corrupt fact content) --------------

    #[test]
    fn comma_inside_a_string_literal_is_preserved() {
        // Only the structural trailing comma (before `}`) is removed; the comma
        // inside the `"x,y"` value must survive untouched.
        let input = r#"{"a":"x,y","b":1,}"#;
        let out = strip_json_trailing_commas(input);
        assert_eq!(out.as_ref(), r#"{"a":"x,y","b":1}"#);
        let v = parses_after_strip(input);
        assert_eq!(v["a"].as_str(), Some("x,y"));
        assert_eq!(v["b"], 1);
    }

    #[test]
    fn comma_after_an_escaped_quote_inside_a_string_is_preserved() {
        // The value of "a" is the three characters  x " ,  (an escaped quote
        // followed by a comma, still inside the string). The state machine must
        // honour the `\"` escape and NOT treat the in-string comma as trailing.
        let input = r#"{"a":"x\",","b":1,}"#;
        let out = strip_json_trailing_commas(input);
        assert_eq!(out.as_ref(), r#"{"a":"x\",","b":1}"#);
        let v = parses_after_strip(input);
        assert_eq!(v["a"].as_str(), Some("x\","));
        assert_eq!(v["b"], 1);
    }

    #[test]
    fn multibyte_content_adjacent_to_a_trailing_comma_stays_valid_utf8() {
        // A non-ASCII byte immediately before the structural comma must not be
        // split; only the ASCII `,` (0x2C) is removed and the result is valid
        // UTF-8 that serde accepts.
        let input = r#"{"a":"café",}"#;
        let out = strip_json_trailing_commas(input);
        assert_eq!(out.as_ref(), r#"{"a":"café"}"#);
        let v = parses_after_strip(input);
        assert_eq!(v["a"].as_str(), Some("café"));
    }

    // ---- fail-closed: do NOT repair genuinely malformed input ------------

    #[test]
    fn adjacent_double_comma_is_not_repaired_into_valid_json() {
        // Adjacency, not count, is the limit: only the comma immediately before
        // the closer is removed, so `[1,,]` becomes `[1,]`, which is STILL
        // invalid — the primitive must not launder genuinely malformed input
        // into a false success.
        let input = "[1,,]";
        let out = strip_json_trailing_commas(input);
        assert_eq!(out.as_ref(), "[1,]");
        assert!(
            serde_json::from_str::<serde_json::Value>(out.as_ref()).is_err(),
            "an adjacent double comma must remain unparseable (fail closed)"
        );
    }

    #[test]
    fn comma_at_document_end_is_not_treated_as_trailing() {
        // A comma whose next non-whitespace byte is end-of-input (not `}`/`]`)
        // is out of scope: it is left in place, so genuinely malformed input is
        // not silently accepted, and the clean path stays borrowed.
        let input = r#"{"a":1},"#;
        let out = strip_json_trailing_commas(input);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "no removable comma ⇒ borrowed"
        );
        assert_eq!(out, input);
    }

    // ---- removal-only + robustness (total, never panics) -----------------

    #[test]
    fn output_is_always_a_byte_subset_of_input() {
        // Removal-only invariant (R6): the primitive can only delete bytes, so
        // it can never inject content the model did not emit.
        for input in [
            "",
            ",",
            ",,,",
            "]",
            "}",
            ",}",
            ",]",
            "\\",
            r#"{"a":"unterminated"#,
            r#"{"facts":[{"x":1},],}"#,
            "café,]",
        ] {
            let out = strip_json_trailing_commas(input);
            assert!(
                out.len() <= input.len(),
                "removal-only: {input:?} → {out:?} must not grow"
            );
        }
    }

    #[test]
    fn is_total_over_adversarial_input_without_panicking() {
        // The primitive sits on an untrusted trust boundary (LLM/subprocess
        // output) and runs in the distill hot loop, so it must be total: no
        // panic and no invalid UTF-8 on any byte sequence, incl. a lone
        // trailing backslash, an unterminated string, and a large all-comma run.
        let big_commas = ",".repeat(100_000);
        for input in [
            "\\",
            "\"",
            "{\"a\":\"x\\",
            "[,",
            "{,",
            big_commas.as_str(),
            "🚀,]",
        ] {
            // Must return a well-formed Cow (implicitly valid UTF-8) and not panic.
            let out = strip_json_trailing_commas(input);
            assert!(out.len() <= input.len());
        }
    }
}
