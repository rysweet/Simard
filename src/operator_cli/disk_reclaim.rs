//! Operator subcommand `simard disk-reclaim` (issue #2704).
//!
//! Runs the agentic disk-reclamation capability. **Dry-run by default** — the
//! operator must pass `--apply` for any deletion, and `--apply` is refused when
//! running as root (exit 2). Two forms:
//!
//! ```text
//! simard disk-reclaim [--apply] [--report-json] [--target-pct <1..=99>]
//! simard disk-reclaim exec --candidates <json|@file|@-> [--apply] [--report-json]
//! ```
//!
//! The bare form invokes the analysis recipe then the guarded executor. The
//! `exec` form feeds a candidate list straight into the guarded executor (the
//! path the recipe uses internally) — **every** path is still re-vetted through
//! [`crate::disk_reclaim::vet_candidate`], so a hand-edited list cannot delete a
//! protected path.
//!
//! Exit codes: `0` success / under threshold; `1` failure (recipe/parse/exec);
//! `2` refused (`--apply` as root).

use std::io::Read;
use std::path::Path;

use crate::disk_reclaim::{
    ReclaimCandidate, ReclaimMode, ReclaimReport, ReclaimSource, is_root, reclaim_candidates,
    reclaim_pct_from_env, run_disk_reclaim,
};

use super::args::reject_extra_args;

/// Parsed flags shared by both forms.
#[derive(Debug)]
struct DiskReclaimArgs {
    apply: bool,
    report_json: bool,
    target_pct: Option<u8>,
    /// `Some(src)` iff the `exec` subform was requested; `src` is the raw
    /// `--candidates` value.
    exec_candidates: Option<String>,
}

pub(crate) fn dispatch_disk_reclaim_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_args(args)? {
        Some(p) => p,
        // --help was printed.
        None => return Ok(()),
    };

    let mode = if parsed.apply {
        ReclaimMode::Apply
    } else {
        ReclaimMode::DryRun
    };

    // Hard refusal: --apply as root would nullify the path-ownership policy.
    // Exit code 2 (refused), distinct from a run failure (1).
    if mode == ReclaimMode::Apply && is_root() {
        eprintln!(
            "[disk-reclaim] refusing --apply as root (euid 0) — it would nullify the path-ownership policy"
        );
        std::process::exit(2);
    }

    let target_pct = parsed.target_pct.unwrap_or_else(reclaim_pct_from_env);
    let state_root = crate::memory_ipc::default_state_root();

    let report = if let Some(ref src) = parsed.exec_candidates {
        // `exec` form: parse candidates and feed the guarded executor directly.
        let candidates = load_candidates(src)?;
        execute_candidates(&candidates, &state_root, mode, target_pct)
    } else {
        // Bare form: run the analysis recipe, then the guarded executor.
        let repo_root = std::env::current_dir()?;
        run_disk_reclaim(
            &repo_root,
            &state_root,
            None,
            mode,
            target_pct,
            ReclaimSource::Cli,
        )?
    };

    if parsed.report_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        render_report(&report);
    }

    Ok(())
}

/// Run the guarded executor over an already-parsed candidate list via the shared
/// production wiring in [`crate::disk_reclaim::reclaim_candidates`]. Exposed for
/// the `exec` form and its tests. Every path is still re-vetted through the
/// non-bypassable guard.
fn execute_candidates(
    candidates: &[ReclaimCandidate],
    state_root: &Path,
    mode: ReclaimMode,
    target_pct: u8,
) -> ReclaimReport {
    reclaim_candidates(
        candidates.to_vec(),
        state_root,
        mode,
        target_pct,
        ReclaimSource::Cli,
    )
}

/// Resolve the `--candidates <src>` value into a candidate list. `@file` reads a
/// file, `@-` reads stdin, anything else is treated as inline JSON. The payload
/// is a JSON array of [`ReclaimCandidate`].
fn load_candidates(src: &str) -> Result<Vec<ReclaimCandidate>, Box<dyn std::error::Error>> {
    let json = if src == "@-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else if let Some(path) = src.strip_prefix('@') {
        std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read --candidates file '{path}': {e}"))?
    } else {
        src.to_string()
    };
    let candidates: Vec<ReclaimCandidate> = serde_json::from_str(json.trim())
        .map_err(|e| format!("--candidates is not a valid ReclaimCandidate JSON array: {e}"))?;
    Ok(candidates)
}

/// Render the human report to stdout (largest-first, matching the executor).
fn render_report(report: &ReclaimReport) {
    let mode = match report.mode {
        ReclaimMode::DryRun => "dry-run",
        ReclaimMode::Apply => "apply",
    };
    println!(
        "disk-reclaim ({mode}) — home partition {}% used, target {}%",
        report.used_pct_before, report.target_pct,
    );
    for r in &report.would_remove {
        println!(
            "WOULD REMOVE  {:<17} {}  {}",
            kind_label(r.kind),
            r.path.display(),
            human_bytes(r.bytes),
        );
    }
    for r in &report.removed {
        println!(
            "REMOVED  {:<17} {}  {}",
            kind_label(r.kind),
            r.path.display(),
            human_bytes(r.bytes),
        );
    }
    for s in &report.skipped {
        println!(
            "SKIP (review) {:<17} {}  {}",
            kind_label(s.kind),
            s.path.display(),
            reason_label(s.reject_reason),
        );
    }
    for f in &report.failures {
        println!("FAILED  {}  {}", f.path.display(), f.error);
    }
    println!(
        "{mode}: {}% used after, freed {}, {} removed, {} for human review",
        report.used_pct_after,
        human_bytes(report.bytes_freed),
        report.removed.len(),
        report.skipped.len(),
    );
}

fn kind_label(kind: crate::disk_reclaim::CandidateKind) -> &'static str {
    use crate::disk_reclaim::CandidateKind::*;
    match kind {
        TrackedWorktree => "tracked_worktree",
        OrphanDir => "orphan_dir",
        StaleBuildCache => "stale_build_cache",
    }
}

fn reason_label(reason: crate::disk_reclaim::RejectReason) -> &'static str {
    use crate::disk_reclaim::RejectReason::*;
    match reason {
        ProtectedPath => "protected path",
        LiveProcess => "referenced by a live process",
        UncommittedOrUnpushed => "uncommitted or unpushed work",
        ActiveWorktree => "active recipe/engineer worktree",
        OutsideAllowRoot => "outside allow-root / symlink refused",
        UnknownPrState => "PR not confirmed merged/closed",
    }
}

fn human_bytes(bytes: u64) -> String {
    crate::disk_pressure::human_bytes(bytes)
}

fn parse_args(
    args: impl Iterator<Item = String>,
) -> Result<Option<DiskReclaimArgs>, Box<dyn std::error::Error>> {
    let mut apply = false;
    let mut report_json = false;
    let mut target_pct: Option<u8> = None;
    let mut exec_mode = false;
    let mut exec_candidates: Option<String> = None;

    let mut iter = args.peekable();
    // Optional leading `exec` subcommand.
    if iter.peek().map(String::as_str) == Some("exec") {
        exec_mode = true;
        iter.next();
    }

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{DISK_RECLAIM_HELP}");
                return Ok(None);
            }
            "--apply" => apply = true,
            "--dry-run" => apply = false,
            "--report-json" => report_json = true,
            "--candidates" => {
                let val = iter
                    .next()
                    .ok_or("--candidates requires a value (json | @file | @-)")?;
                exec_candidates = Some(val);
            }
            "--target-pct" => {
                let val = iter
                    .next()
                    .ok_or("--target-pct requires a value (1..=99)")?;
                target_pct = Some(parse_target_pct(&val)?);
            }
            other => {
                if let Some(v) = other.strip_prefix("--candidates=") {
                    exec_candidates = Some(v.to_string());
                } else if let Some(v) = other.strip_prefix("--target-pct=") {
                    target_pct = Some(parse_target_pct(v)?);
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
            }
        }
    }
    reject_extra_args(std::iter::empty::<String>())?;

    if exec_mode && exec_candidates.is_none() {
        return Err("disk-reclaim exec requires --candidates <json|@file|@->".into());
    }
    if exec_candidates.is_some() && !exec_mode {
        return Err("--candidates is only valid with the `exec` subcommand".into());
    }

    Ok(Some(DiskReclaimArgs {
        apply,
        report_json,
        target_pct,
        exec_candidates,
    }))
}

/// Parse and clamp a `--target-pct` value to `[1, 99]`.
fn parse_target_pct(raw: &str) -> Result<u8, Box<dyn std::error::Error>> {
    let n: u32 = raw
        .trim()
        .parse()
        .map_err(|_| format!("invalid --target-pct value: {raw}"))?;
    Ok(n.clamp(1, 99) as u8)
}

const DISK_RECLAIM_HELP: &str = "\
Usage:
  simard disk-reclaim [--apply] [--report-json] [--target-pct <1..=99>]
  simard disk-reclaim exec --candidates <json|@file|@-> [--apply] [--report-json]

Agentic disk reclamation: an analysis-only agent proposes reclaimable candidates
(worktrees mapped to merged/closed PRs, orphaned de-registered dirs, stale build
caches) and a deterministic Rust executor disposes of them behind non-bypassable
safety rails, largest-first, until the home partition is under the target %-used.

DRY-RUN IS THE DEFAULT — with no flags it makes ZERO destructive changes and
prints a would-remove report plus the human-review list.

  --apply            Perform guarded reclamation. Refused when running as root.
  --report-json      Emit the ReclaimReport as JSON instead of the human table.
  --target-pct N     Override SIMARD_DISK_RECLAIM_PCT for this run (clamped 1..=99).

  exec --candidates <src>
                     Feed a candidate list straight to the guarded executor:
                     @file reads a file, @- reads stdin, otherwise inline JSON.
                     Every path is still re-vetted through the protected-path
                     guard — a hand-edited list cannot delete a protected path.

Safety rails (deterministic, cannot be bypassed by the agent): never removes
worktrees/main or a daemon WorkingDirectory, a path referenced by a live PID, a
worktree with uncommitted/unpushed work not in a merged/closed PR, or an active
recipe/engineer worktree. Anything a rail refuses is reported for human review.
No --admin / --no-verify is ever passed to git.

Exit codes: 0 success / under threshold; 1 failure; 2 refused (--apply as root).
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk_reclaim::{CandidateKind, HARDCODED_PROTECTED_MAIN, RejectReason};
    use std::path::PathBuf;

    #[test]
    fn unknown_flag_is_rejected() {
        let err = parse_args(["--nope".to_string()].into_iter())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--nope"), "{err}");
    }

    #[test]
    fn target_pct_is_clamped() {
        assert_eq!(parse_target_pct("0").unwrap(), 1);
        assert_eq!(parse_target_pct("150").unwrap(), 99);
        assert_eq!(parse_target_pct("80").unwrap(), 80);
        assert!(parse_target_pct("abc").is_err());
    }

    #[test]
    fn exec_requires_candidates() {
        let err = parse_args(["exec".to_string()].into_iter())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--candidates"), "{err}");
    }

    #[test]
    fn candidates_without_exec_is_rejected() {
        let err = parse_args(["--candidates".to_string(), "[]".to_string()].into_iter())
            .unwrap_err()
            .to_string();
        assert!(err.contains("exec"), "{err}");
    }

    #[test]
    fn load_candidates_parses_inline_json() {
        let cands =
            load_candidates(r#"[{"path":"/a","kind":"orphan_dir"}]"#).expect("valid json array");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].kind, CandidateKind::OrphanDir);
    }

    #[test]
    fn load_candidates_rejects_unknown_field() {
        // deny_unknown_fields on ReclaimCandidate propagates through the exec path.
        let err = load_candidates(r#"[{"path":"/a","kind":"orphan_dir","evil":true}]"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ReclaimCandidate"), "{err}");
    }

    /// End-to-end proof through the real CLI exec path: a candidate that names
    /// the hardcoded protected `worktrees/main` is REFUSED (routed to human
    /// review), even in a hermetic tempdir state root and even though the JSON
    /// explicitly instructs its removal. Dry-run → no real `rm` of any path.
    #[test]
    fn exec_refuses_protected_main_even_when_instructed() {
        let state = tempfile::tempdir().expect("state root");
        let candidate = ReclaimCandidate {
            path: PathBuf::from(HARDCODED_PROTECTED_MAIN),
            kind: CandidateKind::TrackedWorktree,
            parent_repo: None,
            reason: Some("delete the daemon's working directory!".to_string()),
            est_bytes: Some(999_999_999),
        };
        let report = execute_candidates(&[candidate], state.path(), ReclaimMode::DryRun, 85);
        assert!(
            report.would_remove.is_empty(),
            "protected main must never be a would-remove candidate",
        );
        assert_eq!(
            report.skipped.len(),
            1,
            "protected main goes to human review"
        );
        assert_eq!(report.skipped[0].reject_reason, RejectReason::ProtectedPath);
        assert!(report.removed.is_empty(), "dry-run removes nothing");
    }

    /// A candidate outside every allow-root is refused with OutsideAllowRoot —
    /// the CLI cannot be tricked into touching an arbitrary absolute path.
    #[test]
    fn exec_refuses_path_outside_allow_roots() {
        let state = tempfile::tempdir().expect("state root");
        let outside = tempfile::tempdir().expect("some unrelated dir");
        let candidate = ReclaimCandidate {
            path: outside.path().to_path_buf(),
            kind: CandidateKind::OrphanDir,
            parent_repo: None,
            reason: None,
            est_bytes: None,
        };
        let report = execute_candidates(&[candidate], state.path(), ReclaimMode::DryRun, 85);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(
            report.skipped[0].reject_reason,
            RejectReason::OutsideAllowRoot,
        );
        assert!(outside.path().exists(), "nothing removed");
    }

    /// An orphan dir *inside* the state-root allow-root is accepted in dry-run
    /// (proves the allow path works and that dry-run still deletes nothing).
    #[test]
    fn exec_allows_orphan_inside_allow_root_but_dry_run_deletes_nothing() {
        let state = tempfile::tempdir().expect("state root");
        // engineer-worktrees is one of the allow-roots for this state root.
        let orphan = state.path().join("engineer-worktrees").join("leftover-9f3");
        std::fs::create_dir_all(&orphan).expect("orphan dir");
        let candidate = ReclaimCandidate {
            path: orphan.clone(),
            kind: CandidateKind::OrphanDir,
            parent_repo: None,
            reason: None,
            est_bytes: None,
        };
        let report = execute_candidates(&[candidate], state.path(), ReclaimMode::DryRun, 85);
        assert_eq!(
            report.would_remove.len(),
            1,
            "orphan in allow-root is reclaimable"
        );
        assert!(report.removed.is_empty(), "dry-run deletes nothing");
        assert!(
            orphan.exists(),
            "the orphan dir must still exist after dry-run"
        );
    }
}
