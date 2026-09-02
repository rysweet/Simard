//! Contract tests for the canonical `simard install` deployment rail.
//!
//! These tests intentionally exercise the real `simard` binary from the
//! outside. They use temporary install roots and a fake `systemctl`, so they
//! prove the installer contract without mutating host user services.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::Duration;

use assert_cmd::Command;
use tempfile::TempDir;

const EXPECTED_VERSION: &str = env!("CARGO_PKG_VERSION");

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

fn assert_systemctl_not_logged(log: &Path, unexpected_terms: &[&str]) {
    if !log.exists() {
        return;
    }
    let contents = read(log);
    let matched = contents
        .lines()
        .any(|line| unexpected_terms.iter().all(|term| line.contains(term)));
    assert!(
        !matched,
        "fake systemctl log {log:?} must NOT contain a line with all terms {unexpected_terms:?}; log:\n{contents}"
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

fn run_capture(bin: &Path, args: &[&str]) -> String {
    let output = StdCommand::new(bin)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to execute {bin:?} {args:?}: {e}"));
    assert!(
        output.status.success(),
        "{bin:?} {args:?} exited {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
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
fn cargo_install_runtime_features_self_installs_with_canonical_installer() {
    let workspace = std::env::current_dir().expect("cwd");
    let install_root = workspace.join("target").join("install-real");
    let _ = fs::remove_dir_all(&install_root);

    let install_status = StdCommand::new(env!("CARGO"))
        .args(["install", "--path", ".", "--root"])
        .arg(&install_root)
        .args([
            "--no-track",
            "--quiet",
            "--debug",
            "--no-default-features",
            "--features",
            "signal",
        ])
        .status()
        .expect("failed to launch cargo install");
    assert!(install_status.success(), "cargo install --path . failed");

    let installed_simard = install_root.join("bin").join("simard");
    assert!(
        installed_simard.exists(),
        "expected installed binary at {installed_simard:?}"
    );

    let version_output = run_capture(&installed_simard, &["--version"]);
    assert_eq!(
        version_output.trim(),
        format!("simard {EXPECTED_VERSION}"),
        "installed simard --version mismatch"
    );

    let ensure_status = StdCommand::new(&installed_simard)
        .arg("ensure-deps")
        .status()
        .expect("failed to launch installed simard ensure-deps");
    assert!(
        ensure_status.success(),
        "installed simard ensure-deps failed (exit {:?})",
        ensure_status.code()
    );

    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("self-install-home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    let self_install_status = StdCommand::new(&installed_simard)
        .arg("install")
        .arg("--simard-home")
        .arg(&simard_home)
        .arg("--systemd-user-dir")
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .status()
        .expect("failed to launch installed simard install");
    assert!(
        self_install_status.success(),
        "installed simard install failed"
    );

    let second_hop = simard_home.join("bin").join("simard");
    assert!(
        second_hop.exists(),
        "second-hop binary missing at {second_hop:?}"
    );
    let mode = fs::metadata(&second_hop)
        .expect("stat second-hop")
        .permissions()
        .mode();
    assert!(
        mode & 0o111 != 0,
        "second-hop binary at {second_hop:?} not executable (mode {mode:o})"
    );

    let second_hop_version = run_capture(&second_hop, &["--version"]);
    assert_eq!(
        second_hop_version.trim(),
        format!("simard {EXPECTED_VERSION}"),
        "second-hop simard --version mismatch"
    );
    assert_systemctl_logged(&systemctl_log, &["--user", "daemon-reload"]);
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "restart", "simard-ooda.service"],
    );
    // Convergence: the separate signal service is decommissioned, never
    // enabled or restarted (Signal is hosted in-process by the OODA daemon).
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "disable", "--now", "simard-signal.service"],
    );
    assert_systemctl_not_logged(
        &systemctl_log,
        &["--user", "enable", "simard-signal.service"],
    );
    assert_systemctl_not_logged(
        &systemctl_log,
        &["--user", "restart", "simard-signal.service"],
    );
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
    assert!(
        !unit_dir.join("simard-signal.service").exists(),
        "the separate signal service must no longer be installed (hosted in-process by OODA daemon)"
    );
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
    assert_file_contains(&unit_dir.join("simard-ooda.service"), "KillMode=process");
    assert_systemctl_logged(&systemctl_log, &["--user", "daemon-reload"]);
    assert_systemctl_logged(&systemctl_log, &["--user", "enable", "simard-ooda.service"]);
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "restart", "simard-ooda.service"],
    );
    // Convergence: the separate signal service is decommissioned, never
    // enabled or restarted.
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "disable", "--now", "simard-signal.service"],
    );
    assert_systemctl_not_logged(
        &systemctl_log,
        &["--user", "enable", "simard-signal.service"],
    );
    assert_systemctl_not_logged(
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
    assert!(
        !unit_dir.join("simard-signal.service").exists(),
        "the separate signal service must no longer be installed (hosted in-process by OODA daemon)"
    );
    assert_file_contains(
        &ooda_unit,
        &format!("WorkingDirectory={}", simard_home.display()),
    );
    assert_file_contains(
        &ooda_unit,
        &format!("ExecStart={}/bin/simard ooda run", simard_home.display()),
    );
    assert_file_contains(
        &ooda_unit,
        &format!(
            "Environment=SIMARD_PROMPT_ASSETS_DIR={}/prompt_assets/simard",
            simard_home.display()
        ),
    );
    {
        let contents = read(&ooda_unit);
        assert!(
            !contents.contains("worktrees/main") && !contents.contains("/target/"),
            "unit file {ooda_unit:?} must not reference a source checkout or build directory:\n{contents}"
        );
        assert!(
            contents.contains("Restart=always"),
            "unit file {ooda_unit:?} must use Restart=always so the daemon self-recovers from a graceful exit-0 shutdown (e.g. a stray SIGTERM):\n{contents}"
        );
        assert!(
            !contents.contains("Restart=on-failure"),
            "unit file {ooda_unit:?} must not use Restart=on-failure, which does not restart a clean exit:\n{contents}"
        );
    }

    assert_systemctl_logged(&systemctl_log, &["--user", "daemon-reload"]);
    assert_systemctl_logged(&systemctl_log, &["--user", "enable", "simard-ooda.service"]);
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "restart", "simard-ooda.service"],
    );
    // Convergence: the separate signal service is decommissioned, never
    // enabled or restarted.
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "disable", "--now", "simard-signal.service"],
    );
    assert_systemctl_not_logged(
        &systemctl_log,
        &["--user", "enable", "simard-signal.service"],
    );
    assert_systemctl_not_logged(
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
fn simard_home_with_path_separator_fails_before_any_mutation_or_systemctl_call() {
    let temp = TempDir::new().expect("tempdir");
    let simard_home = temp.path().join("simard:home");
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
        stderr.contains("SIMARD_HOME") && stderr.contains("unsafe character ':'"),
        "PATH separator in SIMARD_HOME should fail with a precise validation error; stderr:\n{stderr}"
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

#[cfg(unix)]
#[test]
fn install_decommissions_a_preexisting_signal_unit() {
    // A host upgraded from a build that deployed a separate
    // simard-signal.service must converge: install removes the stale unit file
    // and disables it, so the Signal channel runs ONLY in-process in the OODA
    // daemon (never double-connected to signal-cli by two racing processes).
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, systemctl_log) = fake_systemctl(temp.path());

    // Pre-seed the obsolete unit as if a prior install wrote it.
    fs::create_dir_all(&unit_dir).expect("unit dir");
    let stale_signal = unit_dir.join("simard-signal.service");
    fs::write(&stale_signal, "[Unit]\nDescription=Simard Signal service\n")
        .expect("seed stale signal unit");

    simard()
        .args(["install", "--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .env("HOME", &fake_home)
        .assert()
        .success();

    assert!(
        !stale_signal.exists(),
        "install must remove the obsolete simard-signal.service unit file"
    );
    assert!(
        unit_dir.join("simard-ooda.service").is_file(),
        "install must still deploy the OODA unit"
    );
    assert_systemctl_logged(
        &systemctl_log,
        &["--user", "disable", "--now", "simard-signal.service"],
    );
}

// ---------------------------------------------------------------------------
// PATH-entrypoint ownership + orphan reconciliation regression suite (issue
// #4460). These tests exercise the real `simard install` transaction against a
// temporary `$HOME` so the entrypoint dir (`$HOME/.local/bin`) and orphan dir
// (`$HOME/.cargo/bin`) resolve inside the sandbox and never touch the host.
//
// Contract under test (docs/reference/simard-installer.md#path-entrypoint-ownership-guarantee):
//   1. install creates/repairs an owned symlink $HOME/.local/bin/simard ->
//      $HOME/.simard/bin/simard (atomic, idempotent, dir created if absent).
//   2. verified-ours orphans (OursSymlink | OursMarker) at $HOME/.local/bin and
//      $HOME/.cargo/bin are removed so none can shadow the owned entrypoint.
//   3. a Foreign `simard` is NEVER modified/deleted — it is left in place and
//      surfaced loudly with a `[simard]` diagnostic.
//   4. the PATH-resolved entrypoint IS the installed binary: path identity
//      (canonicalizes to $SIMARD_HOME/bin/simard) AND version equality.
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn install_into_home(temp: &Path, fake_home: &Path) {
    let unit_dir = temp.join("systemd-user");
    let (systemctl, _log) = fake_systemctl(temp);
    simard()
        .args(["install", "--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .env("HOME", fake_home)
        .assert()
        .success();
}

#[cfg(unix)]
fn write_exec_script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("script parent dir");
    }
    fs::write(path, body).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("script executable");
}

#[cfg(unix)]
fn version_string(bin: &Path) -> String {
    let output = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("failed to run {bin:?} --version: {error}"));
    assert!(
        output.status.success(),
        "{bin:?} --version exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Absence check that is correct for a removed file *and* a dangling symlink
/// (`Path::exists` follows symlinks and would report a broken symlink as absent
/// even when the link file still exists, so use `symlink_metadata`).
#[cfg(unix)]
fn path_is_absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err()
}

#[cfg(unix)]
#[test]
fn install_creates_owned_entrypoint_symlink_pointing_at_versioned_binary() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");

    // Fresh home: no ~/.local/bin at all — the installer must create it.
    install_into_home(temp.path(), &fake_home);

    let entrypoint = fake_home.join(".local/bin/simard");
    let versioned = fake_home.join(".simard/bin/simard");

    let meta = fs::symlink_metadata(&entrypoint)
        .expect("installer must create the owned ~/.local/bin/simard entrypoint");
    assert!(
        meta.file_type().is_symlink(),
        "the owned entrypoint must be a symlink, not a copy; got {meta:?}"
    );
    assert_eq!(
        fs::canonicalize(&entrypoint).expect("entrypoint must resolve"),
        fs::canonicalize(&versioned).expect("versioned binary must exist"),
        "the owned entrypoint must resolve to the versioned $HOME/.simard/bin/simard"
    );
}

#[cfg(unix)]
#[test]
fn install_entrypoint_satisfies_path_identity_and_version_parity() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");

    install_into_home(temp.path(), &fake_home);

    let entrypoint = fake_home.join(".local/bin/simard");
    let versioned = fake_home.join(".simard/bin/simard");

    // Path identity: canonicalized entrypoint == installed versioned binary.
    assert_eq!(
        fs::canonicalize(&entrypoint).expect("entrypoint resolves"),
        fs::canonicalize(&versioned).expect("versioned binary exists"),
        "PATH-resolved entrypoint must be the installed binary (path identity)"
    );

    // Version parity: entrypoint --version == versioned binary --version, and
    // both carry the identifying `simard ` marker.
    let entry_version = version_string(&entrypoint);
    let versioned_version = version_string(&versioned);
    assert_eq!(
        entry_version, versioned_version,
        "entrypoint --version must equal the installed binary's --version"
    );
    assert!(
        entry_version.starts_with("simard "),
        "installed --version must start with the `simard ` marker; got {entry_version:?}"
    );
}

#[cfg(unix)]
#[test]
fn install_removes_ours_symlink_orphan_in_cargo_bin() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let versioned = fake_home.join(".simard/bin/simard");
    let cargo_orphan = fake_home.join(".cargo/bin/simard");

    // Plant an ours-symlink orphan: a symlink whose canonical target lands
    // inside ~/.simard/bin (the installer places the binary there during the
    // transaction, so classification resolves it as OursSymlink).
    fs::create_dir_all(cargo_orphan.parent().unwrap()).expect("cargo bin dir");
    std::os::unix::fs::symlink(&versioned, &cargo_orphan).expect("plant ours symlink orphan");

    install_into_home(temp.path(), &fake_home);

    assert!(
        path_is_absent(&cargo_orphan),
        "a verified-ours (OursSymlink) orphan in ~/.cargo/bin must be removed so it \
         cannot shadow the owned entrypoint"
    );
}

#[cfg(unix)]
#[test]
fn install_removes_ours_marker_orphan_in_cargo_bin() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let cargo_orphan = fake_home.join(".cargo/bin/simard");

    // Plant an OursMarker orphan: a regular file whose `--version` prints a line
    // starting with `simard ` (our identifying marker). Even though it is not a
    // symlink into ~/.simard/bin, the two-tier classifier treats it as ours.
    write_exec_script(&cargo_orphan, "#!/bin/sh\necho 'simard 0.0.1-orphan'\n");

    install_into_home(temp.path(), &fake_home);

    assert!(
        path_is_absent(&cargo_orphan),
        "a verified-ours (OursMarker: `--version` starts with `simard `) orphan in \
         ~/.cargo/bin must be removed"
    );
}

#[cfg(unix)]
#[test]
fn install_replaces_ours_orphan_at_entrypoint_with_owned_symlink() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let entrypoint = fake_home.join(".local/bin/simard");
    let versioned = fake_home.join(".simard/bin/simard");

    // A prior deploy's ours-marker file already sits at the entrypoint path.
    write_exec_script(&entrypoint, "#!/bin/sh\necho 'simard 0.0.1-stale'\n");

    install_into_home(temp.path(), &fake_home);

    let meta = fs::symlink_metadata(&entrypoint).expect("entrypoint present");
    assert!(
        meta.file_type().is_symlink(),
        "an ours orphan at the entrypoint path must be replaced by the owned symlink"
    );
    assert_eq!(
        fs::canonicalize(&entrypoint).expect("entrypoint resolves"),
        fs::canonicalize(&versioned).expect("versioned binary exists"),
        "the replaced entrypoint must resolve to the versioned binary"
    );
}

#[cfg(unix)]
#[test]
fn install_preserves_foreign_orphan_in_cargo_bin() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let cargo_orphan = fake_home.join(".cargo/bin/simard");

    // A Foreign file: a real binary that is NOT ours (its `--version` does not
    // start with `simard `). The installer must never delete it.
    let foreign_body = "#!/bin/sh\necho 'othertool 9.9.9'\n";
    write_exec_script(&cargo_orphan, foreign_body);

    install_into_home(temp.path(), &fake_home);

    assert!(
        cargo_orphan.is_file(),
        "a Foreign `simard` in ~/.cargo/bin must never be deleted by the installer"
    );
    assert_eq!(
        read(&cargo_orphan),
        foreign_body,
        "a Foreign orphan must be left byte-for-byte untouched"
    );
}

#[cfg(unix)]
#[test]
fn install_does_not_clobber_foreign_shadow_at_entrypoint_and_warns_loudly() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, _log) = fake_systemctl(temp.path());
    let entrypoint = fake_home.join(".local/bin/simard");

    // An unrelated user tool named `simard` occupies the entrypoint path.
    let foreign_body = "#!/bin/sh\necho 'othertool 1.2.3'\n";
    write_exec_script(&entrypoint, foreign_body);

    // The install still succeeds (it protects the user's file rather than
    // failing the whole deploy) but surfaces the foreign shadow loudly.
    let assert = simard()
        .args(["install", "--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .env("HOME", &fake_home)
        .assert()
        .success();

    // The foreign file must survive untouched: still a regular file, same bytes.
    let meta = fs::symlink_metadata(&entrypoint).expect("entrypoint present");
    assert!(
        !meta.file_type().is_symlink(),
        "a Foreign file at the entrypoint path must NOT be replaced by a symlink"
    );
    assert_eq!(
        read(&entrypoint),
        foreign_body,
        "a Foreign file at the entrypoint path must be left byte-for-byte untouched"
    );

    // And the installer must surface it loudly with a `[simard]` diagnostic.
    let output = combined_output(assert.get_output()).to_ascii_lowercase();
    assert!(
        output.contains("[simard]") && output.contains("foreign") && output.contains(".local/bin"),
        "installer must emit a loud [simard] diagnostic about the foreign entrypoint shadow; output:\n{}",
        combined_output(assert.get_output())
    );
}

#[cfg(unix)]
#[test]
fn install_entrypoint_reconciliation_is_idempotent() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let entrypoint = fake_home.join(".local/bin/simard");
    let versioned = fake_home.join(".simard/bin/simard");

    install_into_home(temp.path(), &fake_home);
    install_into_home(temp.path(), &fake_home);

    // Exactly one entrypoint file exists in ~/.local/bin, and it is the owned
    // symlink resolving to the versioned binary.
    let entries: Vec<_> = fs::read_dir(fake_home.join(".local/bin"))
        .expect("entrypoint dir")
        .map(|e| e.expect("dir entry").file_name())
        .collect();
    let simard_entries = entries.iter().filter(|n| *n == "simard").count();
    assert_eq!(
        simard_entries, 1,
        "a second identical install must not accumulate more than one entrypoint; \
         found {simard_entries} in {entries:?}"
    );

    let meta = fs::symlink_metadata(&entrypoint).expect("entrypoint present after re-install");
    assert!(
        meta.file_type().is_symlink(),
        "the entrypoint must remain the owned symlink after a repeat install"
    );
    assert_eq!(
        fs::canonicalize(&entrypoint).expect("entrypoint resolves"),
        fs::canonicalize(&versioned).expect("versioned binary exists"),
        "the entrypoint must still resolve to the versioned binary after a repeat install"
    );
}

#[cfg(unix)]
#[test]
fn dry_run_announces_entrypoint_reconciliation_and_creates_no_symlink() {
    let temp = TempDir::new().expect("tempdir");
    let fake_home = temp.path().join("home");
    let unit_dir = temp.path().join("systemd-user");
    let (systemctl, _log) = fake_systemctl(temp.path());

    let assert = simard()
        .args(["install", "--systemd-user-dir"])
        .arg(&unit_dir)
        .arg("--systemctl")
        .arg(&systemctl)
        .arg("--dry-run")
        .env("HOME", &fake_home)
        .assert()
        .success();

    let output = combined_output(assert.get_output()).to_ascii_lowercase();
    assert!(
        output.contains("entrypoint") && output.contains(".local/bin"),
        "dry-run must announce the PATH-entrypoint reconciliation plan; output:\n{}",
        combined_output(assert.get_output())
    );
    assert!(
        path_is_absent(&fake_home.join(".local/bin/simard")),
        "dry-run must not create the owned entrypoint symlink"
    );
}
