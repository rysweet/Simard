//! Shared control-character log sanitizer.
//!
//! Neutralizes an untrusted string for embedding in single-line,
//! operator-facing output. Two independent callers rely on it:
//!
//! - the self-relaunch canary, which embeds subprocess stderr / a failing test
//!   name into a `GateResult.detail` (#4470); and
//! - the cleanup sweep, which renders an untrusted on-disk quarantine basename
//!   into stderr / a `CleanupReport` (#4469, LOW-1).
//!
//! Living here (a neutral cross-cutting util) rather than inside either caller
//! keeps `cmd_cleanup` from depending on `self_relaunch` for a generic string
//! sanitizer (#4469 philosophy review S6).

/// Strip control characters, collapse to a single line, and bound the result to
/// `max_bytes` on a UTF-8 char boundary.
///
/// Every run of control characters (CR/LF, tabs, ANSI escapes, NUL) collapses to
/// a single space, so the output is one readable line with no log-line-forgery
/// or terminal-control-injection vectors. The length bound never splits a
/// multi-byte character.
pub fn sanitize_to_single_line(raw: &str, max_bytes: usize) -> String {
    let mut collapsed = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_control() {
            if !collapsed.ends_with(' ') {
                collapsed.push(' ');
            }
        } else {
            collapsed.push(c);
        }
    }
    let trimmed = collapsed.trim();
    if trimmed.len() <= max_bytes {
        return trimmed.to_string();
    }
    // Bound on a UTF-8 char boundary so we never split a multi-byte char.
    let mut end = max_bytes;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: usize = 512;

    #[test]
    fn strips_control_chars_and_newlines() {
        let raw = "line one\nline two\r\n\ttabbed\x1b[31mred\x00nul";
        let clean = sanitize_to_single_line(raw, MAX);
        assert!(!clean.contains('\n'), "newlines stripped: {clean:?}");
        assert!(
            !clean.contains('\r'),
            "carriage returns stripped: {clean:?}"
        );
        assert!(
            !clean.contains('\x1b'),
            "escape sequences stripped: {clean:?}"
        );
        assert!(!clean.contains('\0'), "NUL stripped: {clean:?}");
        assert!(
            !clean.contains('\t') || clean.contains(' '),
            "no raw tabs: {clean:?}"
        );
    }

    #[test]
    fn bounds_length() {
        let raw = "a".repeat(2000);
        let clean = sanitize_to_single_line(&raw, MAX);
        assert!(
            clean.len() <= MAX,
            "must bound to {MAX} bytes, got {}",
            clean.len()
        );
    }

    #[test]
    fn utf8_boundary_safe() {
        // Bounding must never split a multi-byte char (no panic, valid UTF-8).
        let raw = "héllo wörld café ".repeat(100);
        let clean = sanitize_to_single_line(&raw, 10);
        assert!(clean.len() <= 10);
        // Round-trips as valid UTF-8 (String is always valid; the point is no panic).
        let _ = clean.chars().count();
    }
}
