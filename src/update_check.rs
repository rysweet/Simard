//! Startup update-version check against GitHub Releases (issue #2250).
//!
//! On launch each binary queries GitHub for the latest release and prints a
//! notice if a newer version is available. Results are cached for 24h at
//! `~/.config/simard/update_cache.json`, so a fresh cache skips the network
//! entirely. The check is **non-blocking and fail-open**: any network, API,
//! or parse failure is surfaced via `tracing::warn!` and resolves to "no
//! update" — it never blocks or crashes the binary.
//!
//! Env-var controls:
//! - `SIMARD_NO_UPDATE_CHECK=1` — skip the check entirely (no cache/network/prompt)
//! - `SIMARD_NONINTERACTIVE=1`  — print the notice but suppress the upgrade prompt

use std::io::IsTerminal;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cmd_self_update::platform::{CURRENT_VERSION, GITHUB_REPO};

/// 24-hour cache TTL, in seconds.
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

/// Canonical Simard releases URL — the anti-phishing fallback and the root of
/// the release-URL host allowlist.
const SIMARD_RELEASES_URL: &str = "https://github.com/rysweet/Simard/releases";

/// Information about an available update.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub release_notes: String,
}

/// Run the update check as fire-and-forget: spawns a detached thread that
/// prints an upgrade notice to stderr if a newer version is available.
/// Never blocks the caller — the thread is not joined.
pub fn run_update_check() {
    if std::env::var("SIMARD_NO_UPDATE_CHECK")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return;
    }

    std::thread::spawn(|| {
        if let Some(info) = check_for_update() {
            print_upgrade_notice(&info);
        }
    });
}

/// Non-blocking variant for TUI: spawns the check in background and returns
/// a channel receiver. The TUI polls `try_recv()` to get the update notice
/// string (if any) and renders it in its own draw cycle — no direct stderr
/// writes that would corrupt the alternate screen.
///
/// Returns `None` if the check is disabled via env var.
pub fn run_update_check_background() -> Option<mpsc::Receiver<String>> {
    if std::env::var("SIMARD_NO_UPDATE_CHECK")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return None;
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(info) = check_for_update() {
            let notice = format!(
                "Update available: {} -> {}  {}",
                info.current_version, info.latest_version, info.release_url,
            );
            let _ = tx.send(notice);
        }
    });
    Some(rx)
}

/// Check for a newer release, consulting the 24h on-disk cache first.
///
/// A fresh cache short-circuits the network. On a cache miss or expiry this
/// queries GitHub, persists the result, and compares against the running
/// version. Returns `Some(UpdateInfo)` only when a strictly newer version is
/// available; any failure resolves to `None` (fail-open).
pub fn check_for_update() -> Option<UpdateInfo> {
    let now = unix_now();

    // 1. A fresh cache short-circuits the network entirely.
    if let Some(cache) = load_cache()
        && cache_is_fresh(cache.last_check_epoch_secs, now)
    {
        return update_from_version(
            &cache.latest_version,
            &cache.release_url,
            &cache.release_notes,
        );
    }

    // 2. Cache miss or expiry: query GitHub (fail-open on any error).
    let (latest_version, release_url, release_notes) = fetch_latest_version()?;

    // 3. Persist the freshly-fetched result (best-effort; never fatal).
    let cache = Cache {
        last_check_epoch_secs: now,
        latest_version: latest_version.clone(),
        release_url: release_url.clone(),
        release_notes: release_notes.clone(),
    };
    if let Err(e) = save_cache(&cache) {
        tracing::warn!("simard update-check: failed to persist update cache: {e}");
    }

    update_from_version(&latest_version, &release_url, &release_notes)
}

/// Build an `UpdateInfo` from raw (untrusted) release fields, but only when
/// `latest` is strictly newer than the running version. Response-derived
/// strings are sanitized and the URL host is allowlisted before use.
fn update_from_version(latest: &str, url: &str, notes: &str) -> Option<UpdateInfo> {
    if is_newer(CURRENT_VERSION, latest) {
        Some(UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version: sanitize(latest),
            release_url: validate_release_url(&sanitize(url)),
            release_notes: sanitize(notes),
        })
    } else {
        None
    }
}

/// Print the upgrade banner to stderr (keeping stdout clean for piping) and,
/// when attached to an interactive terminal, offer a short, timed prompt.
fn print_upgrade_notice(info: &UpdateInfo) {
    eprintln!("{}", format_banner(info));
    maybe_prompt_upgrade();
}

// ── Network / parsing ────────────────────────────────────────────────

/// Query GitHub for the latest release via an in-process `ureq` request.
///
/// Returns `(version, release_url, release_notes)` on success. Any network,
/// HTTP-status, size, or parse failure is logged via `tracing::warn!` and
/// resolves to `None` (fail-open) so a launch is never blocked.
fn fetch_latest_version() -> Option<(String, String, String)> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let user_agent = format!("simard/{CURRENT_VERSION}");

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = config.into();

    // ureq treats 4xx/5xx as `Err(StatusCode)` by default, so a non-2xx
    // response (including rate-limit 403s) fails open here.
    let mut response = match agent
        .get(&url)
        .header("User-Agent", &user_agent)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!("simard update-check: GitHub releases query failed: {e}");
            return None;
        }
    };

    // Cap the body at 256 KiB to bound launch-time work and memory.
    let body = match response
        .body_mut()
        .with_config()
        .limit(256 * 1024)
        .read_to_string()
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("simard update-check: reading GitHub response failed: {e}");
            return None;
        }
    };

    match parse_latest(&body) {
        Some(v) => Some(v),
        None => {
            tracing::warn!("simard update-check: could not parse GitHub release JSON");
            None
        }
    }
}

/// Parse a GitHub `releases/latest` JSON payload into
/// `(version, release_url, release_notes)`. A leading `v` on `tag_name` is
/// stripped. Returns `None` (never panics) on any malformed or missing field.
fn parse_latest(body: &str) -> Option<(String, String, String)> {
    let release: serde_json::Value = serde_json::from_str(body).ok()?;
    if !release.is_object() {
        return None;
    }
    let tag = release.get("tag_name")?.as_str()?;
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    let url = release
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let notes = release
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((version, url, notes))
}

/// Strip terminal-control characters (ESC/BEL/CR and all other C0/C1 controls)
/// from response-derived text before it is printed, defeating escape-sequence
/// injection via a spoofed release payload.
fn sanitize(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Allowlist the release URL host: only an `https://github.com/rysweet/Simard`
/// URL is trusted. Anything else (foreign host, look-alike subdomain, or a
/// userinfo trick) is replaced with the hardcoded canonical Simard URL.
fn validate_release_url(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://") {
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if host == "github.com"
            && (path == "/rysweet/Simard" || path.starts_with("/rysweet/Simard/"))
        {
            return url.to_string();
        }
    }
    SIMARD_RELEASES_URL.to_string()
}

/// Format the launch banner. The first line is exactly the spec literal
/// `simard: update available X.Y.Z -> A.B.C` (ASCII arrow), followed by the
/// (already host-validated) release URL.
fn format_banner(info: &UpdateInfo) -> String {
    format!(
        "simard: update available {} -> {}\n  {}",
        info.current_version, info.latest_version, info.release_url
    )
}

/// Returns `true` iff `latest` is strictly newer than `current` under semver
/// ordering. Fail-open: if either string is not valid semver, returns `false`
/// (never panics).
fn is_newer(current: &str, latest: &str) -> bool {
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(c), Ok(l)) => l > c,
        _ => false,
    }
}

/// Current UNIX time in seconds (0 if the clock predates the epoch).
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pure 24h TTL predicate: a cache written at `last_check` is fresh at `now`
/// while no more than [`CACHE_TTL_SECS`] have elapsed. Clock skew (a future
/// `last_check`) is treated as fresh.
fn cache_is_fresh(last_check: u64, now: u64) -> bool {
    now.saturating_sub(last_check) <= CACHE_TTL_SECS
}

// ── On-disk cache ────────────────────────────────────────────────────

/// On-disk update-check cache (a single JSON record).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cache {
    last_check_epoch_secs: u64,
    latest_version: String,
    release_url: String,
    release_notes: String,
}

/// Resolve the cache file path, honoring `XDG_CONFIG_HOME` via `dirs`.
fn cache_path() -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("simard");
    p.push("update_cache.json");
    Some(p)
}

/// Load and parse the cache. Returns `None` on any error (absent, corrupt, or
/// a refused symlink) — the caller then recomputes. Cached content is treated
/// as untrusted and re-validated/re-sanitized downstream.
fn load_cache() -> Option<Cache> {
    let path = cache_path()?;
    if let Ok(meta) = std::fs::symlink_metadata(&path)
        && meta.file_type().is_symlink()
    {
        tracing::warn!("simard update-check: refusing to read update cache via symlink");
        return None;
    }
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<Cache>(&data).ok()
}

/// Persist the cache with an atomic temp-file + rename, `0700` dir / `0600`
/// file permissions, and a symlink refusal at the target path.
fn save_cache(cache: &Cache) -> std::io::Result<()> {
    let path = cache_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no config directory for update cache",
        )
    })?;
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid cache path")
    })?;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }

    if let Ok(meta) = std::fs::symlink_metadata(&path)
        && meta.file_type().is_symlink()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to write update cache via symlink",
        ));
    }

    let json = serde_json::to_string(cache)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(json.as_bytes())?;
        f.flush()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

// ── Interactive prompt ───────────────────────────────────────────────

/// Offer a short, timed upgrade prompt on an interactive terminal.
///
/// Suppressed when `SIMARD_NONINTERACTIVE=1` or when stdin/stderr are not
/// TTYs (e.g. the TUI's alternate screen, which renders the banner via its own
/// channel instead). Any of a ~10s timeout, EOF, or a non-affirmative answer
/// resolves to "No". A `y`/`yes` only prints the `simard update` hint — it
/// NEVER auto-installs.
fn maybe_prompt_upgrade() {
    if std::env::var("SIMARD_NONINTERACTIVE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return;
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return;
    }

    eprint!("Upgrade now? [y/N] ");
    let _ = std::io::Write::flush(&mut std::io::stderr());

    match read_line_with_timeout(Duration::from_secs(10)) {
        Some(answer) => {
            let a = answer.trim();
            if a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes") {
                eprintln!("Run `simard update` to upgrade.");
            } else {
                eprintln!();
            }
        }
        None => {
            // Timeout or EOF: default to No.
            eprintln!();
        }
    }
}

/// Read one line from stdin with a hard timeout. Returns `None` on timeout or
/// read error. The reader thread is detached; on timeout it is abandoned,
/// which is acceptable for a short-lived launch-time convenience.
fn read_line_with_timeout(timeout: Duration) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_ok() {
            let _ = tx.send(line);
        }
    });
    rx.recv_timeout(timeout).ok()
}

#[cfg(test)]
mod tests {
    //! TDD contract for the reworked update-check module (issue #2250).
    //!
    //! These tests specify the **target** behaviour of the spec-conformant
    //! rework and are written *before* the implementation exists — several
    //! reference seams (`parse_latest`, `format_banner`, `sanitize`,
    //! `validate_release_url`, `Cache`, `load_cache`, `save_cache`,
    //! `cache_is_fresh`) and the new `is_newer(current, latest)` /
    //! `UpdateInfo { .., release_notes }` shape are introduced by the
    //! implementation step. Until then this module fails to compile — that
    //! is the intended TDD "red".
    //!
    //! Contract map (T1–T12):
    //!   T1  semver newer  => `is_newer` true
    //!   T2  same version  => `is_newer` false
    //!   T3  older latest  => `is_newer` false
    //!   T4  prerelease ordering via the `semver` crate
    //!   T5  malformed version strings => false, never panic (fail-open)
    //!   T6  `parse_latest` extracts (version, url, notes), strips leading `v`
    //!   T7  malformed API JSON => `parse_latest` None, never panic
    //!   T8  `format_banner` prints exactly `simard: update available X.Y.Z -> A.B.C`
    //!   T9  `sanitize` strips ESC/BEL/CR/C0 terminal-control chars
    //!   T10 `validate_release_url` allowlists the Simard GitHub host
    //!   T11 `cache_is_fresh` enforces the 24h TTL (pure predicate)
    //!   T12 fresh cache short-circuits the network in `check_for_update`
    //!   plus: opt-out env => no-op, fire-and-forget launch, current version parses.

    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const DAY_SECS: u64 = 24 * 60 * 60;

    fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_secs()
    }

    /// A well-formed GitHub `releases/latest` payload for parser tests.
    fn sample_release_json() -> String {
        serde_json::json!({
            "tag_name": "v1.2.3",
            "html_url": "https://github.com/rysweet/Simard/releases/tag/v1.2.3",
            "body": "## What's new\n- shiny things",
            "assets": []
        })
        .to_string()
    }

    // ── T1–T3: strictly-newer comparison, new (current, latest) order ──

    #[test]
    fn t1_is_newer_true_when_latest_is_newer() {
        // Signature is is_newer(current, latest): true iff `latest` > `current`.
        assert!(is_newer("0.19.0", "1.0.0"));
        assert!(is_newer("0.19.0", "0.20.0"));
        assert!(is_newer("0.19.0", "0.19.1"));
    }

    #[test]
    fn t2_is_newer_false_when_equal() {
        assert!(!is_newer("0.19.0", "0.19.0"));
        assert!(!is_newer("1.2.3", "1.2.3"));
    }

    #[test]
    fn t3_is_newer_false_when_latest_is_older() {
        assert!(!is_newer("1.0.0", "0.19.0"));
        assert!(!is_newer("0.19.0", "0.18.0"));
    }

    // ── T4: prerelease ordering (semver crate semantics) ──────────────

    #[test]
    fn t4_is_newer_prerelease_ordering() {
        // A released X.Y.Z is newer than its own prerelease.
        assert!(
            is_newer("1.0.0-rc1", "1.0.0"),
            "release 1.0.0 must be newer than prerelease 1.0.0-rc1"
        );
        // A prerelease is NOT newer than the corresponding release.
        assert!(
            !is_newer("1.0.0", "1.0.0-rc1"),
            "prerelease 1.0.0-rc1 must NOT be newer than release 1.0.0"
        );
        // A prerelease of a higher core version is still newer.
        assert!(
            is_newer("1.0.0", "2.0.0-beta.1"),
            "prerelease of a higher version is still newer"
        );
    }

    // ── T5: fail-open on malformed version strings (no panic) ─────────

    #[test]
    fn t5_is_newer_fail_open_on_garbage() {
        let result = std::panic::catch_unwind(|| {
            assert!(!is_newer("not-a-version", "1.0.0"));
            assert!(!is_newer("1.0.0", "not-a-version"));
            assert!(!is_newer("", ""));
            assert!(!is_newer("1.2", "1.2.3")); // incomplete core
        });
        assert!(result.is_ok(), "is_newer must never panic on bad input");
    }

    // ── T6: parse_latest extracts fields and strips a leading `v` ─────

    #[test]
    fn t6_parse_latest_extracts_fields() {
        let (version, url, notes) =
            parse_latest(&sample_release_json()).expect("well-formed release should parse");
        assert_eq!(
            version, "1.2.3",
            "leading 'v' must be stripped from tag_name"
        );
        assert_eq!(url, "https://github.com/rysweet/Simard/releases/tag/v1.2.3");
        assert!(notes.contains("shiny things"), "release body -> notes");
    }

    #[test]
    fn t6b_parse_latest_accepts_tag_without_v_prefix() {
        let json = serde_json::json!({
            "tag_name": "2.5.0",
            "html_url": "https://github.com/rysweet/Simard/releases/tag/2.5.0",
            "body": ""
        })
        .to_string();
        let (version, _url, _notes) = parse_latest(&json).expect("bare tag should parse");
        assert_eq!(version, "2.5.0");
    }

    // ── T7: malformed API responses => None, never panic ──────────────

    #[test]
    fn t7_parse_latest_malformed_returns_none_no_panic() {
        let result = std::panic::catch_unwind(|| {
            assert!(parse_latest("this is not json").is_none());
            assert!(parse_latest("{}").is_none(), "missing tag_name => None");
            assert!(parse_latest("[]").is_none(), "wrong top-level type => None");
            assert!(
                parse_latest(r#"{"tag_name": 123}"#).is_none(),
                "wrong tag_name type => None"
            );
            assert!(parse_latest("").is_none());
        });
        assert!(
            result.is_ok(),
            "parse_latest must never panic on malformed input"
        );
    }

    // ── T8: banner text is exactly the spec string (ASCII, no `v`) ────

    #[test]
    fn t8_format_banner_exact_text() {
        let info = UpdateInfo {
            current_version: "0.19.0".to_string(),
            latest_version: "1.2.3".to_string(),
            release_url: "https://github.com/rysweet/Simard/releases/tag/v1.2.3".to_string(),
            release_notes: "notes".to_string(),
        };
        let banner = format_banner(&info);
        let first_line = banner.lines().next().unwrap_or_default();
        assert_eq!(
            first_line, "simard: update available 0.19.0 -> 1.2.3",
            "banner headline must match the spec literal exactly"
        );
        assert!(
            !banner.contains('\u{2192}'),
            "banner must use ASCII '->', never the unicode arrow"
        );
    }

    // ── T9: sanitize strips terminal-control characters ───────────────

    #[test]
    fn t9_sanitize_strips_control_chars() {
        let hostile = "safe\x1b[31mtext\x07more\rend";
        let cleaned = sanitize(hostile);
        assert!(!cleaned.contains('\x1b'), "ESC must be stripped");
        assert!(!cleaned.contains('\x07'), "BEL must be stripped");
        assert!(!cleaned.contains('\r'), "CR must be stripped");
        // Visible payload survives.
        for token in ["safe", "text", "more", "end"] {
            assert!(
                cleaned.contains(token),
                "visible text `{token}` must remain"
            );
        }
    }

    // ── T10: release URL host allowlist (anti-phishing) ───────────────

    #[test]
    fn t10_validate_release_url_allowlist() {
        // Legitimate Simard GitHub URL passes through unchanged.
        let good = "https://github.com/rysweet/Simard/releases/tag/v1.2.3";
        assert_eq!(validate_release_url(good), good);

        // A foreign host is rejected in favour of the hardcoded Simard fallback.
        let evil = validate_release_url("https://evil.example.com/rysweet/Simard/x");
        assert!(
            !evil.contains("evil.example.com"),
            "foreign host must be dropped"
        );
        assert!(
            evil.contains("github.com/rysweet/Simard"),
            "fallback must point at the real Simard repo"
        );

        // Look-alike subdomain trick (github.com.evil.com) must not slip through.
        let spoof = validate_release_url("https://github.com.evil.com/rysweet/Simard");
        assert!(!spoof.contains("evil"), "look-alike host must be rejected");
    }

    // ── T11: 24h cache TTL is a pure, deterministic predicate ─────────

    #[test]
    fn t11_cache_is_fresh_ttl_boundary() {
        let base: u64 = 1_000_000_000;
        assert!(cache_is_fresh(base, base), "same instant is fresh");
        assert!(cache_is_fresh(base, base + 1_000), "1000s old is fresh");
        assert!(
            cache_is_fresh(base, base + DAY_SECS - 1),
            "just under 24h is fresh"
        );
        assert!(
            !cache_is_fresh(base, base + DAY_SECS + 1),
            "just over 24h is stale"
        );
    }

    // ── Cache round-trip through the real on-disk path (XDG-isolated) ──

    #[test]
    #[serial_test::serial(update_check_env, cognitive_memory)]
    fn cache_save_load_round_trip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };

        let cache = Cache {
            last_check_epoch_secs: now_epoch(),
            latest_version: "3.4.5".to_string(),
            release_url: "https://github.com/rysweet/Simard/releases/tag/v3.4.5".to_string(),
            release_notes: "round-trip".to_string(),
        };
        save_cache(&cache).expect("save_cache should succeed under a writable XDG dir");
        let loaded = load_cache();

        // Restore env before asserting so a failure can't leak state.
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }

        let loaded = loaded.expect("load_cache should read back what save_cache wrote");
        assert_eq!(loaded.latest_version, "3.4.5");
        assert_eq!(loaded.release_notes, "round-trip");
    }

    // ── T12: a fresh cache short-circuits the network ─────────────────

    #[test]
    #[serial_test::serial(update_check_env, cognitive_memory)]
    fn t12_fresh_cache_newer_returns_some_without_network() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_noupd = std::env::var_os("SIMARD_NO_UPDATE_CHECK");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp.path());
            std::env::remove_var("SIMARD_NO_UPDATE_CHECK");
        }

        // A fresh cache advertising an impossibly-high version. If the network
        // were consulted this could never be returned offline, so a `Some`
        // with this version proves the cache short-circuited the fetch.
        let cache = Cache {
            last_check_epoch_secs: now_epoch(),
            latest_version: "999.0.0".to_string(),
            release_url: "https://github.com/rysweet/Simard/releases/tag/v999.0.0".to_string(),
            release_notes: "from cache".to_string(),
        };
        save_cache(&cache).expect("save_cache");

        let info = check_for_update();

        match prev_xdg {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        if let Some(v) = prev_noupd {
            unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", v) };
        }

        let info = info.expect("fresh cache with a newer version must yield Some(UpdateInfo)");
        assert_eq!(info.latest_version, "999.0.0");
        assert_eq!(info.current_version, CURRENT_VERSION);
        assert_eq!(info.release_notes, "from cache");
    }

    #[test]
    #[serial_test::serial(update_check_env, cognitive_memory)]
    fn t12b_fresh_cache_not_newer_returns_none_without_network() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_noupd = std::env::var_os("SIMARD_NO_UPDATE_CHECK");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp.path());
            std::env::remove_var("SIMARD_NO_UPDATE_CHECK");
        }

        // Cache says the latest release is ancient (< current 0.26.0). A fresh
        // cache must short-circuit and, being not-newer, resolve to None.
        let cache = Cache {
            last_check_epoch_secs: now_epoch(),
            latest_version: "0.0.1".to_string(),
            release_url: "https://github.com/rysweet/Simard/releases/tag/v0.0.1".to_string(),
            release_notes: String::new(),
        };
        save_cache(&cache).expect("save_cache");

        let info = check_for_update();

        match prev_xdg {
            Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
        }
        if let Some(v) = prev_noupd {
            unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", v) };
        }

        assert!(
            info.is_none(),
            "a fresh cache older than the current version must yield None"
        );
    }

    // ── Opt-out env var fully disables the check (no-op) ──────────────

    #[test]
    // #2360 class: these tests set/remove the process-global
    // `SIMARD_NO_UPDATE_CHECK`, which `run_update_check[_background]` reads.
    // cargo runs lib tests multi-threaded and glibc getenv/setenv are not
    // thread-safe, so all tests touching the var share the `update_check_env`
    // serial key.
    #[serial_test::serial(update_check_env, cognitive_memory)]
    fn opt_out_env_disables_background_channel() {
        let prev = std::env::var_os("SIMARD_NO_UPDATE_CHECK");
        unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", "1") };
        let rx = run_update_check_background();
        match prev {
            Some(v) => unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", v) },
            None => unsafe { std::env::remove_var("SIMARD_NO_UPDATE_CHECK") },
        }
        assert!(
            rx.is_none(),
            "SIMARD_NO_UPDATE_CHECK=1 must make run_update_check_background a no-op"
        );
    }

    #[test]
    #[serial_test::serial(update_check_env, cognitive_memory)]
    fn opt_out_env_returns_immediately_from_run_update_check() {
        let prev = std::env::var_os("SIMARD_NO_UPDATE_CHECK");
        unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", "1") };
        let start = std::time::Instant::now();
        run_update_check();
        let elapsed = start.elapsed();
        match prev {
            Some(v) => unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", v) },
            None => unsafe { std::env::remove_var("SIMARD_NO_UPDATE_CHECK") },
        }
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "run_update_check() must return immediately when opted out, took {elapsed:?}"
        );
    }

    // ── Launch path is fire-and-forget (never blocks startup) ─────────

    #[test]
    #[serial_test::serial(update_check_env)]
    fn run_update_check_is_fire_and_forget() {
        // Even when enabled, run_update_check() must return immediately because
        // it spawns a detached thread (no join). A reintroduced handle.join()
        // would make this take ~5s and fail.
        let start = std::time::Instant::now();
        run_update_check();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "run_update_check() must be fire-and-forget (no join), took {elapsed:?}"
        );
    }

    #[test]
    #[serial_test::serial(update_check_env, cognitive_memory)]
    fn run_update_check_background_returns_some_when_enabled() {
        let prev = std::env::var_os("SIMARD_NO_UPDATE_CHECK");
        unsafe { std::env::remove_var("SIMARD_NO_UPDATE_CHECK") };
        let rx = run_update_check_background();
        if let Some(v) = prev {
            unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", v) };
        }
        assert!(
            rx.is_some(),
            "run_update_check_background() should return Some(Receiver) when enabled"
        );
    }

    // ── CARGO_PKG_VERSION must be valid semver (comparable) ───────────

    #[test]
    fn current_version_is_valid_and_comparable() {
        // Indirectly proves CURRENT_VERSION parses under the semver crate: it
        // must be older than an absurd ceiling and newer than the zero version.
        assert!(
            is_newer(CURRENT_VERSION, "9999.0.0"),
            "CARGO_PKG_VERSION ({CURRENT_VERSION}) must compare as valid semver"
        );
        assert!(!is_newer(CURRENT_VERSION, "0.0.0"));
    }
}
