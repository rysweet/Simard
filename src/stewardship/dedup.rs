//! Dedup primitives: ANSI/whitespace normalization, noise-stripped failure
//! signature, and signature lookup against existing GitHub issues.

use sha2::{Digest, Sha256};

use super::gh_client::GhIssue;

/// Strip ANSI escape sequences and collapse internal whitespace runs to a
/// single space. Trims leading/trailing whitespace.
pub fn normalize(msg: &str) -> String {
    // Pass 1: strip ANSI CSI escapes (`ESC [ ... <final-byte>`). We accept the
    // common case where the final byte is a letter (m, K, J, etc.).
    let mut stripped = String::with_capacity(msg.len());
    let mut chars = msg.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip optional '[', then characters until a letter terminator.
            for inner in chars.by_ref() {
                if inner.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        stripped.push(c);
    }
    // Pass 2: collapse whitespace.
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replace volatile tokens (paths, ISO timestamps, run IDs, long hex blobs)
/// with stable placeholders so two runs of the same underlying failure
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
    t.to_string()
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
