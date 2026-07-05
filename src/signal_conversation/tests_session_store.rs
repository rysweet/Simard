//! Tests for the Signal per-operator durable session store (issue #2577).
//!
//! Written test-first: they pin the exact persistence + per-operator-index
//! contract the store delivers. They use only explicit `tempdir` state roots —
//! no env, no network, no clock — so they run in parallel and never touch
//! `~/.simard`.
//!
//! Contract pinned here:
//!   * `append_turn_at` accumulates the FULL, uncapped history in one
//!     `<session_id>.json` file, lazily created on the first turn.
//!   * The traversal guard rejects a hostile id (and a raw E.164) as a filename
//!     **before** any write.
//!   * `operators.json` maps operator E.164 -> active session id, durably, and
//!     per-operator (isolated), with overwrite-on-rotation (what `/new` uses).
//!   * `list_sessions_at` enumerates stored sessions; empty store -> `vec![]`.

use super::session_store::{
    active_session_for, append_turn_at, list_sessions_at, load_session_at, set_active_session,
};
use crate::meeting_backend::{ConversationMessage, Role};

const OPERATOR: &str = "+12062591306";
const OPERATOR_B: &str = "+12065559999";

/// A valid session id (satisfies `^[A-Za-z0-9_-]{1,64}$`). Literal so the store
/// tests don't depend on the (also-red) `new_session_id` generator.
const SID_ALPHA: &str = "sess-alpha-0001";
const SID_BRAVO: &str = "sess-bravo-0002";

fn msg(role: Role, content: &str, ts: &str) -> ConversationMessage {
    ConversationMessage {
        role,
        content: content.to_string(),
        timestamp: ts.to_string(),
    }
}

fn joined(history: &[ConversationMessage]) -> String {
    history
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

// ── per-operator active-session index (operators.json) ───────────────────────

#[test]
fn active_session_for_absent_operator_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    let got = active_session_for(tmp.path(), OPERATOR).unwrap();
    assert_eq!(
        got, None,
        "an operator with no session yet resolves to None"
    );
}

#[test]
fn set_then_get_active_session_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    set_active_session(tmp.path(), OPERATOR, SID_ALPHA).unwrap();
    let got = active_session_for(tmp.path(), OPERATOR).unwrap();
    assert_eq!(got.as_deref(), Some(SID_ALPHA));
}

#[test]
fn active_session_index_is_durable_on_disk() {
    // Each call re-reads operators.json from disk, so a fresh lookup after the
    // write proves the mapping survives (the restart-resume foundation).
    let tmp = tempfile::tempdir().unwrap();
    set_active_session(tmp.path(), OPERATOR, SID_ALPHA).unwrap();
    // A second, independent lookup (no in-memory carryover) still sees it.
    let got = active_session_for(tmp.path(), OPERATOR).unwrap();
    assert_eq!(got.as_deref(), Some(SID_ALPHA));
}

#[test]
fn active_session_is_isolated_per_operator() {
    let tmp = tempfile::tempdir().unwrap();
    set_active_session(tmp.path(), OPERATOR, SID_ALPHA).unwrap();
    set_active_session(tmp.path(), OPERATOR_B, SID_BRAVO).unwrap();

    assert_eq!(
        active_session_for(tmp.path(), OPERATOR).unwrap().as_deref(),
        Some(SID_ALPHA)
    );
    assert_eq!(
        active_session_for(tmp.path(), OPERATOR_B)
            .unwrap()
            .as_deref(),
        Some(SID_BRAVO)
    );
    assert_eq!(
        active_session_for(tmp.path(), "+15550001111").unwrap(),
        None,
        "an unrelated operator has no active session"
    );
}

#[test]
fn set_active_session_overwrites_on_rotation() {
    // `/new` rotates the operator's pointer to a fresh id; the index must
    // overwrite, not append.
    let tmp = tempfile::tempdir().unwrap();
    set_active_session(tmp.path(), OPERATOR, SID_ALPHA).unwrap();
    set_active_session(tmp.path(), OPERATOR, SID_BRAVO).unwrap();
    assert_eq!(
        active_session_for(tmp.path(), OPERATOR).unwrap().as_deref(),
        Some(SID_BRAVO),
        "the newest set wins"
    );
}

#[test]
fn operator_index_accepts_e164_as_a_lookup_key() {
    // The E.164 is a VALUE/key inside operators.json (allowed), never a path
    // component — so a `+`-prefixed operator address is a fine index key even
    // though it is rejected as a session-id filename.
    let tmp = tempfile::tempdir().unwrap();
    set_active_session(tmp.path(), "+12062591306", SID_ALPHA).unwrap();
    assert_eq!(
        active_session_for(tmp.path(), "+12062591306")
            .unwrap()
            .as_deref(),
        Some(SID_ALPHA)
    );
}

#[test]
fn set_active_session_rejects_invalid_session_id_value() {
    // The stored session id is validated before it is recorded, so the index
    // can never point at a traversal id or a raw E.164 filename.
    let tmp = tempfile::tempdir().unwrap();
    assert!(set_active_session(tmp.path(), OPERATOR, "../evil").is_err());
    assert!(set_active_session(tmp.path(), OPERATOR, "+12065551234").is_err());
}

// ── per-session durable history (<session_id>.json) ──────────────────────────

#[test]
fn load_absent_session_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    let got = load_session_at(tmp.path(), SID_ALPHA).unwrap();
    assert!(got.is_none(), "no file yet -> None, not an error");
}

#[test]
fn append_then_load_accumulates_history_in_one_session() {
    let tmp = tempfile::tempdir().unwrap();
    append_turn_at(
        tmp.path(),
        SID_ALPHA,
        &msg(
            Role::User,
            "the code word is BANANA",
            "2026-01-01T00:00:00Z",
        ),
    )
    .unwrap();
    append_turn_at(
        tmp.path(),
        SID_ALPHA,
        &msg(Role::Assistant, "noted", "2026-01-01T00:00:01Z"),
    )
    .unwrap();
    append_turn_at(
        tmp.path(),
        SID_ALPHA,
        &msg(Role::User, "what is the code word", "2026-01-01T00:00:02Z"),
    )
    .unwrap();

    let sess = load_session_at(tmp.path(), SID_ALPHA)
        .unwrap()
        .expect("session persisted after the first append");
    assert_eq!(sess.session_id, SID_ALPHA);
    assert_eq!(sess.history.len(), 3, "all appended turns accumulate");
    assert_eq!(sess.history[0].content, "the code word is BANANA");
    assert_eq!(sess.history[2].content, "what is the code word");
    assert!(joined(&sess.history).contains("BANANA"));
}

#[test]
fn history_is_persisted_uncapped_beyond_max_history() {
    // The durable file holds the FULL history, independent of the in-memory
    // MeetingBackend MAX_HISTORY (500) working-set cap — so a restart can replay
    // the complete conversation.
    let tmp = tempfile::tempdir().unwrap();
    let total = 505;
    for i in 0..total {
        append_turn_at(
            tmp.path(),
            SID_ALPHA,
            &msg(
                Role::User,
                &format!("turn {i}"),
                &format!("2026-01-01T00:00:{:02}Z", i % 60),
            ),
        )
        .unwrap();
    }
    let sess = load_session_at(tmp.path(), SID_ALPHA).unwrap().unwrap();
    assert_eq!(
        sess.history.len(),
        total,
        "durable history must NOT be capped at MAX_HISTORY"
    );
}

#[test]
fn sessions_with_distinct_ids_are_independent() {
    let tmp = tempfile::tempdir().unwrap();
    append_turn_at(
        tmp.path(),
        SID_ALPHA,
        &msg(Role::User, "APPLE", "2026-01-01T00:00:00Z"),
    )
    .unwrap();
    append_turn_at(
        tmp.path(),
        SID_BRAVO,
        &msg(Role::User, "BANANA", "2026-01-01T00:00:00Z"),
    )
    .unwrap();

    let a = load_session_at(tmp.path(), SID_ALPHA).unwrap().unwrap();
    let b = load_session_at(tmp.path(), SID_BRAVO).unwrap().unwrap();
    assert!(joined(&a.history).contains("APPLE") && !joined(&a.history).contains("BANANA"));
    assert!(joined(&b.history).contains("BANANA") && !joined(&b.history).contains("APPLE"));
}

#[test]
fn append_rejects_traversal_session_id_before_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let hostile = msg(Role::User, "x", "2026-01-01T00:00:00Z");
    assert!(append_turn_at(tmp.path(), "../evil", &hostile).is_err());
    assert!(append_turn_at(tmp.path(), "a/b", &hostile).is_err());
}

#[test]
fn append_rejects_e164_as_a_session_filename() {
    // The core "E.164 is never a filename" invariant at the write boundary.
    let tmp = tempfile::tempdir().unwrap();
    let m = msg(Role::User, "x", "2026-01-01T00:00:00Z");
    assert!(append_turn_at(tmp.path(), "+12062591306", &m).is_err());
}

#[test]
fn load_rejects_traversal_session_id() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(load_session_at(tmp.path(), "../secret").is_err());
    assert!(load_session_at(tmp.path(), "+12062591306").is_err());
}

// ── listing ──────────────────────────────────────────────────────────────────

#[test]
fn list_sessions_empty_store_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions = list_sessions_at(tmp.path()).unwrap();
    assert!(
        sessions.is_empty(),
        "a fresh store lists nothing, not an error"
    );
}

#[test]
fn list_sessions_reports_each_created_session() {
    let tmp = tempfile::tempdir().unwrap();
    append_turn_at(
        tmp.path(),
        SID_ALPHA,
        &msg(Role::User, "a", "2026-01-01T00:00:00Z"),
    )
    .unwrap();
    append_turn_at(
        tmp.path(),
        SID_BRAVO,
        &msg(Role::User, "b", "2026-01-01T00:00:00Z"),
    )
    .unwrap();

    let ids: Vec<String> = list_sessions_at(tmp.path())
        .unwrap()
        .into_iter()
        .map(|m| m.session_id)
        .collect();
    assert_eq!(ids.len(), 2, "both created sessions are listed");
    assert!(ids.iter().any(|s| s == SID_ALPHA));
    assert!(ids.iter().any(|s| s == SID_BRAVO));
}
