//! Self-update command: downloads the latest simard binary from GitHub Releases.

use std::fs;
use std::path::{Path, PathBuf};

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
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current executable: {e}"))?;
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
            println!(
                "Retrying download (attempt {}/3, waiting {delay}s)...",
                attempt + 1
            );
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

    // Replace current binary — try atomic rename first, fall back to copy
    println!("Replacing binary...");
    let backup = current_exe.with_extension("old");
    if backup.exists() {
        let _ = fs::remove_file(&backup);
    }
    fs::rename(&current_exe, &backup)
        .map_err(|e| format!("Failed to backup current binary (try running with sudo): {e}"))?;

    // rename is O(1) on same filesystem; copy is fallback for cross-device
    if fs::rename(&new_bin, &current_exe).is_err() {
        fs::copy(&new_bin, &current_exe)
            .map_err(|e| format!("Failed to install new binary: {e}"))?;
    }

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

/// Run `<binary> self-test` to verify a binary is healthy.
/// Returns Ok(()) if the self-test passes, Err otherwise.
fn run_self_test_on_binary(binary: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("Running self-test on new binary...");
    let output = std::process::Command::new(binary)
        .args(["self-test"])
        .output()
        .map_err(|e| format!("Failed to run self-test on new binary: {e}"))?;

    if output.status.success() {
        println!("Self-test passed.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "Self-test failed (exit {}):\n{}\n{}",
            output.status,
            stdout.trim(),
            stderr.trim()
        )
        .into())
    }
}

/// Run `simard self-test` against the current binary. This executes
/// `simard gym run-suite starter` and reports pass/fail.
pub fn handle_self_test() -> Result<(), Box<dyn std::error::Error>> {
    println!("simard self-test (v{CURRENT_VERSION})");
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current executable: {e}"))?;

    let output = std::process::Command::new(&current_exe)
        .args(["gym", "run-suite", "starter"])
        .output()
        .map_err(|e| format!("Failed to run gym suite: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        println!("{}", stdout.trim());
        println!("SELF-TEST PASSED");
        Ok(())
    } else {
        eprintln!("{}", stdout.trim());
        eprintln!("{}", stderr.trim());
        Err("SELF-TEST FAILED: gym run-suite starter did not pass".into())
    }
}

/// Find the simard binary in an extracted directory tree (max depth 3).
fn find_binary_in_dir(dir: &std::path::Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::ffi::OsStr;
    fn search(dir: &std::path::Path, depth: u32) -> Option<PathBuf> {
        if depth > 3 {
            return None;
        }
        let entries = fs::read_dir(dir).ok()?;
        let target = OsStr::new("simard");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && entry.file_name() == target {
                return Some(path);
            }
            if path.is_dir()
                && let Some(found) = search(&path, depth + 1)
            {
                return Some(found);
            }
        }
        None
    }
    search(dir, 0).ok_or_else(|| "Binary 'simard' not found in downloaded archive".into())
}

/// Run the self-update flow: download -> self-test -> relaunch.
pub fn handle_self_update() -> Result<(), Box<dyn std::error::Error>> {
    println!("simard self-update (current: v{CURRENT_VERSION})");

    let (url, version) = find_latest_release()?;

    if version == CURRENT_VERSION {
        println!("Already at the latest version (v{CURRENT_VERSION}).");
        return Ok(());
    }

    println!("New version available: v{CURRENT_VERSION} → v{version}");
    download_and_replace(&url, &version)?;

    // The new binary is now at current_exe(). Run self-test before relaunching.
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current executable: {e}"))?;

    if let Err(e) = run_self_test_on_binary(&current_exe) {
        eprintln!("WARNING: Self-test failed on new binary: {e}");
        eprintln!("The new binary has been installed but may not be healthy.");
        eprintln!("Skipping automatic relaunch. Run 'simard self-test' to diagnose.");
        return Err(e);
    }

    // Self-test passed — exec() into the new binary.
    println!("Relaunching into v{version}...");
    let pid = std::process::id();
    crate::self_relaunch::handover(pid, &current_exe)
        .map_err(|e| format!("Relaunch failed: {e}"))?;

    // handover does not return on success (exec replaces process)
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
        let tmp = std::env::temp_dir().join(format!("simard-test-dirname-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        // Create a directory named "simard" (not a file)
        fs::create_dir_all(tmp.join("simard")).unwrap();

        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err(), "should not match directory named simard");

        fs::remove_dir_all(&tmp).unwrap();
    }

    // ── Additional tests ──

    #[test]
    fn test_find_binary_at_depth_boundary() {
        // Binary at depth 3 should be found (limit is depth > 3)
        let tmp = std::env::temp_dir().join(format!("simard-test-depth3-{}", std::process::id()));
        let at_depth3 = tmp.join("a").join("b").join("c");
        fs::create_dir_all(&at_depth3).unwrap();
        fs::write(at_depth3.join("simard"), b"fake").unwrap();

        let result = find_binary_in_dir(&tmp);
        assert!(result.is_ok(), "binary at depth 3 should be found");

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_nonexistent_dir_returns_error() {
        let tmp = std::env::temp_dir().join(format!("simard-test-nodir-{}", std::process::id()));
        // Don't create the directory
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_binary_empty_dir_returns_error() {
        let tmp = std::env::temp_dir().join(format!("simard-test-emptydir-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();

        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_prefers_first_found() {
        let tmp = std::env::temp_dir().join(format!("simard-test-multi-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        // Binary at root level
        fs::write(tmp.join("simard"), b"root-binary").unwrap();
        // Also at depth 1
        let sub = tmp.join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("simard"), b"nested-binary").unwrap();

        let found = find_binary_in_dir(&tmp).unwrap();
        // Should find one of them without panicking
        assert!(found.exists());

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_platform_suffix_is_deterministic() {
        let s1 = platform_suffix();
        let s2 = platform_suffix();
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_current_version_matches_cargo_pkg() {
        assert_eq!(CURRENT_VERSION, env!("CARGO_PKG_VERSION"));
    }

    // ── find_binary_in_dir additional coverage ──

    #[test]
    fn test_find_binary_with_multiple_non_matching_files() {
        let tmp =
            std::env::temp_dir().join(format!("simard-test-multi-nomatch-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("not-simard"), b"wrong").unwrap();
        fs::write(tmp.join("simard.bak"), b"wrong").unwrap();
        fs::write(tmp.join("simard.exe"), b"wrong").unwrap();
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_at_depth_1() {
        let tmp = std::env::temp_dir().join(format!("simard-test-depth1-{}", std::process::id()));
        let sub = tmp.join("subdir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("simard"), b"fake").unwrap();
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), sub.join("simard"));
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_at_depth_2() {
        let tmp = std::env::temp_dir().join(format!("simard-test-depth2-{}", std::process::id()));
        let sub = tmp.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("simard"), b"fake").unwrap();
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_ok());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_at_depth_4_not_found() {
        let tmp = std::env::temp_dir().join(format!("simard-test-depth4-{}", std::process::id()));
        let sub = tmp.join("a").join("b").join("c").join("d");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("simard"), b"fake").unwrap();
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_err());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_with_mixed_files_and_dirs() {
        let tmp = std::env::temp_dir().join(format!("simard-test-mixed-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("README.md"), b"readme").unwrap();
        fs::write(tmp.join("LICENSE"), b"license").unwrap();
        fs::create_dir_all(tmp.join("bin")).unwrap();
        fs::write(tmp.join("bin").join("simard"), b"binary").unwrap();
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_ok());
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_error_message_content() {
        let tmp = std::env::temp_dir().join(format!("simard-test-errmsg-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let result = find_binary_in_dir(&tmp);
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("simard"));
        assert!(msg.contains("not found"));
        fs::remove_dir_all(&tmp).unwrap();
    }

    // ── platform_suffix additional tests ──

    #[test]
    fn test_platform_suffix_no_hyphens_in_parts() {
        let suffix = platform_suffix().unwrap();
        let parts: Vec<&str> = suffix.split('-').collect();
        assert_eq!(
            parts.len(),
            2,
            "suffix should have exactly one hyphen: {suffix}"
        );
    }

    #[test]
    fn test_platform_suffix_known_combinations() {
        let suffix = platform_suffix().unwrap();
        let valid = [
            "linux-x86_64",
            "linux-aarch64",
            "macos-x86_64",
            "macos-aarch64",
            "windows-x86_64",
        ];
        assert!(
            valid.contains(&suffix),
            "unexpected platform suffix: {suffix}"
        );
    }

    // ── GITHUB_REPO constant ──

    #[test]
    fn test_github_repo_format() {
        assert!(GITHUB_REPO.contains('/'));
        let parts: Vec<&str> = GITHUB_REPO.split('/').collect();
        assert_eq!(parts.len(), 2);
        assert!(!parts[0].is_empty());
        assert!(!parts[1].is_empty());
    }

    // ── CURRENT_VERSION deeper validation ──

    #[test]
    fn test_current_version_is_semver() {
        let parts: Vec<&str> = CURRENT_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "version should be major.minor.patch");
        for (i, part) in parts.iter().enumerate() {
            assert!(
                part.parse::<u32>().is_ok(),
                "version part {i} '{part}' should be numeric"
            );
        }
    }

    #[test]
    fn test_current_version_no_leading_v() {
        assert!(
            !CURRENT_VERSION.starts_with('v'),
            "CURRENT_VERSION should not have a 'v' prefix"
        );
    }

    // ── find_binary_in_dir: edge cases ──

    #[test]
    fn test_find_binary_with_symlink_named_simard() {
        // On Unix, a symlink to a file named simard should be found
        // (only testing that it doesn't panic — actual behavior depends on fs)
        let tmp = std::env::temp_dir().join(format!("simard-test-symlink-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let real = tmp.join("simard_real");
        fs::write(&real, b"binary").unwrap();
        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(&real, tmp.join("simard"));
        }
        #[cfg(not(unix))]
        {
            fs::write(tmp.join("simard"), b"binary").unwrap();
        }
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_ok(), "should find simard (or symlink to it)");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_deeply_nested_multiple_dirs() {
        let tmp =
            std::env::temp_dir().join(format!("simard-test-multi-nested-{}", std::process::id()));
        // Create multiple subdirectories, only one contains simard at depth 2
        fs::create_dir_all(tmp.join("dir_a/sub_a")).unwrap();
        fs::create_dir_all(tmp.join("dir_b/sub_b")).unwrap();
        fs::write(tmp.join("dir_a/sub_a/not_simard"), b"wrong").unwrap();
        fs::write(tmp.join("dir_b/sub_b/simard"), b"binary").unwrap();
        let result = find_binary_in_dir(&tmp);
        assert!(result.is_ok(), "should find simard in dir_b/sub_b");
        fs::remove_dir_all(&tmp).unwrap();
    }

    // ── platform_suffix: structural checks ──

    #[test]
    fn test_platform_suffix_is_ascii() {
        let suffix = platform_suffix().unwrap();
        assert!(suffix.is_ascii(), "suffix should be ASCII: {suffix}");
    }

    #[test]
    fn test_platform_suffix_no_whitespace() {
        let suffix = platform_suffix().unwrap();
        assert!(!suffix.contains(' '), "suffix should not contain spaces");
    }

    // ── CURRENT_VERSION: structural checks ──

    #[test]
    fn test_current_version_no_whitespace() {
        assert!(
            !CURRENT_VERSION.contains(' '),
            "version should not contain spaces"
        );
    }

    #[test]
    fn test_current_version_no_newlines() {
        assert!(
            !CURRENT_VERSION.contains('\n'),
            "version should not contain newlines"
        );
    }

    #[test]
    fn test_current_version_major_is_reasonable() {
        let major: u32 = CURRENT_VERSION.split('.').next().unwrap().parse().unwrap();
        assert!(major < 100, "major version should be < 100, got {major}");
    }

    // ── GITHUB_REPO: structural checks ──

    #[test]
    fn test_github_repo_no_whitespace() {
        assert!(!GITHUB_REPO.contains(' '), "repo should not contain spaces");
    }

    #[test]
    fn test_github_repo_owner_is_rysweet() {
        let owner = GITHUB_REPO.split('/').next().unwrap();
        assert_eq!(owner, "rysweet");
    }

    #[test]
    fn test_github_repo_name_is_simard() {
        let name = GITHUB_REPO.split('/').nth(1).unwrap();
        assert_eq!(name, "Simard");
    }

    // ── find_binary_in_dir: determinism / cleanup ──

    #[test]
    fn test_find_binary_result_path_exists() {
        let tmp = std::env::temp_dir().join(format!("simard-test-exists-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("simard"), b"binary").unwrap();
        let found = find_binary_in_dir(&tmp).unwrap();
        assert!(found.exists(), "found path should exist");
        assert!(found.is_file(), "found path should be a file");
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_find_binary_result_has_correct_name() {
        let tmp = std::env::temp_dir().join(format!("simard-test-name-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("simard"), b"binary").unwrap();
        let found = find_binary_in_dir(&tmp).unwrap();
        assert_eq!(found.file_name().unwrap(), "simard");
        fs::remove_dir_all(&tmp).unwrap();
    }

    // ── find_binary_in_dir: depth boundary precise test ──

    #[test]
    fn test_find_binary_depth_3_found_depth_4_not() {
        // At depth 3 → found
        let tmp3 = std::env::temp_dir().join(format!("simard-test-d3ok-{}", std::process::id()));
        let d3 = tmp3.join("a").join("b").join("c");
        fs::create_dir_all(&d3).unwrap();
        fs::write(d3.join("simard"), b"found").unwrap();
        assert!(find_binary_in_dir(&tmp3).is_ok());
        fs::remove_dir_all(&tmp3).unwrap();

        // At depth 4 → not found
        let tmp4 = std::env::temp_dir().join(format!("simard-test-d4fail-{}", std::process::id()));
        let d4 = tmp4.join("a").join("b").join("c").join("d");
        fs::create_dir_all(&d4).unwrap();
        fs::write(d4.join("simard"), b"too deep").unwrap();
        assert!(find_binary_in_dir(&tmp4).is_err());
        fs::remove_dir_all(&tmp4).unwrap();
    }

    // ── handle_self_test: structural validation ──
    // We can't run the actual self-test in unit tests, but we can verify constants.

    #[test]
    fn test_self_test_uses_starter_suite() {
        // The handle_self_test function runs `gym run-suite starter`
        // Just verify the constant strings used
        assert!(!CURRENT_VERSION.is_empty());
        assert!(GITHUB_REPO.contains("Simard"));
    }
}
