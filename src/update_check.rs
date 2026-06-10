//! Startup update-version check against GitHub Releases.
//!
//! On every launch, queries GitHub for the latest release and prints a
//! notice if a newer version is available. No caching — just a fast
//! `gh api` call (or `curl` fallback).
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
}

/// Run the full update check: query GitHub → notify user if newer version exists.
///
/// Silently returns on any error (network, parse, etc.) so it never
/// blocks normal startup.
pub fn run_update_check() {
    if std::env::var("SIMARD_NO_UPDATE_CHECK")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return;
    }

    if let Some(info) = check_for_update() {
        print_upgrade_notice(&info);
    }
}

/// Check GitHub for a newer release.
/// Returns `Some(UpdateInfo)` if a newer version exists, `None` otherwise.
pub fn check_for_update() -> Option<UpdateInfo> {
    let (latest_version, release_url) = fetch_latest_version()?;

    if is_newer(&latest_version, CURRENT_VERSION) {
        Some(UpdateInfo {
            current_version: CURRENT_VERSION.to_string(),
            latest_version,
            release_url,
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
    eprintln!("  Run `simard self-update` to upgrade.\n");
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Query GitHub for the latest release tag and URL.
/// Uses `gh api` first (authenticated, no rate limits), falls back to `curl`.
fn fetch_latest_version() -> Option<(String, String)> {
    let output = std::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{GITHUB_REPO}/releases/latest"),
            "--jq",
            ".",
        ])
        .output()
        .or_else(|_| {
            std::process::Command::new("curl")
                .args([
                    "-sS",
                    "--connect-timeout",
                    "5",
                    "--max-time",
                    "10",
                    "-H",
                    "Accept: application/vnd.github+json",
                    &format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"),
                ])
                .output()
        })
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let release: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    let tag = release["tag_name"].as_str()?;
    let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
    let url = release["html_url"].as_str().unwrap_or("").to_string();

    Some((version, url))
}

/// Parse a semver string "major.minor.patch" into a comparable tuple.
fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() < 3 {
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
    fn parse_semver_invalid() {
        assert_eq!(parse_semver("not.a.version"), None);
        assert_eq!(parse_semver("1.2"), None);
        assert_eq!(parse_semver(""), None);
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
    fn current_version_is_valid_semver() {
        let parsed = parse_semver(CURRENT_VERSION);
        assert!(
            parsed.is_some(),
            "CARGO_PKG_VERSION should be valid semver: {CURRENT_VERSION}"
        );
    }
}
