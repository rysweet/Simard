//! Turn-by-turn transcript formatting for the meeting REPL.
//!
//! Provides `format_turn_prefix` which generates a `[role HH:MM:SS]` prefix
//! for each conversational turn, giving clear visual separation between
//! facilitator and user messages. Issue #1986.

use chrono::{DateTime, Local, Utc};

use crate::meeting_backend::Role;

/// Format a turn prefix as `[role HH:MM:SS]` for display in the REPL.
///
/// - `role`: the participant role (User, Assistant, System).
/// - `timestamp_rfc3339`: an RFC3339 timestamp string (as stored in
///   `ConversationMessage::timestamp`).
///
/// Falls back to the current local time if the timestamp cannot be parsed.
///
/// # Examples
///
/// ```ignore
/// let prefix = format_turn_prefix(&Role::Assistant, "2026-05-23T22:14:03Z");
/// // => "[facilitator 22:14:03]" (in UTC; actual display uses local time)
/// ```
pub fn format_turn_prefix(role: &Role, timestamp_rfc3339: &str) -> String {
    let role_label = role_display_name(role);
    let time_str = format_time_from_rfc3339(timestamp_rfc3339);
    format!("[{role_label} {time_str}]")
}

/// Human-readable display name for a [`Role`].
fn role_display_name(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "facilitator",
        Role::System => "system",
    }
}

/// Extract `HH:MM:SS` from an RFC3339 timestamp, converting to local time.
///
/// Returns `"??:??:??"` if parsing fails.
fn format_time_from_rfc3339(ts: &str) -> String {
    match DateTime::parse_from_rfc3339(ts) {
        Ok(dt) => {
            let local: DateTime<Local> = dt.with_timezone(&Local);
            local.format("%H:%M:%S").to_string()
        }
        Err(_) => {
            // Fallback: use current local time
            Local::now().format("%H:%M:%S").to_string()
        }
    }
}

/// Format a turn prefix using the current time (for turns where the
/// timestamp is captured at display time rather than read from history).
pub fn format_turn_prefix_now(role: &Role) -> String {
    let role_label = role_display_name(role);
    let time_str = Utc::now().with_timezone(&Local).format("%H:%M:%S");
    format!("[{role_label} {time_str}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_turn_prefix_assistant() {
        let prefix = format_turn_prefix(&Role::Assistant, "2026-05-23T22:14:03+00:00");
        assert!(
            prefix.starts_with("[facilitator "),
            "prefix should start with [facilitator: got {prefix:?}"
        );
        assert!(
            prefix.ends_with(']'),
            "prefix should end with ]: got {prefix:?}"
        );
        // The time portion should be HH:MM:SS format (8 chars)
        let inner = &prefix[1..prefix.len() - 1]; // strip [ and ]
        let parts: Vec<&str> = inner.splitn(2, ' ').collect();
        assert_eq!(parts[0], "facilitator");
        assert_eq!(
            parts[1].len(),
            8,
            "time should be HH:MM:SS: got {:?}",
            parts[1]
        );
        assert!(
            parts[1].chars().filter(|c| *c == ':').count() == 2,
            "time should have two colons: got {:?}",
            parts[1]
        );
    }

    #[test]
    fn format_turn_prefix_user() {
        let prefix = format_turn_prefix(&Role::User, "2026-05-23T10:30:45Z");
        assert!(
            prefix.starts_with("[user "),
            "prefix should start with [user: got {prefix:?}"
        );
    }

    #[test]
    fn format_turn_prefix_system() {
        let prefix = format_turn_prefix(&Role::System, "2026-05-23T00:00:00Z");
        assert!(
            prefix.starts_with("[system "),
            "prefix should start with [system: got {prefix:?}"
        );
    }

    #[test]
    fn format_turn_prefix_invalid_timestamp_does_not_panic() {
        // Should not panic; falls back to current time
        let prefix = format_turn_prefix(&Role::Assistant, "not-a-timestamp");
        assert!(prefix.starts_with("[facilitator "));
        assert!(prefix.ends_with(']'));
    }

    #[test]
    fn format_turn_prefix_now_produces_valid_prefix() {
        let prefix = format_turn_prefix_now(&Role::User);
        assert!(prefix.starts_with("[user "));
        assert!(prefix.ends_with(']'));
        let inner = &prefix[1..prefix.len() - 1];
        let parts: Vec<&str> = inner.splitn(2, ' ').collect();
        assert_eq!(parts[1].len(), 8);
    }

    #[test]
    fn role_display_names_are_correct() {
        assert_eq!(role_display_name(&Role::User), "user");
        assert_eq!(role_display_name(&Role::Assistant), "facilitator");
        assert_eq!(role_display_name(&Role::System), "system");
    }

    #[test]
    fn format_turn_prefix_with_timezone_offset() {
        // Timestamp with a non-UTC offset — should still produce valid HH:MM:SS
        let prefix = format_turn_prefix(&Role::User, "2026-05-23T15:30:00+05:30");
        assert!(prefix.starts_with("[user "));
        let inner = &prefix[1..prefix.len() - 1];
        let time_part = inner.split(' ').nth(1).unwrap();
        assert_eq!(time_part.len(), 8);
    }
}
