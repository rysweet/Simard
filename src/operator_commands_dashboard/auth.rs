use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The login code generated at startup, printed to stderr for the operator.
static LOGIN_CODE: OnceLock<String> = OnceLock::new();
/// Session tokens issued after successful login.
static SESSIONS: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn sessions() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    SESSIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Path to the persisted dashboard login code: `~/.simard/.dashkey`.
fn dashkey_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".simard").join(".dashkey"))
}

/// Initialize the login code, persisting it to `~/.simard/.dashkey`.
///
/// If the file exists and contains a non-empty code, reuse it.
/// Otherwise generate a fresh code, write it to disk, and return it.
/// Returns `(code, loaded_from_file)`.
pub fn init_login_code() -> (String, bool) {
    // Try to load an existing dashkey
    if let Some(path) = dashkey_path() {
        if let Ok(contents) = std::fs::read_to_string(&path) {
            let existing = contents.trim().to_string();
            if !existing.is_empty() {
                LOGIN_CODE.set(existing.clone()).ok();
                return (existing, true);
            }
        }
    }

    // Generate a fresh code
    let code: String = uuid::Uuid::now_v7().to_string()[..8].to_string();
    LOGIN_CODE.set(code.clone()).ok();

    // Persist to ~/.simard/.dashkey
    if let Some(path) = dashkey_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, &code);
    }

    (code, false)
}

/// Validate the login code, return a session token if correct.
pub fn try_login(code: &str) -> Option<String> {
    let expected = LOGIN_CODE.get()?;
    if code.trim() == expected.as_str() {
        let token = uuid::Uuid::now_v7().to_string();
        sessions().lock().ok()?.insert(token.clone());
        Some(token)
    } else {
        None
    }
}

fn is_valid_session(token: &str) -> bool {
    sessions()
        .lock()
        .map(|s| s.contains(token))
        .unwrap_or(false)
}

/// Auth middleware. Checks (in order):
/// 1. `session` cookie
/// 2. `Authorization: Bearer <token>` header (for API clients)
/// 3. `?token=` query param (legacy)
///
/// The `/login` path is always allowed through.
pub async fn require_auth(request: Request, next: Next) -> Result<Response, StatusCode> {
    let path = request.uri().path().to_string();

    // Login page and login POST are always accessible
    if path == "/login" || path == "/api/login" {
        return Ok(next.run(request).await);
    }

    // If no login code was configured (SIMARD_DASHBOARD_TOKEN unset + no init), allow all
    if LOGIN_CODE.get().is_none() {
        let expected = std::env::var("SIMARD_DASHBOARD_TOKEN").unwrap_or_default();
        if expected.is_empty() {
            return Ok(next.run(request).await);
        }
    }

    // Check session cookie
    let cookie_header = request
        .headers()
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    for part in cookie_header.split(';') {
        let part = part.trim();
        if let Some(token) = part.strip_prefix("simard_session=")
            && is_valid_session(token)
        {
            return Ok(next.run(request).await);
        }
    }

    // Check Bearer header (for curl/API usage)
    let bearer = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    if let Some(token) = bearer {
        let expected_env = std::env::var("SIMARD_DASHBOARD_TOKEN").unwrap_or_default();
        if !expected_env.is_empty() && token == expected_env {
            return Ok(next.run(request).await);
        }
        if is_valid_session(token) {
            return Ok(next.run(request).await);
        }
    }

    // Check ?token= query param (legacy)
    let query = request.uri().query().unwrap_or_default();
    if let Some(token) = query.split('&').find_map(|p| p.strip_prefix("token=")) {
        let expected_env = std::env::var("SIMARD_DASHBOARD_TOKEN").unwrap_or_default();
        if !expected_env.is_empty() && token == expected_env {
            return Ok(next.run(request).await);
        }
    }

    // Not authenticated — redirect to login page
    if path.starts_with("/api/") {
        Err(StatusCode::UNAUTHORIZED)
    } else {
        Ok(Response::builder()
            .status(303)
            .header("location", "/login")
            .body(axum::body::Body::empty())
            .unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── init_login_code ─────────────────────────────────────────────

    #[test]
    fn init_login_code_returns_8_char_string() {
        let (code, _) = init_login_code();
        assert_eq!(
            code.len(),
            8,
            "Login code should be 8 characters, got: {code}"
        );
    }

    #[test]
    fn init_login_code_is_nonempty() {
        let (code, _) = init_login_code();
        assert!(!code.is_empty());
    }

    // ── try_login ───────────────────────────────────────────────────

    #[test]
    fn try_login_wrong_code_returns_none() {
        // Ensure some login code is set
        init_login_code();
        let result = try_login("definitely-wrong-code");
        assert!(result.is_none());
    }

    #[test]
    fn try_login_correct_code_returns_token() {
        let (code, _) = init_login_code();
        // LOGIN_CODE is a OnceLock, so it may already be set from a prior test;
        // we test with whatever code was stored
        if let Some(stored) = LOGIN_CODE.get() {
            let result = try_login(stored);
            assert!(
                result.is_some(),
                "Correct code should yield a session token"
            );
            let token = result.unwrap();
            assert!(!token.is_empty());
        } else {
            // If init_login_code set it, use code
            let result = try_login(&code);
            assert!(result.is_some());
        }
    }

    #[test]
    fn try_login_trims_whitespace() {
        if let Some(stored) = LOGIN_CODE.get() {
            let padded = format!("  {}  ", stored);
            let result = try_login(&padded);
            assert!(result.is_some(), "try_login should trim whitespace");
        }
    }

    #[test]
    fn try_login_empty_string_returns_none() {
        init_login_code();
        let result = try_login("");
        assert!(result.is_none());
    }

    // ── is_valid_session ────────────────────────────────────────────

    #[test]
    fn is_valid_session_unknown_token() {
        assert!(!is_valid_session("nonexistent-token"));
    }

    #[test]
    fn is_valid_session_after_login() {
        init_login_code();
        if let Some(stored) = LOGIN_CODE.get()
            && let Some(token) = try_login(stored)
        {
            assert!(is_valid_session(&token));
        }
    }

    #[test]
    fn is_valid_session_empty_token() {
        assert!(!is_valid_session(""));
    }

    // ── sessions() helper ───────────────────────────────────────────

    #[test]
    fn sessions_mutex_is_accessible() {
        let guard = sessions().lock().unwrap();
        // Just verify we can acquire the lock
        drop(guard);
    }
}
