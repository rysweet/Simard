//! Runner: glue between the policy + parsers and the actual git/gh shell.
//!
//! The runner owns the side-effecting code paths (`git worktree list`,
//! `git worktree remove`, `gh pr list`, `git ls-remote`, `fs::remove_dir_all`).
//! Everything decision-related is delegated to [`super::policy`] so the
//! tests can stay hermetic.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use super::liveness::LiveProcessProbe;
use super::parse::{WorktreeEntry, parse_worktree_list};
use super::policy::{CandidateInputs, GcCandidate, PruneReason, evaluate_candidate};
use super::{DEFAULT_REMOTE, GcConfig, under_any_root};

/// Indirection for the upstream-branch / merged-PR queries so tests can
/// substitute deterministic answers.
pub trait GhClient {
    /// Return PR numbers (any state) merged for `branch`. Empty vec
    /// means "checked, none merged".
    fn merged_prs_for_branch(&self, branch: &str) -> Result<Vec<u32>, String>;
    /// Return whether the named branch still exists on the remote.
    /// `Ok(None)` means "could not check" — runner treats as inconclusive.
    fn branch_exists_on_remote(&self, remote: &str, branch: &str) -> Result<Option<bool>, String>;
}

/// Production implementation: shells out to `gh` and `git ls-remote`.
pub struct GhClientShell {
    pub repo: String,
    pub parent_repo: PathBuf,
}

impl GhClientShell {
    pub fn new(repo: impl Into<String>, parent_repo: PathBuf) -> Self {
        Self {
            repo: repo.into(),
            parent_repo,
        }
    }
}

impl GhClient for GhClientShell {
    fn merged_prs_for_branch(&self, branch: &str) -> Result<Vec<u32>, String> {
        // gh pr list --repo <repo> --state merged --search "head:<branch>" --json number
        let out = Command::new("gh")
            .args([
                "pr",
                "list",
                "--repo",
                &self.repo,
                "--state",
                "merged",
                "--search",
                &format!("head:{branch}"),
                "--json",
                "number",
            ])
            .output()
            .map_err(|e| format!("gh spawn failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "gh pr list failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Tiny ad-hoc parse: stdout is JSON like `[{"number":42},{"number":43}]`.
        // Avoid pulling serde_json into this leaf by scanning for `"number":N`.
        let mut prs = Vec::new();
        let mut rest = stdout.as_ref();
        while let Some(pos) = rest.find("\"number\":") {
            rest = &rest[pos + "\"number\":".len()..];
            let trimmed = rest.trim_start();
            let end = trimmed
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(trimmed.len());
            if end > 0
                && let Ok(n) = trimmed[..end].parse::<u32>()
            {
                prs.push(n);
            }
            rest = &trimmed[end..];
        }
        Ok(prs)
    }

    fn branch_exists_on_remote(&self, remote: &str, branch: &str) -> Result<Option<bool>, String> {
        // `git -C <parent> ls-remote --heads <remote> <branch>` — empty
        // stdout (status 0) means the branch is not on the remote.
        let out = Command::new("git")
            .args([
                "-C",
                &self.parent_repo.to_string_lossy(),
                "ls-remote",
                "--heads",
                remote,
                branch,
            ])
            .output()
            .map_err(|e| format!("git ls-remote spawn failed: {e}"))?;
        if !out.status.success() {
            // Network/auth failure: inconclusive.
            return Ok(None);
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        Ok(Some(!stdout.trim().is_empty()))
    }
}

/// Outcome of one GC pass.
#[derive(Debug, Default)]
pub struct GcReport {
    pub roots_scanned: Vec<PathBuf>,
    pub worktrees_examined: usize,
    pub candidates: Vec<GcCandidate>,
    /// Paths that were physically pruned in `--apply` mode.
    pub pruned: Vec<PathBuf>,
    /// Per-candidate failures during the prune step.
    pub failures: Vec<(PathBuf, String)>,
}

/// Format a [`PruneReason`] as a single-line operator-readable string.
pub fn render_reason(r: &PruneReason) -> String {
    match r {
        PruneReason::BranchMerged { pr_numbers } => {
            let nums: Vec<String> = pr_numbers.iter().map(|n| format!("#{n}")).collect();
            format!("branch merged ({})", nums.join(", "))
        }
        PruneReason::BranchDeletedFromOrigin => "branch deleted from origin".to_string(),
        PruneReason::IdleTooLong { age_days } => format!("idle {age_days}d"),
    }
}

/// Drive one GC pass against `cfg`. Returns the structured report; the
/// caller is responsible for printing whatever it wants on top of it.
pub fn run_gc(
    cfg: &GcConfig,
    gh: &dyn GhClient,
    probe: &dyn LiveProcessProbe,
) -> Result<GcReport, String> {
    let mut report = GcReport {
        roots_scanned: cfg.roots.clone(),
        ..Default::default()
    };

    // 1. Enumerate worktrees registered under the parent repo.
    let raw = git_capture(&cfg.parent_repo, &["worktree", "list", "--porcelain"])
        .map_err(|e| format!("git worktree list failed: {e}"))?;
    let entries = parse_worktree_list(&raw);

    // 2. For each entry whose path lives under one of the configured
    //    roots, gather inputs and run the policy.
    for entry in &entries {
        if entry.is_bare {
            continue;
        }
        if !under_any_root(&entry.path, &cfg.roots) {
            continue;
        }
        report.worktrees_examined += 1;

        let inputs = gather_inputs(entry, gh, probe, cfg.now);
        if let Some(cand) = evaluate_candidate(entry, &inputs, cfg.now, cfg.idle_days) {
            report.candidates.push(cand);
        }
    }

    // 3. In --apply mode, attempt to prune each candidate. Continue on
    //    individual failures so one bad worktree does not block GC of
    //    the others.
    if cfg.apply {
        for cand in &report.candidates {
            match prune_candidate(&cfg.parent_repo, cand, &cfg.roots) {
                Ok(()) => report.pruned.push(cand.path.clone()),
                Err(e) => report.failures.push((cand.path.clone(), e)),
            }
        }
    }

    Ok(report)
}

fn gather_inputs(
    entry: &WorktreeEntry,
    gh: &dyn GhClient,
    probe: &dyn LiveProcessProbe,
    _now: SystemTime,
) -> CandidateInputs {
    let merged_prs = if let Some(ref branch) = entry.branch {
        gh.merged_prs_for_branch(branch).unwrap_or_else(|e| {
            tracing::warn!(
                target: "simard::worktree_gc",
                error = %e,
                branch = %branch,
                "merged_prs_for_branch failed; treating as none merged",
            );
            Vec::new()
        })
    } else {
        Vec::new()
    };

    let branch_on_origin = if let Some(ref branch) = entry.branch {
        gh.branch_exists_on_remote(DEFAULT_REMOTE, branch)
            .unwrap_or_else(|e| {
                tracing::warn!(
                    target: "simard::worktree_gc",
                    error = %e,
                    branch = %branch,
                    "branch_exists_on_remote failed; treating as inconclusive",
                );
                None
            })
    } else {
        None
    };

    let last_activity = super::policy::worktree_last_activity(&entry.path);
    let has_live_process = probe.worktree_has_live_process(&entry.path);

    CandidateInputs {
        merged_prs,
        branch_on_origin,
        last_activity,
        has_live_process,
    }
}

/// Prune one candidate: `git worktree remove --force` first; on failure,
/// fall back to `fs::remove_dir_all` (gated on the canonical-prefix
/// check). Then best-effort `git worktree prune` to reconcile registry.
fn prune_candidate(
    parent_repo: &Path,
    cand: &GcCandidate,
    roots: &[PathBuf],
) -> Result<(), String> {
    let dir = &cand.path;
    let dir_str = dir.to_string_lossy();

    // Defensive: re-check canonical prefix at prune time. Cheap, and
    // protects against a scan/prune TOCTOU race that lets the path drift.
    if !under_any_root(dir, roots) {
        return Err(format!(
            "refusing to prune {} — not under configured roots",
            dir.display()
        ));
    }

    let remove_status = git_capture(parent_repo, &["worktree", "remove", "--force", &dir_str]);
    if let Err(e) = &remove_status {
        tracing::debug!(
            target: "simard::worktree_gc",
            worktree = %dir.display(),
            error = %e,
            "git worktree remove --force failed; will fall back to manual rmdir",
        );
    }

    if dir.exists() {
        // Fallback rmdir, guarded by canonical prefix.
        let canon = dir
            .canonicalize()
            .map_err(|e| format!("cannot canonicalize {} for prune: {e}", dir.display()))?;
        let mut ok = false;
        for root in roots {
            if let Ok(canon_root) = root.canonicalize()
                && canon.starts_with(&canon_root)
            {
                ok = true;
                break;
            }
        }
        if !ok {
            return Err(format!(
                "refusing to rmdir {} (canonical {}): not contained in any GC root",
                dir.display(),
                canon.display()
            ));
        }
        std::fs::remove_dir_all(&canon)
            .map_err(|e| format!("rm -rf {} failed: {e}", canon.display()))?;
    }

    if let Err(e) = git_capture(parent_repo, &["worktree", "prune"]) {
        tracing::debug!(
            target: "simard::worktree_gc",
            error = %e,
            "best-effort `git worktree prune` failed during GC",
        );
    }
    if let Some(ref branch) = cand.branch
        && let Err(e) = git_capture(parent_repo, &["branch", "-D", branch])
    {
        tracing::debug!(
            target: "simard::worktree_gc",
            error = %e,
            branch = %branch,
            "best-effort `git branch -D` failed during GC (branch may already be gone)",
        );
    }
    Ok(())
}

/// Minimal git wrapper: env-cleared, captures stdout, returns Err on
/// non-zero status with stderr in the message.
fn git_capture(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo).env_clear();
    if let Ok(p) = std::env::var("PATH") {
        cmd.env("PATH", p);
    }
    if let Ok(h) = std::env::var("HOME") {
        cmd.env("HOME", h);
    }
    let out = cmd.output().map_err(|e| format!("spawn git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
