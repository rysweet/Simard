//! High-level self-update and self-test commands.

use std::path::Path;

use super::download::download_and_replace;
use super::platform::CURRENT_VERSION;
use super::release::find_latest_release;

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

    #[test]
    fn current_version_is_set() {
        // CURRENT_VERSION comes from Cargo.toml via env!("CARGO_PKG_VERSION")
        assert!(!CURRENT_VERSION.is_empty());
    }

    #[test]
    fn run_self_test_on_nonexistent_binary_returns_error() {
        let result = run_self_test_on_binary(Path::new("/nonexistent/binary"));
        assert!(result.is_err());
    }

    #[test]
    fn run_self_test_on_binary_with_failing_command() {
        // /usr/bin/false always exits with 1
        let result = run_self_test_on_binary(Path::new("/usr/bin/false"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Self-test failed"));
    }
}
