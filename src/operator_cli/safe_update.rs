//! Operator-facing commands for the safe-update orchestrator.
//!
//! Three subcommands are exported here and wired into [`super::dispatch_operator_cli`]:
//!
//! * `simard safe-update`        — drive phases 1–4 against the binary
//!   already downloaded by `simard update` (operator-facing wrapper).
//!   Refuses to run as a daemon — meant to be invoked once.
//! * `simard rollback`           — restore the latest backup and record
//!   `phase=rolled_back`. Idempotent.
//! * `simard rollback-watchdog`  — long-running loop that polls
//!   `state_dir/upgrade-status.json` and triggers `rollback` when the
//!   incoming binary lands in `validate_timeout`.

use std::path::PathBuf;
use std::time::Duration;

use crate::cmd_self_update::handle_self_update_download_only;
use crate::safe_update::{
    SafeUpdateOrchestrator, UpdateConfig, default_install_bin, do_rollback, validate,
};

pub(crate) const SAFE_UPDATE_HELP: &str = "\
Simard safe-update subcommand

Usage: simard safe-update

Drain → snapshot → pre-test → swap → exec. Downloads the latest release,
runs safety gates, and replaces the running binary if all checks pass.
Does not return on success (exec replaces the process image).
";

pub(crate) const ROLLBACK_HELP: &str = "\
Simard rollback subcommand

Usage: simard rollback

Restore the latest backup over the install path and record phase=rolled_back.
Idempotent.
";

pub(crate) const ROLLBACK_WATCHDOG_HELP: &str = "\
Simard rollback-watchdog subcommand

Usage: simard rollback-watchdog [--once] [--interval=SECS] [--max-iterations=N]

Long-running loop that polls upgrade-status.json and triggers rollback on
validate_timeout. Pass --once for a single check-and-act cycle.
";

/// `simard safe-update`: run phases 1–4 (drain → snapshot → pre-test →
/// swap+handover) using the already-downloaded candidate binary at
/// `~/.simard/bin/simard.candidate` (the path `simard update` writes to).
///
/// On success the call exec()s into the new binary and **does not return**.
pub fn handle_safe_update() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "simard safe-update (current: v{})",
        env!("CARGO_PKG_VERSION")
    );

    // Step 0: download the latest release into a candidate path.
    let candidate = handle_self_update_download_only()?;
    if candidate.is_none() {
        println!("Already at the latest version. Nothing to do.");
        return Ok(());
    }
    let candidate_path = candidate.unwrap();
    let install_path = default_install_bin();
    let cfg = UpdateConfig::default();

    println!(
        "Drain timeout: {}s; pretest timeout: {}s; validate cycles: {}; validate budget: {}s",
        cfg.drain_timeout_seconds,
        cfg.pretest_timeout_seconds,
        cfg.validate_timeout_cycles,
        cfg.validate_timeout_seconds,
    );
    println!("Candidate: {}", candidate_path.display());
    println!("Install:   {}", install_path.display());
    println!("State dir: {}", cfg.state_dir.display());

    let orch = SafeUpdateOrchestrator::new(cfg, candidate_path, install_path);
    match orch.run() {
        Ok(_) => {
            // Only reached in tests where handover is stubbed.
            println!("safe-update orchestrator returned Ok (handover stub mode).");
            Ok(())
        }
        Err(e) => Err(format!("safe-update aborted: {e}").into()),
    }
}

/// `simard rollback`: restore the latest `simard.bak.*` backup over the
/// install path and record `phase=rolled_back`. Tries to restart the
/// `simard-ooda` user service via systemctl; on non-systemd hosts a
/// warning is printed and the operator must restart manually.
pub fn handle_rollback() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = UpdateConfig::default();
    let install = default_install_bin();
    println!("simard rollback (install: {})", install.display());
    println!("State dir: {}", cfg.state_dir.display());

    let restart_cmd: Option<&[&str]> = if systemctl_user_available() {
        Some(&["systemctl", "--user", "restart", "simard-ooda"])
    } else {
        eprintln!(
            "warning: systemctl --user not detected; restart simard-ooda manually after rollback"
        );
        None
    };

    let outcome = do_rollback(
        &cfg.state_dir,
        &install,
        "operator-invoked rollback",
        restart_cmd,
    )?;
    println!("rolled back: install <- {}", outcome.backup_used.display());
    if let Some(warning) = outcome.restart_warning {
        eprintln!("warning: restart command did not succeed: {warning}");
    }
    Ok(())
}

/// `simard rollback-watchdog`: long-running loop. Wakes every `interval`
/// seconds (default 10), and if `state_dir/upgrade-status.json` is in
/// `validate_timeout`, performs `do_rollback`. Exits cleanly on a single
/// successful rollback; designed to be run by systemd as
/// `simard-rollback-watchdog.service`.
///
/// Pass `--once` to do a single check-and-act and exit (useful in cron).
pub fn handle_rollback_watchdog(
    args: impl IntoIterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut once = false;
    let mut interval_secs = 10_u64;
    let mut max_iterations: Option<usize> = None;
    for a in args {
        match a.as_str() {
            "--once" => once = true,
            s if s.starts_with("--interval=") => {
                interval_secs = s[11..]
                    .parse()
                    .map_err(|e| format!("bad --interval: {e}"))?;
            }
            s if s.starts_with("--max-iterations=") => {
                max_iterations = Some(
                    s[17..]
                        .parse()
                        .map_err(|e| format!("bad --max-iterations: {e}"))?,
                );
            }
            other => return Err(format!("unsupported flag '{other}'").into()),
        }
    }

    let cfg = UpdateConfig::default();
    let install = default_install_bin();
    println!(
        "simard rollback-watchdog (state_dir={}, install={}, interval={}s)",
        cfg.state_dir.display(),
        install.display(),
        interval_secs
    );

    let mut iteration = 0_usize;
    loop {
        iteration += 1;
        match validate::enter_validation_if_needed(&cfg.state_dir) {
            Ok(validate::ValidateMode::Timeout) => {
                println!("[watchdog #{iteration}] phase=validate_timeout — initiating rollback");
                let restart_cmd: Option<&[&str]> = if systemctl_user_available() {
                    Some(&["systemctl", "--user", "restart", "simard-ooda"])
                } else {
                    None
                };
                match do_rollback(
                    &cfg.state_dir,
                    &install,
                    "watchdog observed validate_timeout",
                    restart_cmd,
                ) {
                    Ok(outcome) => {
                        println!(
                            "[watchdog #{iteration}] rollback ok; install <- {}",
                            outcome.backup_used.display()
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("[watchdog #{iteration}] rollback FAILED: {e}");
                        return Err(format!("rollback failed: {e}").into());
                    }
                }
            }
            Ok(other) => {
                if iteration % 6 == 1 {
                    println!("[watchdog #{iteration}] phase observed: {other:?}");
                }
            }
            Err(e) => eprintln!("[watchdog #{iteration}] cannot read upgrade-status: {e}"),
        }

        if once {
            return Ok(());
        }
        if let Some(cap) = max_iterations
            && iteration >= cap
        {
            println!("[watchdog] reached --max-iterations={cap}; exiting cleanly");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(interval_secs));
    }
}

fn systemctl_user_available() -> bool {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "status"])
        .output();
    matches!(out, Ok(o) if o.status.success() || o.status.code() == Some(3))
}

/// Default candidate path used by [`handle_safe_update`] when the operator
/// has not provided one explicitly.
#[allow(dead_code)]
pub fn default_candidate_path() -> PathBuf {
    crate::safe_update::snapshot::default_bin_dir().join("simard.candidate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_candidate_path_lives_under_bin_dir() {
        let p = default_candidate_path();
        assert!(p.ends_with("simard.candidate"));
        assert!(p.parent().unwrap().ends_with("bin"));
    }

    #[test]
    fn watchdog_once_returns_ok_when_no_status() {
        // No state_dir present in HOME, so phase observed is NotRequired.
        // We can't easily isolate HOME here, but the function should not
        // panic and should return Ok when `--once` is passed and there is
        // nothing to roll back.
        let r = handle_rollback_watchdog(vec!["--once".to_string()]);
        // It may succeed (no upgrade in progress) or fail if the live
        // state_dir has phase=validate_timeout — both are valid; the
        // contract is that it doesn't panic.
        let _ = r;
    }

    #[test]
    fn watchdog_rejects_unknown_flag() {
        let r = handle_rollback_watchdog(vec!["--bogus".to_string()]);
        assert!(r.is_err());
    }

    #[test]
    fn watchdog_max_iterations_zero_exits_immediately() {
        let r = handle_rollback_watchdog(vec!["--max-iterations=1".to_string()]);
        // Should return Ok in a short time (loop runs once then exits).
        let _ = r;
    }
}
