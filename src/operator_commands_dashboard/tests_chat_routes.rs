//! Failing TDD tests for the Dashboard Chat REST endpoints (issue #2577, Step 7).
//!
//! Covers the two read endpoints and their thin `_at(state_root)` cores in the
//! new `chat_store` module (see `docs/reference/dashboard-chat.md#rest-api`):
//!
//!   * `GET /api/chat/sessions`        -> `chat_sessions_at(state_root)`
//!   * `GET /api/chat/sessions/{id}`   -> `chat_session_by_id_at(state_root, id)`
//!
//! Both return `(StatusCode, Json<Value>)` (the goals.rs handler convention).
//! Status-code contract:
//!   200 — found / empty list      400 — id fails `^[A-Za-z0-9_-]{1,64}$`
//!   404 — valid id, no session    (401 — enforced by the require_auth layer)
//!
//! A source-scan test guards SR-A1: the chat routes MUST be registered **inside**
//! the `require_auth` middleware scope, so an unauthenticated request is rejected
//! before reaching a handler.
//!
//! References not-yet-implemented symbols on purpose — the compile failure is the
//! TDD red state. The `_at` cores take explicit paths, so tests use a `TempDir`
//! and are parallel-safe (no `#[serial]`).

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use axum::Json;
    use axum::http::StatusCode;
    use serde_json::Value;
    use tempfile::TempDir;

    use crate::meeting_backend::{ConversationMessage, Role};
    use crate::operator_commands_dashboard::chat_store::{
        append_turn_at, chat_session_by_id_at, chat_sessions_at, new_session_id,
    };
    use crate::operator_commands_dashboard::routes::build_router;

    fn user_msg(content: &str, ts: &str) -> ConversationMessage {
        ConversationMessage {
            role: Role::User,
            content: content.to_string(),
            timestamp: ts.to_string(),
        }
    }

    fn seed_session(root: &Path, first_user: &str, ts: &str) -> String {
        let id = new_session_id();
        append_turn_at(root, &id, &user_msg(first_user, ts)).unwrap();
        id
    }

    // -----------------------------------------------------------------------
    // GET /api/chat/sessions
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sessions_empty_returns_200_empty_list() {
        let tmp = TempDir::new().unwrap();
        let (status, Json(body)) = chat_sessions_at(tmp.path()).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "empty store must be 200, never 404/500"
        );
        assert_eq!(
            body["sessions"],
            Value::Array(vec![]),
            "empty store returns {{\"sessions\": []}}"
        );
    }

    #[tokio::test]
    async fn sessions_lists_created_sessions_newest_first() {
        let tmp = TempDir::new().unwrap();
        let older = seed_session(tmp.path(), "older topic", "2026-07-04T10:00:00Z");
        let newer = seed_session(tmp.path(), "newer topic", "2026-07-04T20:00:00Z");

        let (status, Json(body)) = chat_sessions_at(tmp.path()).await;
        assert_eq!(status, StatusCode::OK);
        let sessions = body["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 2, "both sessions listed");
        assert_eq!(sessions[0]["id"], newer, "newest first (updated_at desc)");
        assert_eq!(sessions[1]["id"], older);
        // Metadata fields present for the sidebar.
        for key in ["id", "title", "created_at", "updated_at"] {
            assert!(
                sessions[0][key].is_string(),
                "session meta must expose {key}"
            );
        }
        assert_eq!(sessions[0]["title"], "newer topic");
    }

    // -----------------------------------------------------------------------
    // GET /api/chat/sessions/{id}
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn session_by_id_returns_200_with_full_history() {
        let tmp = TempDir::new().unwrap();
        let id = seed_session(
            tmp.path(),
            "How do I unblock a stuck OODA goal?",
            "2026-07-04T15:20:11Z",
        );
        append_turn_at(
            tmp.path(),
            &id,
            &ConversationMessage {
                role: Role::Assistant,
                content: "Inspect the goal board.".to_string(),
                timestamp: "2026-07-04T15:20:19Z".to_string(),
            },
        )
        .unwrap();

        let (status, Json(body)) = chat_session_by_id_at(tmp.path(), &id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["id"], id);
        assert_eq!(body["title"], "How do I unblock a stuck OODA goal?");
        let history = body["history"].as_array().expect("history array");
        assert_eq!(history.len(), 2, "complete turn history returned");
        assert_eq!(history[0]["role"], "user");
        assert_eq!(history[1]["role"], "assistant");
        assert_eq!(history[1]["content"], "Inspect the goal board.");
    }

    #[tokio::test]
    async fn session_by_id_unknown_returns_404() {
        let tmp = TempDir::new().unwrap();
        // Valid-shaped id that does not exist.
        let (status, _body) =
            chat_session_by_id_at(tmp.path(), "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88").await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a valid id with no session must be 404 (non-differential)"
        );
    }

    #[tokio::test]
    async fn session_by_id_invalid_id_returns_400() {
        let tmp = TempDir::new().unwrap();
        let over_len = "a".repeat(65);
        for bad in ["../etc/passwd", "..", "a/b", "a b", over_len.as_str()] {
            let (status, _body) = chat_session_by_id_at(tmp.path(), bad).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "invalid id {bad:?} must be rejected with 400 before store access"
            );
        }
    }

    #[tokio::test]
    async fn session_by_id_does_not_leak_filesystem_paths_on_invalid_id() {
        let tmp = TempDir::new().unwrap();
        let (_status, Json(body)) = chat_session_by_id_at(tmp.path(), "../secret").await;
        let text = body.to_string();
        assert!(
            !text.contains(tmp.path().to_string_lossy().as_ref()),
            "error responses must not leak the on-disk state-root path (SR-E1)"
        );
    }

    // -----------------------------------------------------------------------
    // SR-A1: routes registered INSIDE the require_auth layer
    // -----------------------------------------------------------------------

    fn routes_source() -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/operator_commands_dashboard/routes.rs");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
    }

    #[test]
    fn chat_rest_and_ws_routes_are_registered() {
        let src = routes_source();
        assert!(
            src.contains("/api/chat/sessions"),
            "GET /api/chat/sessions must be registered in routes.rs"
        );
        assert!(
            src.contains("/api/chat/sessions/{id}"),
            "GET /api/chat/sessions/{{id}} must be registered in routes.rs"
        );
        assert!(
            src.contains("/ws/chat"),
            "the chat WebSocket route must be registered in routes.rs"
        );
    }

    #[test]
    fn chat_routes_are_inside_require_auth_scope() {
        let src = routes_source();
        let auth_layer = src
            .find(".layer(middleware::from_fn(require_auth))")
            .expect("require_auth layer must be applied in build_router");

        // Every chat route literal must appear BEFORE the auth layer is applied,
        // so it is covered by require_auth (axum layers apply to routes added
        // before the .layer() call).
        for route in ["/api/chat/sessions", "/ws/chat"] {
            let pos = src
                .find(route)
                .unwrap_or_else(|| panic!("route {route} must be present"));
            assert!(
                pos < auth_layer,
                "route {route} must be registered BEFORE .layer(require_auth) (SR-A1)"
            );
        }
    }

    #[test]
    fn build_router_constructs_with_chat_routes() {
        // Smoke: the router builds (all chat handlers wired) without panicking.
        let _router = build_router();
    }
}
