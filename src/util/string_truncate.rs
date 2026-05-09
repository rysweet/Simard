//! Char-boundary-safe `String` truncation.
//!
//! `String::truncate(new_len)` panics when `new_len` is not a UTF-8 char
//! boundary — the standard library asserts `self.is_char_boundary(new_len)`
//! before truncating. Several Simard sites (transcript previews, evidence
//! buffers, log lines) call `s.truncate(N)` with `N` as a byte budget
//! rather than a code-point count, so any input where a multi-byte sequence
//! (em-dash, CJK character, emoji) crosses byte `N` panics the runtime
//! worker.
//!
//! See `docs/reference/string-truncation-helpers.md` for the design.

/// Truncate `s` so its byte length does not exceed `max_bytes`, backing up
/// to the previous UTF-8 char boundary if `max_bytes` falls inside a
/// multi-byte sequence.
///
/// Properties:
///
/// - If `s.len() <= max_bytes` the string is left unchanged.
/// - Otherwise the result satisfies `s.len() <= max_bytes` and remains
///   valid UTF-8.
/// - Never panics. Byte 0 is always a valid char boundary, so the
///   boundary search always terminates.
///
/// Stable Rust only — does not depend on the nightly
/// `floor_char_boundary` API.
pub fn truncate_to_char_boundary(s: &mut String, max_bytes: usize) {
    let _ = (s, max_bytes);
    unimplemented!(
        "truncate_to_char_boundary: TDD stub — implementation lands in \
         step 8 of issue #1590 follow-up"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_op_when_within_budget() {
        let mut s = "short ascii".to_string();
        truncate_to_char_boundary(&mut s, 1024);
        assert_eq!(s, "short ascii");
    }

    #[test]
    fn no_op_when_exactly_at_budget() {
        let mut s = "exactly".to_string();
        let n = s.len();
        truncate_to_char_boundary(&mut s, n);
        assert_eq!(s, "exactly");
    }

    #[test]
    fn ascii_longer_than_budget_truncates_at_byte_boundary() {
        let mut s = "a".repeat(20);
        truncate_to_char_boundary(&mut s, 5);
        assert_eq!(s.len(), 5);
        assert_eq!(s, "aaaaa");
    }

    #[test]
    fn em_dash_at_boundary_does_not_panic() {
        // Em-dash '—' is 3 bytes (0xE2 0x80 0x94).
        // Build a string where an em-dash straddles byte 512.
        let mut s = String::new();
        s.push_str(&"a".repeat(511));
        s.push('—'); // 3 bytes start at index 511, end at index 514
        s.push_str(&"b".repeat(20));

        // Sanity: byte 512 is mid-em-dash.
        assert!(!s.is_char_boundary(512), "test fixture invariant");

        truncate_to_char_boundary(&mut s, 512);

        assert!(
            s.len() <= 512,
            "result must respect byte budget; got {}",
            s.len()
        );
        // The em-dash must be fully present or fully removed — never
        // partial. After truncating at byte 511 (the last valid boundary
        // ≤ 512), the em-dash is dropped.
        assert!(
            std::str::from_utf8(s.as_bytes()).is_ok(),
            "result must remain valid UTF-8"
        );
        assert_eq!(s.len(), 511);
        assert!(!s.contains('—'));
    }

    #[test]
    fn cjk_at_boundary_does_not_panic() {
        // CJK characters are 3 bytes each in UTF-8.
        let mut s = String::new();
        s.push_str(&"a".repeat(99));
        s.push('日'); // 3 bytes start at 99, end at 102
        s.push('本');
        s.push('語');

        truncate_to_char_boundary(&mut s, 100);

        assert!(s.len() <= 100);
        assert!(std::str::from_utf8(s.as_bytes()).is_ok());
        assert_eq!(s.len(), 99); // backed up before the CJK char
    }

    #[test]
    fn emoji_at_boundary_does_not_panic() {
        // 4-byte emoji 🎉 (U+1F389)
        let mut s = String::new();
        s.push_str(&"a".repeat(50));
        s.push('🎉'); // 4 bytes start at 50, end at 54
        s.push_str("trailing");

        truncate_to_char_boundary(&mut s, 51);

        assert!(s.len() <= 51);
        assert!(std::str::from_utf8(s.as_bytes()).is_ok());
        assert_eq!(s.len(), 50);
        assert!(!s.contains('🎉'));
    }

    #[test]
    fn emoji_at_boundary_keeps_emoji_when_budget_includes_it() {
        let mut s = String::new();
        s.push_str(&"a".repeat(50));
        s.push('🎉');
        s.push_str(&"b".repeat(50));

        truncate_to_char_boundary(&mut s, 54);
        // 50 ASCII + 4-byte emoji = 54 bytes — exactly at boundary.
        assert_eq!(s.len(), 54);
        assert!(s.contains('🎉'));
    }

    #[test]
    fn empty_string_no_op() {
        let mut s = String::new();
        truncate_to_char_boundary(&mut s, 100);
        assert_eq!(s, "");
        truncate_to_char_boundary(&mut s, 0);
        assert_eq!(s, "");
    }

    #[test]
    fn zero_max_bytes_truncates_to_empty() {
        let mut s = "anything".to_string();
        truncate_to_char_boundary(&mut s, 0);
        assert_eq!(s, "");
    }

    #[test]
    fn zero_max_bytes_with_multibyte_lead_does_not_panic() {
        let mut s = "—🎉日".to_string();
        truncate_to_char_boundary(&mut s, 0);
        assert_eq!(s, "");
    }

    #[test]
    fn budget_one_truncates_to_empty_when_first_char_is_multibyte() {
        // Em-dash starts at byte 0; max_bytes=1 falls mid-em-dash.
        let mut s = "—trailing".to_string();
        truncate_to_char_boundary(&mut s, 1);
        // Backs up to byte 0 (the only char boundary ≤ 1 in this prefix).
        assert_eq!(s, "");
    }
}
