//! Binary download, extraction, and replacement logic.

use std::fs;
use std::path::{Path, PathBuf};

use super::platform::CURRENT_VERSION;

/// Download and extract the binary, replacing the current executable.
pub(crate) fn download_and_replace(
    url: &str,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current executable: {e}"))?;
    let tmp_dir = std::env::temp_dir().join(format!("simard-update-{}", std::process::id()));
    fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join("simard.tar.gz");

    println!("Downloading simard v{version}...");

    let archive_str = archive_path.to_str().unwrap_or("simard.tar.gz");
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
            archive_path.to_str().ok_or_else(|| {
                format!(
                    "archive path is not valid UTF-8: {}",
                    archive_path.display()
                )
            })?,
            "-C",
            tmp_dir.to_str().ok_or_else(|| {
                format!("temp dir path is not valid UTF-8: {}", tmp_dir.display())
            })?,
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

/// Find the simard binary in an extracted directory tree (max depth 3).
pub(crate) fn find_binary_in_dir(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use std::ffi::OsStr;
    fn search(dir: &Path, depth: u32) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- find_binary_in_dir --

    #[test]
    fn find_binary_in_dir_finds_at_root() {
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("simard");
        fs::write(&bin_path, "fake-binary").unwrap();
        let found = find_binary_in_dir(dir.path()).unwrap();
        assert_eq!(found, bin_path);
    }

    #[test]
    fn find_binary_in_dir_finds_nested() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        let bin_path = sub.join("simard");
        fs::write(&bin_path, "fake-binary").unwrap();
        let found = find_binary_in_dir(dir.path()).unwrap();
        assert_eq!(found, bin_path);
    }

    #[test]
    fn find_binary_in_dir_finds_deeply_nested() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        fs::create_dir_all(&deep).unwrap();
        let bin_path = deep.join("simard");
        fs::write(&bin_path, "fake-binary").unwrap();
        let found = find_binary_in_dir(dir.path()).unwrap();
        assert_eq!(found, bin_path);
    }

    #[test]
    fn find_binary_in_dir_errors_when_too_deep() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a").join("b").join("c").join("d");
        fs::create_dir_all(&deep).unwrap();
        let bin_path = deep.join("simard");
        fs::write(&bin_path, "fake-binary").unwrap();
        let result = find_binary_in_dir(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn find_binary_in_dir_errors_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("not_simard"), "nope").unwrap();
        let result = find_binary_in_dir(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn find_binary_in_dir_ignores_directories_named_simard() {
        let dir = tempfile::tempdir().unwrap();
        let fake_dir = dir.path().join("simard");
        fs::create_dir(&fake_dir).unwrap();
        let result = find_binary_in_dir(dir.path());
        assert!(result.is_err());
    }
}
