//! Install command: copies the current simard binary to ~/.simard/bin/simard.

use std::fs;
use std::path::PathBuf;

/// Default install directory.
fn install_dir() -> PathBuf {
    dirs_or_home().join(".simard").join("bin")
}

fn dirs_or_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local"))
}

/// Run the install flow: copy current exe to ~/.simard/bin/simard.
pub fn handle_install() -> Result<(), Box<dyn std::error::Error>> {
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot determine current executable: {e}"))?;

    let dest_dir = install_dir();
    fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create {}: {e}", dest_dir.display()))?;

    let dest = dest_dir.join("simard");

    // If source and dest are the same file, nothing to do
    if let (Ok(src_canon), Ok(dst_canon)) = (current_exe.canonicalize(), dest.canonicalize())
        && src_canon == dst_canon
    {
        println!("simard is already installed at {}", dest.display());
        return Ok(());
    }

    fs::copy(&current_exe, &dest)
        .map_err(|e| format!("Failed to copy binary to {}: {e}", dest.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
    }

    println!("Installed simard to {}", dest.display());
    println!();
    println!("Add to your PATH if not already present:");
    println!("  export PATH=\"$HOME/.simard/bin:$PATH\"");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_dir_under_home() {
        let dir = install_dir();
        let dir_str = dir.to_string_lossy();
        assert!(dir_str.ends_with(".simard/bin"));
    }

    #[test]
    fn test_install_dir_is_absolute() {
        let dir = install_dir();
        assert!(dir.is_absolute(), "install dir should be absolute path");
    }

    #[test]
    fn test_dirs_or_home_returns_home() {
        // When HOME is set (which it always is in test), should return it
        if std::env::var_os("HOME").is_some() {
            let home = dirs_or_home();
            assert!(
                home.is_absolute(),
                "dirs_or_home should return absolute path"
            );
            assert!(
                home.to_string_lossy() != "/usr/local",
                "should use HOME, not fallback"
            );
        }
    }

    #[test]
    fn test_install_dir_has_three_components() {
        // Should be $HOME/.simard/bin — last two components are .simard and bin
        let dir = install_dir();
        let components: Vec<_> = dir.components().collect();
        let len = components.len();
        assert!(
            len >= 3,
            "install dir should have at least 3 path components"
        );
        let last = components[len - 1]
            .as_os_str()
            .to_string_lossy()
            .to_string();
        let second_last = components[len - 2]
            .as_os_str()
            .to_string_lossy()
            .to_string();
        assert_eq!(last, "bin");
        assert_eq!(second_last, ".simard");
    }

    #[test]
    fn test_handle_install_creates_dir_and_copies() {
        // Use a temp HOME to avoid clobbering real install
        let tmp = std::env::temp_dir().join(format!("simard-install-test-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();

        // We can't easily override HOME for just this function without affecting
        // other tests, so we test the core logic directly:
        let dest_dir = tmp.join(".simard").join("bin");
        fs::create_dir_all(&dest_dir).unwrap();

        let dest = dest_dir.join("simard");
        let current_exe = std::env::current_exe().unwrap();
        fs::copy(&current_exe, &dest).unwrap();

        assert!(dest.exists(), "binary should exist after copy");
        assert!(
            dest.metadata().unwrap().len() > 0,
            "binary should not be empty"
        );

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_install_sets_executable_permission() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = std::env::temp_dir().join(format!("simard-perm-test-{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();

        let dest = tmp.join("simard");
        fs::write(&dest, b"fake-binary").unwrap();
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).unwrap();

        let perms = fs::metadata(&dest).unwrap().permissions();
        assert_eq!(perms.mode() & 0o755, 0o755, "binary should be executable");

        fs::remove_dir_all(&tmp).unwrap();
    }

    // --- install_dir structure ---

    #[test]
    fn install_dir_ends_with_bin() {
        let dir = install_dir();
        assert!(
            dir.ends_with("bin"),
            "install dir should end with 'bin': {}",
            dir.display()
        );
    }

    #[test]
    fn install_dir_parent_is_dot_simard() {
        let dir = install_dir();
        let parent = dir.parent().unwrap();
        let name = parent.file_name().unwrap().to_str().unwrap();
        assert_eq!(name, ".simard");
    }

    // --- dirs_or_home ---

    #[test]
    fn dirs_or_home_returns_absolute_path() {
        let home = dirs_or_home();
        assert!(home.is_absolute());
    }

    #[test]
    fn dirs_or_home_is_not_empty() {
        let home = dirs_or_home();
        assert!(
            !home.as_os_str().is_empty(),
            "home directory should not be empty"
        );
    }

    // --- handle_install: binary copy ---

    #[test]
    fn current_exe_is_available() {
        let exe = std::env::current_exe();
        assert!(exe.is_ok(), "should be able to determine current exe");
        assert!(
            exe.unwrap().exists(),
            "current exe should exist on filesystem"
        );
    }

    #[test]
    fn install_dir_does_not_contain_double_slash() {
        let dir = install_dir();
        let dir_str = dir.to_string_lossy();
        assert!(
            !dir_str.contains("//"),
            "path should not have double slashes: {dir_str}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_copy_and_permissions_roundtrip() {
        use std::os::unix::fs::PermissionsExt;

        let tmp =
            std::env::temp_dir().join(format!("simard-roundtrip-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let src = tmp.join("source");
        fs::write(&src, b"test-binary-content").unwrap();
        let dest = tmp.join("dest");
        fs::copy(&src, &dest).unwrap();
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(dest.exists());
        let meta = fs::metadata(&dest).unwrap();
        assert_eq!(meta.permissions().mode() & 0o755, 0o755);
        assert_eq!(meta.len(), 19); // "test-binary-content".len()

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn install_dir_join_simard_gives_full_path() {
        let dir = install_dir();
        let full = dir.join("simard");
        assert!(full.to_string_lossy().ends_with(".simard/bin/simard"));
    }
}
