use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use std::sync::OnceLock;

/// The login code generated at startup, printed to stderr for the operator.
static LOGIN_CODE: OnceLock<String> = OnceLock::new();
/// Session tokens issued after successful login.
static SESSIONS: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn sessions() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    SESSIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Generate and print the one-time login code. Call once at startup.
pub fn init_login_code() -> String {
    let code: String = uuid::Uuid::now_v7().to_string()[..8].to_string();
    LOGIN_CODE.set(code.clone()).ok();
    code
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
        if let Some(token) = part.strip_prefix("simard_session=") {
            if is_valid_session(token) {
                return Ok(next.run(request).await);
            }
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
