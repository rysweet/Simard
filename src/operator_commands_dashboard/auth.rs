use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};

/// Simple bearer-token auth middleware.
/// Set `SIMARD_DASHBOARD_TOKEN` to enable; if unset, all requests are allowed.
pub async fn require_auth(request: Request, next: Next) -> Result<Response, StatusCode> {
    let expected = std::env::var("SIMARD_DASHBOARD_TOKEN").unwrap_or_default();
    if expected.is_empty() {
        return Ok(next.run(request).await);
    }

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    let token = auth_header.strip_prefix("Bearer ").unwrap_or_default();
    if token == expected {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}
