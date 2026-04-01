//! Self-update command: downloads the latest simard binary from GitHub Releases.

use std::fs;
use std::path::PathBuf;

const GITHUB_REPO: &str = "rysweet/Simard";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Platform suffix for GitHub Release assets.
fn platform_suffix() -> Option<&'static str> {
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("linux-x86_64")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        Some("linux-aarch64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        Some("macos-x86_64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some("macos-aarch64")
    } else if cfg!(target_os = "windows") {
        Some("windows-x86_64")
    } else {
        None
    }
}

/// Query GitHub API for the latest release.
/// Returns (download_url, version) or error.
fn find_latest_release() -> Result<(String, String), Box<dyn std::error::Error>> {
    let suffix = platform_suffix().ok_or("Unsupported platform for self-update")?;

    // Try gh CLI first (authenticated, no rate limits), fall back to curl
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

/// Download and extract the binary, replacing the current executable.
fn download_and_replace(url: &str, version: &str) -> Result<(), Box<dyn std::error::Error>> {
    let current_exe = std::env::current_exe().map_err(|e| format!("Cannot determine current executable: {e}"))?;
    let tmp_dir = std::env::temp_dir().join(format!("simard-update-{}", std::process::id()));
    fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join("simard.tar.gz");

    println!("Downloading simard v{version}...");

    let archive_str = archive_path.to_str().unwrap();
    let mut last_err = String::from("Download failed");
    let mut downloaded = false;
    for attempt in 0..3u32 {
        if attempt > 0 {
            let delay = 1u64 << attempt; // 2s, 4s
            println!("Retrying download (attempt {}/3, waiting {delay}s)...", attempt + 1);
            std::thread::sleep(std::time::Duration::from_secs(delay));
        }
        match std::process::Command::new("curl")
            .args([
                "-sS",
                "-L",
                "--connect-timeout",
                "15",
                "--max-time",
                "120",
                "--retry",
                "2",
                "-o",
                archive_str,
                url,
            ])
            .status()
        {
            Ok(status) if status.success() => {
                downloaded = true;
                break;
            }
            Ok(status) => {
                last_err = format!("curl exited with status {status}");
            }
            Err(e) => {
                last_err = format!("Failed to run curl: {e}");
            }
        }
    }

    if !downloaded {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(format!("Download failed after 3 attempts: {last_err}").into());
    }

    println!("Extracting...");

    let tar_status = std::process::Command::new("tar")
        .args([
            "xzf",
            archive_path.to_str().unwrap(),
            "-C",
            tmp_dir.to_str().unwrap(),
        ])
        .status()
        .map_err(|e| format!("Failed to extract archive: {e}"))?;

    if !tar_status.success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err("Extraction failed".into());
    }

    let new_bin = find_binary_in_dir(&tmp_dir)?;

    // Replace current binary
    println!("Replacing binary...");
    let backup = current_exe.with_extension("old");
    if backup.exists() {
        let _ = fs::remove_file(&backup);
    }
    fs::rename(&current_exe, &backup)
        .map_err(|e| format!("Failed to backup current binary (try running with sudo): {e}"))?;

    fs::copy(&new_bin, &current_exe)
        .map_err(|e| format!("Failed to install new binary: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&current_exe, fs::Permissions::from_mode(0o755))?;
    }

    // Clean up
    let _ = fs::remove_file(&backup);
    let _ = fs::remove_dir_all(&tmp_dir);

    println!("Updated simard: {CURRENT_VERSION} → {version}");
    Ok(())
}

/// Find the simard binary in an extracted directory tree (max depth 3).
fn find_binary_in_dir(dir: &std::path::Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fn search(dir: &std::path::Path, depth: u32) -> Option<PathBuf> {
        if depth > 3 {
            return None;
        }
        let entries = fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_file() && name == "simard" {
                return Some(path);
            }
            if path.is_dir() {
                if let Some(found) = search(&path, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    search(dir, 0).ok_or_else(|| "Binary 'simard' not found in downloaded archive".into())
}

/// Run the self-update flow.
pub fn handle_self_update() -> Result<(), Box<dyn std::error::Error>> {
    println!("simard self-update (current: v{CURRENT_VERSION})");

    let (url, version) = find_latest_release()?;

    if version == CURRENT_VERSION {
        println!("Already at the latest version (v{CURRENT_VERSION}).");
        return Ok(());
    }

    println!("New version available: v{CURRENT_VERSION} → v{version}");
    download_and_replace(&url, &version)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_platform_suffix_not_none() {
        // On any CI/dev platform we support, this should return Some
        assert!(platform_suffix().is_some());
    }

    #[test]
    fn test_platform_suffix_contains_os_and_arch() {
        let suffix = platform_suffix().unwrap();
        // Must follow pattern: {os}-{arch}
        assert!(suffix.contains('-'), "suffix should be os-arch: {suffix}");
        let parts: Vec<&str> = suffix.split('-').collect();
        assert_eq!(parts.len(), 2);
        assert!(
            ["linux", "macos", "windows"].contains(&parts[0]),
            "unexpected OS: {}",
            parts[0]
        );
        assert!(
            ["x86_64", "aarch64"].contains(&parts[1]),
            "unexpected arch: {}",
            parts[1]
        );
    }

    #[test]
    fn test_current_version_format() {
        // Version should be semver-like (x.y.z)
        assert!(CURRENT_VERSION.contains('.'));
        assert!(!CURRENT_VERSION.is_empty());
        let parts: Vec<&str> = CURRENT_VERSION.split('.').collect();
        assert!(parts.len() >= 2, "version should have at least major.minor");
        for part in &parts {
            assert!(
                part.parse::<u32>().is_ok(),
                "version component '{}' should be numeric",
                part
            );
        }
    }

    #[test]
    fn test_github_repo_constant() {
        assert_eq!(GITHUB_REPO, "rysweet/Simard");
    }

    #[test]
    fn test_find_binary_in_flat_dir() {
        let tmp = std::env::temp_dir().join(format!("simard-test-flat-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let bin_path = tmp.join("simard");
        fs::write(&bin_path, b"fake-binary").unwrap();

        let found = find_binary_in_dir(&tmp).unwrap();
        assert_eq!(found, bin_path);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_in_nested_dir() {
        let tmp = std::env::temp_dir().join(format!("simard-test-nested-{}", std::process::id()));
        let nested = tmp.join("subdir");
        fs::create_dir_all(&nested).unwrap();
        let bin_path = nested.join("simard");
        fs::write(&bin_path, b"fake-binary").unwrap();

        let found = find_binary_in_dir(&tmp).unwrap();
        assert_eq!(found, bin_path);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_missing_returns_error() {
        let tmp = std::env::temp_dir().join(format!("simard-test-empty-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        // Write a file with wrong name
        fs::write(tmp.join("not-simard"), b"wrong").unwrap();

        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found in downloaded archive")
        );

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_respects_depth_limit() {
        let tmp = std::env::temp_dir().join(format!("simard-test-deep-{}", std::process::id()));
        // Create binary at depth 5 (beyond limit of 3)
        let deep = tmp.join("a").join("b").join("c").join("d").join("e");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("simard"), b"fake-binary").unwrap();

        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err(), "should not find binary beyond depth 3");

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_ignores_directories_named_simard() {
        let tmp =
            std::env::temp_dir().join(format!("simard-test-dirname-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        // Create a directory named "simard" (not a file)
        fs::create_dir_all(tmp.join("simard")).unwrap();

        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err(), "should not match directory named simard");

        fs::remove_dir_all(&tmp).unwrap();
    }
}
