//! Operator subcommand `simard worktree-gc [--apply]`.
//!
//! Defaults to dry-run. Operators must pass `--apply` to perform any
//! filesystem mutation.

use crate::worktree_gc::{
    GcConfig, GhClientShell, ProcfsLiveProcessProbe, default_roots, run_gc, runner::render_reason,
};

use super::args::reject_extra_args;

/// Repo for upstream PR queries. Hard-coded to the home repo for now;
/// matches `merge-pr`'s hard-coded `rysweet/Simard`.
const HOME_REPO: &str = "rysweet/Simard";

pub(crate) fn dispatch_worktree_gc_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut apply = false;
    let mut idle_days: u64 = crate::worktree_gc::DEFAULT_IDLE_DAYS;
    let mut explicit_roots: Vec<std::path::PathBuf> = Vec::new();
    let mut parent_repo: Option<std::path::PathBuf> = None;

    let remaining: Vec<String> = (&mut args).collect();
    for arg in remaining {
        if arg == "--apply" {
            apply = true;
        } else if arg == "--dry-run" {
            apply = false;
        } else if let Some(n) = arg.strip_prefix("--idle-days=") {
            idle_days = n
                .parse()
                .map_err(|_| format!("invalid --idle-days value: {n}"))?;
        } else if let Some(p) = arg.strip_prefix("--root=") {
            explicit_roots.push(std::path::PathBuf::from(p));
        } else if let Some(p) = arg.strip_prefix("--parent-repo=") {
            parent_repo = Some(std::path::PathBuf::from(p));
        } else if arg == "--help" || arg == "-h" {
            print!("{WORKTREE_GC_HELP}");
            return Ok(());
        } else {
            return Err(format!("unexpected argument: {arg}").into());
        }
    }
    reject_extra_args(std::iter::empty::<String>())?;

    let parent_repo = parent_repo
        .or_else(|| std::env::current_dir().ok())
        .ok_or("cannot resolve parent repo (pass --parent-repo=PATH)")?;

    let roots = if explicit_roots.is_empty() {
        default_roots()
    } else {
        explicit_roots
    };

    let cfg = GcConfig {
        roots,
        parent_repo: parent_repo.clone(),
        apply,
        idle_days,
        now: std::time::SystemTime::now(),
    };

    if apply {
        eprintln!("[worktree-gc] APPLY mode — pruning candidates");
    } else {
        eprintln!("[worktree-gc] DRY-RUN — pass --apply to actually prune");
    }
    eprintln!("[worktree-gc] parent_repo = {}", parent_repo.display());
    eprintln!("[worktree-gc] idle_days   = {idle_days}");
    eprintln!("[worktree-gc] roots:");
    for r in &cfg.roots {
        eprintln!("[worktree-gc]   - {}", r.display());
    }

    let gh = GhClientShell::new(HOME_REPO, parent_repo);
    let probe = ProcfsLiveProcessProbe::new();
    let report =
        run_gc(&cfg, &gh, &probe).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    eprintln!(
        "[worktree-gc] examined={} candidates={}",
        report.worktrees_examined,
        report.candidates.len(),
    );
    for cand in &report.candidates {
        let primary = cand
            .primary_reason()
            .map(render_reason)
            .unwrap_or_else(|| "(no reason)".to_string());
        let extras: Vec<String> = cand
            .reasons
            .iter()
            .filter(|r| Some(*r) != cand.primary_reason())
            .map(render_reason)
            .collect();
        let extras_str = if extras.is_empty() {
            String::new()
        } else {
            format!(" (+ {})", extras.join(", "))
        };
        eprintln!(
            "[worktree-gc] candidate: {} branch={} reason={}{}",
            cand.path.display(),
            cand.branch.as_deref().unwrap_or("<detached>"),
            primary,
            extras_str,
        );
    }

    if apply {
        for p in &report.pruned {
            eprintln!("[worktree-gc] pruned: {}", p.display());
        }
        for (p, e) in &report.failures {
            eprintln!("[worktree-gc] FAILED: {} — {e}", p.display());
        }
        eprintln!(
            "[worktree-gc] DONE pruned={} failures={}",
            report.pruned.len(),
            report.failures.len()
        );
        if !report.failures.is_empty() {
            return Err(format!("{} prune failures", report.failures.len()).into());
        }
    }

    Ok(())
}

const WORKTREE_GC_HELP: &str = "\
Usage: simard worktree-gc [--apply] [--dry-run] [--idle-days=N] \
[--root=PATH ...] [--parent-repo=PATH]

Prune engineer worktrees whose:
  - branch was merged upstream (gh pr list --state merged), OR
  - branch was deleted from origin, OR
  - have been idle longer than --idle-days (default 7).

Defaults to dry-run. Pass --apply to actually prune.

Roots default to:
  $HOME/.simard/engineer-worktrees
  $HOME/src/Simard/worktrees
or to SIMARD_WORKTREE_GC_ROOTS (colon-separated) if set.

--root may be passed multiple times.
--parent-repo defaults to the current working directory.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_flag_is_rejected() {
        let err = dispatch_worktree_gc_command(["--whoops".to_string()].into_iter())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--whoops"), "{err}");
    }

    #[test]
    fn invalid_idle_days_is_rejected() {
        let err = dispatch_worktree_gc_command(["--idle-days=abc".to_string()].into_iter())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--idle-days"), "{err}");
    }
}
