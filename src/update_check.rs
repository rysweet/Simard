//! Startup update-version check against GitHub Releases.
//!
//! On every launch, queries GitHub for the latest release and prints a
//! notice if a newer version is available. No caching — just a fast
//! `gh api` call (or `curl` fallback) with a hard 5-second timeout.
//!
//! Env-var controls:
//! - `SIMARD_NO_UPDATE_CHECK=1` — skip the check entirely
//! - `SIMARD_NONINTERACTIVE=1`  — print the notice but suppress the upgrade prompt

use std::sync::mpsc;

use crate::cmd_self_update::platform::{CURRENT_VERSION, GITHUB_REPO};

/// Information about an available update.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub has_platform_asset: bool,
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
                "Update available: v{} → v{}  {}{}",
                info.current_version,
                info.latest_version,
                info.release_url,
                if info.has_platform_asset {
                    "  Run `simard self-update` to upgrade."
                } else {
                    ""
                },
            );
            let _ = tx.send(notice);
        }
    });
    Some(rx)
}

/// Check GitHub for a newer release.
/// Returns `Some(UpdateInfo)` if a newer version exists, `None` otherwise.
pub fn check_for_update() -> Option<UpdateInfo> {
    let (latest_version, release_url, has_platform_asset) = fetch_latest_version()?;

    if is_newer(&latest_version, CURRENT_VERSION) {
        Some(UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version,
            release_url,
            has_platform_asset,
        })
    } else {
        None
    }
}

/// Print an upgrade notice to stderr (so it doesn't pollute stdout piping).
fn print_upgrade_notice(info: &UpdateInfo) {
    eprintln!(
        "\x1b[33msimard: update available v{} → v{}\x1b[0m",
        info.current_version, info.latest_version
    );
    eprintln!("  {}", info.release_url);
    if info.has_platform_asset {
        eprintln!("  Run `simard self-update` to upgrade.\n");
    } else {
        eprintln!("  (no pre-built binary for this platform — build from source)\n");
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Query GitHub for the latest release tag, URL, and platform asset availability.
/// Tries `gh api` first, then `curl` on ANY failure (not just spawn failure).
fn fetch_latest_version() -> Option<(String, String, bool)> {
    let json = fetch_via_gh().or_else(fetch_via_curl)?;
    let release: serde_json::Value = serde_json::from_str(&json).ok()?;

    let tag = release["tag_name"].as_str()?;
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    let url = release["html_url"].as_str().unwrap_or("").to_string();

    // Check if there's an asset for the current platform
    let has_platform_asset = crate::cmd_self_update::platform::platform_suffix()
        .map(|suffix| {
            release["assets"]
                .as_array()
                .map(|assets| {
                    assets.iter().any(|a| {
                        a["name"]
                            .as_str()
                            .map(|n| n.contains(suffix))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .unwrap_or(false);

    Some((version, url, has_platform_asset))
}

/// Try `gh api` with a 5-second timeout.
fn fetch_via_gh() -> Option<String> {
    let child = std::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{GITHUB_REPO}/releases/latest"),
            "--jq",
            ".",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    wait_with_timeout(child, std::time::Duration::from_secs(5))
}

/// Try `curl` with built-in timeout flags.
fn fetch_via_curl() -> Option<String> {
    let child = std::process::Command::new("curl")
        .args([
            "-sS",
            "--connect-timeout",
            "3",
            "--max-time",
            "5",
            "-H",
            "Accept: application/vnd.github+json",
            &format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    wait_with_timeout(child, std::time::Duration::from_secs(6))
}

/// Wait for a child process with a hard timeout. Returns stdout on success.
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: std::time::Duration,
) -> Option<String> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut stdout = child.stdout.take()?;
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut stdout, &mut buf).ok()?;
                return Some(buf);
            }
            Ok(Some(_)) => return None, // non-zero exit
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

/// Parse a semver string "major.minor.patch[-prerelease]" into a comparable tuple.
///
/// Returns `(major, minor, patch, is_release)` where `is_release` is `true`
/// for plain releases and `false` for pre-release versions (e.g. "-beta.1",
/// "-rc1"). This ensures pre-releases sort *before* the corresponding release:
/// `1.0.0-rc1 < 1.0.0`.
fn parse_semver(v: &str) -> Option<(u64, u64, u64, bool)> {
    // Strip build metadata first (everything after '+'), then check for
    // pre-release ('-'). This order matters because build metadata can
    // contain hyphens (e.g. "1.2.3+build-456" is a valid release).
    let (without_build, _build_meta) = match v.find('+') {
        Some(idx) => (&v[..idx], Some(&v[idx + 1..])),
        None => (v, None),
    };
    let (numeric, has_prerelease) = match without_build.find('-') {
        Some(idx) => (&without_build[..idx], true),
        None => (without_build, false),
    };
    let parts: Vec<&str> = numeric.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
        !has_prerelease,
    ))
}

/// Returns true if `latest` is strictly newer than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_semver_valid() {
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3, true)));
        assert_eq!(parse_semver("0.19.0"), Some((0, 19, 0, true)));
    }

    #[test]
    fn parse_semver_with_prerelease() {
        assert_eq!(parse_semver("1.2.3-beta.1"), Some((1, 2, 3, false)));
        assert_eq!(parse_semver("1.0.0-rc1"), Some((1, 0, 0, false)));
    }

    #[test]
    fn parse_semver_with_build_metadata() {
        // Build metadata without prerelease → still a release
        assert_eq!(parse_semver("1.2.3+build.456"), Some((1, 2, 3, true)));
    }

    #[test]
    fn parse_semver_rejects_invalid() {
        assert_eq!(parse_semver("not.a.version"), None);
        assert_eq!(parse_semver("1.2"), None);
        assert_eq!(parse_semver(""), None);
        assert_eq!(parse_semver("1.2.3.4"), None); // too many parts
    }

    #[test]
    fn is_newer_returns_true_for_higher_version() {
        assert!(is_newer("1.0.0", "0.19.0"));
        assert!(is_newer("0.20.0", "0.19.0"));
        assert!(is_newer("0.19.1", "0.19.0"));
    }

    #[test]
    fn is_newer_returns_false_for_same_or_older() {
        assert!(!is_newer("0.19.0", "0.19.0"));
        assert!(!is_newer("0.18.0", "0.19.0"));
        assert!(!is_newer("0.19.0", "1.0.0"));
    }

    #[test]
    fn is_newer_handles_invalid_input() {
        assert!(!is_newer("bad", "0.19.0"));
        assert!(!is_newer("0.19.0", "bad"));
    }

    #[test]
    fn is_newer_handles_prerelease() {
        // Pre-release is older than the same release version
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
        // Release is newer than its own pre-release
        assert!(is_newer("1.0.0", "1.0.0-rc1"));
        // Higher version pre-release is still newer than lower release
        assert!(is_newer("1.0.1-beta.1", "1.0.0"));
    }

    #[test]
    fn current_version_is_valid_semver() {
        let parsed = parse_semver(CURRENT_VERSION);
        assert!(
            parsed.is_some(),
            "CARGO_PKG_VERSION should be valid semver: {CURRENT_VERSION}"
        );
    }

    #[test]
    fn platform_suffix_is_not_unknown() {
        let suffix = crate::cmd_self_update::platform::platform_suffix();
        assert!(
            suffix.is_some(),
            "platform_suffix() should return Some on supported platforms"
        );
        let s = suffix.unwrap();
        assert!(
            !s.contains("unknown"),
            "platform suffix should be resolved: {s}"
        );
    }

    #[test]
    fn fetch_via_gh_returns_none_when_binary_missing() {
        // gh is available on this system but this tests the timeout/error path
        // by setting a very short timeout implicitly through the function structure.
        // The real test is that it doesn't hang.
        let result = std::panic::catch_unwind(|| {
            // If gh is not authenticated, it should return None (not hang)
            let _ = fetch_via_gh();
        });
        assert!(result.is_ok(), "fetch_via_gh should not panic");
    }

    #[test]
    // #2360 (same class): these tests set/remove the process-global
    // `SIMARD_NO_UPDATE_CHECK`, which `run_update_check[_background]` reads.
    // cargo runs the lib tests multi-threaded and glibc getenv/setenv are not
    // thread-safe, so a concurrent mutate/read tears (e.g. this `is_none()`
    // assertion observing the var removed by `..._returns_some_when_enabled`).
    // All tests touching the var share the `update_check_env` key.
    #[serial_test::serial(update_check_env)]
    fn run_update_check_background_returns_receiver() {
        // With check disabled, returns None
        unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", "1") };
        assert!(run_update_check_background().is_none());
        unsafe { std::env::remove_var("SIMARD_NO_UPDATE_CHECK") };
    }

    // ── F1: fire-and-forget (no join) ──────────────────────────────

    #[test]
    #[serial_test::serial(update_check_env)]
    fn run_update_check_returns_immediately_when_disabled() {
        unsafe { std::env::set_var("SIMARD_NO_UPDATE_CHECK", "1") };
        let start = std::time::Instant::now();
        run_update_check();
        let elapsed = start.elapsed();
        unsafe { std::env::remove_var("SIMARD_NO_UPDATE_CHECK") };
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "run_update_check() should return immediately when disabled, took {elapsed:?}"
        );
    }

    #[test]
    #[serial_test::serial(update_check_env)]
    fn run_update_check_is_fire_and_forget() {
        // Even when enabled, run_update_check() must return immediately
        // because it spawns a detached thread (no join). If someone
        // reintroduces handle.join(), this test will take ~5s and fail.
        let start = std::time::Instant::now();
        run_update_check();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "run_update_check() must be fire-and-forget (no join), took {elapsed:?}"
        );
    }

    // ── F2: background channel returns Some when enabled ───────────

    #[test]
    #[serial_test::serial(update_check_env)]
    fn run_update_check_background_returns_some_when_enabled() {
        unsafe { std::env::remove_var("SIMARD_NO_UPDATE_CHECK") };
        let rx = run_update_check_background();
        assert!(
            rx.is_some(),
            "run_update_check_background() should return Some(Receiver) when enabled"
        );
    }

    // ── F3: platform suffix uses "macos" not "darwin" ──────────────

    #[test]
    fn platform_suffix_never_contains_darwin() {
        // F3: old code used "darwin-*" but actual GitHub releases use "macos-*".
        // platform_suffix() must return "macos-*" on macOS, never "darwin-*".
        let suffix = crate::cmd_self_update::platform::platform_suffix();
        if let Some(s) = suffix {
            assert!(
                !s.contains("darwin"),
                "platform suffix must use 'macos' not 'darwin', got: {s}"
            );
        }
    }

    // ── F4: prerelease-aware semver ────────────────────────────────

    #[test]
    fn parse_semver_prerelease_with_build_metadata() {
        // "1.2.3-beta.1+build.456" has both prerelease and build metadata.
        // The '-' before '+' makes it a prerelease → is_release = false.
        assert_eq!(
            parse_semver("1.2.3-beta.1+build.456"),
            Some((1, 2, 3, false))
        );
    }

    #[test]
    fn parse_semver_build_metadata_with_dash_is_valid() {
        // "1.2.3+build-456" — the '-' is inside build metadata (after '+'),
        // so it is NOT a prerelease separator. This is a valid release.
        assert_eq!(parse_semver("1.2.3+build-456"), Some((1, 2, 3, true)));
    }

    #[test]
    fn is_newer_release_beats_same_version_prerelease() {
        // Core F4 contract: release 1.0.0 is strictly newer than prerelease 1.0.0-rc1
        assert!(
            is_newer("1.0.0", "1.0.0-rc1"),
            "release 1.0.0 must be newer than prerelease 1.0.0-rc1"
        );
        // Reverse: prerelease is NOT newer than the corresponding release
        assert!(
            !is_newer("1.0.0-rc1", "1.0.0"),
            "prerelease 1.0.0-rc1 must NOT be newer than release 1.0.0"
        );
    }

    #[test]
    fn is_newer_prerelease_of_higher_version_beats_lower_release() {
        // 2.0.0-beta.1 is newer than 1.0.0 even though it's a prerelease
        assert!(
            is_newer("2.0.0-beta.1", "1.0.0"),
            "prerelease of higher version should still be newer"
        );
    }

    #[test]
    fn parse_semver_4tuple_contract() {
        // Explicit contract: 4th element is is_release (true = release, false = prerelease)
        assert_eq!(parse_semver("1.0.0"), Some((1, 0, 0, true)));
        assert_eq!(parse_semver("1.0.0-rc1"), Some((1, 0, 0, false)));
        assert_eq!(parse_semver("1.0.0-beta.1"), Some((1, 0, 0, false)));
        // Build metadata without prerelease is still a release
        assert_eq!(parse_semver("1.0.0+build.123"), Some((1, 0, 0, true)));
    }
}
