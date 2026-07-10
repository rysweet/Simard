//! Contract tests for the canonical `simard install` deployment rail.
//!
//! These tests intentionally exercise the real `simard` binary from the
//! outside. They use temporary install roots and a fake `systemctl`, so they
//! prove the installer contract without mutating host user services.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;
use tempfile::TempDir;

fn simard() -> Command {
    let mut command = Command::cargo_bin("simard").expect("simard binary must be buildable");
    command
        .timeout(Duration::from_secs(30))
        .env_remove("SIMARD_HOME")
        .env_remove("SIMARD_INSTALL_PROMPT_ASSETS_ROOT")
        .env_remove("SIMARD_PROMPT_ASSET_ROOT")
        .env_remove("SIMARD_PROMPT_ASSETS_DIR");
    command
}

#[cfg(unix)]
fn fake_systemctl(root: &Path) -> (PathBuf, PathBuf) {
    let bin = root.join("fake-systemctl");
    let log = root.join("systemctl.log");
    let script = format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display());
    fs::write(&bin, script).expect("fake systemctl should be writable");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755))
        .expect("fake systemctl should be executable");
    (bin, log)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"))
}

fn combined_output(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_file_contains(path: &Path, expected: &str) {
    let contents = read(path);
    assert!(
        contents.contains(expected),
        "expected {path:?} to contain {expected:?}; contents:\n{contents}"
    );
}

fn assert_systemctl_logged(log: &Path, expected_terms: &[&str]) {
    let contents = read(log);
    let matched = contents
        .lines()
        .any(|line| expected_terms.iter().all(|term| line.contains(term)));
    assert!(
        matched,
        "expected fake systemctl log {log:?} to contain one line with all terms {expected_terms:?}; log:\n{contents}"
    );
}

fn assert_no_systemctl_invocation(log: &Path) {
    assert!(
        !log.exists(),
        "systemctl must not be invoked on a failed or dry-run install; log:\n{}",
        read(log)
    );
}

fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        if !path.exists() {
            continue;
        }
        let metadata = fs::metadata(&path).expect("metadata should be readable");
        if metadata.is_file() {
            files.push(path);
        } else if metadata.is_dir() {
            for entry in fs::read_dir(&path).expect("directory should be readable") {
                stack.push(entry.expect("directory entry should be readable").path());
            }
        }
    }
    files.sort();
    files
}

fn assert_backup_contains(root: &Path, expected: &[u8]) {
    let files = collect_files(root);
    let matched = files.iter().any(|path| {
        fs::read(path)
            .map(|contents| contents == expected)
            .unwrap_or(false)
    });
    assert!(
        matched,
        "expected one backup under {root:?} to contain the prior binary bytes; files: {files:?}"
    );
}

#[test]
fn install_help_documents_the_canonical_installer_contract() {
    let assert = simard().args(["install", "--help"]).assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    for expected in [
        "--simard-home",
        "--dry-run",
        "--systemd-user-dir",
        "--systemctl",
        "SIMARD_HOME",
        "simard-ooda.service",
        "simard-signal.service",
        "prompt_assets",
    ] {
        assert!(
            stdout.contains(expected),
            "`simard install --help` should document {expected:?}; stdout:\n{stdout}"
        );
    }
}

#[cfg(unix)]
#[test]
fn install_defaults_simard_home_under_home_and_uses_fake_systemctl() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    simard()
        .args(["install", "--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .env("HOME", &fake_home)
        .assert()
        .success();

    let simard_home = fake_home.join(".simard");
    assert!(
        simard_home.join("bin/simard").is_file(),
        "installer should place the live binary under the default $HOME/.simard"
    );
    assert!(unit_dir.join("simard-ooda.service").is_file());
    assert!(unit_dir.join("simard-signal.service").is_file());
    let expected_service_path = format!(
        "Environment=PATH={}/.local/bin:{}/.cargo/bin:{}/bin:/usr/local/bin:/usr/bin:/bin",
        fake_home.display(),
        fake_home.display(),
        simard_home.display()
    );
    assert_file_contains(
        &unit_dir.join("simard-ooda.service"),
        &expected_service_path,
    );
    assert_file_contains(
        &unit_dir.join("simard-signal.service"),
        &expected_service_path,
    );
    assert_systemctl_logged(&systemctl_log, &["--user", "daemon-reload"]);
    assert_systemctl_logged(&systemctl_log, &["--user", "enable", "simard-ooda.service"]);
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "enable", "simard-signal.service"],
    );
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "restart", "simard-ooda.service"],
    );
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "restart", "simard-signal.service"],
    );
}

#[cfg(unix)]
#[test]
fn install_uses_simard_home_env_when_cli_home_is_absent() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("env-home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, _systemctl_log) = fake_systemctl(temp.path());

    simard()
        .args(["install", "--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .env("SIMARD_HOME", &simard_home)
        .assert()
        .success();

    assert!(
        simard_home.join("bin/simard").is_file(),
        "SIMARD_HOME env var should choose the install home"
    );
    assert!(
        unit_dir.join("simard-ooda.service").is_file(),
        "installer should write units to the explicit user unit dir"
    );
}

#[cfg(unix)]
#[test]
fn cli_simard_home_overrides_simard_home_env() {
    let temp = TempDir::new().expect("tempdir");
    let env_home = temp.path().join("env-home");
    let cli_home = temp.path().join("cli-home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, _systemctl_log) = fake_systemctl(temp.path());

    simard()
        .args(["install", "--simard-home"])
        .arg(&cli_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .env("SIMARD_HOME", &env_home)
        .assert()
        .success();

    assert!(cli_home.join("bin/simard").is_file());
    assert!(
        !env_home.join("bin/simard").exists(),
        "--simard-home should override SIMARD_HOME instead of installing to both homes"
    );
}

#[cfg(unix)]
#[test]
fn install_writes_binary_prompt_assets_and_systemd_units_atomically_with_safe_paths() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("simard-home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    simard()
        .args(["install", "--simard-home"])
        .arg(&simard_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .assert()
        .success();

    let live_binary = simard_home.join("bin/simard");
    assert!(
        live_binary.is_file(),
        "live binary missing at {live_binary:?}"
    );
    let mode = fs::metadata(&live_binary)
        .expect("live binary metadata")
        .permissions()
        .mode();
    assert!(
        mode & 0o111 != 0,
        "installed binary should be executable; mode={mode:o}"
    );
    assert!(
        !simard_home.join("bin/simard.new").exists(),
        "installer must not leave a sibling temp binary as the live update mechanism"
    );

    assert!(
        simard_home
            .join("prompt_assets/simard/ooda_orient.md")
            .is_file(),
        "installer should install prompt markdown assets"
    );
    assert!(
        simard_home
            .join("prompt_assets/simard/recipes/ooda-orient.yaml")
            .is_file(),
        "installer should install recipe assets"
    );

    let ooda_unit = unit_dir.join("simard-ooda.service");
    let signal_unit = unit_dir.join("simard-signal.service");
    assert_file_contains(
        &ooda_unit,
        &format!("WorkingDirectory={}", simard_home.display()),
    );
    assert_file_contains(
        &signal_unit,
        &format!("WorkingDirectory={}", simard_home.display()),
    );
    assert_file_contains(
        &ooda_unit,
        &format!("ExecStart={}/bin/simard ooda run", simard_home.display()),
    );
    assert_file_contains(
        &signal_unit,
        &format!("ExecStart={}/bin/simard signal run", simard_home.display()),
    );
    assert_file_contains(
        &ooda_unit,
        &format!(
            "Environment=SIMARD_PROMPT_ASSETS_DIR={}/prompt_assets/simard",
            simard_home.display()
        ),
    );
    assert_file_contains(
        &signal_unit,
        &format!(
            "Environment=SIMARD_PROMPT_ASSETS_DIR={}/prompt_assets/simard",
            simard_home.display()
        ),
    );
    for unit in [&ooda_unit, &signal_unit] {
        let contents = read(unit);
        assert!(
            !contents.contains("worktrees/main") && !contents.contains("/target/"),
            "unit file {unit:?} must not reference a source checkout or build directory:\n{contents}"
        );
    }

    assert_systemctl_logged(&systemctl_log, &["--user", "daemon-reload"]);
    assert_systemctl_logged(&systemctl_log, &["--user", "enable", "simard-ooda.service"]);
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "enable", "simard-signal.service"],
    );
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "restart", "simard-ooda.service"],
    );
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "restart", "simard-signal.service"],
    );
}

#[cfg(unix)]
#[test]
fn install_can_source_prompt_assets_from_distribution_env_root() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("simard-home");
    let unit_dir = temp.path().join("systemd-user");
    let source_root = temp.path().join("packaged-prompt-assets");
    let (systemctl, _systemctl_log) = fake_systemctl(temp.path());

    fs::create_dir_all(source_root.join("simard/recipes")).expect("asset dirs");
    fs::write(
        source_root.join("simard/ooda_orient.md"),
        "packaged orient prompt",
    )
    .expect("orient asset");
    fs::write(
        source_root.join("simard/recipes/ooda-orient.yaml"),
        "packaged recipe",
    )
    .expect("recipe asset");

    simard()
        .args(["install", "--simard-home"])
        .arg(&simard_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .env("SIMARD_INSTALL_PROMPT_ASSETS_ROOT", &source_root)
        .assert()
        .success();

    assert_eq!(
        read(&simard_home.join("prompt_assets/simard/ooda_orient.md")),
        "packaged orient prompt"
    );
    assert_eq!(
        read(&simard_home.join("prompt_assets/simard/recipes/ooda-orient.yaml")),
        "packaged recipe"
    );
}

#[cfg(unix)]
#[test]
fn install_preserves_prior_binary_and_prints_memory_backup_guidance_before_restart() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("simard-home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, _systemctl_log) = fake_systemctl(temp.path());
    let old_binary = b"old-simard-binary";

    fs::create_dir_all(simard_home.join("bin")).expect("bin dir");
    fs::write(simard_home.join("bin/simard"), old_binary).expect("old binary");
    fs::set_permissions(
        simard_home.join("bin/simard"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("old binary executable");

    let assert = simard()
        .args(["install", "--simard-home"])
        .arg(&simard_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .assert()
        .success();

    assert_backup_contains(&simard_home.join(".install-backups"), old_binary);
    let output = combined_output(assert.get_output());
    for expected in ["memory backup", "rollback", ".install-backups"] {
        assert!(
            output.to_ascii_lowercase().contains(expected),
            "installer should print rollback and memory backup guidance containing {expected:?}; output:\n{output}"
        );
    }
}

#[cfg(unix)]
#[test]
fn dry_run_does_not_invoke_systemctl_but_prints_activation_plan() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("simard-home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    let assert = simard()
        .args(["install", "--simard-home"])
        .arg(&simard_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .arg("--dry-run")
        .assert()
        .success();

    assert_no_systemctl_invocation(&systemctl_log);
    let output = combined_output(assert.get_output());
    for expected in [
        "dry-run",
        "systemctl --user daemon-reload",
        "simard-ooda.service",
        "simard-signal.service",
    ] {
        assert!(
            output.contains(expected),
            "dry-run output should include activation plan term {expected:?}; output:\n{output}"
        );
    }
}

#[cfg(unix)]
#[test]
fn invalid_simard_home_fails_closed_before_any_systemctl_call() {
    let temp = TempDir::new().expect("tempdir");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    let assert = simard()
        .args(["install", "--simard-home", "relative-home"])
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("SIMARD_HOME") && stderr.contains("absolute"),
        "invalid install home should fail with a precise validation error; stderr:\n{stderr}"
    );
    assert_no_systemctl_invocation(&systemctl_log);
}

#[cfg(unix)]
#[test]
fn simard_home_with_spaces_fails_before_any_mutation_or_systemctl_call() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("simard home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    let assert = simard()
        .args(["install", "--simard-home"])
        .arg(&simard_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("SIMARD_HOME") && stderr.contains("unsafe character"),
        "whitespace in SIMARD_HOME should fail with a precise validation error; stderr:\n{stderr}"
    );
    assert!(
        !simard_home.exists(),
        "invalid SIMARD_HOME must fail before creating install directories"
    );
    assert_no_systemctl_invocation(&systemctl_log);
}

#[cfg(unix)]
#[test]
fn unsafe_systemd_path_characters_fail_closed_before_any_live_swap() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("bad%nhome");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    let assert = simard()
        .args(["install", "--simard-home"])
        .arg(&simard_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("SIMARD_HOME")
            && stderr.contains("unsafe")
            && stderr.contains('%')
            && stderr.contains("systemd unit"),
        "unsafe systemd path characters should be rejected explicitly instead of failing argument parsing; stderr:\n{stderr}"
    );
    assert!(
        !simard_home.join("bin/simard").exists(),
        "failed validation must not swap a live binary"
    );
    assert_no_systemctl_invocation(&systemctl_log);
}

#[cfg(unix)]
#[test]
fn prompt_asset_staging_failure_fails_closed_without_partial_binary_or_units() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("simard-home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());
    fs::create_dir_all(&simard_home).expect("simard home");
    fs::write(simard_home.join("prompt_assets"), "not a directory")
        .expect("conflicting prompt_assets file");

    let assert = simard()
        .args(["install", "--simard-home"])
        .arg(&simard_home)
        .args(["--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("prompt_assets"),
        "asset staging failures should identify the conflicting path; stderr:\n{stderr}"
    );
    assert!(
        !simard_home.join("bin/simard").exists(),
        "asset staging failure must not leave a live binary behind"
    );
    assert!(
        !unit_dir.join("simard-ooda.service").exists()
            && !unit_dir.join("simard-signal.service").exists(),
        "asset staging failure must not install systemd units"
    );
    assert_no_systemctl_invocation(&systemctl_log);
}
