//! GitHub Releases API query logic.

use super::platform::{GITHUB_REPO, platform_suffix};

/// Query GitHub API for the latest release.
/// Returns (download_url, version) or error.
pub(crate) fn find_latest_release() -> Result<(String, String), Box<dyn std::error::Error>> {
    let suffix = platform_suffix().ok_or("Unsupported platform for self-update")?;

    // Try gh CLI first (authenticated, no rate limits), then curl (unauthenticated)
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
                    "10",
                    "--max-time",
                    "30",
                    "-H",
                    "Accept: application/vnd.github+json",
                    &format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest"),
                ])
                .output()
        })
        .map_err(|e| format!("Failed to query GitHub releases (need gh or curl): {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "GitHub API request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let release: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse GitHub release JSON: {e}"))?;

    let tag = release["tag_name"]
        .as_str()
        .ok_or("Missing tag_name in release")?;

    let assets = release["assets"]
        .as_array()
        .ok_or("Missing assets in release")?;

    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        if name.contains(suffix) && name.ends_with(".tar.gz") {
            let dl_url = asset["browser_download_url"]
                .as_str()
                .ok_or("Missing download URL")?;
            let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
            return Ok((dl_url.to_string(), version));
        }
    }

    Err(format!("No release asset found for platform '{suffix}'").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_constant_is_set() {
        assert!(!GITHUB_REPO.is_empty());
        assert!(GITHUB_REPO.contains('/'));
    }

    #[test]
    fn platform_suffix_returns_some_on_known_platform() {
        let suffix = platform_suffix();
        assert!(suffix.is_some(), "expected a platform suffix on this host");
    }
}
