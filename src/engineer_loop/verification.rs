use std::path::Path;

use crate::error::{SimardError, SimardResult};

use super::inspect_workspace;
use super::types::{
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, VerificationReport,
};

/// Per #1209: an engineer worktree may legitimately rename its branch from
/// `engineer/<initial>` to `engineer/<better-name>`. Returns true when the
/// transition stays within (or starts inside) the `engineer/*` namespace.
/// Used to relax both the HEAD-stability and branch-equality checks for the
/// engineer sandbox case.
pub(crate) fn rename_within_engineer_namespace(inspection_branch: &str, post_branch: &str) -> bool {
    inspection_branch.starts_with("engineer/") && post_branch.starts_with("engineer/")
}

// Re-export per-action verifiers so `use super::verification::*` in tests still works.
pub(crate) use super::verification_actions::*;

pub(crate) fn verify_grounding_stable(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    state_root: &Path,
    checks: &mut Vec<String>,
) -> SimardResult<RepoInspection> {
    let post = inspect_workspace(&inspection.repo_root, state_root)?;

    if post.repo_root != inspection.repo_root {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "repo root changed from '{}' to '{}'",
                inspection.repo_root.display(),
                post.repo_root.display()
            ),
        });
    }
    checks.push(format!("repo-root={}", post.repo_root.display()));

    // On an isolated engineer worktree (per #1197), the engineer is sandboxed
    // on a per-goal `engineer/*` branch. HEAD movement on that branch is the
    // engineer doing legitimate work (committing, applying patches), not a
    // worktree contamination. Only enforce the strict no-HEAD-change rule
    // when the engineer is somehow running on a shared / non-engineer branch.
    //
    // Per #1209: the engineer LLM occasionally renames its branch
    // (`git checkout -b engineer/<better-name>`). Renames within the engineer/*
    // namespace are legitimate; only require both inspection and post branches
    // to be in `engineer/`. Jumping out of the engineer/ namespace is still
    // a real failure handled by the strict branch-equality check below.
    let on_engineer_branch = rename_within_engineer_namespace(&inspection.branch, &post.branch);
    match &action.selected.kind {
        EngineerActionKind::GitCommit(_) => {
            if post.head == inspection.head {
                return Err(SimardError::VerificationFailed {
                    reason: "HEAD did not change after git commit".to_string(),
                });
            }
            checks.push(format!("repo-head-changed={}", post.head));
        }
        _ if on_engineer_branch => {
            // Engineer branch: allow HEAD movement (commits within the sandbox).
            if post.head == inspection.head {
                checks.push(format!("repo-head={}", post.head));
            } else {
                checks.push(format!(
                    "repo-head-advanced-on-engineer-branch={}",
                    post.head
                ));
            }
        }
        _ => {
            if post.head != inspection.head {
                return Err(SimardError::VerificationFailed {
                    reason: format!("HEAD changed from '{}' to '{}'", inspection.head, post.head),
                });
            }
            checks.push(format!("repo-head={}", post.head));
        }
    }

    // Per #1209: allow rename within the engineer/* namespace (engineer/foo
    // -> engineer/bar is legitimate). Jumping out of engineer/* (e.g. to
    // main) is still a real failure.
    let branch_changed = post.branch != inspection.branch;
    let rename_within_engineer = rename_within_engineer_namespace(&inspection.branch, &post.branch);
    if branch_changed && !rename_within_engineer {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "branch changed from '{}' to '{}'",
                inspection.branch, post.branch
            ),
        });
    }
    checks.push(format!("repo-branch={}", post.branch));
    Ok(post)
}

pub(crate) fn verify_worktree_state(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    post: &RepoInspection,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    // Engineer worktrees (per #1197): the engineer is sandboxed on her own
    // branch in her own worktree, so file mutations from RunShellCommand
    // (e.g. git apply, sed -i) are legitimate. Only apply the strict
    // "non-mutating actions must not dirty the worktree" rule when running
    // on a shared / non-engineer branch.
    // Per #1209: rename within engineer/* namespace is legitimate.
    let on_engineer_branch = rename_within_engineer_namespace(&inspection.branch, &post.branch);
    match &action.selected.kind {
        EngineerActionKind::ReadOnlyScan
        | EngineerActionKind::CargoTest
        | EngineerActionKind::CargoCheck
        | EngineerActionKind::RunShellCommand(_)
        | EngineerActionKind::OpenIssue(_) => {
            if !on_engineer_branch
                && (post.worktree_dirty != inspection.worktree_dirty
                    || post.changed_files != inspection.changed_files)
            {
                return Err(SimardError::VerificationFailed {
                    reason: "worktree state changed during a non-mutating local engineer action"
                        .to_string(),
                });
            }
            checks.push(format!("worktree-dirty={}", post.worktree_dirty));
            checks.push(if post.changed_files.is_empty() {
                "changed-files-after-action=<none>".to_string()
            } else {
                format!(
                    "changed-files-after-action={}",
                    post.changed_files.join(", ")
                )
            });
        }
        EngineerActionKind::StructuredTextReplace(_)
        | EngineerActionKind::CreateFile(_)
        | EngineerActionKind::AppendToFile(_) => {
            if !post.worktree_dirty {
                return Err(SimardError::VerificationFailed {
                    reason: "file-mutating action succeeded but the repo still appears clean"
                        .to_string(),
                });
            }
            if post.changed_files != action.selected.expected_changed_files {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "file-mutating action changed unexpected files: expected {:?}, got {:?}",
                        action.selected.expected_changed_files, post.changed_files
                    ),
                });
            }
            if action.changed_files != action.selected.expected_changed_files {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "executed action reported changed files {:?}, expected {:?}",
                        action.changed_files, action.selected.expected_changed_files
                    ),
                });
            }
            checks.push(format!("worktree-dirty={}", post.worktree_dirty));
            checks.push(format!(
                "changed-files-after-action={}",
                post.changed_files.join(", ")
            ));
        }
        EngineerActionKind::GitCommit(_) => {
            checks.push(format!(
                "worktree-dirty-after-commit={}",
                post.worktree_dirty
            ));
        }
    }

    if post.active_goals != inspection.active_goals {
        return Err(SimardError::VerificationFailed {
            reason: "active goal set changed during a non-mutating local engineer action"
                .to_string(),
        });
    }
    checks.push(format!("active-goals={}", post.active_goals.len()));

    if post.carried_meeting_decisions != inspection.carried_meeting_decisions {
        return Err(SimardError::VerificationFailed {
            reason: "carried meeting decision memory changed during a non-mutating local engineer action".to_string(),
        });
    }
    checks.push(format!(
        "carried-meeting-decisions={}",
        post.carried_meeting_decisions.len()
    ));
    Ok(())
}

pub(crate) fn verify_kind_specific(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    match &action.selected.kind {
        EngineerActionKind::ReadOnlyScan => match action.selected.label.as_str() {
            "cargo-metadata-scan" => {
                verify_cargo_metadata(&inspection.repo_root, &action.stdout, checks)?;
            }
            "git-tracked-file-scan" => {
                if action.stdout.lines().next().is_none() {
                    return Err(SimardError::VerificationFailed {
                        reason: "git tracked-file scan returned no tracked files".to_string(),
                    });
                }
                checks.push("tracked-files-present=true".to_string());
            }
            other => {
                return Err(SimardError::VerificationFailed {
                    reason: format!("verification rules are missing for selected action '{other}'"),
                });
            }
        },
        EngineerActionKind::StructuredTextReplace(edit_request) => verify_structured_text_replace(
            &inspection.repo_root,
            edit_request,
            &action.stdout,
            checks,
        )?,
        EngineerActionKind::CargoTest => verify_cargo_test(action, checks)?,
        EngineerActionKind::CargoCheck => verify_cargo_check(action, checks),
        EngineerActionKind::CreateFile(req) => verify_create_file(inspection, req, checks)?,
        EngineerActionKind::AppendToFile(req) => verify_append_to_file(inspection, req, checks)?,
        EngineerActionKind::RunShellCommand(_) => {
            checks.push(format!("shell-command-exit-code={}", action.exit_code));
        }
        EngineerActionKind::GitCommit(_) => {
            checks.push("git-commit-created=true".to_string());
        }
        EngineerActionKind::OpenIssue(_) => verify_open_issue(action, checks)?,
    }
    Ok(())
}

pub fn verify_engineer_action(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    state_root: &Path,
) -> SimardResult<VerificationReport> {
    if action.exit_code != 0 {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "selected action '{}' exited with code {}",
                action.selected.label, action.exit_code
            ),
        });
    }

    let mut checks = Vec::new();
    let post = verify_grounding_stable(inspection, action, state_root, &mut checks)?;
    verify_worktree_state(inspection, action, &post, &mut checks)?;
    verify_kind_specific(inspection, action, &mut checks)?;

    Ok(VerificationReport {
        status: "verified".to_string(),
        summary: build_verification_summary(action),
        checks,
    })
}
