//! Failing TDD tests for the durable chat-session store (issue #2577, Step 7).
//!
//! These tests encode the storage contract for the new
//! `operator_commands_dashboard::chat_store` module described in
//! `docs/reference/dashboard-chat.md`:
//!
//!   * Sessions are keyed strictly by `session_id` under
//!     `<state_root>/chat_sessions/` (`index.json` + `<id>.json`).
//!   * `session_id` is validated against `^[A-Za-z0-9_-]{1,64}$` **before any
//!     path join** (SR-B1 path-traversal guard).
//!   * The full turn history is persisted **uncapped** on disk — independent of
//!     the in-memory `MeetingBackend` `MAX_HISTORY = 500` working-set cap.
//!   * The session list is returned newest-first (`updated_at` descending).
//!   * A session is created lazily on the first turn; its title is derived from
//!     the first user message (truncated to ~60 chars, timestamp fallback).
//!
//! Every function/type referenced here is intentionally not-yet-implemented, so
//! this module fails to compile until `chat_store.rs` lands. That compile
//! failure IS the red state for TDD Step 7.
//!
//! The `_at(state_root)` cores take an EXPLICIT path (the goals.rs convention),
//! so these tests use a `tempfile::TempDir` directly and never mutate the
//! ambient `SIMARD_STATE_ROOT`. They are therefore parallel-safe (no
//! `#[serial]` needed).

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use crate::meeting_backend::{ConversationMessage, Role};
    use crate::operator_commands_dashboard::chat_store::{
        ChatSession, ChatSessionIndex, ChatSessionMeta, append_turn_at, list_sessions_at,
        load_session_at, new_session_id, validate_session_id,
    };

    /// Build a `ConversationMessage` with an explicit RFC3339 timestamp so the
    /// tests can assert deterministic ordering / `updated_at` behavior.
    fn msg(role: Role, content: &str, ts: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            content: content.to_string(),
            timestamp: ts.to_string(),
        }
    }

    fn root(tmp: &TempDir) -> &Path {
        tmp.path()
    }

    // -----------------------------------------------------------------------
    // Session ID validation (SR-B1 — path-traversal guard)
    // -----------------------------------------------------------------------

    #[test]
    fn validate_session_id_accepts_valid_ids() {
        // UUIDv7 (hyphenated), plain alnum, underscores/dashes, boundary lengths.
        let max_len = "a".repeat(64); // 64 chars (upper bound)
        for id in [
            "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88",
            "abc123",
            "A_b-C_9",
            "x", // 1 char (lower bound)
            max_len.as_str(),
        ] {
            assert!(
                validate_session_id(id),
                "id {id:?} should be accepted by ^[A-Za-z0-9_-]{{1,64}}$"
            );
        }
    }

    #[test]
    fn validate_session_id_rejects_traversal_and_bad_charset() {
        let over_len = "a".repeat(65); // 65 chars (over bound)
        for id in [
            "", // empty
            over_len.as_str(),
            "../etc/passwd", // traversal
            "..",            // dot-dot
            "a/b",           // path separator
            "a\\b",          // windows separator
            "a.b",           // dot not allowed
            "a b",           // space
            "a\0b",          // NUL
            "..%2f",         // encoded traversal literal
            "sess?id",       // query char
            "%2e%2e",        // encoded dots
        ] {
            assert!(
                !validate_session_id(id),
                "id {id:?} MUST be rejected by the traversal guard"
            );
        }
    }

    #[test]
    fn new_session_id_is_valid_and_unique() {
        let a = new_session_id();
        let b = new_session_id();
        assert!(
            validate_session_id(&a),
            "generated id {a:?} must satisfy validate_session_id"
        );
        assert_ne!(a, b, "two generated ids must differ");
    }

    // -----------------------------------------------------------------------
    // Empty / missing store
    // -----------------------------------------------------------------------

    #[test]
    fn list_sessions_empty_when_store_absent() {
        let tmp = TempDir::new().unwrap();
        // No chat_sessions/ dir exists yet — must be Ok([]), never an error.
        let sessions = list_sessions_at(root(&tmp)).expect("empty store must not error");
        assert!(sessions.is_empty(), "fresh store must list zero sessions");
    }

    #[test]
    fn load_missing_session_returns_none() {
        let tmp = TempDir::new().unwrap();
        let out = load_session_at(root(&tmp), "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88")
            .expect("loading a missing (valid-id) session must not error");
        assert!(
            out.is_none(),
            "missing session must load as None, not error"
        );
    }

    // -----------------------------------------------------------------------
    // Append / persist / round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn append_creates_session_and_persists_history_in_order() {
        let tmp = TempDir::new().unwrap();
        let id = "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88";

        append_turn_at(
            root(&tmp),
            id,
            &msg(
                Role::User,
                "How do I unblock a stuck OODA goal?",
                "2026-07-04T15:20:11Z",
            ),
        )
        .unwrap();
        append_turn_at(
            root(&tmp),
            id,
            &msg(
                Role::Assistant,
                "Start by inspecting the goal board.",
                "2026-07-04T15:20:19Z",
            ),
        )
        .unwrap();

        // The per-session file is keyed strictly by id under chat_sessions/.
        let session_file = tmp.path().join("chat_sessions").join(format!("{id}.json"));
        assert!(
            session_file.exists(),
            "expected per-session file at {session_file:?}"
        );

        let loaded = load_session_at(root(&tmp), id)
            .unwrap()
            .expect("session exists");
        assert_eq!(loaded.meta.id, id);
        assert_eq!(loaded.history.len(), 2, "both turns persisted");
        assert_eq!(loaded.history[0].role, Role::User);
        assert_eq!(
            loaded.history[0].content,
            "How do I unblock a stuck OODA goal?"
        );
        assert_eq!(loaded.history[1].role, Role::Assistant);
        assert_eq!(loaded.history[1].timestamp, "2026-07-04T15:20:19Z");
    }

    #[test]
    fn append_derives_title_from_first_user_message() {
        let tmp = TempDir::new().unwrap();
        let id = new_session_id();
        append_turn_at(
            root(&tmp),
            &id,
            &msg(
                Role::User,
                "Summarize last cycle's actions",
                "2026-07-04T14:02:55Z",
            ),
        )
        .unwrap();

        let loaded = load_session_at(root(&tmp), &id).unwrap().unwrap();
        assert_eq!(
            loaded.meta.title, "Summarize last cycle's actions",
            "short first user message becomes the verbatim title"
        );
    }

    #[test]
    fn append_title_truncated_to_about_60_chars() {
        let tmp = TempDir::new().unwrap();
        let id = new_session_id();
        let long = "a".repeat(200);
        append_turn_at(
            root(&tmp),
            &id,
            &msg(Role::User, &long, "2026-07-04T14:02:55Z"),
        )
        .unwrap();

        let title = load_session_at(root(&tmp), &id)
            .unwrap()
            .unwrap()
            .meta
            .title;
        let char_count = title.chars().count();
        assert!(
            char_count <= 61,
            "title should be truncated to ~60 chars (+ ellipsis), got {char_count} chars"
        );
        assert!(
            title.ends_with('…'),
            "a truncated title should end with an ellipsis, got {title:?}"
        );
    }

    #[test]
    fn append_empty_first_message_title_falls_back_to_timestamp() {
        let tmp = TempDir::new().unwrap();
        let id = new_session_id();
        let created = "2026-07-04T14:02:55Z";
        append_turn_at(root(&tmp), &id, &msg(Role::User, "", created)).unwrap();

        let meta = load_session_at(root(&tmp), &id).unwrap().unwrap().meta;
        assert!(
            !meta.title.is_empty(),
            "empty first message must not produce an empty title"
        );
        assert_eq!(
            meta.title, meta.created_at,
            "empty first message title falls back to the creation timestamp"
        );
    }

    #[test]
    fn updated_at_advances_with_each_turn_created_at_is_stable() {
        let tmp = TempDir::new().unwrap();
        let id = new_session_id();

        append_turn_at(
            root(&tmp),
            &id,
            &msg(Role::User, "first", "2026-07-04T15:20:11Z"),
        )
        .unwrap();
        let after_first = load_session_at(root(&tmp), &id).unwrap().unwrap().meta;

        append_turn_at(
            root(&tmp),
            &id,
            &msg(Role::Assistant, "reply", "2026-07-04T15:20:19Z"),
        )
        .unwrap();
        let after_second = load_session_at(root(&tmp), &id).unwrap().unwrap().meta;

        assert_eq!(
            after_first.created_at, after_second.created_at,
            "created_at is set once and never changes"
        );
        assert!(
            after_second.updated_at >= after_first.updated_at,
            "updated_at must advance (or hold) across turns: {} then {}",
            after_first.updated_at,
            after_second.updated_at
        );
        assert_eq!(
            after_second.updated_at, "2026-07-04T15:20:19Z",
            "updated_at tracks the most recently appended turn"
        );
    }

    #[test]
    fn roundtrip_preserves_all_three_roles() {
        let tmp = TempDir::new().unwrap();
        let id = new_session_id();
        let turns = [
            msg(Role::User, "u", "2026-07-04T15:20:11Z"),
            msg(Role::Assistant, "a", "2026-07-04T15:20:12Z"),
            msg(Role::System, "s", "2026-07-04T15:20:13Z"),
        ];
        for t in &turns {
            append_turn_at(root(&tmp), &id, t).unwrap();
        }
        let loaded = load_session_at(root(&tmp), &id).unwrap().unwrap();
        assert_eq!(
            loaded.history,
            turns.to_vec(),
            "history round-trips exactly"
        );
    }

    // -----------------------------------------------------------------------
    // Listing / ordering
    // -----------------------------------------------------------------------

    #[test]
    fn list_returns_sessions_newest_first() {
        let tmp = TempDir::new().unwrap();

        // Older session.
        let older = new_session_id();
        append_turn_at(
            root(&tmp),
            &older,
            &msg(Role::User, "older", "2026-07-04T10:00:00Z"),
        )
        .unwrap();
        // Newer session.
        let newer = new_session_id();
        append_turn_at(
            root(&tmp),
            &newer,
            &msg(Role::User, "newer", "2026-07-04T20:00:00Z"),
        )
        .unwrap();

        let sessions = list_sessions_at(root(&tmp)).unwrap();
        assert_eq!(sessions.len(), 2, "both sessions listed");
        assert_eq!(
            sessions[0].id, newer,
            "list must be sorted by updated_at descending (newest first)"
        );
        assert_eq!(sessions[1].id, older);
    }

    #[test]
    fn index_metadata_matches_session_file() {
        let tmp = TempDir::new().unwrap();
        let id = new_session_id();
        append_turn_at(
            root(&tmp),
            &id,
            &msg(Role::User, "hello world", "2026-07-04T15:20:11Z"),
        )
        .unwrap();

        // The index entry and the per-session file must agree on metadata,
        // and index.json must exist (written after <id>.json).
        assert!(
            tmp.path().join("chat_sessions").join("index.json").exists(),
            "index.json must be written on append"
        );
        let listed = list_sessions_at(root(&tmp)).unwrap();
        let session = load_session_at(root(&tmp), &id).unwrap().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0], session.meta,
            "index meta must match session meta"
        );
    }

    // -----------------------------------------------------------------------
    // Uncapped history on disk (FR-1)
    // -----------------------------------------------------------------------

    #[test]
    fn history_is_uncapped_on_disk_beyond_max_history() {
        // The in-memory MeetingBackend working set caps at MAX_HISTORY = 500,
        // but the durable store must record EVERY turn so a restart restores
        // the complete conversation. Append just over the cap.
        let tmp = TempDir::new().unwrap();
        let id = new_session_id();
        let total = 505;
        for i in 0..total {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            let ts = format!("2026-07-04T15:{:02}:{:02}Z", i / 60, i % 60);
            append_turn_at(root(&tmp), &id, &msg(role, &format!("turn {i}"), &ts)).unwrap();
        }
        let loaded = load_session_at(root(&tmp), &id).unwrap().unwrap();
        assert_eq!(
            loaded.history.len(),
            total,
            "disk history must be uncapped (>MAX_HISTORY): expected {total}"
        );
        assert_eq!(loaded.history[0].content, "turn 0", "oldest turn preserved");
        assert_eq!(
            loaded.history[total - 1].content,
            format!("turn {}", total - 1),
            "newest turn preserved"
        );
    }

    // -----------------------------------------------------------------------
    // Path-traversal defense at the store layer (SR-B1)
    // -----------------------------------------------------------------------

    #[test]
    fn load_rejects_invalid_id_before_path_join() {
        let tmp = TempDir::new().unwrap();
        let err = load_session_at(root(&tmp), "../../etc/passwd");
        assert!(
            err.is_err(),
            "an invalid session_id must be rejected (Err), not resolved to a path"
        );
    }

    #[test]
    fn append_rejects_invalid_id_and_writes_nothing_outside_store() {
        let tmp = TempDir::new().unwrap();
        let bad = "../escape";
        let res = append_turn_at(
            root(&tmp),
            bad,
            &msg(Role::User, "malicious", "2026-07-04T15:20:11Z"),
        );
        assert!(res.is_err(), "append with an invalid id must return Err");

        // Nothing must have been written outside the chat_sessions/ subtree.
        let escaped = tmp.path().join("escape.json");
        assert!(
            !escaped.exists(),
            "traversal id must not create a file outside chat_sessions/"
        );
    }

    // -----------------------------------------------------------------------
    // On-disk schema shape (serde) — schema_version present, forward-compatible
    // -----------------------------------------------------------------------

    #[test]
    fn chat_session_json_schema_shape() {
        // The persisted <id>.json envelope carries a schema_version, a `meta`
        // object, and a `history` array of ConversationMessage.
        let session = ChatSession {
            schema_version: 1,
            meta: ChatSessionMeta {
                id: "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88".to_string(),
                title: "How do I unblock a stuck OODA goal?".to_string(),
                created_at: "2026-07-04T15:20:11Z".to_string(),
                updated_at: "2026-07-04T15:41:02Z".to_string(),
            },
            history: vec![msg(Role::User, "hi", "2026-07-04T15:20:11Z")],
        };
        let v: serde_json::Value = serde_json::to_value(&session).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["meta"]["id"], "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88");
        assert_eq!(v["history"][0]["role"], "user");

        // Round-trips back to the strongly-typed struct.
        let back: ChatSession = serde_json::from_value(v).unwrap();
        assert_eq!(back.history.len(), 1);
        assert_eq!(back.meta.title, session.meta.title);
    }

    #[test]
    fn chat_session_index_json_schema_shape() {
        let index = ChatSessionIndex {
            schema_version: 1,
            sessions: vec![ChatSessionMeta {
                id: "abc123".to_string(),
                title: "t".to_string(),
                created_at: "2026-07-04T15:20:11Z".to_string(),
                updated_at: "2026-07-04T15:20:11Z".to_string(),
            }],
        };
        let v = serde_json::to_value(&index).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["sessions"][0]["id"], "abc123");
        let back: ChatSessionIndex = serde_json::from_value(v).unwrap();
        assert_eq!(back.sessions.len(), 1);
    }
}
