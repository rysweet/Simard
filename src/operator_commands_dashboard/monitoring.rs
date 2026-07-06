use axum::Json;
use serde_json::{Value, json};

pub(crate) async fn metrics() -> Json<Value> {
    let recent = crate::self_metrics::recent_metrics(100).unwrap_or_default();
    let report = crate::self_metrics::daily_report().unwrap_or_default();

    let entries: Vec<Value> = recent
        .iter()
        .map(|e| {
            json!({
                "timestamp": e.timestamp.to_rfc3339(),
                "metric_name": e.metric_name,
                "value": e.value,
                "context": e.context,
            })
        })
        .collect();

    Json(json!({
        "recent": entries,
        "daily_report": report,
    }))
}

pub(crate) async fn costs() -> Json<Value> {
    let daily = crate::cost_tracking::daily_summary()
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .unwrap_or_else(|e| json!({"error": format!("daily: {e}")}));
    let weekly = crate::cost_tracking::weekly_summary()
        .map(|s| serde_json::to_value(s).unwrap_or_default())
        .unwrap_or_else(|e| json!({"error": format!("weekly: {e}")}));
    Json(json!({
        "daily": daily,
        "weekly": weekly,
    }))
}

/// Budget config file path.
fn budget_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
    std::path::PathBuf::from(home)
        .join(".simard")
        .join("budget.json")
}

pub(crate) async fn get_budget() -> Json<Value> {
    let path = budget_config_path();
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    match serde_json::from_str::<Value>(&content) {
        Ok(v) => Json(v),
        // Single-source the daily ceiling through the canonical resolver
        // (bug #6): the budget is always guarded, so the fallback must apply
        // the same `DEFAULT_DAILY_BUDGET_USD` the Overseer's `BudgetGate`
        // enforces instead of duplicating a `500.0` literal here.
        Err(_) => Json(json!({
            "daily_budget_usd": crate::overseer::config::daily_budget_usd(),
            "weekly_budget_usd": std::env::var("SIMARD_WEEKLY_BUDGET_USD")
                .ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(2500.0),
        })),
    }
}

pub(crate) async fn set_budget(Json(body): Json<Value>) -> Json<Value> {
    let path = budget_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(
        &path,
        serde_json::to_string_pretty(&body).unwrap_or_default(),
    ) {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"error": format!("{e}")})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // ---- budget_config_path -----------------------------------------------

    #[test]
    fn budget_config_path_ends_with_budget_json() {
        let path = budget_config_path();
        assert!(
            path.ends_with("budget.json"),
            "expected path ending in budget.json, got: {path:?}"
        );
    }

    #[test]
    fn budget_config_path_contains_simard_dir() {
        let path = budget_config_path();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains(".simard"),
            "expected .simard in path: {path_str}"
        );
    }

    #[test]
    fn budget_config_path_is_absolute() {
        let path = budget_config_path();
        assert!(path.is_absolute(), "expected absolute path: {path:?}");
    }

    // ---- get_budget defaults ----------------------------------------------

    #[tokio::test]
    async fn get_budget_returns_defaults_when_no_config_file() {
        let result = get_budget().await;
        let val = result.0;
        assert!(
            val.get("daily_budget_usd").is_some(),
            "missing daily_budget_usd in: {val}"
        );
        assert!(
            val.get("weekly_budget_usd").is_some(),
            "missing weekly_budget_usd in: {val}"
        );
    }

    #[tokio::test]
    async fn get_budget_default_values() {
        // When no env vars or config file override, defaults are 500/2500
        let result = get_budget().await;
        let val = result.0;
        let daily = val["daily_budget_usd"].as_f64().unwrap_or(0.0);
        let weekly = val["weekly_budget_usd"].as_f64().unwrap_or(0.0);
        assert!(daily > 0.0, "daily budget should be > 0");
        assert!(weekly > 0.0, "weekly budget should be > 0");
        assert!(weekly > daily, "weekly should exceed daily");
    }

    // ---- get_budget daily budget resolution (issue #6) --------------------
    //
    // The dashboard monitoring JSON must report the *actual* daily guard. The
    // budget is always guarded, so the env/default fallback must single-source
    // through `crate::overseer::config::daily_budget_usd()` (which applies the
    // canonical `DEFAULT_DAILY_BUDGET_USD` for unset/empty/unparseable/non-positive
    // values) rather than parsing the raw env with a duplicated `500.0` literal.

    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: these tests are `#[serial(budget_env)]`.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: these tests are `#[serial(budget_env)]`.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: serialized via `#[serial(budget_env)]`.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    /// Point `HOME` at a fresh temp dir (so `budget.json` is absent, forcing the
    /// env/default fallback branch), set the daily-budget env to `value`, and
    /// return the `daily_budget_usd` `get_budget` reports.
    async fn daily_budget_with_env(value: Option<&str>) -> f64 {
        let home = tempfile::tempdir().expect("tempdir");
        let _home = EnvGuard::set("HOME", home.path().to_str().unwrap());
        let _weekly = EnvGuard::unset("SIMARD_WEEKLY_BUDGET_USD");
        let _daily = match value {
            Some(v) => EnvGuard::set("SIMARD_DAILY_BUDGET_USD", v),
            None => EnvGuard::unset("SIMARD_DAILY_BUDGET_USD"),
        };
        let val = get_budget().await.0;
        val["daily_budget_usd"]
            .as_f64()
            .expect("daily_budget_usd f64")
    }

    #[tokio::test]
    #[serial(budget_env)]
    async fn get_budget_daily_defaults_to_guard_when_env_unset() {
        assert_eq!(
            daily_budget_with_env(None).await,
            crate::overseer::config::DEFAULT_DAILY_BUDGET_USD,
        );
    }

    #[tokio::test]
    #[serial(budget_env)]
    async fn get_budget_daily_reflects_explicit_env() {
        assert_eq!(daily_budget_with_env(Some("250")).await, 250.0);
    }

    #[tokio::test]
    #[serial(budget_env)]
    async fn get_budget_daily_nonpositive_env_falls_back_to_guard() {
        // `0` is not a real ceiling; the canonical resolver applies the default
        // guard rather than reporting `0` (single-sourcing, bug #6).
        assert_eq!(
            daily_budget_with_env(Some("0")).await,
            crate::overseer::config::DEFAULT_DAILY_BUDGET_USD,
        );
    }
}
