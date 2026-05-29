//! Dashboard audit pass 01 — Rust replacement for `audit_pass_01.py` (issue #2156).
//!
//! One-shot headless-Chrome audit:
//! 1. Reads dashkey from `~/.simard/.dashkey` and authenticates via `/api/login`.
//! 2. Visits `http://localhost:8080/` and dynamically enumerates top-level nav targets.
//! 3. For each route: full-page screenshot (`out/NN-<slug>.png`) + visible-text
//!    dump (`out/NN-<slug>.txt`).
//! 4. Writes `out/_index.json` summarising the capture.
//!
//! Build: `cargo build --features dashboard-audit --bin simard-audit-pass01`
//! Run:   `target/debug/simard-audit-pass01`
//!
//! Prerequisites:
//! - Simard daemon running locally on `:8080`
//! - `~/.simard/.dashkey` populated
//! - Chrome/Chromium installed (or set `CHROME_PATH` env var)

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;
use std::time::Instant;

use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::{Browser, LaunchOptions};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

const BASE_URL: &str = "http://localhost:8080";
const NAV_WAIT_MS: u64 = 2500;
const PANEL_SETTLE_MS: u64 = 1500;

static SLUG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-zA-Z0-9._-]+").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NavTarget {
    label: String,
    href: String,
    slug: String,
    kind: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn scripts_out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("dashboard_audit")
        .join("out")
}

fn slugify(text: &str) -> String {
    let lowered = text.trim().to_lowercase();
    let s = SLUG_RE.replace_all(&lowered, "-");
    let s = s.trim_matches('-');
    if s.is_empty() {
        "untitled".into()
    } else {
        s.to_string()
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
}

fn load_dashkey() -> Result<String, String> {
    let path = dirs::home_dir()
        .ok_or("cannot determine home directory")?
        .join(".simard")
        .join(".dashkey");
    if !path.exists() {
        return Err(format!("dashkey not found at {}", path.display()));
    }
    let code = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .trim()
        .to_string();
    if code.is_empty() {
        return Err(format!("dashkey at {} is empty", path.display()));
    }
    Ok(code)
}

fn launch_browser() -> Result<Browser, String> {
    let path = std::env::var("CHROME_PATH").ok().map(PathBuf::from);
    Browser::new(
        LaunchOptions::default_builder()
            .headless(true)
            .window_size(Some((1440, 900)))
            .path(path)
            .build()
            .map_err(|e| format!("bad launch options: {e}"))?,
    )
    .map_err(|e| format!("failed to launch Chrome: {e}"))
}

/// Authenticate by POSTing `/api/login` via an in-browser `fetch()`.
fn login(tab: &Tab, key: &str) -> Result<(), String> {
    tab.navigate_to(BASE_URL)
        .map_err(|e| format!("navigate to {BASE_URL}: {e}"))?;
    tab.wait_until_navigated()
        .map_err(|e| format!("wait for navigation: {e}"))?;
    sleep_ms(1000);

    let escaped_key = key.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        r#"(async () => {{
            const r = await fetch('/api/login', {{
                method: 'POST',
                headers: {{'Content-Type': 'application/x-www-form-urlencoded'}},
                body: 'code=' + encodeURIComponent('{escaped_key}'),
                credentials: 'include'
            }});
            const b = await r.json().catch(() => ({{}}));
            return {{ok: r.ok, status: r.status, body: b}};
        }})()"#
    );

    let result = tab
        .evaluate(&js, true)
        .map_err(|e| format!("login fetch: {e}"))?;
    let val = result.value.unwrap_or(Value::Null);
    if !val.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let status = val.get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
        return Err(format!("/api/login returned status {status}"));
    }
    println!("[auth] logged in");
    Ok(())
}

/// Dynamically enumerate nav targets from the live dashboard DOM.
fn discover_nav(tab: &Tab) -> Result<Vec<NavTarget>, String> {
    tab.navigate_to(&format!("{BASE_URL}/"))
        .map_err(|e| format!("navigate: {e}"))?;
    tab.wait_until_navigated()
        .map_err(|e| format!("wait: {e}"))?;
    sleep_ms(NAV_WAIT_MS);

    let js = r#"(() => {
        const seen = new Map();
        const push = (label, href, kind) => {
            if (!label || !href) return;
            const key = href.split('#')[0] + '#' + (href.split('#')[1] || '');
            if (seen.has(key)) return;
            seen.set(key, { label: label.trim(), href, kind });
        };
        document.querySelectorAll('nav a[href], header nav a[href]').forEach(a => {
            push(a.innerText || a.getAttribute('aria-label') || a.title,
                 a.getAttribute('href'), 'nav-anchor');
        });
        document.querySelectorAll('[role="tab"], [data-tab], .tab, button.tab, .nav-tab').forEach(el => {
            const label = el.innerText || el.getAttribute('aria-label') ||
                          el.title || el.dataset.tab;
            const href = el.getAttribute('href') ||
                         ('#' + (el.dataset.tab || el.id ||
                                 (label||'').toLowerCase().replace(/\s+/g,'-')));
            push(label, href, 'tab');
        });
        document.querySelectorAll('header a[href], .header a[href], .topbar a[href]').forEach(a => {
            push(a.innerText, a.getAttribute('href'), 'header-anchor');
        });
        return Array.from(seen.values());
    })()"#;

    let result = tab
        .evaluate(js, false)
        .map_err(|e| format!("discover_nav JS: {e}"))?;
    let raw: Vec<Value> =
        serde_json::from_value(result.value.unwrap_or(Value::Array(vec![]))).unwrap_or_default();

    let base_host = Url::parse(BASE_URL)
        .ok()
        .and_then(|u| u.host_str().map(String::from));

    let mut nav = Vec::new();
    let mut used_slugs = HashSet::new();

    for (i, item) in raw.iter().enumerate() {
        let href = item
            .get("href")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = item
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if href.starts_with("javascript:") || href.starts_with("mailto:") {
            continue;
        }
        // Skip external links
        if href.starts_with("http")
            && let Ok(parsed) = Url::parse(&href)
        {
            let host = parsed.host_str().map(String::from);
            if host != base_host
                && host.as_deref() != Some("localhost")
                && host.as_deref() != Some("127.0.0.1")
            {
                continue;
            }
        }

        let label_raw = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let label = if label_raw.is_empty() {
            format!("tab-{i}")
        } else {
            label_raw.to_string()
        };

        let slug_base = {
            let s = slugify(&label);
            if s.is_empty() { format!("tab-{i}") } else { s }
        };
        let mut slug = slug_base.clone();
        let mut n = 2;
        while used_slugs.contains(&slug) {
            slug = format!("{slug_base}-{n}");
            n += 1;
        }
        used_slugs.insert(slug.clone());

        nav.push(NavTarget {
            label,
            href,
            slug,
            kind,
        });
    }

    // Always include root
    if !nav
        .iter()
        .any(|n| n.href == "/" || n.href.is_empty() || n.href == "#")
    {
        nav.insert(
            0,
            NavTarget {
                label: "root".into(),
                href: "/".into(),
                slug: "00-root".into(),
                kind: "synthetic".into(),
            },
        );
    }

    Ok(nav)
}

/// Visit a single route, capture screenshot + text, return a JSON result entry.
fn visit(tab: &Tab, target: &NavTarget, index: usize, out_dir: &Path) -> Result<Value, String> {
    let slug = format!("{index:02}-{}", target.slug);
    let href = &target.href;

    if let Some(stripped) = href.strip_prefix('#') {
        // SPA hash route — set hash and click matching tab
        let hash_val = if stripped.is_empty() { "" } else { stripped };
        let set_hash = format!(
            "((h) => {{ window.location.hash = h; }})({})",
            serde_json::to_string(hash_val).unwrap()
        );
        tab.evaluate(&set_hash, false).ok();

        let click_tab = format!(
            r#"((targetHref) => {{
                const candidates = Array.from(document.querySelectorAll(
                    'a[href], [data-tab], [role="tab"]'
                ));
                for (const el of candidates) {{
                    const h = el.getAttribute('href') ||
                              ('#' + (el.dataset.tab || ''));
                    if (h === targetHref) {{ el.click(); return true; }}
                }}
                return false;
            }})({})"#,
            serde_json::to_string(href).unwrap()
        );
        tab.evaluate(&click_tab, false).ok();
    } else if href.starts_with('/') {
        tab.navigate_to(&format!("{BASE_URL}{href}"))
            .map_err(|e| format!("navigate: {e}"))?;
        tab.wait_until_navigated().ok();
    } else {
        tab.navigate_to(href)
            .map_err(|e| format!("navigate: {e}"))?;
        tab.wait_until_navigated().ok();
    }

    sleep_ms(PANEL_SETTLE_MS);

    // Screenshot
    let png_path = out_dir.join(format!("{slug}.png"));
    match tab.capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true) {
        Ok(data) => {
            fs::write(&png_path, data).ok();
        }
        Err(e) => {
            fs::write(&png_path, format!("screenshot failed: {e}\n")).ok();
        }
    }

    // Text dump
    let text = tab
        .evaluate("document.body ? document.body.innerText : ''", false)
        .ok()
        .and_then(|r| r.value)
        .and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
        .unwrap_or_default();
    let txt_path = out_dir.join(format!("{slug}.txt"));
    fs::write(&txt_path, &text).ok();

    let text_chars = text.len();
    println!("[visit] {slug:40} -> {text_chars:6} chars text  href={href}");

    Ok(json!({
        "label": target.label,
        "href": href,
        "slug": slug,
        "kind": target.kind,
        "screenshot": format!("out/{slug}.png"),
        "text_file": format!("out/{slug}.txt"),
        "text_chars": text_chars,
    }))
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn run() -> Result<(), String> {
    let out_dir = scripts_out_dir();
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let started = Instant::now();

    let key = load_dashkey()?;
    let browser = launch_browser()?;
    let tab = browser.new_tab().map_err(|e| format!("new tab: {e}"))?;

    login(&tab, &key)?;

    let nav = discover_nav(&tab)?;
    println!("[nav]   discovered {} target(s):", nav.len());
    for item in &nav {
        println!(
            "        - {:30} {:14} {}",
            format!("{:?}", item.label),
            item.kind,
            item.href
        );
    }

    // Re-navigate to SPA root before iterating hash routes
    tab.navigate_to(&format!("{BASE_URL}/"))
        .map_err(|e| format!("navigate root: {e}"))?;
    tab.wait_until_navigated().ok();
    sleep_ms(NAV_WAIT_MS);

    let mut results = Vec::new();
    for (i, target) in nav.iter().enumerate() {
        match visit(&tab, target, i, &out_dir) {
            Ok(result) => results.push(result),
            Err(e) => {
                println!("[visit] FAILED {target:?}: {e}");
                results.push(json!({
                    "label": target.label,
                    "href": target.href,
                    "slug": target.slug,
                    "error": e.to_string(),
                }));
            }
        }
    }

    let now = chrono::Utc::now();
    let index = json!({
        "base_url": BASE_URL,
        "captured_at": now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "duration_seconds": started.elapsed().as_secs_f64().round() as u64,
        "nav": nav,
        "results": results,
    });

    let index_path = out_dir.join("_index.json");
    fs::write(
        &index_path,
        serde_json::to_string_pretty(&index).unwrap_or_default(),
    )
    .map_err(|e| format!("write index: {e}"))?;

    println!(
        "[done]  wrote {} ({} captures, {:.1}s)",
        index_path.display(),
        results.len(),
        started.elapsed().as_secs_f64()
    );

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("FATAL: {e}");
            ExitCode::from(2)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Overview"), "overview");
        assert_eq!(slugify("a.b-c_d"), "a.b-c_d");
    }

    #[test]
    fn slugify_strips_edges() {
        assert_eq!(slugify("  !!Hello!!  "), "hello");
        assert_eq!(slugify("---"), "untitled");
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("   "), "untitled");
    }

    #[test]
    fn slugify_preserves_dots_and_dashes() {
        assert_eq!(slugify("file.name"), "file.name");
        assert_eq!(slugify("my-slug"), "my-slug");
    }
}
