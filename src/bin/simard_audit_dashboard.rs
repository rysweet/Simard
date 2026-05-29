//! Structured dashboard audit + REPORT.md generator — Rust replacement
//! for `audit_dashboard.py` (issue #2156, parent #1990).
//!
//! Like `simard-audit-pass01` but additionally:
//! - Captures per-page HTTP errors (status ≥ 400) and console errors.
//! - Runs a jargon scan across every captured DOM dump.
//! - Emits a consolidated `out/REPORT.md` ready to paste into an epic body.
//!
//! Build: `cargo build --features dashboard-audit --bin simard-audit-dashboard`
//! Run:   `target/debug/simard-audit-dashboard`
//!
//! Prerequisites: same as `simard-audit-pass01` (daemon on :8080, dashkey, Chrome).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::LazyLock;

use headless_chrome::browser::tab::Tab;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use headless_chrome::{Browser, LaunchOptions};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

const BASE_URL: &str = "http://localhost:8080";
const MAX_ROUTES: usize = 50;
const TIMEOUT_MS: u64 = 8000;

static SLUG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9_-]+").unwrap());

const FALLBACK_TABS: &[&str] = &[
    "overview",
    "goals",
    "traces",
    "logs",
    "processes",
    "memory",
    "costs",
    "chat",
    "whiteboard",
    "thinking",
    "terminal",
];

const JARGON_TERMS: &[&str] = &[
    "OODA",
    "OODA loop",
    "cognitive memory",
    "handoff bundle",
    "facilitator",
    "recipe runner",
    "consolidation",
    "episodic",
    "semantic memory",
    "procedural memory",
    "LadybugDB",
    "spawn_engineer",
    "workboard",
    "whiteboard",
];

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Route {
    label: String,
    href: String,
    slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PageResult {
    slug: String,
    href: String,
    url: String,
    label: String,
    title: String,
    h1: Option<String>,
    excerpt: String,
    text_chars: usize,
    http_errors: usize,
    console_errors: usize,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn scripts_out_dir() -> PathBuf {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("dashboard_audit")
        .join("out");
    // Safety: never write under prompt_assets
    let resolved = out.canonicalize().unwrap_or_else(|_| out.clone());
    let s = resolved.to_string_lossy();
    assert!(
        !s.contains("/prompt_assets/"),
        "OUT_DIR under prompt_assets"
    );
    if let Ok(pa) = std::env::var("SIMARD_PROMPT_ASSETS_DIR") {
        let pa = pa.trim();
        if !pa.is_empty() {
            let pa_path = PathBuf::from(pa)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(pa));
            assert!(
                !resolved.starts_with(&pa_path),
                "OUT_DIR under SIMARD_PROMPT_ASSETS_DIR"
            );
        }
    }
    out
}

fn slug(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    let result = SLUG_RE.replace_all(&lowered, "-");
    let result = result.trim_matches('-');
    if result.is_empty() {
        "page".into()
    } else {
        result.to_string()
    }
}

fn norm_href(h: &str) -> String {
    if h.starts_with('#') {
        let rest = h.trim_start_matches('#').trim_start_matches('/');
        let rest = rest.split('?').next().unwrap_or("").trim_end_matches('/');
        format!("#{rest}")
    } else if h.starts_with('/') {
        let rest = h.trim_start_matches('/');
        let rest = rest.split('?').next().unwrap_or("");
        let rest = rest.split('#').next().unwrap_or("").trim_end_matches('/');
        format!("/{rest}")
    } else {
        h.to_string()
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
    let key = fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        .trim()
        .to_string();
    if key.is_empty() || key.len() > 256 {
        return Err(format!(
            "dashkey at {} fails 1<=len<=256 sanity check (len={})",
            path.display(),
            key.len()
        ));
    }
    Ok(key)
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

/// Authenticate via in-browser fetch.
fn authenticate(tab: &Tab, key: &str) -> Result<(), String> {
    tab.navigate_to(BASE_URL)
        .map_err(|e| format!("navigate: {e}"))?;
    tab.wait_until_navigated()
        .map_err(|e| format!("wait: {e}"))?;
    sleep_ms(1000);

    let escaped = key.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        r#"(async () => {{
            const r = await fetch('/api/login', {{
                method: 'POST',
                headers: {{'Content-Type': 'application/x-www-form-urlencoded'}},
                body: 'code=' + encodeURIComponent('{escaped}'),
                credentials: 'include'
            }});
            return {{ok: r.ok, status: r.status}};
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
    Ok(())
}

/// Install JS monkey-patches for capturing console/HTTP errors.
fn inject_error_capture(tab: &Tab) {
    let js = r#"(() => {
        if (window.__audit_patched) return;
        window.__audit = { http: [], console: [] };
        const _consErr = console.error;
        console.error = function() {
            window.__audit.console.push({
                text: Array.from(arguments).map(String).join(' ')
            });
            _consErr.apply(console, arguments);
        };
        const _fetch = window.fetch;
        window.fetch = function() {
            return _fetch.apply(this, arguments).then(function(resp) {
                if (resp.status >= 400) {
                    window.__audit.http.push({url: resp.url, status: resp.status});
                }
                return resp;
            });
        };
        window.__audit_patched = true;
    })()"#;
    tab.evaluate(js, false).ok();
}

fn reset_error_capture(tab: &Tab) {
    tab.evaluate("window.__audit = { http: [], console: [] }", false)
        .ok();
    // Re-inject if page was reloaded (full navigation resets globals)
    inject_error_capture(tab);
}

fn collect_errors(tab: &Tab) -> (Vec<Value>, Vec<Value>) {
    let js = r#"(() => {
        const a = window.__audit || { http: [], console: [] };
        return { http: a.http || [], console: a.console || [] };
    })()"#;
    let result = tab.evaluate(js, false).ok();
    let val = result.and_then(|r| r.value).unwrap_or(Value::Null);
    let http: Vec<Value> = val
        .get("http")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let console: Vec<Value> = val
        .get("console")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    (http, console)
}

// ---------------------------------------------------------------------------
// Route discovery
// ---------------------------------------------------------------------------

fn discover_routes(tab: &Tab) -> Result<Vec<Route>, String> {
    tab.navigate_to(&format!("{BASE_URL}/"))
        .map_err(|e| format!("navigate: {e}"))?;
    tab.wait_until_navigated()
        .map_err(|e| format!("wait: {e}"))?;
    sleep_ms(TIMEOUT_MS.min(3000));

    let js = r#"(() => {
      const out=[],seen=new Set(),P=(l,h)=>{
        if(!h)return;const k=(l||'').trim()+'|'+h;
        if(seen.has(k))return;seen.add(k);
        out.push({label:(l||'').trim(),href:h});
      };
      document.querySelectorAll('a[href]').forEach(a =>
        P(a.innerText||a.title, a.getAttribute('href')));
      document.querySelectorAll('[role="tab"],[data-tab],.tab,.nav-tab').forEach(el => {
        const l=el.innerText||el.getAttribute('aria-label')||el.dataset.tab||el.id||'';
        P(l, el.getAttribute('href') ||
             ('#/'+(el.dataset.tab||l.toLowerCase().replace(/\s+/g,'-'))));
      });
      return out;
    })()"#;

    let result = tab
        .evaluate(js, false)
        .map_err(|e| format!("discover JS: {e}"))?;
    let raw: Vec<Value> =
        serde_json::from_value(result.value.unwrap_or(Value::Array(vec![]))).unwrap_or_default();

    let base_host = Url::parse(BASE_URL)
        .ok()
        .and_then(|u| u.host_str().map(String::from));

    let mut seen = HashSet::new();
    let mut routes = Vec::new();

    for item in &raw {
        let h = item
            .get("href")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let lab = item
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if h.is_empty()
            || h.starts_with("javascript:")
            || h.starts_with("mailto:")
            || h.starts_with("data:")
            || h.starts_with("file:")
            || h.starts_with("tel:")
        {
            continue;
        }

        // Skip external links
        if h.starts_with("http")
            && let Ok(parsed) = Url::parse(&h)
        {
            let host = parsed.host_str().map(String::from);
            if host != base_host {
                continue;
            }
            // Rewrite to relative
            let path = parsed.path().to_string();
            let frag = parsed
                .fragment()
                .map(|f| format!("#{f}"))
                .unwrap_or_default();
            let href = format!("{path}{frag}");
            let n = norm_href(&href);
            if seen.contains(&n) {
                continue;
            }
            seen.insert(n);
            let label = if lab.is_empty() {
                href.clone()
            } else {
                lab.clone()
            };
            routes.push(Route {
                label: label.clone(),
                href,
                slug: slug(&label),
            });
            continue;
        }

        let n = norm_href(&h);
        if seen.contains(&n) {
            continue;
        }
        seen.insert(n);
        let label = if lab.is_empty() { h.clone() } else { lab };
        routes.push(Route {
            label: label.clone(),
            href: h,
            slug: slug(&label),
        });
    }

    // Union with fallback tabs
    for &tab_name in FALLBACK_TABS {
        let n = norm_href(&format!("#/{tab_name}"));
        if seen.contains(&n) {
            continue;
        }
        seen.insert(n);
        let label = {
            let mut c = tab_name.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        };
        routes.push(Route {
            label: label.clone(),
            href: format!("#/{tab_name}"),
            slug: slug(tab_name),
        });
    }

    // Ensure root is present
    if !routes
        .iter()
        .any(|r| matches!(norm_href(&r.href).as_str(), "/" | "#"))
    {
        routes.insert(
            0,
            Route {
                label: "root".into(),
                href: "/".into(),
                slug: "root".into(),
            },
        );
    }

    // Dedup slugs
    let mut counts: HashMap<String, usize> = HashMap::new();
    for r in &mut routes {
        let c = counts.entry(r.slug.clone()).or_insert(0);
        *c += 1;
        if *c > 1 {
            r.slug = format!("{}_{c}", r.slug);
        }
    }

    routes.truncate(MAX_ROUTES);
    Ok(routes)
}

// ---------------------------------------------------------------------------
// Page capture
// ---------------------------------------------------------------------------

fn capture_page(tab: &Tab, route: &Route, out_dir: &Path) -> Result<PageResult, String> {
    reset_error_capture(tab);

    let href = &route.href;
    let s = &route.slug;

    if let Some(stripped) = href.strip_prefix('#') {
        let hash_val = if stripped.is_empty() { "" } else { stripped };
        let js = format!(
            "((h) => {{ window.location.hash = h; }})({})",
            serde_json::to_string(hash_val).unwrap()
        );
        tab.evaluate(&js, false).ok();
    } else if href.starts_with('/') {
        tab.navigate_to(&format!("{BASE_URL}{href}"))
            .map_err(|e| format!("navigate: {e}"))?;
        tab.wait_until_navigated().ok();
    }

    sleep_ms(TIMEOUT_MS.min(3000));

    // Screenshot
    let png_path = out_dir.join(format!("{s}.png"));
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
    fs::write(out_dir.join(format!("{s}.txt")), &text).ok();

    // Title and h1
    let title = tab
        .evaluate("document.title || ''", false)
        .ok()
        .and_then(|r| r.value)
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();

    let h1 = tab
        .evaluate(
            "(()=>{const h=document.querySelector('h1');return h?h.innerText.trim():null;})()",
            false,
        )
        .ok()
        .and_then(|r| r.value)
        .and_then(|v| match v {
            Value::String(s) if !s.is_empty() => Some(s),
            _ => None,
        });

    // Collect errors
    let (http_errs, console_errs) = collect_errors(tab);
    fs::write(
        out_dir.join(format!("{s}.errors.json")),
        serde_json::to_string_pretty(&json!({
            "http": http_errs,
            "console": console_errs,
        }))
        .unwrap_or_default(),
    )
    .ok();

    let excerpt = text.chars().take(400).collect::<String>();
    let text_chars = text.len();

    Ok(PageResult {
        slug: s.clone(),
        href: href.clone(),
        url: href.clone(),
        label: route.label.clone(),
        title,
        h1,
        excerpt,
        text_chars,
        http_errors: http_errs.len(),
        console_errors: console_errs.len(),
    })
}

// ---------------------------------------------------------------------------
// Jargon scan
// ---------------------------------------------------------------------------

fn scan_jargon(text_dumps: &HashMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut result = BTreeMap::new();
    for term in JARGON_TERMS {
        let lower_term = term.to_lowercase();
        let mut hits: Vec<String> = text_dumps
            .iter()
            .filter(|(_, body)| body.to_lowercase().contains(&lower_term))
            .map(|(slug, _)| slug.clone())
            .collect();
        hits.sort();
        if !hits.is_empty() {
            result.insert(term.to_string(), hits);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Report generation
// ---------------------------------------------------------------------------

fn page_why(p: &PageResult) -> Vec<String> {
    let mut reasons = Vec::new();
    if p.text_chars < 200 {
        reasons.push(format!("only {} chars of body text", p.text_chars));
    }
    if p.h1.is_none() {
        reasons.push("no <h1>".into());
    }
    reasons
}

fn page_score(p: &PageResult, jargon: &BTreeMap<String, Vec<String>>) -> usize {
    let mut score = 3 * p.http_errors + 2 * p.console_errors;
    if p.h1.is_none() {
        score += 2;
    }
    if p.text_chars < 200 {
        score += 2;
    }
    score += jargon
        .values()
        .filter(|hits| hits.contains(&p.slug))
        .count();
    score
}

fn write_report(
    pages: &[PageResult],
    jargon: &BTreeMap<String, Vec<String>>,
    out_dir: &Path,
) -> Result<(), String> {
    let now = chrono::Utc::now();
    let timestamp = now.format("%Y-%m-%d %H:%M UTC").to_string();

    let mut lines: Vec<String> = vec![
        format!(
            "# Dashboard audit — first pass (issue #1990)\n\n_Generated {timestamp}; base {BASE_URL}_\n"
        ),
        "## 1. Pages found\n".into(),
    ];

    for p in pages {
        lines.push(format!(
            "- `{}` — {} ({}) [{} chars; http_errors={}, console_errors={}]",
            p.slug, p.label, p.href, p.text_chars, p.http_errors, p.console_errors
        ));
    }

    lines.push("\n## 2. What each page appears to convey\n".into());
    for p in pages {
        let display_title = if p.title.is_empty() {
            p.label.clone()
        } else {
            p.title.clone()
        };
        lines.push(format!("### `{}` — {display_title}", p.slug));
        lines.push(format!("- H1: {}", p.h1.as_deref().unwrap_or("(no <h1>)")));
        let excerpt_oneline = p
            .excerpt
            .replace('\n', " ")
            .trim()
            .chars()
            .take(240)
            .collect::<String>();
        lines.push(format!("- Excerpt: {excerpt_oneline:?}\n"));
    }

    lines.push(
        "## 3. Jargon inventory (terms that read as jargon to a non-Simard-developer)\n".into(),
    );
    if jargon.is_empty() {
        lines.push("_No flagged jargon terms detected._".into());
    } else {
        let mut sorted: Vec<_> = jargon.iter().collect();
        sorted.sort_by(|a, b| {
            b.1.len()
                .cmp(&a.1.len())
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        for (term, hits) in sorted {
            lines.push(format!("- **{term}** → {}", hits.join(", ")));
        }
    }

    lines.push("\n## 4. Missing context — pages a human would struggle to interpret\n".into());
    let short: Vec<_> = pages.iter().filter(|p| !page_why(p).is_empty()).collect();
    if short.is_empty() {
        lines.push("_All pages cleared the basic-context heuristics._".into());
    } else {
        for p in &short {
            lines.push(format!("- `{}` — {}", p.slug, page_why(p).join(", ")));
        }
    }

    lines.push(
        "\n## 5. Top-5 highest-impact usability fixes (heuristic ranking — engineer curates)\n"
            .into(),
    );
    let mut ranked: Vec<_> = pages.iter().collect();
    ranked.sort_by_key(|p| std::cmp::Reverse(page_score(p, jargon)));
    for (i, p) in ranked.iter().take(5).enumerate() {
        let score = page_score(p, jargon);
        lines.push(format!(
            "{}. **`{}`** (heuristic score {score}) — add plain-English H1, \
             de-jargon labels, surface 'what this is for', fix errors. Acceptance: a \
             first-time visitor can describe in one sentence what `{}` is for.",
            i + 1,
            p.slug,
            p.slug
        ));
    }

    let content = lines.join("\n") + "\n";
    fs::write(out_dir.join("REPORT.md"), content).map_err(|e| format!("write REPORT.md: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn run() -> Result<i32, String> {
    let out_dir = scripts_out_dir();
    fs::create_dir_all(&out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;

    let key = load_dashkey()?;
    let browser = launch_browser()?;
    let tab = browser.new_tab().map_err(|e| format!("new tab: {e}"))?;

    authenticate(&tab, &key)?;
    inject_error_capture(&tab);

    let routes = discover_routes(&tab)?;
    println!("[discover] {} route(s)", routes.len());

    let mut results = Vec::new();
    let mut dumps: HashMap<String, String> = HashMap::new();

    for route in &routes {
        match capture_page(&tab, route, &out_dir) {
            Ok(res) => {
                // Read back text dump
                let text_path = out_dir.join(format!("{}.txt", res.slug));
                let text = fs::read_to_string(&text_path).unwrap_or_default();
                println!("[capture] {:20} {:6} chars", res.slug, res.text_chars);
                dumps.insert(res.slug.clone(), text);
                results.push(res);
            }
            Err(e) => {
                println!("[capture] {}: FAILED — {e}", route.slug);
            }
        }
    }

    let jargon = scan_jargon(&dumps);
    write_report(&results, &jargon, &out_dir)?;

    let index = json!({
        "base": BASE_URL,
        "routes": routes,
        "pages": results,
        "jargon": jargon,
    });
    fs::write(
        out_dir.join("_audit_dashboard_index.json"),
        serde_json::to_string_pretty(&index).unwrap_or_default(),
    )
    .map_err(|e| format!("write index: {e}"))?;

    if results.is_empty() { Ok(2) } else { Ok(0) }
}

fn main() -> ExitCode {
    match run() {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code as u8),
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
    fn slug_basic() {
        assert_eq!(slug("Hello World"), "hello-world");
        assert_eq!(slug("Overview"), "overview");
        assert_eq!(slug("COSTS"), "costs");
    }

    #[test]
    fn slug_empty() {
        assert_eq!(slug(""), "page");
        assert_eq!(slug("   "), "page");
        assert_eq!(slug("---"), "page");
    }

    #[test]
    fn slug_preserves_underscore_dash() {
        assert_eq!(slug("my_tab"), "my_tab");
        assert_eq!(slug("my-tab"), "my-tab");
    }

    #[test]
    fn norm_hash() {
        assert_eq!(norm_href("#/memory"), "#memory");
        assert_eq!(norm_href("#/costs?v=1"), "#costs");
        assert_eq!(norm_href("#"), "#");
    }

    #[test]
    fn norm_path() {
        assert_eq!(norm_href("/api/login"), "/api/login");
        assert_eq!(norm_href("/overview/"), "/overview");
        assert_eq!(norm_href("/page?q=1"), "/page");
    }

    #[test]
    fn norm_other() {
        assert_eq!(norm_href("http://example.com"), "http://example.com");
    }

    #[test]
    fn jargon_scan_finds_terms() {
        let mut dumps = HashMap::new();
        dumps.insert(
            "overview".into(),
            "The OODA loop drives the decision cycle. We use cognitive memory.".into(),
        );
        dumps.insert("logs".into(), "Plain log output here.".into());

        let result = scan_jargon(&dumps);
        assert!(result.contains_key("OODA"));
        assert!(result.contains_key("OODA loop"));
        assert!(result.contains_key("cognitive memory"));
        assert!(result["OODA"].contains(&"overview".to_string()));
        assert!(!result.contains_key("LadybugDB"));
    }

    #[test]
    fn jargon_scan_empty() {
        let dumps = HashMap::new();
        let result = scan_jargon(&dumps);
        assert!(result.is_empty());
    }

    #[test]
    fn jargon_scan_case_insensitive() {
        let mut dumps = HashMap::new();
        dumps.insert("page1".into(), "The ooda loop is important.".into());
        let result = scan_jargon(&dumps);
        assert!(result.contains_key("OODA loop"));
    }

    #[test]
    fn page_score_calculation() {
        let p = PageResult {
            slug: "test".into(),
            href: "#/test".into(),
            url: "#/test".into(),
            label: "Test".into(),
            title: "Test Page".into(),
            h1: None,
            excerpt: "short".into(),
            text_chars: 50,
            http_errors: 2,
            console_errors: 1,
        };
        let jargon = BTreeMap::new();
        // 3*2 + 2*1 + 2 (no h1) + 2 (short text) = 12
        assert_eq!(page_score(&p, &jargon), 12);
    }

    #[test]
    fn page_score_with_jargon() {
        let p = PageResult {
            slug: "overview".into(),
            href: "#/overview".into(),
            url: "#/overview".into(),
            label: "Overview".into(),
            title: "Overview".into(),
            h1: Some("Overview".into()),
            excerpt: "Lots of content here...".repeat(20),
            text_chars: 500,
            http_errors: 0,
            console_errors: 0,
        };
        let mut jargon = BTreeMap::new();
        jargon.insert("OODA".into(), vec!["overview".into()]);
        jargon.insert(
            "cognitive memory".into(),
            vec!["overview".into(), "other".into()],
        );
        // 0 + 0 + 0 + 0 + 2 (two jargon terms match "overview") = 2
        assert_eq!(page_score(&p, &jargon), 2);
    }

    #[test]
    fn page_why_identifies_issues() {
        let p = PageResult {
            slug: "x".into(),
            href: "/x".into(),
            url: "/x".into(),
            label: "X".into(),
            title: "X".into(),
            h1: None,
            excerpt: "".into(),
            text_chars: 50,
            http_errors: 0,
            console_errors: 0,
        };
        let reasons = page_why(&p);
        assert_eq!(reasons.len(), 2);
        assert!(reasons[0].contains("50 chars"));
        assert!(reasons[1].contains("<h1>"));
    }

    #[test]
    fn page_why_clean() {
        let p = PageResult {
            slug: "ok".into(),
            href: "/ok".into(),
            url: "/ok".into(),
            label: "OK".into(),
            title: "OK".into(),
            h1: Some("OK Page".into()),
            excerpt: "x".repeat(200),
            text_chars: 300,
            http_errors: 0,
            console_errors: 0,
        };
        assert!(page_why(&p).is_empty());
    }
}
