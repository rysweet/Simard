//! `simard self-deploy` — close the merged-but-not-running gap on demand.
//!
//! The **operator** runs this (the recipe never live-redeploys a daemon). With
//! `--check` it only reports deploy drift (is the running binary behind merged
//! `main`?) and makes no changes. Without `--check` it drives the full
//! [`SelfDeployOrchestrator`](crate::self_deploy::SelfDeployOrchestrator)
//! sequence — build-from-source → gate → dual backup → drain → orphan-reap →
//! atomic swap → systemd restart → health check → rollback on failure — ending
//! with the new binary verified-running or rolled back.
//!
//! See `docs/concepts/reconcile-and-self-deploy.md` and
//! `docs/howto/verify-and-roll-back-a-self-deploy.md`.

use crate::self_deploy::{
    DeploySource, GitDeploySource, GitSourcePreparer, ReconcileDetector, SelfDeployOrchestrator,
    SystemdOrExecRestarter,
};

pub(super) const SELF_DEPLOY_HELP: &str = "\
Simard self-deploy subcommand

Usage: simard self-deploy [--check] [--json]

  --check   Report deploy drift only (running-vs-merged); make no changes.
  --json    Emit machine-readable output (drift JSON under --check).

Without --check this performs the full build-from-source self-deploy and ends
with the new binary verified-running, or rolls back on failure. Operator-only:
the recipe must never live-redeploy the daemon.
";

/// Parsed `self-deploy` flags.
struct Flags {
    check: bool,
    json: bool,
}

fn parse_flags(args: impl Iterator<Item = String>) -> Result<Flags, Box<dyn std::error::Error>> {
    let mut check = false;
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--json" => json = true,
            other => {
                return Err(format!(
                    "unexpected argument '{other}' (see `simard self-deploy --help`)"
                )
                .into());
            }
        }
    }
    Ok(Flags { check, json })
}

/// Dispatch `simard self-deploy`.
pub(super) fn dispatch_self_deploy_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let flags = parse_flags(args)?;

    if flags.check {
        return report_drift(flags.json);
    }

    run_self_deploy()
}

/// `--check`: compute and print deploy drift; never mutate anything.
fn report_drift(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let source = GitDeploySource::new();
    let drift = ReconcileDetector::new(GitDeploySource::new()).detect();

    if json {
        println!("{}", serde_json::to_string_pretty(&drift)?);
    } else {
        let running = source
            .running_commit()
            .unwrap_or_else(|_| "unknown".to_string());
        let merged = source
            .merged_head()
            .unwrap_or_else(|_| "unknown".to_string());
        println!("simard self-deploy --check:");
        println!("  running commit : {running}");
        println!("  merged head    : {merged}");
        println!("  behind commits : {}", drift.behind_commits);
        println!(
            "  drifted pins   : {}",
            if drift.drifted_pins.is_empty() {
                "(none)".to_string()
            } else {
                drift.drifted_pins.join(", ")
            }
        );
        println!(
            "  needs deploy   : {}",
            if drift.needs_deploy { "YES" } else { "no" }
        );
    }
    Ok(())
}

/// Full operator self-deploy. Effectful: builds from source and restarts the
/// daemon. Returns an error (loudly) on build/gate/backup/health failure; a
/// failed health check rolls back to the previous binary.
///
/// Issue #2467: cwd-independent. The canonical source repo is resolved via
/// `SIMARD_SELF_DEPLOY_REPO` → persistent `~/.simard/self-deploy-src` →
/// clone-from-origin, then fetched so the merged head we read (and deploy) is
/// the real one — regardless of the directory the operator runs from. The
/// orchestrator then builds that fetched+checked-out merged commit into the
/// warm target dir.
fn run_self_deploy() -> Result<(), Box<dyn std::error::Error>> {
    // Resolve the build source independent of the cwd, and fetch so
    // `origin/main` (the merged head) is current before we read it.
    let preparer = GitSourcePreparer::new();
    let repo = preparer.resolve_repo()?;
    preparer.fetch_origin(&repo)?;

    let source = GitDeploySource::at(&repo);
    let drift = ReconcileDetector::new(GitDeploySource::at(&repo)).detect();
    if !drift.needs_deploy {
        println!("simard self-deploy: running binary is already at merged head — nothing to do.");
        return Ok(());
    }

    let target_commit = source.merged_head()?;
    let install_path =
        std::env::current_exe().map_err(|e| format!("cannot resolve current executable: {e}"))?;

    println!(
        "simard self-deploy: deploying merged head {target_commit} (binary {} behind)…",
        drift.behind_commits
    );

    let orchestrator = SelfDeployOrchestrator::with_source(
        crate::safe_update::UpdateConfig::default(),
        Box::new(SystemdOrExecRestarter::new()),
        target_commit,
        install_path,
        Box::new(GitSourcePreparer::new()),
    );

    match orchestrator.run() {
        Ok(outcome) => {
            println!(
                "simard self-deploy: SUCCESS — new binary verified running (restarter={}, orphans reaped={}).",
                outcome.restarter_kind, outcome.reaped_orphans
            );
            Ok(())
        }
        Err(e) => Err(format!("simard self-deploy FAILED: {e}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flags_defaults() {
        let f = parse_flags(Vec::<String>::new().into_iter()).unwrap();
        assert!(!f.check);
        assert!(!f.json);
    }

    #[test]
    fn parse_flags_check_and_json() {
        let f = parse_flags(vec!["--check".to_string(), "--json".to_string()].into_iter()).unwrap();
        assert!(f.check);
        assert!(f.json);
    }

    #[test]
    fn parse_flags_rejects_unknown() {
        assert!(parse_flags(vec!["--bogus".to_string()].into_iter()).is_err());
    }

    #[test]
    fn check_mode_reports_drift_without_error() {
        // `--check` is read-only and must never error on the live checkout: the
        // reconcile detector is fail-safe (a git error reports "no drift").
        report_drift(true).unwrap();
        report_drift(false).unwrap();
    }
}
