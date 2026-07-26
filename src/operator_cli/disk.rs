//! Operator/agent subcommand `simard disk` (issue #4722).
//!
//! The agent-facing surface the disk-health recipe calls to *act*. A thin,
//! **delete-free** adapter: it parses arguments, builds [`ReclaimCandidate`]
//! values (classifying `kind` conservatively), and delegates to the shared,
//! non-bypassable [`crate::disk_reclaim::reclaim_candidates`] guarded executor.
//! No deletion logic lives here — every removal is performed by the guarded core
//! after re-vetting.
//!
//! ```text
//! simard disk report  (--path <P> | --paths @<file> | --paths @-)…
//! simard disk reclaim (--path <P> | --paths @<file> | --paths @-)… [--dry-run]
//! ```
//!
//! Default mode differs from `disk-reclaim`: `simard disk reclaim` **applies**
//! (guarded delete) by default; `--dry-run` opts into vet-only. `report` is
//! always dry-run. Apply-as-root is hard-refused (exit 2).
//!
//! Exit codes: `0` handled (reclaimed or safely skipped — a per-candidate guard
//! rejection is **not** a failure); `1` operational failure; `2` refused
//! (`reclaim` apply mode as root).
//!
//! `disk_health.rs` is a thin exit-status trigger and the recipe acts via this
//! tool — see `docs/reference/simard-disk-tool.md`.

use std::path::{Path, PathBuf};

use crate::disk_reclaim::{
    CandidateKind, ReclaimCandidate, ReclaimMode, ReclaimReport, ReclaimSource, is_root,
    reclaim_candidates, reclaim_pct_from_env,
};

/// The two `simard disk` subcommands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiskSubcommand {
    /// Vet + summarise only; never deletes.
    Report,
    /// Guarded reclamation (apply by default, `--dry-run` to vet only).
    Reclaim,
}

/// Fully-parsed `simard disk` invocation. `paths` is the already-resolved
/// candidate path list (both literal `--path` values and expanded `--paths @…`
/// entries).
#[derive(Debug)]
struct DiskArgs {
    subcommand: DiskSubcommand,
    paths: Vec<PathBuf>,
    dry_run: bool,
}

/// Entry point wired from `operator_cli::mod`. Parses, refuses apply-as-root
/// (exit 2), runs the guarded core, and maps the report to a process exit code.
pub(crate) fn dispatch_disk_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_args(args)? {
        Some(p) => p,
        // --help was printed.
        None => return Ok(()),
    };

    let mode = mode_for(parsed.subcommand, parsed.dry_run);

    // Hard refusal: reclaim --apply as root would nullify the path-ownership
    // policy. Exit code 2 (refused), distinct from an operational failure (1).
    if should_refuse_apply_as_root(mode, is_root()) {
        eprintln!(
            "[disk] refusing reclaim apply mode as root (euid 0) — it would nullify the path-ownership policy"
        );
        std::process::exit(2);
    }

    let target_pct = reclaim_pct_from_env();
    let state_root = crate::memory_ipc::default_state_root();
    let candidates = build_candidates(&parsed.paths);
    let report = reclaim_candidates(
        candidates,
        &state_root,
        mode,
        target_pct,
        ReclaimSource::Cli,
    );

    render_report(&report);

    let code = exit_code_for(&report);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Map the subcommand + `--dry-run` flag to a [`ReclaimMode`]. `report` is
/// always dry-run; `reclaim` applies unless `--dry-run` was given.
fn mode_for(subcommand: DiskSubcommand, dry_run: bool) -> ReclaimMode {
    match subcommand {
        DiskSubcommand::Report => ReclaimMode::DryRun,
        DiskSubcommand::Reclaim if dry_run => ReclaimMode::DryRun,
        DiskSubcommand::Reclaim => ReclaimMode::Apply,
    }
}

/// `true` iff the run is `reclaim` apply mode **and** the process is root.
fn should_refuse_apply_as_root(mode: ReclaimMode, is_root: bool) -> bool {
    mode == ReclaimMode::Apply && is_root
}

/// One-line usage printed for `--help` / `-h`.
fn print_usage() {
    println!(
        "\
simard disk — agent-facing, guard-enforcing disk reclamation

USAGE:
    simard disk report  (--path <P> | --paths @<file> | --paths @-)…
    simard disk reclaim (--path <P> | --paths @<file> | --paths @-)… [--dry-run]

`report` vets and summarises only (never deletes). `reclaim` APPLIES a guarded
delete by default — pass `--dry-run` to vet only. Apply mode is refused as root.
Every removal is re-vetted by the shared guard; a per-candidate rejection routes
to human review and is NOT a failure. Large path lists must use `--paths @file`
(newline-delimited paths, not JSON) or `--paths @-` (stdin).

EXIT CODES:
    0  handled (reclaimed or safely skipped)
    1  operational failure
    2  refused (reclaim apply mode as root)"
    );
}

/// Parse the `simard disk` argument vector. Returns `Ok(None)` when `--help`
/// was printed. Refuses unknown flags and leading-dash `--path` values.
fn parse_args(
    args: impl Iterator<Item = String>,
) -> Result<Option<DiskArgs>, Box<dyn std::error::Error>> {
    let mut iter = args;

    let subcommand = match iter.next() {
        None => {
            return Err(
                "missing subcommand: expected `report` or `reclaim` (try `simard disk --help`)"
                    .into(),
            );
        }
        Some(s) if s == "--help" || s == "-h" => {
            print_usage();
            return Ok(None);
        }
        Some(s) => match s.as_str() {
            "report" => DiskSubcommand::Report,
            "reclaim" => DiskSubcommand::Reclaim,
            other => {
                return Err(format!(
                    "unknown subcommand `{other}`: expected `report` or `reclaim`"
                )
                .into());
            }
        },
    };

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut dry_run = false;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return Ok(None);
            }
            "--dry-run" => dry_run = true,
            "--path" => {
                let val = iter.next().ok_or("`--path` requires a value")?;
                // Argv-injection guard: a path that looks like a flag must never
                // be swallowed as a candidate.
                if val.starts_with('-') {
                    return Err(format!(
                        "refusing leading-dash `--path` value `{val}` (looks like a flag)"
                    )
                    .into());
                }
                paths.push(PathBuf::from(val));
            }
            "--paths" => {
                let src = iter
                    .next()
                    .ok_or("`--paths` requires a `@<file>` or `@-` argument")?;
                paths.extend(load_path_list(&src)?);
            }
            other => return Err(format!("unknown flag `{other}`").into()),
        }
    }

    Ok(Some(DiskArgs {
        subcommand,
        paths,
        dry_run,
    }))
}

/// Classify a candidate path conservatively. A `.git` marker always wins
/// (routes through the tracked-worktree PR rails); a `target/`-style cache is
/// `StaleBuildCache`; anything else is `OrphanDir`. `kind` is advisory — the
/// guard re-derives the real primitive — but the classification must never
/// *shorten* vetting.
fn classify_kind(path: &Path) -> CandidateKind {
    // A `.git` entry (dir for a primary worktree, file for a linked worktree)
    // forces the tracked-worktree rails regardless of the dir name. Probe with
    // `symlink_metadata` (fail-closed, matching the guard) rather than `exists()`:
    // it detects the entry itself — including a symlink — without following it,
    // so a `.git` symlink still routes through the longer tracked-worktree vetting.
    if path.join(".git").symlink_metadata().is_ok() {
        return CandidateKind::TrackedWorktree;
    }
    if path.file_name().and_then(|n| n.to_str()) == Some("target") {
        return CandidateKind::StaleBuildCache;
    }
    CandidateKind::OrphanDir
}

/// Build one [`ReclaimCandidate`] per path with a conservatively-classified
/// `kind` and no advisory fields (`parent_repo`/`reason`/`est_bytes` are all
/// `None` — the guard re-derives everything it trusts).
fn build_candidates(paths: &[PathBuf]) -> Vec<ReclaimCandidate> {
    paths
        .iter()
        .map(|p| ReclaimCandidate {
            path: p.clone(),
            kind: classify_kind(p),
            parent_repo: None,
            reason: None,
            est_bytes: None,
        })
        .collect()
}

/// Resolve a `--paths <src>` value into a path list. `@file` reads a file, `@-`
/// reads stdin. The payload is **newline-delimited paths** (one per line) — NOT
/// JSON. Blank lines and `#` comment lines are ignored. This is deliberately a
/// different loader from `disk-reclaim exec --candidates` (which reads JSON).
fn load_path_list(src: &str) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let spec = src.strip_prefix('@').ok_or_else(|| {
        format!("`--paths` value must start with `@` (use `@<file>` or `@-`), got `{src}`")
    })?;
    let buf = if spec == "-" {
        std::io::read_to_string(std::io::stdin())?
    } else {
        std::fs::read_to_string(spec)?
    };
    Ok(parse_path_lines(&buf))
}

/// Read newline-delimited paths from an already-read buffer, skipping blank and
/// `#` comment lines. Split out so file/stdin ingestion share one parser.
fn parse_path_lines(buf: &str) -> Vec<PathBuf> {
    buf.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect()
}

/// Map a completed [`ReclaimReport`] to a process exit code. `0` when the run
/// handled every candidate (reclaimed or safely skipped for review); `1` when
/// any operational failure occurred. Apply-as-root refusal (`2`) is handled
/// before the run, not here.
fn exit_code_for(report: &ReclaimReport) -> i32 {
    if report.failures.is_empty() { 0 } else { 1 }
}

/// Human-readable one-line render of the run (no JSON envelope — the recipe
/// interprets the tool by exit status alone).
fn render_report(report: &ReclaimReport) {
    println!("{}", report.summary());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk_reclaim::{
        HARDCODED_PROTECTED_MAIN, ReclaimFailure, ReclaimReport, RejectReason,
    };

    // ================================================================
    // parse_args — argument grammar
    // ================================================================

    #[test]
    fn report_with_single_path() {
        let parsed = parse_args(
            [
                "report".to_string(),
                "--path".to_string(),
                "/some/dir".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(parsed.subcommand, DiskSubcommand::Report);
        assert_eq!(parsed.paths, vec![PathBuf::from("/some/dir")]);
        assert!(!parsed.dry_run, "report is not a --dry-run flag carrier");
    }

    #[test]
    fn reclaim_defaults_to_apply_mode_not_dry_run() {
        let parsed = parse_args(
            [
                "reclaim".to_string(),
                "--path".to_string(),
                "/a".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(parsed.subcommand, DiskSubcommand::Reclaim);
        assert!(
            !parsed.dry_run,
            "bare `reclaim` must default to apply (dry_run=false)"
        );
    }

    #[test]
    fn reclaim_dry_run_flag_is_parsed() {
        let parsed = parse_args(
            [
                "reclaim".to_string(),
                "--path".to_string(),
                "/a".to_string(),
                "--dry-run".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse ok")
        .expect("not --help");
        assert!(parsed.dry_run, "--dry-run must set dry_run=true");
    }

    #[test]
    fn multiple_path_flags_accumulate() {
        let parsed = parse_args(
            [
                "reclaim".to_string(),
                "--path".to_string(),
                "/a".to_string(),
                "--path".to_string(),
                "/b".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(parsed.paths, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let err = parse_args(["reclaim".to_string(), "--nope".to_string()].into_iter())
            .unwrap_err()
            .to_string();
        assert!(err.contains("--nope"), "{err}");
    }

    #[test]
    fn missing_subcommand_is_rejected() {
        let err = parse_args(["--path".to_string(), "/a".to_string()].into_iter())
            .unwrap_err()
            .to_string();
        assert!(
            err.to_lowercase().contains("report") || err.to_lowercase().contains("reclaim"),
            "error should name the valid subcommands: {err}"
        );
    }

    #[test]
    fn leading_dash_path_value_is_refused() {
        // A path that looks like a flag must not be swallowed as a candidate —
        // this is the argv-injection guard (`--` terminates flag parsing).
        let err = parse_args(
            [
                "reclaim".to_string(),
                "--path".to_string(),
                "--rf".to_string(),
            ]
            .into_iter(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("--rf") || err.to_lowercase().contains("dash"),
            "leading-dash path must be refused: {err}"
        );
    }

    #[test]
    fn paths_from_file_are_expanded_into_the_path_list() {
        let tmp = tempfile::tempdir().unwrap();
        let list = tmp.path().join("candidates.txt");
        std::fs::write(&list, "/one\n/two\n").unwrap();
        let parsed = parse_args(
            [
                "reclaim".to_string(),
                "--paths".to_string(),
                format!("@{}", list.display()),
            ]
            .into_iter(),
        )
        .expect("parse ok")
        .expect("not --help");
        assert_eq!(
            parsed.paths,
            vec![PathBuf::from("/one"), PathBuf::from("/two")]
        );
    }

    // ================================================================
    // load_path_list / parse_path_lines — newline-delimited, NOT JSON
    // ================================================================

    #[test]
    fn parse_path_lines_skips_blank_and_comment_lines() {
        let buf = "\
# a comment
/home/azureuser/.simard/engineer-worktrees/x

  /home/azureuser/.simard/engineer-worktrees/y  
# trailing comment
";
        let paths = parse_path_lines(buf);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/azureuser/.simard/engineer-worktrees/x"),
                PathBuf::from("/home/azureuser/.simard/engineer-worktrees/y"),
            ],
            "blank and #-comment lines must be ignored; values trimmed"
        );
    }

    #[test]
    fn load_path_list_reads_a_file_via_at_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let list = tmp.path().join("c.txt");
        std::fs::write(&list, "/p1\n/p2\n").unwrap();
        let paths = load_path_list(&format!("@{}", list.display())).expect("reads file");
        assert_eq!(paths, vec![PathBuf::from("/p1"), PathBuf::from("/p2")]);
    }

    #[test]
    fn load_path_list_is_not_json() {
        // A JSON array must NOT be accepted as a path list — the loaders are
        // deliberately distinct from `disk-reclaim exec --candidates`.
        let tmp = tempfile::tempdir().unwrap();
        let list = tmp.path().join("c.txt");
        std::fs::write(&list, r#"[{"path":"/a","kind":"orphan_dir"}]"#).unwrap();
        let paths = load_path_list(&format!("@{}", list.display())).expect("reads file");
        assert_ne!(
            paths,
            vec![PathBuf::from("/a")],
            "the JSON payload must not be parsed as a single '/a' path",
        );
    }

    // ================================================================
    // classify_kind — conservative classification (.git wins)
    // ================================================================

    #[test]
    fn classify_dir_with_git_subdir_is_tracked_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("some-worktree");
        std::fs::create_dir_all(wt.join(".git")).unwrap();
        assert_eq!(classify_kind(&wt), CandidateKind::TrackedWorktree);
    }

    #[test]
    fn classify_dir_with_git_file_is_tracked_worktree() {
        // A linked git worktree has a `.git` FILE (a gitdir pointer), not a dir.
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("linked-wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            wt.join(".git"),
            "gitdir: /somewhere/.git/worktrees/linked-wt",
        )
        .unwrap();
        assert_eq!(
            classify_kind(&wt),
            CandidateKind::TrackedWorktree,
            "a .git file (linked worktree) must classify as TrackedWorktree",
        );
    }

    #[test]
    fn classify_target_dir_is_stale_build_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        assert_eq!(classify_kind(&target), CandidateKind::StaleBuildCache);
    }

    #[test]
    fn classify_plain_dir_is_orphan_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("leftover-9f3");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(classify_kind(&dir), CandidateKind::OrphanDir);
    }

    #[test]
    fn build_candidates_classifies_and_leaves_advisory_fields_none() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(wt.join(".git")).unwrap();
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();

        let cands = build_candidates(&[wt.clone(), plain.clone()]);
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].path, wt);
        assert_eq!(cands[0].kind, CandidateKind::TrackedWorktree);
        assert_eq!(cands[1].kind, CandidateKind::OrphanDir);
        for c in &cands {
            assert!(c.parent_repo.is_none(), "parent_repo is guard-derived");
            assert!(c.reason.is_none(), "reason is not agent-trusted");
            assert!(c.est_bytes.is_none(), "size is re-measured by the guard");
        }
    }

    // ================================================================
    // mode_for / should_refuse_apply_as_root — mode + root refusal
    // ================================================================

    #[test]
    fn report_is_always_dry_run() {
        assert_eq!(mode_for(DiskSubcommand::Report, false), ReclaimMode::DryRun);
        assert_eq!(mode_for(DiskSubcommand::Report, true), ReclaimMode::DryRun);
    }

    #[test]
    fn reclaim_mode_follows_dry_run_flag() {
        assert_eq!(mode_for(DiskSubcommand::Reclaim, false), ReclaimMode::Apply);
        assert_eq!(mode_for(DiskSubcommand::Reclaim, true), ReclaimMode::DryRun);
    }

    #[test]
    fn apply_as_root_is_refused_dry_run_and_non_root_are_not() {
        assert!(
            should_refuse_apply_as_root(ReclaimMode::Apply, true),
            "reclaim apply as root -> refuse (exit 2)"
        );
        assert!(
            !should_refuse_apply_as_root(ReclaimMode::Apply, false),
            "apply as non-root is allowed"
        );
        assert!(
            !should_refuse_apply_as_root(ReclaimMode::DryRun, true),
            "dry-run as root is allowed (no deletion)"
        );
    }

    // ================================================================
    // exit_code_for — 0 handled / 1 operational failure
    // ================================================================

    fn empty_report(mode: ReclaimMode) -> ReclaimReport {
        ReclaimReport {
            mode,
            used_pct_before: 90,
            used_pct_after: 90,
            target_pct: 85,
            bytes_freed: 0,
            removed: vec![],
            would_remove: vec![],
            skipped: vec![],
            failures: vec![],
        }
    }

    #[test]
    fn exit_code_zero_when_no_failures() {
        let report = empty_report(ReclaimMode::Apply);
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn exit_code_zero_when_only_skipped_for_review() {
        // A per-candidate guard rejection is NOT a tool failure.
        let mut report = empty_report(ReclaimMode::Apply);
        report.skipped.push(crate::disk_reclaim::SkippedPath {
            path: PathBuf::from("/home/azureuser/src/Simard/worktrees/main"),
            kind: CandidateKind::TrackedWorktree,
            reject_reason: RejectReason::ProtectedPath,
        });
        assert_eq!(
            exit_code_for(&report),
            0,
            "skips route to human review and still exit 0"
        );
    }

    #[test]
    fn exit_code_one_when_a_removal_failed() {
        let mut report = empty_report(ReclaimMode::Apply);
        report.failures.push(ReclaimFailure {
            path: PathBuf::from("/home/azureuser/.simard/engineer-worktrees/x"),
            error: "rm failed: EACCES".to_string(),
        });
        assert_eq!(exit_code_for(&report), 1);
    }

    // ================================================================
    // End-to-end safety: adapter -> guarded core enforces the heuristic
    // (these run through the real reclaim_candidates; dry-run deletes nothing)
    // ================================================================

    #[test]
    fn reclaim_refuses_protected_main_even_when_named() {
        let state = tempfile::tempdir().expect("state root");
        let candidates = build_candidates(&[PathBuf::from(HARDCODED_PROTECTED_MAIN)]);
        let report = reclaim_candidates(
            candidates,
            state.path(),
            ReclaimMode::DryRun,
            85,
            ReclaimSource::Cli,
        );
        assert!(
            report.would_remove.is_empty(),
            "protected main is never removable"
        );
        assert_eq!(report.skipped.len(), 1, "protected main -> human review");
        assert_eq!(report.skipped[0].reject_reason, RejectReason::ProtectedPath);
        assert!(report.removed.is_empty());
        // A pure guard rejection is not a failure.
        assert_eq!(exit_code_for(&report), 0);
    }

    #[test]
    fn reclaim_refuses_path_outside_allow_roots() {
        let state = tempfile::tempdir().expect("state root");
        let outside = tempfile::tempdir().expect("unrelated dir");
        let candidates = build_candidates(&[outside.path().to_path_buf()]);
        let report = reclaim_candidates(
            candidates,
            state.path(),
            ReclaimMode::DryRun,
            85,
            ReclaimSource::Cli,
        );
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(
            report.skipped[0].reject_reason,
            RejectReason::OutsideAllowRoot
        );
        assert!(outside.path().exists(), "nothing removed");
    }

    #[test]
    fn dry_run_performs_zero_destructive_ops() {
        let state = tempfile::tempdir().expect("state root");
        // engineer-worktrees is an allow-root for this state root.
        let orphan = state.path().join("engineer-worktrees").join("leftover-9f3");
        std::fs::create_dir_all(&orphan).unwrap();
        let candidates = build_candidates(std::slice::from_ref(&orphan));
        let report = reclaim_candidates(
            candidates,
            state.path(),
            ReclaimMode::DryRun,
            85,
            ReclaimSource::Cli,
        );
        assert_eq!(
            report.would_remove.len(),
            1,
            "orphan in allow-root is reclaimable"
        );
        assert!(report.removed.is_empty(), "dry-run removes nothing");
        assert!(
            orphan.exists(),
            "the orphan dir must still exist after dry-run"
        );
    }
}
