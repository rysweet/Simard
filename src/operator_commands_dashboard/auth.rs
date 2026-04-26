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
    if let Some(path) = dashkey_path()
        && let Ok(contents) = std::fs::read_to_string(&path)
    {
        let existing = contents.trim().to_string();
        if !existing.is_empty() {
            LOGIN_CODE.set(existing.clone()).ok();
            return (existing, true);
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

    // If no login code was configured, deny all — never silently allow traffic
    if LOGIN_CODE.get().is_none() {
        tracing::warn!("dashboard auth: no login code configured — denying request to {path}");
        return Ok(Response::builder()
            .status(401)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"error":"auth not initialized","login_url":"/login"}"#,
            ))
            .unwrap());
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

    // Not authenticated — JSON error for API, redirect for pages
    if path.starts_with("/api/") || path.starts_with("/ws/") {
        Ok(Response::builder()
            .status(401)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"error":"not authenticated","login_url":"/login"}"#,
            ))
            .unwrap())
    } else {
        Ok(Response::builder()
            .status(303)
            .header("location", "/login")
            .body(axum::body::Body::empty())
            .unwrap())
    }
}

/// Check whether the login code has been initialized.
pub fn is_auth_initialized() -> bool {
    LOGIN_CODE.get().is_some()
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


// === Login HTTP handlers ===

use axum::{Json, response};
use serde_json::{Value, json};

pub(crate) async fn login(Json(body): Json<Value>) -> response::Response {
    let code = body.get("code").and_then(|v| v.as_str()).unwrap_or("");
    match try_login(code) {
        Some(session_token) => response::Response::builder()
            .status(200)
            .header(
                "set-cookie",
                format!(
                    "simard_session={session_token}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400"
                ),
            )
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"ok": true}).to_string(),
            ))
            .unwrap(),
        None => response::Response::builder()
            .status(401)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"ok": false, "error": "invalid code"}).to_string(),
            ))
            .unwrap(),
    }
}

pub(crate) async fn login_page() -> response::Html<String> {
    response::Html(LOGIN_HTML.to_string())
}

pub(crate) const LOGIN_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Simard — Login</title>
  <style>
    :root { --bg: #0d1117; --fg: #c9d1d9; --accent: #58a6ff; --card: #161b22; --border: #30363d; }
    * { margin: 0; padding: 0; box-sizing: border-box; }
    body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: var(--bg); color: var(--fg); display: flex; align-items: center; justify-content: center; min-height: 100vh; }
    .login-card { background: var(--card); border: 1px solid var(--border); border-radius: 12px; padding: 2rem; width: 340px; text-align: center; }
    h1 { color: var(--accent); font-size: 1.3rem; margin-bottom: 0.5rem; }
    p { color: #8b949e; font-size: 0.85rem; margin-bottom: 1.5rem; }
    input { width: 100%; padding: 0.6rem; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--fg); font-size: 1.1rem; text-align: center; letter-spacing: 0.15em; }
    input:focus { outline: none; border-color: var(--accent); }
    button { width: 100%; margin-top: 1rem; padding: 0.6rem; border: none; border-radius: 6px; background: var(--accent); color: #0d1117; font-weight: 600; font-size: 0.95rem; cursor: pointer; }
    button:hover { opacity: 0.9; }
    .error { color: #f85149; margin-top: 0.75rem; font-size: 0.85rem; display: none; }
  </style>
</head>
<body>
  <div class="login-card">
    <h1>🌲 Simard</h1>
    <p>Enter the login code from the server terminal</p>
    <form id="login-form">
      <input id="code" type="text" placeholder="code" autocomplete="off" autofocus maxlength="8">
      <button type="submit">Log in</button>
    </form>
    <div class="error" id="error">Invalid code. Check terminal output.</div>
  </div>



  <script>
    document.getElementById('login-form').addEventListener('submit', async (e) => {
      e.preventDefault();
      const code = document.getElementById('code').value;
      const r = await fetch('/api/login', { method: 'POST', headers: {'content-type':'application/json'}, body: JSON.stringify({code}) });
      if (r.ok) { window.location.href = '/'; }
      else { document.getElementById('error').style.display = 'block'; }
    });
  </script>
</body>
</html>
"#;
