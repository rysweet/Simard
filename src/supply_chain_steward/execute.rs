//! Drive a [`Decision`] to completion behind mockable traits (issue #2741).
//!
//! [`execute`] performs the I/O the pure [`super::decide`] deliberately does
//! not: it files the tracking issue, applies the bump / writes the justified
//! ignore / escalates, opens the remediation PR, and self-merges only its own
//! green-CI PR. All GitHub / cargo / git effects go through [`SupplyChainGh`]
//! and the ignore-file writes through [`IgnoreFiles`], so the whole path is
//! unit-testable.
//!
//! ## Hard-rail ordering (enforced here, not in `decide`)
//!
//! For a [`Decision::JustifiedIgnore`], `execute` files the tracking issue
//! **first**; only once an issue URL exists does it write the ignore to both
//! files, embedding that URL, and then open a PR that **commits** those edits
//! (never self-merged — a security suppression stays under human review). If
//! issue filing yields no URL it returns
//! [`SimardError::SupplyChainSuppressionWithoutTracker`] and writes **no**
//! ignore — so the reasoner can never silently suppress an advisory.
//!
//! Every mutating remediation ([`Decision::Bump`] and
//! [`Decision::JustifiedIgnore`]) first resets the working tree to the scan's
//! pristine base via [`SupplyChainGh::reset_to_scan_base`], so each advisory's
//! PR branch and commit contains only its own change even when a single scan
//! remediates several advisories.

use crate::error::{SimardError, SimardResult};
use crate::stewardship::gh_client::IssueMutationTransport;
use crate::stewardship::mutation_guard::MutationGuard;
use crate::stewardship::{
    ArtifactProvenance, CycleId, GhIssue, IssueMutationIdentity, IssueMutationOutcome,
    IssueMutationRequest, LineageId, MergeOutcome,
};

use super::config::IgnoreFiles;
use super::gh::{OpenedPr, PrSpec, SupplyChainGh};
use super::types::{Advisory, Decision};

/// The concrete result of remediating one advisory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemediationOutcome {
    /// Opened a bump PR (and self-merged it when CI passed and a bot token was
    /// present).
    OpenedBumpPr {
        pr_number: u32,
        url: String,
        merged: bool,
    },
    /// Filed the tracking issue, wrote the justified ignore to both files, and
    /// opened a PR that commits those edits (never self-merged — a security
    /// suppression is left for human review).
    FiledJustifiedIgnore {
        advisory_id: String,
        issue_url: String,
        pr_number: u32,
        pr_url: String,
    },
    /// Filed the tracking issue only; no PR, no ignore.
    Escalated {
        advisory_id: String,
        issue_url: String,
    },
    /// Matched an existing tracking issue, or an already-mitigated advisory —
    /// nothing to do.
    Skipped { advisory_id: String, reason: String },
}

/// Remediate one advisory per its decided action. See the module docs for the
/// hard-rail ordering enforced for `JustifiedIgnore`.
pub fn execute(
    decision: Decision,
    advisory: &Advisory,
    files: &IgnoreFiles,
    gh: &dyn SupplyChainGh,
    guard: &mut MutationGuard,
    cycle_id: &CycleId,
) -> SimardResult<RemediationOutcome> {
    gh.validate_repo_target()?;
    let signature = signature_for(advisory);

    match decision {
        Decision::NoAction => Ok(RemediationOutcome::Skipped {
            advisory_id: advisory.id.clone(),
            reason: "already mitigated: justified ignore with no upstream fix".to_string(),
        }),

        Decision::Bump {
            crate_name,
            from,
            to,
        } => execute_bump(
            advisory,
            &signature,
            &crate_name,
            &from,
            &to,
            files,
            gh,
            guard,
            cycle_id,
        ),

        Decision::JustifiedIgnore {
            advisory_id,
            crate_name,
            reason,
        } => execute_justified_ignore(
            advisory,
            &signature,
            &advisory_id,
            &crate_name,
            &reason,
            files,
            gh,
            guard,
            cycle_id,
        ),

        Decision::Escalate {
            advisory_id,
            reason,
        } => {
            let title = format!("[supply-chain] {advisory_id}: manual remediation required");
            let body = issue_body(
                &signature,
                advisory,
                &format!("Escalation — a fix exists but cannot be auto-applied here: {reason}"),
            );
            let issue = guarded_issue_create(
                gh,
                guard,
                cycle_id,
                advisory,
                "escalate",
                &title,
                &body,
                &labels(false),
            )?;
            Ok(RemediationOutcome::Escalated {
                advisory_id,
                issue_url: issue.url,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_bump(
    advisory: &Advisory,
    signature: &str,
    crate_name: &str,
    from: &str,
    to: &str,
    files: &IgnoreFiles,
    gh: &dyn SupplyChainGh,
    guard: &mut MutationGuard,
    cycle_id: &CycleId,
) -> SimardResult<RemediationOutcome> {
    let title = format!(
        "[supply-chain] {}: bump {crate_name} {from} → {to}",
        advisory.id
    );
    let body = issue_body(
        signature,
        advisory,
        &format!(
            "A patched version is available; opening a minimal bump PR (`cargo update -p {crate_name} --precise {to}`)."
        ),
    );
    let issue = guarded_issue_create(
        gh,
        guard,
        cycle_id,
        advisory,
        "bump",
        &title,
        &body,
        &labels(false),
    )?;

    // Start every remediation from the pristine scan base so this PR's commit
    // contains only this advisory's change (no cross-contamination when a single
    // scan remediates several advisories).
    gh.reset_to_scan_base()?;
    gh.cargo_update_precise(crate_name, to)?;
    // A fix has shipped — any ignore previously added as "no fix" is now stale.
    files.remove_ignore(&advisory.id)?;

    let can_trigger = gh.has_ci_trigger_token();
    let branch = branch_name(&advisory.id);
    let pr_title = format!("chore(deps): {} — bump {crate_name} to {to}", advisory.id);
    let pr_body = pr_body(advisory, to, &issue.url, can_trigger);
    let spec = PrSpec {
        branch,
        title: pr_title,
        body: pr_body,
        labels: labels(!can_trigger),
    };
    let pr = open_remediation_pr(gh, &spec)?;

    // Self-merge only when the PR's CI can (and did) run green. A PR whose CI
    // cannot run is never self-merged.
    let merged = if pr.ci_will_run {
        matches!(
            gh.self_merge_if_green(pr.number)?,
            MergeOutcome::Merged { .. }
        )
    } else {
        false
    };

    Ok(RemediationOutcome::OpenedBumpPr {
        pr_number: pr.number,
        url: pr.url,
        merged,
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_justified_ignore(
    advisory: &Advisory,
    signature: &str,
    advisory_id: &str,
    crate_name: &str,
    reason: &str,
    files: &IgnoreFiles,
    gh: &dyn SupplyChainGh,
    guard: &mut MutationGuard,
    cycle_id: &CycleId,
) -> SimardResult<RemediationOutcome> {
    // 1. File the tracking issue FIRST.
    let title = format!(
        "[supply-chain] {advisory_id}: no upstream fix for {crate_name} — justified ignore"
    );
    let body = issue_body(
        signature,
        advisory,
        "No fixed upstream release exists; adding a justified, tracked ignore to deny.toml and .cargo/audit.toml.",
    );
    let issue = guarded_issue_create(
        gh,
        guard,
        cycle_id,
        advisory,
        "justified-ignore",
        &title,
        &body,
        &labels(false),
    )?;

    // 2. Hard rail: never write an ignore without a tracker URL.
    if issue.url.trim().is_empty() {
        return Err(SimardError::SupplyChainSuppressionWithoutTracker {
            advisory_id: advisory_id.to_string(),
        });
    }

    // 3. Reset to the pristine scan base, then write the ignore to BOTH files,
    //    embedding the issue URL.
    gh.reset_to_scan_base()?;
    files.add_justified_ignore(advisory_id, reason, &issue.url)?;

    // 4. Open a PR that COMMITS the ignore edits. Without this the edits live
    //    only in the ephemeral scan checkout and are discarded, so the two
    //    advisory gates would never actually receive the ignore. Deliberately
    //    NOT self-merged: suppressing a security advisory is a judgement call
    //    that stays under human review.
    let can_trigger = gh.has_ci_trigger_token();
    let spec = PrSpec {
        branch: branch_name(advisory_id),
        title: format!("chore(deps): {advisory_id} — justified ignore for {crate_name}"),
        body: ignore_pr_body(advisory, &issue.url, can_trigger),
        labels: labels(!can_trigger),
    };
    let pr = open_remediation_pr(gh, &spec)?;

    Ok(RemediationOutcome::FiledJustifiedIgnore {
        advisory_id: advisory_id.to_string(),
        issue_url: issue.url,
        pr_number: pr.number,
        pr_url: pr.url,
    })
}

#[allow(clippy::too_many_arguments)]
fn guarded_issue_create(
    gh: &dyn SupplyChainGh,
    guard: &mut MutationGuard,
    cycle_id: &CycleId,
    advisory: &Advisory,
    action: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> SimardResult<GhIssue> {
    let request = IssueMutationRequest::create_with_labels(
        gh.repo(),
        IssueMutationIdentity::new(format!(
            "supply-chain:{}:{action}",
            advisory.id.to_ascii_lowercase()
        ))?,
        ArtifactProvenance::system(LineageId::new("supply-chain-steward")?),
        title,
        body,
        labels.to_vec(),
    )?;
    let transport = SupplyChainIssueTransport(gh);
    match guard.execute(cycle_id, &request, &transport)? {
        IssueMutationOutcome::Completed { issue }
        | IssueMutationOutcome::AlreadyCompleted { issue } => Ok(issue),
    }
}

struct SupplyChainIssueTransport<'a>(&'a dyn SupplyChainGh);

impl IssueMutationTransport for SupplyChainIssueTransport<'_> {
    fn create_issue(
        &self,
        _repo: &str,
        identity: &IssueMutationIdentity,
        title: &str,
        body: &str,
        labels: &[String],
        _assignees: &[String],
    ) -> SimardResult<GhIssue> {
        let body = format!(
            "{body}\n\nsimard-mutation-id: {}\nsimard-provenance: stewardship\n",
            identity.as_str()
        );
        self.0.create_issue(title, &body, labels)
    }
}

fn open_remediation_pr(gh: &dyn SupplyChainGh, spec: &PrSpec) -> SimardResult<OpenedPr> {
    gh.prepare_remediation_pr(spec)?;
    gh.push_remediation_branch(&spec.branch)?;
    gh.create_remediation_pr(spec)
}

/// Deterministic dedup signature for an advisory (ID + affected crate).
fn signature_for(advisory: &Advisory) -> String {
    IssueMutationIdentity::from_source(
        "supply-chain-advisory",
        &format!("{}\0{}", advisory.id, advisory.crate_name),
    )
    .as_str()
    .to_string()
}

/// Deterministic remediation branch name, so a re-run updates the same branch.
fn branch_name(advisory_id: &str) -> String {
    format!("chore/advisory-{}", advisory_id.to_lowercase())
}

/// Standard issue labels; adds `needs-CI-trigger` when the PR's CI cannot run.
fn labels(needs_ci_trigger: bool) -> Vec<String> {
    let mut v = vec!["supply-chain".to_string()];
    if needs_ci_trigger {
        v.push("needs-CI-trigger".to_string());
    }
    v
}

fn issue_body(signature: &str, advisory: &Advisory, action: &str) -> String {
    format!(
        "filed-by: simard-supply-chain-steward\n\
         stewardship-signature: {signature}\n\
         advisory: {id}\n\
         crate: {krate}\n\
         installed: {installed}\n\
         \n\
         ## {title}\n\
         {url}\n\
         \n\
         ## Action\n\
         {action}\n",
        id = advisory.id,
        krate = advisory.crate_name,
        installed = advisory.installed,
        title = advisory.title,
        url = advisory.url,
        action = action,
    )
}

fn pr_body(advisory: &Advisory, to: &str, issue_url: &str, can_trigger: bool) -> String {
    let ci_note = if can_trigger {
        "This PR was opened with a CI-triggering token and self-merges once every required check is green."
    } else {
        "No CI-triggering bot token was configured, so this PR is labelled `needs-CI-trigger` and will NOT self-merge — re-trigger CI and merge manually."
    };
    format!(
        "Proactive supply-chain remediation for **{id}** ({krate} → {to}).\n\n\
         Tracking issue: {issue_url}\n\n\
         Applied: `cargo update -p {krate} --precise {to}`.\n\n\
         {ci_note}\n",
        id = advisory.id,
        krate = advisory.crate_name,
        to = to,
        issue_url = issue_url,
        ci_note = ci_note,
    )
}

/// PR body for a `JustifiedIgnore` remediation — a tracked, no-fix suppression
/// added to both advisory gates. Never self-merged; the note tells reviewers
/// why the PR exists and (when no CI-triggering token was present) that its CI
/// must be re-triggered before a human merges it.
fn ignore_pr_body(advisory: &Advisory, issue_url: &str, can_trigger: bool) -> String {
    let ci_note = if can_trigger {
        "This PR was opened with a CI-triggering token; review the justification and merge once every required check is green. It is NOT self-merged — a security suppression stays under human review."
    } else {
        "No CI-triggering bot token was configured, so this PR is labelled `needs-CI-trigger`: re-trigger CI, review the justification, and merge manually. It is never self-merged."
    };
    format!(
        "Proactive supply-chain remediation for **{id}** ({krate}).\n\n\
         No fixed upstream release exists, so a justified, tracked ignore is added \
         to `deny.toml` and `.cargo/audit.toml` (kept in sync).\n\n\
         Tracking issue: {issue_url}\n\n\
         {ci_note}\n",
        id = advisory.id,
        krate = advisory.crate_name,
        issue_url = issue_url,
        ci_note = ci_note,
    )
}
