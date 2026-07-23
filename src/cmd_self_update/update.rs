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
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            eprintln!("{stderr}");
        }
        eprintln!("SELF-TEST FAILED");
        Err("SELF-TEST FAILED: gym run-suite starter did not pass".into())
    }
}

/// Run the self-update flow: download -> self-test -> relaunch.
pub fn handle_self_update() -> Result<(), Box<dyn std::error::Error>> {
    println!("simard self-update (current: v{CURRENT_VERSION})");

    let (url, version) = find_latest_release()?;

    if !crate::update_check::is_newer(CURRENT_VERSION, &version) {
        println!("Already at the latest version (v{CURRENT_VERSION}).");
        return Ok(());
    }

    println!("New version available: v{CURRENT_VERSION} → v{version}");
    let report = download_and_replace(&url, &version)?;

    // Surface the full-binary-set outcome. The main binary is guaranteed
    // installed here (a main failure aborts inside download_and_replace);
    // auxiliary failures are non-fatal and only warned about.
    if report.aux_installed.is_empty() {
        println!("Updated binaries: simard");
    } else {
        println!(
            "Updated binaries: simard, {}",
            report.aux_installed.join(", ")
        );
    }
    if !report.aux_failed.is_empty() {
        eprintln!(
            "WARNING: {} auxiliary binary(ies) could not be updated (the core update still succeeded):",
            report.aux_failed.len()
        );
        for (name, reason) in &report.aux_failed {
            eprintln!("  - {name}: {reason}");
        }
    }

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

/// Download-only variant used by `simard safe-update`. Returns the path
/// to the freshly extracted candidate binary, or `None` if the running
/// version is already the latest. The caller is responsible for moving
/// the binary into the install location after pre-test passes.
pub fn handle_self_update_download_only()
-> Result<Option<std::path::PathBuf>, Box<dyn std::error::Error>> {
    println!("simard safe-update (current: v{CURRENT_VERSION})");
    let (url, version) = find_latest_release()?;
    if !crate::update_check::is_newer(CURRENT_VERSION, &version) {
        return Ok(None);
    }
    println!("New version available: v{CURRENT_VERSION} → v{version}");
    let bin = super::download::download_to_temp(&url, &version)?;
    Ok(Some(bin))
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

    #[test]
    fn run_self_test_on_binary_with_passing_command() {
        // /usr/bin/true always exits 0, so the binary is treated as healthy.
        let result = run_self_test_on_binary(Path::new("/usr/bin/true"));
        assert!(
            result.is_ok(),
            "a zero-exit self-test must be healthy: {result:?}"
        );
    }

    /// Full false-green chain regression (issue #2548): a binary whose starter
    /// suite fails now exits non-zero from `gym run-suite`, so its `self-test`
    /// exits non-zero, so `run_self_test_on_binary` — the exact gate
    /// `handle_self_update` branches on before relaunching — returns `Err` and
    /// the relaunch is refused. This fabricates such a binary as a tiny script
    /// that mimics `simard self-test` printing `Suite passed: false` and exiting
    /// non-zero, and asserts the gate rejects it.
    #[test]
    fn self_update_relaunch_gate_rejects_failing_self_test() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("simard-2548-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let script = dir.join("fake-simard-failing");
        {
            let mut f = std::fs::File::create(&script).expect("create script");
            writeln!(
                f,
                "#!/bin/sh\necho 'Suite: starter'\necho 'Suite passed: false'\necho 'SELF-TEST FAILED' 1>&2\nexit 1"
            )
            .expect("write script");
        }
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");

        let result = run_self_test_on_binary(&script);
        let _ = std::fs::remove_dir_all(&dir);

        let err = result.expect_err("a binary whose self-test fails must not be relaunched");
        let msg = err.to_string();
        assert!(
            msg.contains("Self-test failed"),
            "gate error should be explicit: {msg}"
        );
        // The captured suite diagnostics are surfaced for the operator.
        assert!(
            msg.contains("Suite passed: false"),
            "gate should surface the failing suite output: {msg}"
        );
    }
}
