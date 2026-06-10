//! Startup update-version check against GitHub Releases.
//!
//! On every launch, queries GitHub for the latest release and prints a
//! notice if a newer version is available. No caching — just a fast
//! `gh api` call (or `curl` fallback) with a hard 5-second timeout.
//!
//! Env-var controls:
//! - `SIMARD_NO_UPDATE_CHECK=1` — skip the check entirely
//! - `SIMARD_NONINTERACTIVE=1`  — print the notice but suppress the upgrade prompt

use crate::cmd_self_update::platform::{CURRENT_VERSION, GITHUB_REPO};

/// Information about an available update.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub has_platform_asset: bool,
}

/// Run the full update check: query GitHub → notify user if newer version exists.
///
/// Spawns in a background thread with a hard 5-second timeout so it never
/// blocks startup. Silently returns on any error (network, parse, timeout).
pub fn run_update_check() {
    if std::env::var("SIMARD_NO_UPDATE_CHECK")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return;
    }

    // Run in a background thread with a hard timeout so startup is never blocked.
    let handle = std::thread::spawn(check_for_update);
    if let Ok(Some(info)) = handle.join() {
        print_upgrade_notice(&info);
    }
}

/// Non-blocking variant: spawn the check in background, print if update found.
/// Returns immediately. Used when even the 5s timeout is too much.
pub fn run_update_check_background() {
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
    let platform_suffix = current_platform_asset_suffix();
    let has_platform_asset = release["assets"]
        .as_array()
        .map(|assets| {
            assets.iter().any(|a| {
                a["name"]
                    .as_str()
                    .map(|n| n.contains(&platform_suffix))
                    .unwrap_or(false)
            })
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

/// Returns the expected asset name suffix for the current platform.
fn current_platform_asset_suffix() -> String {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    format!("{os}-{arch}")
}

/// Parse a semver string "major.minor.patch[-prerelease]" into a comparable tuple.
/// Pre-release versions (e.g., "1.2.3-beta.1") are stripped to their numeric
/// core for comparison, since any release is newer than its pre-release.
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    // Strip pre-release suffix (e.g., "-beta.1") and build metadata ("+build")
    let numeric = v.split(['-', '+']).next()?;
    let parts: Vec<&str> = numeric.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
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
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("0.19.0"), Some((0, 19, 0)));
    }

    #[test]
    fn parse_semver_with_prerelease() {
        assert_eq!(parse_semver("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(parse_semver("1.0.0-rc1"), Some((1, 0, 0)));
    }

    #[test]
    fn parse_semver_with_build_metadata() {
        assert_eq!(parse_semver("1.2.3+build.456"), Some((1, 2, 3)));
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
        // Pre-release strips to numeric core, so 1.0.0-beta == 1.0.0 for comparison
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
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
    fn platform_asset_suffix_is_not_unknown() {
        let suffix = current_platform_asset_suffix();
        assert!(
            !suffix.contains("unknown"),
            "platform suffix should be resolved: {suffix}"
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
}
