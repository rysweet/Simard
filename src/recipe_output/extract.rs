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

/// Lowercase hex digits for `\u00XX` escape synthesis in
/// [`escape_json_string_control_chars`].
const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

/// Escape **unescaped ASCII control characters that appear INSIDE a JSON string
/// literal** — a raw newline/tab/CR (and any other byte `< 0x20`) sitting in a
/// string value — as a last-resort recovery view for otherwise-well-formed LLM
/// JSON.
///
/// After the trailing comma, an unescaped control character inside a string is
/// the most common real-world LLM JSON defect: a model emitting a multi-line
/// `content`/`rationale` value routinely writes a *literal* newline (or tab)
/// instead of the `\n`/`\t` escape the JSON grammar requires. `serde_json` is
/// spec-strict and rejects it (`control character (\u0000-\u001F) found while
/// parsing a string`), so the model's WHOLE structured decision is dropped on a
/// single stray byte — a parse-failure default, not a real model decision.
///
/// Like [`strip_json_trailing_commas`] this is a **provable no-op on valid
/// JSON**: valid JSON strings never contain a raw control character (the grammar
/// forbids `U+0000`–`U+001F` unescaped), so [`has_unescaped_string_control`]
/// returns `false` and the input borrows back byte-for-byte unchanged (the
/// zero-allocation clean path). A caller therefore retries a strict-parse
/// failure on this view without any risk of altering behaviour on well-formed
/// output.
///
/// String-literal aware in exactly the same way as the rest of this module: the
/// scan tracks `in_string` (respecting `\"` escapes), so a control character is
/// escaped **only** inside a string value. A raw newline/tab BETWEEN tokens is
/// legitimate JSON whitespace and is left untouched, and a `\`-escaped byte is
/// copied verbatim (so an already-`\n` stays `\n`, never `\\n`). Control bytes
/// are mapped to their short escapes (`\b \t \n \f \r`) or a `\u00XX` sequence;
/// only ASCII bytes are ever emitted, so the result is always valid UTF-8 and
/// every multibyte UTF-8 body byte (all `>= 0x80`) is copied through unchanged.
///
/// Leniency never widens beyond this one defect: a control character is the only
/// shape rewritten, so a genuinely malformed object (unquoted key, elided
/// element, missing value) is left still-malformed and the caller's strict parse
/// still rejects it.
pub fn escape_json_string_control_chars(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    // Cheap detection pass first so clean JSON borrows unchanged (zero-alloc).
    if !has_unescaped_string_control(bytes) {
        return Cow::Borrowed(s);
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + 8);
    let mut in_string = false;
    let mut escaped = false;
    for &c in bytes {
        if in_string {
            if escaped {
                out.push(c);
                escaped = false;
            } else if c == b'\\' {
                out.push(c);
                escaped = true;
            } else if c == b'"' {
                out.push(c);
                in_string = false;
            } else if c < 0x20 {
                // Unescaped control char inside a string literal — escape it.
                match c {
                    0x08 => out.extend_from_slice(b"\\b"),
                    0x09 => out.extend_from_slice(b"\\t"),
                    0x0a => out.extend_from_slice(b"\\n"),
                    0x0c => out.extend_from_slice(b"\\f"),
                    0x0d => out.extend_from_slice(b"\\r"),
                    other => {
                        out.extend_from_slice(b"\\u00");
                        out.push(HEX_LOWER[(other >> 4) as usize]);
                        out.push(HEX_LOWER[(other & 0x0f) as usize]);
                    }
                }
            } else {
                out.push(c);
            }
        } else if c == b'"' {
            in_string = true;
            out.push(c);
        } else {
            out.push(c);
        }
    }
    match String::from_utf8(out) {
        Ok(escaped) => Cow::Owned(escaped),
        // Unreachable in practice (only ASCII escape bytes are inserted and
        // every original byte is preserved), but never panic on recovery input.
        Err(_) => Cow::Borrowed(s),
    }
}

/// Does `bytes` contain at least one unescaped ASCII control character inside a
/// JSON string literal?
///
/// Mirrors the string-aware scan in [`escape_json_string_control_chars`] but
/// only detects, so the common clean case can borrow the input without
/// allocating. A control byte OUTSIDE a string (valid JSON whitespace) does not
/// count; a `\`-escaped byte is consumed as an escape target and never counts.
fn has_unescaped_string_control(bytes: &[u8]) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    for &c in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            } else if c < 0x20 {
                return true;
            }
        } else if c == b'"' {
            in_string = true;
        }
    }
    false
}

/// The set of bytes that may legitimately follow a backslash inside a JSON
/// string literal (the JSON grammar's escape initiators): `\" \\ \/ \b \f \n
/// \r \t` and the `\uXXXX` unicode form. Any other byte after a backslash is an
/// **invalid escape** that `serde_json` rejects.
fn is_valid_json_escape_char(c: u8) -> bool {
    matches!(
        c,
        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' | b'u'
    )
}

/// Escape **an invalid backslash escape sequence that appears INSIDE a JSON
/// string literal** — a lone `\` whose following byte is not a JSON escape
/// initiator (`" \ / b f n r t u`) — as a last-resort recovery view for
/// otherwise-well-formed LLM JSON.
///
/// After the trailing comma and the unescaped control character, an invalid
/// backslash escape is the next most common real-world LLM JSON defect: a model
/// emitting a Windows path (`C:\Users`), a regular expression (`\d+`), or a
/// LaTeX/Markdown fragment (`\alpha`) inside a `content`/`rationale` value writes
/// a *literal* backslash that is not part of a valid `\n`/`\t`/`\uXXXX` escape.
/// `serde_json` is spec-strict and rejects it (`invalid escape` while parsing a
/// string), so the model's WHOLE structured decision is dropped on a single stray
/// backslash — a parse-failure default, not a real model decision.
///
/// Recovery doubles the offending backslash (`\d` → `\\d`), which is the JSON
/// spelling of a literal backslash and yields exactly the bytes the model plainly
/// intended. A legitimate escape (`\n`, `\"`, `\\`, `\uXXXX`) is copied verbatim,
/// so an already-valid sequence is never touched — in particular an existing
/// `\\` is consumed as a single valid escape and never becomes `\\\\`.
///
/// Like [`strip_json_trailing_commas`] and [`escape_json_string_control_chars`]
/// this is a **provable no-op on valid JSON**: valid JSON strings never contain
/// an invalid escape (the grammar forbids a backslash not followed by an escape
/// initiator), so [`has_invalid_string_escape`] returns `false` and the input
/// borrows back byte-for-byte unchanged (the zero-allocation clean path). A
/// caller therefore retries a strict-parse failure on this view without any risk
/// of altering behaviour on well-formed output.
///
/// String-literal aware in exactly the same way as the rest of this module: the
/// scan tracks `in_string` (consuming a valid `\`-escape as a unit so an escaped
/// quote `\"` never closes the string), so a backslash is doubled **only** inside
/// a string value. A backslash BETWEEN tokens is not valid JSON at all and is
/// left untouched (the strict parse still rejects that shape). Only the ASCII
/// backslash byte is ever inserted, so the result is always valid UTF-8 and every
/// multibyte UTF-8 body byte (all `>= 0x80`) is copied through unchanged.
///
/// Leniency never widens beyond this one defect: an invalid escape is the only
/// shape rewritten, so a genuinely malformed object (unquoted key, elided
/// element, missing value) is left still-malformed and the caller's strict parse
/// still rejects it.
pub fn escape_json_string_invalid_escapes(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    // Cheap detection pass first so clean JSON borrows unchanged (zero-alloc).
    if !has_invalid_string_escape(bytes) {
        return Cow::Borrowed(s);
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + 8);
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\\' {
                match bytes.get(i + 1) {
                    // Legitimate escape — copy the backslash and its target byte
                    // verbatim as a unit (so `\"` does not close the string and
                    // an existing `\\` is never re-doubled).
                    Some(&next) if is_valid_json_escape_char(next) => {
                        out.push(c);
                        out.push(next);
                        i += 2;
                        continue;
                    }
                    // Invalid escape (lone backslash, or a trailing `\` at the
                    // very end of input) — double it to a literal backslash.
                    _ => out.extend_from_slice(b"\\\\"),
                }
            } else if c == b'"' {
                out.push(c);
                in_string = false;
            } else {
                out.push(c);
            }
        } else if c == b'"' {
            in_string = true;
            out.push(c);
        } else {
            out.push(c);
        }
        i += 1;
    }
    match String::from_utf8(out) {
        Ok(fixed) => Cow::Owned(fixed),
        // Unreachable in practice (only ASCII backslash bytes are inserted and
        // every original byte is preserved), but never panic on recovery input.
        Err(_) => Cow::Borrowed(s),
    }
}

/// Does `bytes` contain at least one invalid backslash escape inside a JSON
/// string literal (a `\` not followed by a JSON escape initiator)?
///
/// Mirrors the string-aware scan in [`escape_json_string_invalid_escapes`] but
/// only detects, so the common clean case can borrow the input without
/// allocating. A backslash OUTSIDE a string does not count (that shape is not
/// valid JSON at all and is left for the strict parse to reject); a legitimate
/// escape is consumed as a unit and never counts.
fn has_invalid_string_escape(bytes: &[u8]) -> bool {
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\\' {
                match bytes.get(i + 1) {
                    Some(&next) if is_valid_json_escape_char(next) => {
                        // Valid escape — consume both bytes as a unit.
                        i += 2;
                        continue;
                    }
                    // Lone/invalid backslash (or trailing `\` at end of input).
                    _ => return true,
                }
            } else if c == b'"' {
                in_string = false;
            }
        } else if c == b'"' {
            in_string = true;
        }
        i += 1;
    }
    false
}

/// Strip **JavaScript-style comments that appear OUTSIDE a JSON string literal**
/// — a `// …` line comment (to end-of-line) or a `/* … */` block comment — as a
/// last-resort recovery view for otherwise-well-formed LLM JSON.
///
/// After the trailing comma, the unescaped control character, and the invalid
/// backslash escape, an interleaved comment is the next most common real-world
/// LLM JSON defect: a model narrating its structured decision writes
/// `"confidence": 0.8 // high` or `/* rationale below */` between fields, the
/// "JSONC" shape it has seen throughout its training data. `serde_json` is
/// spec-strict — the JSON grammar has no comment production — and rejects the
/// stray `/` (`expected value` / `trailing characters`), so the model's WHOLE
/// structured decision is dropped on the annotation — a parse-failure default,
/// not a real model decision.
///
/// Like [`strip_json_trailing_commas`] this is a **provable no-op on valid
/// JSON**: valid JSON contains a `/` byte only INSIDE a string literal (a URL,
/// a path, a date) — outside a string the grammar permits only structural
/// punctuation, numbers, the three literals, and whitespace, none of which is a
/// `/`. So [`has_json_comment`] returns `false` on any well-formed payload and
/// the input borrows back byte-for-byte unchanged (the zero-allocation clean
/// path). A caller therefore retries a strict-parse failure on this view without
/// any risk of altering behaviour on well-formed output.
///
/// String-literal aware in exactly the same way as the rest of this module: the
/// scan tracks `in_string` (respecting `\"` escapes), so a `//` or `/*` **inside**
/// a string value — the `//` in `"http://example.com"`, the `/*` in a glob — is a
/// legitimate content byte and is copied through untouched. A comment is
/// recognised **only** outside a string. A lone `/` outside a string that is not
/// followed by `/` or `*` is not a comment (it is simply invalid JSON) and is
/// left in place for the strict parse to reject; leniency never widens beyond a
/// real comment. Only whole comment spans — each bounded by ASCII delimiters
/// (`//`…newline, `/*`…`*/`) — are removed, so every surviving byte, including
/// any multibyte UTF-8 body byte (all `>= 0x80`) outside a comment, is copied
/// through unchanged and the result is always valid UTF-8.
pub fn strip_json_comments(s: &str) -> Cow<'_, str> {
    let bytes = s.as_bytes();
    // Cheap detection pass first so clean JSON borrows unchanged (zero-alloc).
    if !has_json_comment(bytes) {
        return Cow::Borrowed(s);
    }
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
            i += 1;
        } else if c == b'"' {
            in_string = true;
            out.push(c);
            i += 1;
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            // Line comment — drop from `//` to (but not including) the next
            // newline, leaving the newline as legitimate JSON whitespace.
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            // Block comment — drop from `/*` through the closing `*/`. An
            // unterminated block (no `*/`) is dropped to end of input; the
            // truncated object then fails the strict parse (never masked).
            i += 2;
            while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            // Skip the closing `*/` when present.
            if i < bytes.len() {
                i += 2;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    match String::from_utf8(out) {
        Ok(stripped) => Cow::Owned(stripped),
        // Unreachable in practice (only whole ASCII-delimited comment spans are
        // dropped and every surviving byte is preserved), but never panic on
        // recovery input — fall back to the original text.
        Err(_) => Cow::Borrowed(s),
    }
}

/// Does `bytes` contain at least one JavaScript-style comment (`//` or `/*`)
/// OUTSIDE a JSON string literal?
///
/// Mirrors the string-aware scan in [`strip_json_comments`] but only detects, so
/// the common clean case can borrow the input without allocating. A `//` or `/*`
/// INSIDE a string (a URL, a glob) is a content byte and does not count; a lone
/// `/` not followed by `/` or `*` is not a comment and does not count.
fn has_json_comment(bytes: &[u8]) -> bool {
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
        } else if c == b'/' && matches!(bytes.get(i + 1), Some(&b'/') | Some(&b'*')) {
            return true;
        }
        i += 1;
    }
    false
}

/// Compose every JSON **recovery view** this module applies to an
/// otherwise-well-formed LLM JSON payload that failed a strict parse — currently
/// JavaScript-comment stripping ([`strip_json_comments`]), then
/// invalid-backslash-escape doubling ([`escape_json_string_invalid_escapes`]),
/// then unescaped-control-character escaping ([`escape_json_string_control_chars`]),
/// then trailing-comma stripping ([`strip_json_trailing_commas`]).
///
/// Returns [`Cow::Borrowed`] byte-for-byte unchanged when NONE of the defects is
/// present (each sub-view is a provable no-op on valid JSON), and
/// [`Cow::Owned`] only when at least one recovery actually rewrote the input.
/// A caller retries a strict-parse failure **only** on the `Cow::Owned` arm, so
/// identical bytes are never re-parsed and a payload malformed for any OTHER
/// reason (unquoted key, elided element, missing value) preserves the strict
/// miss unchanged.
///
/// The four recoveries are independent — the two string-view recoveries touch
/// only bytes inside string literals, comment-stripping and trailing-comma
/// stripping only touch bytes outside them. Comment-stripping runs **first**,
/// ahead of the two string-aware views, on purpose: a `"` byte inside a `//` or
/// `/* */` comment (`// see "foo"`) is comment text, not a string delimiter, so
/// removing whole comment spans before any string-tracking view runs keeps every
/// downstream `in_string` scan aligned with the real string boundaries. On a
/// comment-free payload [`strip_json_comments`] borrows unchanged, so the
/// pre-existing three-view recovery and its ordering are preserved exactly.
///
/// The two string views are ordered invalid-escape **before** control-char on
/// purpose: a model that emits a lone backslash immediately followed by a raw
/// newline (`\` + the newline byte) is a backslash-then-control-char pair;
/// doubling the backslash first (`\\` + raw newline) lets the control-char view
/// then escape the newline (`\\` + `\n`), reproducing the intended two
/// characters. Running control-char first would instead treat the raw newline as
/// the (invalid) escape target of the backslash and leave it a raw control byte.
pub fn recover_json_view(s: &str) -> Cow<'_, str> {
    // Step 0: strip any JavaScript-style comment outside string literals, before
    // the string-aware views run so a quote inside a comment can never desync
    // their string tracking.
    let step0 = strip_json_comments(s);
    // Step 1: double any invalid backslash escape (string-literal aware),
    // carrying the borrow/owned distinction through so a full no-op stays borrowed.
    let step1 = match step0 {
        Cow::Borrowed(b) => escape_json_string_invalid_escapes(b),
        Cow::Owned(o) => Cow::Owned(escape_json_string_invalid_escapes(&o).into_owned()),
    };
    // Step 2: escape any unescaped control char inside a string literal, carrying
    // the borrow/owned distinction through so a full no-op stays borrowed.
    let step2 = match step1 {
        Cow::Borrowed(b) => escape_json_string_control_chars(b),
        Cow::Owned(o) => Cow::Owned(escape_json_string_control_chars(&o).into_owned()),
    };
    // Step 3: strip any structural trailing comma outside string literals.
    match step2 {
        Cow::Borrowed(b) => strip_json_trailing_commas(b),
        Cow::Owned(o) => Cow::Owned(strip_json_trailing_commas(&o).into_owned()),
    }
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

/// Extract a balanced JSON object from noisy recipe stdout (via
/// [`extract_json_payload`]) and deserialize it into `T`, applying the shared
/// **recovery views** on a strict-parse failure (issues #2484 / #2658).
///
/// This is the shared reasoner-side extract-and-parse chokepoint. Every
/// recipe-backed reasoning phase (engineer/resource admission, idea dedup /
/// consolidation, outcome, decide, orient) reads its structured decision by
/// extracting the balanced `{…}` object and `serde_json`-deserializing it.
/// [`extract_json_payload`] strips the banner / ANSI / log noise but returns
/// the object body **verbatim**, so four common real-world LLM JSON defects
/// survive into the payload and fail a strict `serde_json::from_str`:
///
///  1. a `,` immediately before a closing `}`/`]` (the trailing comma,
///     issue #2658),
///  2. an UNescaped ASCII control character (a raw newline/tab/CR) inside a
///     string value — the shape a model emits for a multi-line
///     `content`/`rationale` field,
///  3. an INVALID backslash escape inside a string value — a lone `\` not
///     followed by a JSON escape initiator, the shape a model emits for a
///     Windows path (`C:\Users`), a regex (`\d+`), or a LaTeX fragment
///     (`\alpha`), and
///  4. a JavaScript-style COMMENT outside a string value — a `// …` line or
///     `/* … */` block comment, the "JSONC" shape a model emits to annotate a
///     field (`"confidence": 0.8 // high`).
///
/// Before this helper each phase parsed strictly and silently dropped its whole
/// structured decision on any one stray byte, falling back to a permissive
/// default and discarding the reasoner's actual judgment.
///
/// Recovery retries the strict parse on the composed [`recover_json_view`]
/// (comment stripping + invalid-escape doubling + control-character escaping +
/// trailing-comma stripping). Each sub-view is a *provable no-op on valid JSON* —
/// it returns [`Cow::Borrowed`] byte-for-byte unchanged unless its specific
/// defect is present — so recovery is attempted **only** when a view actually
/// rewrote the payload (the [`Cow::Owned`] arm). Any other malformed shape
/// (unquoted key, elided element, missing value) yields [`Cow::Borrowed`] and
/// returns `None` unchanged: leniency never widens beyond the four named defects
/// and a genuine parse error is never masked.
///
/// # Visibility (issue #969)
///
/// A drop is **never silent**. Before returning `None`, this function fires a
/// structured `tracing::error!` (target `simard::recipe_output`) naming the
/// drop site (`failure_kind`), the destination type, the raw length, and a
/// truncated view of the offending text — the same visibility contract the
/// #1890 brain parse-failure fix established. Callers that want the typed
/// failure instead of a bare `None` can use [`extract_and_parse_json_result`].
pub fn extract_and_parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    match extract_and_parse_json_result::<T>(raw) {
        Ok(value) => Some(value),
        Err(err) => {
            // #969: never a *silent* `None`. Every drop that the `Option`
            // callers turn into a fail-open default is surfaced first through
            // the structured `tracing::error!` channel, mirroring the #1890
            // brain parse-failure visibility fix (`ooda_brain::parse_failure`).
            err.emit_trace::<T>(raw);
            None
        }
    }
}

/// Maximum bytes of `raw` recipe output / extracted payload embedded in a
/// [`JsonExtractError`] and its `tracing::error!` record. Matches the 8 KiB
/// cap used by [`crate::ooda_brain::parse_failure::RAW_RESPONSE_TRUNCATE_BYTES`]
/// and `truncate_for_log` in `ooda_actions/advance_goal/spawn.rs` so a
/// pathological multi-megabyte agent response cannot flood `~/.simard/logs`.
pub const RAW_EXTRACT_TRUNCATE_BYTES: usize = 8 * 1024;

/// Truncate `s` to at most [`RAW_EXTRACT_TRUNCATE_BYTES`] on a UTF-8 char
/// boundary for log/error embedding. Never panics on multi-byte input.
fn truncate_for_record(s: &str) -> String {
    // Only the retained prefix is allocated: a pathological multi-megabyte
    // agent response is capped to a `&str` view *before* the single `to_string`,
    // so this error path allocates at most `RAW_EXTRACT_TRUNCATE_BYTES` rather
    // than copying the whole input just to discard almost all of it.
    crate::util::string_truncate::head_within_budget(s, RAW_EXTRACT_TRUNCATE_BYTES).to_string()
}

/// Why [`extract_and_parse_json_result`] could not turn noisy recipe stdout
/// into a `T` — the typed, diagnosable failure that #969 asked for in place of
/// the former silent `None`.
///
/// Each variant carries a **truncated** copy of the relevant text (via
/// [`RAW_EXTRACT_TRUNCATE_BYTES`]) so an operator reading a surfaced error can
/// see the shape that defeated the parser without the record ever growing
/// unbounded. The three variants map one-to-one onto the three drop sites the
/// old `Option`-returning code had:
///
///  1. [`NoBalancedObject`](JsonExtractError::NoBalancedObject) — neither the
///     `strip_recipe_noise` view nor the `strip_ansi` view contained a
///     balanced top-level `{…}` object.
///  2. [`Unrecoverable`](JsonExtractError::Unrecoverable) — a balanced object
///     was found but strict `serde_json` rejected it and no recovery view
///     rewrote it (some malformed shape outside the four named defects, e.g.
///     an unquoted key or an elided element).
///  3. [`RecoveredParseFailed`](JsonExtractError::RecoveredParseFailed) — a
///     recovery view rewrote the payload but the retry parse still failed.
#[derive(Debug)]
pub enum JsonExtractError {
    /// No balanced top-level `{…}` object in either cleaned view.
    NoBalancedObject {
        /// Truncated copy of the raw recipe stdout that yielded no object.
        raw_truncated: String,
    },
    /// A balanced object was found but strict parse failed with no recovery.
    Unrecoverable {
        /// Truncated copy of the extracted (but unparseable) payload.
        payload_truncated: String,
        /// The strict-parse `serde_json` error.
        source: serde_json::Error,
    },
    /// A recovery view rewrote the payload but the retry parse still failed.
    RecoveredParseFailed {
        /// Truncated copy of the recovered payload that still failed to parse.
        recovered_truncated: String,
        /// The `serde_json` error from the retry parse of the recovered view.
        source: serde_json::Error,
    },
}

impl JsonExtractError {
    /// Stable, low-cardinality tag identifying the drop site, for the
    /// `failure_kind` structured field and metric slicing.
    pub fn kind(&self) -> &'static str {
        match self {
            JsonExtractError::NoBalancedObject { .. } => "no_balanced_object",
            JsonExtractError::Unrecoverable { .. } => "unrecoverable",
            JsonExtractError::RecoveredParseFailed { .. } => "recovered_parse_failed",
        }
    }

    /// Fire the structured `tracing::error!` visibility channel for this
    /// failure, tagged with the destination type `T`. Called by the
    /// `Option`-returning [`extract_and_parse_json`] so no caller that keeps
    /// the ergonomic `Option` API loses the drop diagnostics.
    fn emit_trace<T>(&self, raw: &str) {
        let target_type = std::any::type_name::<T>();
        match self {
            JsonExtractError::NoBalancedObject { raw_truncated } => {
                tracing::error!(
                    target: "simard::recipe_output",
                    failure_kind = self.kind(),
                    target_type,
                    raw_len = raw.len(),
                    raw = %raw_truncated,
                    "recipe output finalization: no balanced JSON object in agent response (#969)"
                );
            }
            JsonExtractError::Unrecoverable {
                payload_truncated,
                source,
            } => {
                tracing::error!(
                    target: "simard::recipe_output",
                    failure_kind = self.kind(),
                    target_type,
                    raw_len = raw.len(),
                    payload = %payload_truncated,
                    error = %source,
                    "recipe output finalization: extracted JSON object failed strict parse and was not recoverable (#969)"
                );
            }
            JsonExtractError::RecoveredParseFailed {
                recovered_truncated,
                source,
            } => {
                tracing::error!(
                    target: "simard::recipe_output",
                    failure_kind = self.kind(),
                    target_type,
                    raw_len = raw.len(),
                    recovered = %recovered_truncated,
                    error = %source,
                    "recipe output finalization: recovered JSON view still failed to parse (#969)"
                );
            }
        }
    }
}

impl std::fmt::Display for JsonExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsonExtractError::NoBalancedObject { .. } => {
                write!(f, "no balanced JSON object in recipe output")
            }
            JsonExtractError::Unrecoverable { source, .. } => {
                write!(
                    f,
                    "recipe output JSON failed strict parse (unrecoverable): {source}"
                )
            }
            JsonExtractError::RecoveredParseFailed { source, .. } => {
                write!(
                    f,
                    "recovered recipe output JSON still failed to parse: {source}"
                )
            }
        }
    }
}

impl std::error::Error for JsonExtractError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            JsonExtractError::NoBalancedObject { .. } => None,
            JsonExtractError::Unrecoverable { source, .. }
            | JsonExtractError::RecoveredParseFailed { source, .. } => Some(source),
        }
    }
}

/// Typed sibling of [`extract_and_parse_json`]: extract a balanced JSON object
/// from noisy recipe stdout and deserialize it into `T`, returning a
/// [`JsonExtractError`] that names *why* on a miss instead of a bare `None`
/// (issue #969).
///
/// Parsing semantics are **identical** to [`extract_and_parse_json`] — the
/// same two cleaned views, the same single bounded recovery retry, the same
/// "leniency never widens beyond the four named defects" guarantee. The only
/// difference is the return shape: every success path yields the same `T`, and
/// every drop path yields a diagnosable, `std::error::Error`-implementing
/// value. [`extract_and_parse_json`] is a thin wrapper that maps `Err(_)` back
/// to `None` after firing the `tracing::error!` visibility channel, so the
/// existing `Option` call sites keep their exact behaviour while a caller that
/// wants the typed error can call this function directly.
pub fn extract_and_parse_json_result<T: serde::de::DeserializeOwned>(
    raw: &str,
) -> Result<T, JsonExtractError> {
    let payload = extract_json_payload(raw).ok_or_else(|| JsonExtractError::NoBalancedObject {
        raw_truncated: truncate_for_record(raw),
    })?;
    match serde_json::from_str::<T>(&payload) {
        Ok(value) => Ok(value),
        Err(strict_err) => match recover_json_view(&payload) {
            // A recovery view actually rewrote the payload — retry the strict
            // parse on the recovered view.
            Cow::Owned(recovered) => serde_json::from_str::<T>(&recovered).map_err(|source| {
                JsonExtractError::RecoveredParseFailed {
                    recovered_truncated: truncate_for_record(&recovered),
                    source,
                }
            }),
            // Nothing was recovered: the payload is malformed for some other
            // reason. Do not re-parse identical bytes; preserve the strict miss.
            Cow::Borrowed(_) => Err(JsonExtractError::Unrecoverable {
                payload_truncated: truncate_for_record(&payload),
                source: strict_err,
            }),
        },
    }
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

    // ---- extract_and_parse_json ------------------------------------------

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Env {
        decision: String,
        #[serde(default)]
        items: Vec<String>,
    }

    #[test]
    fn extract_and_parse_json_parses_clean_object() {
        let raw = r#"{"decision": "admit", "items": ["a", "b"]}"#;
        let env: Env = extract_and_parse_json(raw).expect("clean object parses");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_trailing_comma_before_brace() {
        // The strict `serde_json::from_str` on the extracted payload rejects
        // this; the trailing-comma recovery view rescues it.
        let raw = r#"{"decision": "admit", "items": ["a"],}"#;
        let env: Env = extract_and_parse_json(raw).expect("trailing comma recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["a".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_trailing_comma_before_bracket() {
        let raw = r#"{"decision": "defer", "items": ["a", "b",]}"#;
        let env: Env = extract_and_parse_json(raw).expect("trailing comma in array recovered");
        assert_eq!(env.items, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_trailing_comma_through_banner_noise() {
        // Banner + interleaved log line AND a trailing comma: the extractor
        // strips the noise, then recovery strips the comma — the two hardening
        // passes compose.
        let raw = "Recipe: ooda SUCCESS (3.0s)\n\
                   \x1b[2m2026-07-20T00:00:00.000000Z\x1b[0m INFO decide\n\
                   {\"decision\": \"admit\", \"items\": [\"a\",],}";
        let env: Env = extract_and_parse_json(raw).expect("noise + trailing comma recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["a".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_none_for_non_comma_malformed() {
        // Leniency never widens past the trailing-comma defect: an unquoted key
        // or missing value is still a miss (returns None, not a wrong default).
        assert_eq!(
            extract_and_parse_json::<Env>(r#"{decision: "admit"}"#),
            None
        );
        assert_eq!(extract_and_parse_json::<Env>(r#"{"decision":}"#), None);
    }

    #[test]
    fn extract_and_parse_json_none_when_no_object_present() {
        assert_eq!(
            extract_and_parse_json::<Env>("2026-07-20 INFO no json here"),
            None
        );
    }

    #[test]
    fn extract_and_parse_json_preserves_comma_inside_string_content() {
        // A comma inside a string value is a legitimate content byte and must
        // survive both the strict parse and the recovery view unchanged.
        let raw = r#"{"decision": "admit", "items": ["a, b, c"]}"#;
        let env: Env = extract_and_parse_json(raw).expect("string-content comma preserved");
        assert_eq!(env.items, vec!["a, b, c".to_string()]);
    }

    // ---- extract_and_parse_json_result (typed errors, issue #969) ---------

    #[test]
    fn result_ok_matches_option_some_on_clean_object() {
        // The typed `Result` API returns the same value the `Option` API does
        // on success — no behavioural divergence between the two entry points.
        let raw = r#"{"decision": "admit", "items": ["a", "b"]}"#;
        let via_result: Env =
            extract_and_parse_json_result(raw).expect("clean object parses via result");
        let via_option: Env = extract_and_parse_json(raw).expect("clean object parses via option");
        assert_eq!(via_result, via_option);
        assert_eq!(via_result.decision, "admit");
    }

    #[test]
    fn result_ok_on_recovered_trailing_comma() {
        // Recovery success is a success in the typed API too.
        let raw = r#"{"decision": "admit", "items": ["a"],}"#;
        let env: Env =
            extract_and_parse_json_result(raw).expect("trailing comma recovered via result");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["a".to_string()]);
    }

    #[test]
    fn result_no_balanced_object_when_no_object_present() {
        // The "no `{…}` in either cleaned view" drop is a typed
        // `NoBalancedObject`, carrying a truncated copy of the raw input — the
        // #969 replacement for the former silent `None`.
        let raw = "2026-07-20 INFO no json here";
        let err =
            extract_and_parse_json_result::<Env>(raw).expect_err("no object must be a typed error");
        match err {
            JsonExtractError::NoBalancedObject { ref raw_truncated } => {
                assert!(raw_truncated.contains("no json here"));
            }
            ref other => panic!("expected NoBalancedObject, got {other:?}"),
        }
        assert_eq!(err.kind(), "no_balanced_object");
        // The `Option` wrapper still drops to `None` on the same input.
        assert_eq!(extract_and_parse_json::<Env>(raw), None);
    }

    #[test]
    fn result_unrecoverable_for_non_comma_malformed() {
        // A balanced object is present, strict parse fails, and no recovery view
        // rewrites it (unquoted key / missing value are outside the four named
        // defects) — a typed `Unrecoverable` carrying the serde source error.
        for raw in [r#"{decision: "admit"}"#, r#"{"decision":}"#] {
            let err = extract_and_parse_json_result::<Env>(raw)
                .expect_err("non-comma malformed must be a typed error");
            match &err {
                JsonExtractError::Unrecoverable {
                    payload_truncated,
                    source,
                } => {
                    assert!(!payload_truncated.is_empty());
                    // The serde error is preserved and reachable via Display.
                    assert!(!source.to_string().is_empty());
                }
                other => panic!("expected Unrecoverable for {raw:?}, got {other:?}"),
            }
            assert_eq!(err.kind(), "unrecoverable");
            // Parity: the `Option` API drops the same inputs to `None`.
            assert_eq!(extract_and_parse_json::<Env>(raw), None);
        }
    }

    #[test]
    fn result_recovered_parse_failed_when_recovery_rewrites_but_still_invalid() {
        // A trailing comma triggers the recovery view (so it is rewritten,
        // taking the `Cow::Owned` arm), but the type constraint is still
        // violated: `items` must be a `Vec<String>`, here it holds a number.
        // Recovery strips the comma, the retry parse still fails on the type
        // mismatch -> typed `RecoveredParseFailed`.
        let raw = r#"{"decision": "admit", "items": [1, 2],}"#;
        let err = extract_and_parse_json_result::<Env>(raw)
            .expect_err("recovered-but-type-invalid must be a typed error");
        match &err {
            JsonExtractError::RecoveredParseFailed {
                recovered_truncated,
                source,
            } => {
                // The comma was stripped in the recovered view.
                assert!(!recovered_truncated.contains(",]"));
                assert!(!source.to_string().is_empty());
            }
            other => panic!("expected RecoveredParseFailed, got {other:?}"),
        }
        assert_eq!(err.kind(), "recovered_parse_failed");
        // Parity with the `Option` API.
        assert_eq!(extract_and_parse_json::<Env>(raw), None);
    }

    #[test]
    fn json_extract_error_display_and_source_are_populated() {
        // `Display` is human-readable and `source()` chains to the serde error
        // for the two parse-failure variants (and is `None` for the
        // no-object variant).
        let no_obj = extract_and_parse_json_result::<Env>("nope")
            .expect_err("no object")
            .to_string();
        assert!(no_obj.contains("no balanced JSON object"));

        let unrecoverable =
            extract_and_parse_json_result::<Env>(r#"{"decision":}"#).expect_err("unrecoverable");
        assert!(std::error::Error::source(&unrecoverable).is_some());

        let no_source =
            extract_and_parse_json_result::<Env>("no object here").expect_err("no object");
        assert!(std::error::Error::source(&no_source).is_none());
    }

    #[test]
    fn record_truncation_caps_embedded_raw_at_budget() {
        // A pathological multi-megabyte agent response must not embed unbounded
        // text into the error / log record: the raw copy is capped at
        // `RAW_EXTRACT_TRUNCATE_BYTES`.
        let huge = "x".repeat(RAW_EXTRACT_TRUNCATE_BYTES * 4);
        let err =
            extract_and_parse_json_result::<Env>(&huge).expect_err("no object in a wall of x's");
        match err {
            JsonExtractError::NoBalancedObject { raw_truncated } => {
                assert!(
                    raw_truncated.len() <= RAW_EXTRACT_TRUNCATE_BYTES,
                    "embedded raw must respect the byte budget, got {}",
                    raw_truncated.len()
                );
            }
            other => panic!("expected NoBalancedObject, got {other:?}"),
        }
    }

    // ---- escape_json_string_control_chars --------------------------------

    #[test]
    fn escape_control_chars_valid_json_is_borrowed_zero_copy() {
        // Valid JSON (no raw control char inside any string) borrows unchanged.
        for s in [
            r#"{"a": "b"}"#,
            r#"{"a": "line1\nline2"}"#, // already-escaped \n must NOT double-escape
            r#"{"items": ["x", "y"], "n": 3}"#,
            "{\n  \"a\": \"b\"\n}", // raw newlines are OUTSIDE strings (whitespace)
        ] {
            assert!(
                matches!(escape_json_string_control_chars(s), Cow::Borrowed(_)),
                "expected borrow for {s:?}"
            );
        }
    }

    #[test]
    fn escape_control_chars_escapes_raw_newline_and_tab_inside_string() {
        let raw = "{\"content\": \"line one\nline two\ttabbed\"}";
        let fixed = escape_json_string_control_chars(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        assert_eq!(
            fixed.as_ref(),
            r#"{"content": "line one\nline two\ttabbed"}"#
        );
        // And the recovered view is now strict-parseable.
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["content"], "line one\nline two\ttabbed");
    }

    #[test]
    fn escape_control_chars_escapes_carriage_return_and_other_controls() {
        // CR -> \r, and a NUL (0x00) -> \u0000 via the generic arm.
        let raw = "{\"a\": \"x\ry\u{0}z\"}";
        let fixed = escape_json_string_control_chars(raw);
        assert_eq!(fixed.as_ref(), r#"{"a": "x\ry\u0000z"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "x\ry\u{0}z");
    }

    #[test]
    fn escape_control_chars_leaves_control_bytes_outside_strings_untouched() {
        // A raw newline/tab BETWEEN tokens is valid JSON whitespace, not a
        // string-interior defect: it must be left exactly as-is (borrow).
        let raw = "{\"a\":\t\"b\",\n\"c\":\t1}";
        assert!(matches!(
            escape_json_string_control_chars(raw),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn escape_control_chars_respects_escaped_quote_in_string() {
        // The `\"` must not end the string, so the raw newline that follows it
        // is still inside the string and gets escaped.
        let raw = "{\"a\": \"he said \\\"hi\\\"\nbye\"}";
        let fixed = escape_json_string_control_chars(raw);
        assert_eq!(fixed.as_ref(), r#"{"a": "he said \"hi\"\nbye"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "he said \"hi\"\nbye");
    }

    #[test]
    fn escape_control_chars_preserves_multibyte_utf8() {
        // A multibyte body char (é, 日) plus a raw newline: the newline is
        // escaped, the multibyte bytes pass through intact.
        let raw = "{\"a\": \"café\n日本\"}";
        let fixed = escape_json_string_control_chars(raw);
        assert_eq!(fixed.as_ref(), r#"{"a": "café\n日本"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "café\n日本");
    }

    // ---- escape_json_string_invalid_escapes ------------------------------

    #[test]
    fn escape_invalid_escapes_valid_json_is_borrowed_zero_copy() {
        // Valid JSON (every backslash is a legitimate escape) borrows unchanged.
        for s in [
            r#"{"a": "b"}"#,
            r#"{"a": "line1\nline2"}"#,    // \n is a valid escape
            r#"{"a": "quote \" here"}"#,   // \" is a valid escape
            r#"{"a": "back \\ slash"}"#,   // \\ is a valid escape (not re-doubled)
            r#"{"a": "tab\tend"}"#,        // \t is a valid escape
            r#"{"a": "u \u00e9 nicode"}"#, // \u is a valid escape initiator
            r#"{"a": "slash \/ ok"}"#,     // \/ is a valid escape
            r#"{"items": ["x", "y"], "n": 3}"#,
        ] {
            assert!(
                matches!(escape_json_string_invalid_escapes(s), Cow::Borrowed(_)),
                "expected borrow for {s:?}"
            );
        }
    }

    #[test]
    fn escape_invalid_escapes_doubles_lone_backslash_inside_string() {
        // A regex `\d+` inside a string: the lone `\` is not a valid escape and
        // is doubled to a literal backslash, making the payload strict-parseable.
        let raw = r#"{"pattern": "\d+ digits"}"#;
        let fixed = escape_json_string_invalid_escapes(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        assert_eq!(fixed.as_ref(), r#"{"pattern": "\\d+ digits"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["pattern"], r"\d+ digits");
    }

    #[test]
    fn escape_invalid_escapes_doubles_windows_path_backslashes() {
        // A Windows path `C:\Users\model` — each lone backslash is doubled.
        let raw = "{\"path\": \"C:\\Users\\model\"}";
        let fixed = escape_json_string_invalid_escapes(raw);
        assert_eq!(fixed.as_ref(), r#"{"path": "C:\\Users\\model"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["path"], r"C:\Users\model");
    }

    #[test]
    fn escape_invalid_escapes_leaves_valid_escape_untouched_next_to_invalid() {
        // A string mixing a VALID escape (`\n`) and an INVALID one (`\a`): only
        // the invalid backslash is doubled; the valid `\n` is preserved as-is.
        let raw = "{\"a\": \"line\\none\\atwo\"}";
        let fixed = escape_json_string_invalid_escapes(raw);
        assert_eq!(fixed.as_ref(), r#"{"a": "line\none\\atwo"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "line\none\\atwo");
    }

    #[test]
    fn escape_invalid_escapes_respects_escaped_quote_in_string() {
        // The `\"` is a valid escape and must NOT end the string, so a lone
        // backslash that follows it is still inside the string and gets doubled.
        let raw = "{\"a\": \"say \\\"hi\\\" then \\x done\"}";
        let fixed = escape_json_string_invalid_escapes(raw);
        assert_eq!(fixed.as_ref(), r#"{"a": "say \"hi\" then \\x done"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], r#"say "hi" then \x done"#);
    }

    #[test]
    fn escape_invalid_escapes_leaves_backslash_outside_string_untouched() {
        // A backslash OUTSIDE any string is not a string-interior defect (that
        // shape is not valid JSON at all); the view leaves it for the strict
        // parse to reject and borrows the input unchanged.
        let raw = r#"{"a": "b"} \ trailing"#;
        assert!(matches!(
            escape_json_string_invalid_escapes(raw),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn escape_invalid_escapes_preserves_multibyte_utf8() {
        // A multibyte body char (é, 日) plus a lone backslash: the backslash is
        // doubled, the multibyte bytes pass through intact.
        let raw = "{\"a\": \"café \\d 日本\"}";
        let fixed = escape_json_string_invalid_escapes(raw);
        assert_eq!(fixed.as_ref(), r#"{"a": "café \\d 日本"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], r"café \d 日本");
    }

    #[test]
    fn escape_invalid_escapes_valid_escaped_backslash_adjacent_to_invalid() {
        // Adversarial: a valid escaped backslash `\\` IMMEDIATELY followed by an
        // invalid escape `\d`. The `\\` must be consumed as a unit (not seen as a
        // backslash that then swallows the next `\`), and only the trailing lone
        // backslash doubled. Bytes in the string body: \ \ \ d
        // -> valid `\\` (one literal backslash) + invalid `\d` (literal `\` + d).
        let raw = "{\"a\": \"\\\\\\d\"}";
        let fixed = escape_json_string_invalid_escapes(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        assert_eq!(fixed.as_ref(), r#"{"a": "\\\\d"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], r"\\d");
    }

    #[test]
    fn escape_invalid_escapes_terminated_single_backslash_body() {
        // A clearly-terminated string body `x\y` (x, lone backslash, y): the lone
        // backslash is doubled to a parseable single-backslash string.
        let terminated = "{\"a\": \"x\\y\"}"; // body: x, backslash, y
        let fixed = escape_json_string_invalid_escapes(terminated);
        assert!(matches!(fixed, Cow::Owned(_)));
        assert_eq!(fixed.as_ref(), r#"{"a": "x\\y"}"#);
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], r"x\y");
    }

    #[test]
    fn escape_invalid_escapes_trailing_backslash_at_end_of_input_no_panic() {
        // A lone backslash as the very last byte (truncated capture): the
        // bytes.get(i + 1) == None arm must double it without panicking. The
        // string stays unterminated so it is not spuriously accepted downstream.
        let raw = "{\"a\": \"tail\\"; // ends with a lone backslash, no closing quote
        let fixed = escape_json_string_invalid_escapes(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        assert_eq!(fixed.as_ref(), "{\"a\": \"tail\\\\");
        // Still not valid JSON (unterminated string) — recovery never masks that.
        assert!(serde_json::from_str::<serde_json::Value>(fixed.as_ref()).is_err());
    }

    #[test]
    fn recover_json_view_recovers_invalid_escape_only() {
        // A lone backslash (regex) with no other defect: owned and parseable.
        let fixed = recover_json_view(r#"{"pattern": "\d+"}"#);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["pattern"], r"\d+");
    }

    #[test]
    fn recover_json_view_composes_all_three_defects() {
        // Invalid escape (`\d`), raw control char (newline), AND a trailing comma
        // in one payload: all three recovery views compose to a parseable object.
        let raw = "{\"a\": \"re \\d\nmulti\", \"b\": [1, 2,],}";
        let fixed = recover_json_view(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "re \\d\nmulti");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn recover_json_view_orders_invalid_escape_before_control_char() {
        // The ordering hazard: a lone backslash immediately followed by a RAW
        // newline byte (`\` + 0x0a) is a backslash-then-newline pair. Doubling the
        // backslash first (`\\` + raw newline) lets the control-char view then
        // escape the newline, reproducing the intended two characters. If the
        // control-char view ran first it would treat the raw newline as the
        // backslash's (invalid) escape target and leave it a raw control byte,
        // yielding invalid JSON.
        let raw = "{\"a\": \"x\\\ny\"}"; // string body: x, backslash, newline, y
        let fixed = recover_json_view(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value =
            serde_json::from_str(fixed.as_ref()).expect("ordered recovery is parseable");
        assert_eq!(v["a"], "x\\\ny");
    }

    #[test]
    fn recover_json_view_invalid_escape_only_still_owned() {
        assert!(matches!(
            recover_json_view(r#"{"a": "C:\Temp"}"#),
            Cow::Owned(_)
        ));
    }

    // ---- strip_json_comments ---------------------------------------------

    #[test]
    fn strip_comments_valid_json_is_borrowed_zero_copy() {
        // Valid JSON has no `/` outside a string, so the view borrows unchanged.
        // A `/` (and even `//`, `/*`) INSIDE a string is content and must borrow.
        for s in [
            r#"{"a": "b"}"#,
            r#"{"url": "http://example.com/x"}"#, // `//` inside a string is content
            r#"{"glob": "src/*/mod.rs"}"#,        // `/*` inside a string is content
            r#"{"path": "a/b/c", "n": 3}"#,
            r#"{"note": "closes a block */ here"}"#, // `*/` inside a string is content
        ] {
            assert!(
                matches!(strip_json_comments(s), Cow::Borrowed(_)),
                "expected borrow for {s:?}"
            );
        }
    }

    #[test]
    fn strip_comments_removes_line_comment() {
        // A `// …` line comment outside a string is dropped to end-of-line; the
        // newline (JSON whitespace) is kept so the object stays parseable.
        let raw = "{\"a\": 1, // trailing note\n\"b\": 2}";
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strip_comments_removes_line_comment_at_end_of_input() {
        // A `//` comment as the last thing in the payload (no trailing newline)
        // is dropped to end of input without panicking.
        let raw = "{\"a\": 1} // final";
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn strip_comments_removes_block_comment() {
        // A `/* … */` block comment between fields is dropped in full.
        let raw = r#"{"a": 1, /* rationale */ "b": 2}"#;
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strip_comments_removes_multiline_block_comment() {
        // A block comment may span lines (raw newlines inside the comment are
        // dropped with it, not mistaken for a string control char).
        let raw = "{\"a\": 1, /* line one\nline two */ \"b\": 2}";
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strip_comments_leaves_double_slash_inside_string() {
        // The whole defining property: a `//` inside a string value is content,
        // never a comment — the URL survives byte-for-byte and borrows.
        let raw = r#"{"url": "https://a.example//b", "ok": true}"#;
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Borrowed(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["url"], "https://a.example//b");
    }

    #[test]
    fn strip_comments_respects_escaped_quote_before_comment_delimiter() {
        // An escaped quote inside a string must NOT close it early, so a `//`
        // sequence that follows while still inside the string stays content.
        let raw = r#"{"a": "he said \"hi//\" there", "b": 2}"#;
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Borrowed(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], r#"he said "hi//" there"#);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strip_comments_lone_slash_is_not_a_comment() {
        // A single `/` outside a string is invalid JSON but NOT a comment: it is
        // left in place (borrow) for the strict parse to reject. Leniency never
        // widens beyond a real `//`/`/*` comment.
        let raw = r#"{"a": 1 / 2}"#;
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Borrowed(_)));
        assert!(serde_json::from_str::<serde_json::Value>(fixed.as_ref()).is_err());
    }

    #[test]
    fn strip_comments_preserves_multibyte_outside_comment() {
        // A multibyte UTF-8 string body outside any comment is copied through
        // unchanged when a comment elsewhere forces the owned path.
        let raw = "{\"a\": \"héllo→x\", /* note */ \"b\": 2}";
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "héllo→x");
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn strip_comments_unterminated_block_no_panic() {
        // An unterminated block comment (no closing `*/`) is dropped to end of
        // input without panicking; the truncated object is still not valid JSON,
        // so recovery never masks the miss.
        let raw = "{\"a\": 1, /* never closed";
        let fixed = strip_json_comments(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        assert!(serde_json::from_str::<serde_json::Value>(fixed.as_ref()).is_err());
    }

    #[test]
    fn strip_comments_other_malformed_stays_borrowed() {
        // No comment present (unquoted key) => borrow => the strict miss is
        // preserved for the caller.
        assert!(matches!(
            strip_json_comments(r#"{a: "b"}"#),
            Cow::Borrowed(_)
        ));
    }

    // ---- recover_json_view (composed) ------------------------------------

    #[test]
    fn recover_json_view_valid_json_is_borrowed() {
        assert!(matches!(
            recover_json_view(r#"{"a": "b", "c": [1, 2]}"#),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn recover_json_view_composes_control_char_and_trailing_comma() {
        // BOTH defects at once: raw newline inside a string AND a trailing comma.
        let raw = "{\"a\": \"one\ntwo\", \"b\": [1, 2,],}";
        let fixed = recover_json_view(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "one\ntwo");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn recover_json_view_trailing_comma_only_still_owned() {
        assert!(matches!(
            recover_json_view(r#"{"a": [1,],}"#),
            Cow::Owned(_)
        ));
    }

    #[test]
    fn recover_json_view_other_malformed_stays_borrowed() {
        // Neither target defect present (unquoted key) => borrow => caller's
        // strict miss is preserved.
        assert!(matches!(recover_json_view(r#"{a: "b"}"#), Cow::Borrowed(_)));
    }

    #[test]
    fn recover_json_view_recovers_comment_only() {
        // A line comment alone: owned and parseable.
        let fixed = recover_json_view("{\"a\": 1 // note\n}");
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn recover_json_view_comment_before_string_views_keeps_tracking_aligned() {
        // The ordering hazard the comment view guards: a `"` INSIDE a block
        // comment (`/* "x */`) must not be seen as a string delimiter by the
        // later string-aware views. Comment-stripping runs first, so the quote in
        // the comment is removed before any `in_string` scan, and a genuine
        // string defect (a raw newline in `a`) still recovers correctly.
        let raw = "{\"a\": \"one\ntwo\" /* an odd \" quote */, \"b\": 2}";
        let fixed = recover_json_view(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value =
            serde_json::from_str(fixed.as_ref()).expect("comment-first recovery is parseable");
        assert_eq!(v["a"], "one\ntwo");
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn recover_json_view_composes_all_four_defects() {
        // A comment, an invalid escape (`\d`), a raw control char (newline), and
        // a trailing comma in one payload: all four recovery views compose to a
        // parseable object.
        let raw = "{\"a\": \"re \\d\nmulti\", /* note */ \"b\": [1, 2,],}";
        let fixed = recover_json_view(raw);
        assert!(matches!(fixed, Cow::Owned(_)));
        let v: serde_json::Value = serde_json::from_str(fixed.as_ref()).unwrap();
        assert_eq!(v["a"], "re \\d\nmulti");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn recover_json_view_comment_only_still_owned() {
        assert!(matches!(
            recover_json_view("{\"a\": 1 /* x */}"),
            Cow::Owned(_)
        ));
    }

    #[test]
    fn extract_and_parse_json_recovers_control_char_in_string() {
        // End-to-end: a multi-line string value with a raw newline (the shape a
        // model emits for a `rationale`) is recovered through the shared
        // chokepoint instead of dropping the whole decision.
        let raw = "{\"decision\": \"admit\", \"items\": [\"first line\nsecond line\"]}";
        let env: Env = extract_and_parse_json(raw).expect("control char recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["first line\nsecond line".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_control_char_through_banner_noise() {
        // Banner + interleaved ANSI log line AND a raw newline inside a string:
        // extractor strips the noise, then recovery escapes the control char.
        let raw = "Recipe: ooda SUCCESS (3.0s)\n\
                   \x1b[2m2026-07-20T00:00:00.000000Z\x1b[0m INFO decide\n\
                   {\"decision\": \"defer\", \"items\": [\"a\tb\"]}";
        let env: Env = extract_and_parse_json(raw).expect("noise + control char recovered");
        assert_eq!(env.decision, "defer");
        assert_eq!(env.items, vec!["a\tb".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_control_char_and_trailing_comma_together() {
        let raw = "{\"decision\": \"admit\", \"items\": [\"x\ny\",],}";
        let env: Env = extract_and_parse_json(raw).expect("both defects recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["x\ny".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_invalid_escape_in_string() {
        // End-to-end: a `rationale`/`items` value carrying a regex `\d+` (a lone
        // backslash) is recovered through the shared chokepoint instead of
        // dropping the whole decision on the invalid escape.
        let raw = r#"{"decision": "admit", "items": ["match \d+ digits"]}"#;
        let env: Env = extract_and_parse_json(raw).expect("invalid escape recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec![r"match \d+ digits".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_invalid_escape_through_banner_noise() {
        // Banner + interleaved ANSI log line AND a Windows path (lone backslashes)
        // inside a string: the extractor strips the noise, then recovery doubles
        // the invalid escapes.
        let raw = "Recipe: ooda SUCCESS (3.0s)\n\
                   \x1b[2m2026-07-20T00:00:00.000000Z\x1b[0m INFO decide\n\
                   {\"decision\": \"defer\", \"items\": [\"C:\\Users\\log\"]}";
        let env: Env = extract_and_parse_json(raw).expect("noise + invalid escape recovered");
        assert_eq!(env.decision, "defer");
        assert_eq!(env.items, vec![r"C:\Users\log".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_all_three_defects_together() {
        // A single payload with all three recoverable defects: an invalid escape
        // (`\d`), a raw control char (newline), and a trailing comma.
        let raw = "{\"decision\": \"admit\", \"items\": [\"re \\d\nline\",],}";
        let env: Env = extract_and_parse_json(raw).expect("all three defects recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["re \\d\nline".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_line_comment_in_envelope() {
        // End-to-end: a model annotating a field with a `// …` line comment (the
        // JSONC shape) is recovered through the shared chokepoint instead of
        // dropping the whole decision.
        let raw = "{\"decision\": \"admit\", // high confidence\n\"items\": [\"a\"]}";
        let env: Env = extract_and_parse_json(raw).expect("line comment recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["a".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_block_comment_through_banner_noise() {
        // Banner + interleaved ANSI log line AND a `/* … */` block comment
        // between fields: the extractor strips the noise, then recovery strips
        // the comment.
        let raw = "Recipe: ooda SUCCESS (3.0s)\n\
                   \x1b[2m2026-07-20T00:00:00.000000Z\x1b[0m INFO decide\n\
                   {\"decision\": \"defer\", /* rationale */ \"items\": [\"a\"]}";
        let env: Env = extract_and_parse_json(raw).expect("noise + block comment recovered");
        assert_eq!(env.decision, "defer");
        assert_eq!(env.items, vec!["a".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_recovers_all_four_defects_together() {
        // A single payload with every recoverable defect: a comment, an invalid
        // escape (`\d`), a raw control char (newline), and a trailing comma.
        let raw = "{\"decision\": \"admit\", /* note */ \"items\": [\"re \\d\nline\",],}";
        let env: Env = extract_and_parse_json(raw).expect("all four defects recovered");
        assert_eq!(env.decision, "admit");
        assert_eq!(env.items, vec!["re \\d\nline".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_preserves_url_slashes_in_string() {
        // A `//` inside a string value (a URL) is content, never a comment: the
        // clean payload parses strictly and the recovery view never mangles it.
        let raw = r#"{"decision": "admit", "items": ["see https://x.example//y"]}"#;
        let env: Env = extract_and_parse_json(raw).expect("url slashes preserved");
        assert_eq!(env.items, vec!["see https://x.example//y".to_string()]);
    }

    #[test]
    fn extract_and_parse_json_none_for_non_escape_malformed() {
        // Leniency never widens past the three named defects: an unquoted key is
        // still a miss even though the new invalid-escape view exists.
        assert_eq!(
            extract_and_parse_json::<Env>(r#"{decision: "admit"}"#),
            None
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
