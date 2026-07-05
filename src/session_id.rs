//! Shared, channel-agnostic conversation session-id helpers (issue #2577).
//!
//! These two small, pure helpers are the single choke point every conversation
//! `session_id` passes before it is joined onto a filesystem path. They were
//! originally private to `operator_commands_dashboard::chat_store`; the Signal
//! continuous-conversation store needs the *same* traversal guard and id shape,
//! so they are promoted here (a shared `crate::session_id`) rather than
//! duplicated. Both the dashboard chat store and the Signal session store import
//! from this module so the security-critical validation lives in exactly one
//! place.
//!
//! The contract these helpers satisfy:
//!
//!   * `validate_session_id` accepts exactly `^[A-Za-z0-9_-]{1,64}$` and nothing
//!     else — every `.`, `/`, `\`, NUL, `+`, space, or non-ASCII byte is
//!     rejected. In particular a raw E.164 (`+12065551234`) is **rejected**, so
//!     an operator phone number can never become a path component.
//!   * `new_session_id` returns a fresh, unique, time-ordered id (UUIDv7,
//!     hyphenated) that always satisfies `validate_session_id`.

/// Return `true` when `id` matches `^[A-Za-z0-9_-]{1,64}$`.
///
/// The single traversal guard every `session_id` clears before any path join.
/// Rejecting `.`, `/`, `\`, NUL, `+`, and every other character keeps a hostile
/// or accidental id (e.g. a raw E.164) from escaping the session subtree.
pub fn validate_session_id(id: &str) -> bool {
    let len = id.len();
    if len == 0 || len > 64 {
        return false;
    }
    // Every accepted character is ASCII, so byte length == char length and a
    // byte-wise scan is both correct and traversal-safe.
    id.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Generate a fresh, unique, time-ordered session id (UUIDv7, hyphenated).
///
/// The hyphenated UUID form is pure `[0-9a-f-]`, so it always satisfies
/// [`validate_session_id`].
pub fn new_session_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    //! Pin the contract for the shared id helpers: the exact accepted alphabet,
    //! the length bound, E.164 rejection, traversal rejection, and generator
    //! validity/uniqueness.

    use super::*;

    #[test]
    fn accepts_the_full_allowed_alphabet() {
        assert!(validate_session_id("a"));
        assert!(validate_session_id("A"));
        assert!(validate_session_id("0"));
        assert!(validate_session_id("abcXYZ012_-"));
        assert!(validate_session_id(
            "0191b2c3-4d5e-7f80-9abc-def012345678" // a UUIDv7-shaped id
        ));
    }

    #[test]
    fn rejects_empty_and_over_length() {
        assert!(!validate_session_id(""));
        assert!(validate_session_id(&"a".repeat(64)), "64 chars is the max");
        assert!(
            !validate_session_id(&"a".repeat(65)),
            "65 chars must be rejected"
        );
    }

    #[test]
    fn rejects_an_e164_phone_number() {
        // The core "E.164 is never a filename" invariant: a `+`-prefixed phone
        // number must fail the guard so it can never become a path component.
        assert!(!validate_session_id("+12065551234"));
        assert!(!validate_session_id("12065551234 "));
        assert!(!validate_session_id(" 12065551234"));
    }

    #[test]
    fn rejects_path_traversal_and_separators() {
        for hostile in [
            "..",
            "../etc/passwd",
            "a/b",
            "a\\b",
            "a.b",
            "a b",
            "a\0b",
            "sess/../../secret",
            "señor", // non-ASCII
        ] {
            assert!(
                !validate_session_id(hostile),
                "must reject hostile id {hostile:?}"
            );
        }
    }

    #[test]
    fn new_session_id_is_always_valid() {
        let id = new_session_id();
        assert!(
            validate_session_id(&id),
            "generated id {id:?} must satisfy the guard"
        );
    }

    #[test]
    fn new_session_id_is_unique_per_call() {
        let a = new_session_id();
        let b = new_session_id();
        assert_ne!(a, b, "each call must mint a distinct id");
    }
}
