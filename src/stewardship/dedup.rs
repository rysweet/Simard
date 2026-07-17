//! Dedup primitives: ANSI/whitespace normalization, noise-stripped failure
//! signature, and signature lookup against existing GitHub issues.

use sha2::{Digest, Sha256};

use super::gh_client::GhIssue;

/// Strip ANSI escape sequences and collapse internal whitespace runs to a
/// single space. Trims leading/trailing whitespace.
pub fn normalize(msg: &str) -> String {
    // Pass 1: strip ANSI escapes via the single shared, hardened stripper
    // (issue #2484) — one ANSI/CSI/OSC implementation for the whole crate
    // instead of this formerly copy-pasted CSI-only loop.
    let stripped = crate::recipe_output::strip_ansi(msg);
    // Pass 2: collapse whitespace.
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replace volatile tokens (paths, ISO timestamps, run IDs, long hex blobs,
/// UUIDs) with stable placeholders so two runs of the same underlying failure
/// produce identical signatures.
fn redact_token(t: &str) -> String {
    if t.starts_with('/') {
        return "<PATH>".to_string();
    }
    if t.starts_with("run-") || t.starts_with("Run-") || t.starts_with("RUN-") {
        return "<RUNID>".to_string();
    }
    if is_iso_timestamp(t) {
        return "<TS>".to_string();
    }
    if t.len() >= 7 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        return "<HEX>".to_string();
    }
    // Canonical UUIDs are the system's ubiquitous session / node / run
    // identifiers (UUIDv7 via `uuid::Uuid::now_v7()`, plus prefixed forms like
    // `ooda-<uuid>`), and they are pure volatility: two runs of the SAME failure
    // differ only in the embedded id. They slip past the all-hex arm above
    // because their interior hyphens make the token non-hex, so without this
    // they defeat both issue dedup and recurrence recall. Fold every embedded
    // UUID substring to `<UUID>` — substring-level so a prefixed token like
    // `ooda-<uuid>` collapses to `ooda-<UUID>` rather than being left volatile.
    redact_uuids(t)
}

/// Replace every canonical-UUID substring (`8-4-4-4-12` hex groups, case-
/// insensitive) inside `t` with the stable `<UUID>` placeholder, leaving all
/// other characters untouched. Returns `t` unchanged (as an owned `String`)
/// when it contains no UUID.
///
/// A manual scanner (matching the regex-free style of [`is_iso_timestamp`])
/// keeps this off any regex dependency and linear in `t`. The canonical shape is
/// strict — exactly `8-4-4-4-12` hex digits with hyphens only at the four fixed
/// offsets — so a hyphenated hex run of any other shape (e.g. a git range or an
/// ISO date) is never mistaken for a UUID.
fn redact_uuids(t: &str) -> String {
    // UUIDs are ASCII, so a byte scan is correct and index-safe.
    let bytes = t.as_bytes();
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < bytes.len() {
        if is_uuid_at(bytes, i) {
            out.push_str("<UUID>");
            i += UUID_LEN;
        } else {
            // Copy this whole UTF-8 char (not just one byte) so non-ASCII text
            // around a UUID is preserved verbatim.
            let ch_len = utf8_char_len(bytes[i]);
            out.push_str(&t[i..i + ch_len]);
            i += ch_len;
        }
    }
    out
}

/// Length in bytes of a canonical hyphenated UUID (`8-4-4-4-12`).
const UUID_LEN: usize = 36;

/// Byte offsets (within a 36-byte canonical UUID) that must hold a hyphen.
const UUID_HYPHEN_OFFSETS: [usize; 4] = [8, 13, 18, 23];

/// `true` when a canonical `8-4-4-4-12` hyphenated UUID begins at `bytes[start]`.
/// Every non-hyphen offset must be an ASCII hex digit and the four fixed offsets
/// must be hyphens.
fn is_uuid_at(bytes: &[u8], start: usize) -> bool {
    if start + UUID_LEN > bytes.len() {
        return false;
    }
    for off in 0..UUID_LEN {
        let b = bytes[start + off];
        if UUID_HYPHEN_OFFSETS.contains(&off) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Length in bytes of the UTF-8 char whose first byte is `b`.
fn utf8_char_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn is_iso_timestamp(s: &str) -> bool {
    // Accept e.g. 2026-04-22T10:00:00Z (with optional fractional seconds /
    // tz offset). Heuristic: starts with YYYY-MM-DD, contains 'T'.
    if s.len() < 19 {
        return false;
    }
    let bytes = s.as_bytes();
    bytes[0..4].iter().all(|b| b.is_ascii_digit())
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(|b| b.is_ascii_digit())
        && bytes[10] == b'T'
}

fn normalize_for_signature(msg: &str) -> String {
    normalize(msg)
        .split_whitespace()
        .map(redact_token)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Compute a stable 16-hex-character signature for a failure: the first 8
/// bytes of `sha256(failure_kind || "\n" || normalized_message)`.
pub fn failure_signature(failure_kind: &str, error_text: &str) -> String {
    let normalized = normalize_for_signature(error_text);
    let mut hasher = Sha256::new();
    hasher.update(failure_kind.as_bytes());
    hasher.update(b"\n");
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for b in &digest[..8] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Find the first issue whose body embeds `stewardship-signature: <sig>`.
pub fn find_existing<'a>(issues: &'a [GhIssue], signature: &str) -> Option<&'a GhIssue> {
    let needle = format!("stewardship-signature: {signature}");
    issues.iter().find(|i| i.body.contains(&needle))
}

#[cfg(test)]
mod uuid_redaction_tests {
    //! Pin the UUID-redaction contract: the system's ubiquitous hyphenated
    //! UUIDv7 identifiers (session / node / run ids) are volatile noise, so two
    //! runs of the SAME underlying failure that differ ONLY in an embedded UUID
    //! must fold to ONE `failure_signature` — the property that lets a recurring
    //! failure dedup to a single issue and be recalled as recurring rather than
    //! filed afresh every time.

    use super::{failure_signature, redact_token, redact_uuids};

    const UUID_A: &str = "0191b2c3-4d5e-7f80-9abc-def012345678";
    const UUID_B: &str = "1a2b3c4d-5e6f-7a8b-9c0d-1e2f3a4b5c6d";

    #[test]
    fn bare_uuid_token_is_redacted() {
        assert_eq!(redact_token(UUID_A), "<UUID>");
        // Uppercase hex is still a UUID.
        assert_eq!(redact_token(&UUID_A.to_ascii_uppercase()), "<UUID>");
    }

    #[test]
    fn prefixed_and_wrapped_uuid_is_redacted_in_place() {
        // The `ooda-<uuid>` session-id shape this codebase emits.
        assert_eq!(redact_token(&format!("ooda-{UUID_A}")), "ooda-<UUID>");
        // A UUID embedded mid-token keeps the surrounding, semantic text.
        assert_eq!(
            redact_token(&format!("session={UUID_A},")),
            "session=<UUID>,"
        );
        // Multiple UUIDs in one token are each folded.
        assert_eq!(
            redact_uuids(&format!("{UUID_A}->{UUID_B}")),
            "<UUID>-><UUID>"
        );
    }

    #[test]
    fn same_failure_differing_only_by_uuid_shares_a_signature() {
        let kind = "OODAStepFailure";
        let a = format!("goal session ooda-{UUID_A} failed to advance in orient");
        let b = format!("goal session ooda-{UUID_B} failed to advance in orient");
        assert_ne!(a, b, "the two error texts genuinely differ (by the UUID)");
        assert_eq!(
            failure_signature(kind, &a),
            failure_signature(kind, &b),
            "identical failure differing only by session UUID must dedup to one signature"
        );
    }

    #[test]
    fn genuinely_different_failures_still_differ() {
        // Redaction must not over-collapse: two DIFFERENT failures (different
        // surrounding words) keep distinct signatures even after UUID folding.
        let kind = "OODAStepFailure";
        let a = format!("goal session ooda-{UUID_A} failed to advance in orient");
        let b = format!("goal session ooda-{UUID_A} failed to advance in decide");
        assert_ne!(failure_signature(kind, &a), failure_signature(kind, &b));
    }

    #[test]
    fn non_uuid_hyphenated_hex_is_left_untouched() {
        // A git-range / short-hash shape with the wrong group lengths is NOT a
        // UUID and must pass through verbatim (the strict `8-4-4-4-12` gate).
        assert_eq!(redact_uuids("abc123-def456"), "abc123-def456");
        // An ISO date (`YYYY-MM-DD`) is not a UUID either.
        assert_eq!(redact_uuids("2026-07-17"), "2026-07-17");
        // A truncated UUID (missing the final group) is not redacted.
        let truncated = "0191b2c3-4d5e-7f80-9abc";
        assert_eq!(redact_uuids(truncated), truncated);
    }

    #[test]
    fn non_ascii_text_around_a_uuid_is_preserved() {
        // The byte scanner must copy whole UTF-8 chars, not split them.
        assert_eq!(redact_uuids(&format!("café-{UUID_A}-π")), "café-<UUID>-π");
    }

    #[test]
    fn uuid_free_token_is_unchanged() {
        assert_eq!(redact_uuids("plain-text_token"), "plain-text_token");
    }
}
