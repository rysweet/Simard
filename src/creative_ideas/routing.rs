//! Routing of reviewed ideas (design spike #2419).
//!
//! Once synthesis has set an idea's status, routing dispatches it:
//! - accepted + not flagged → a **goal** ([`route_idea_to_goal`]);
//! - accepted but flagged → a **GitHub issue** tagging the owner
//!   ([`route_idea_to_issue`]);
//! - a PR arising from a creative-idea goal → the **human-review gate**
//!   ([`mark_idea_pr`]): draft + blocking label + owner review-requested,
//!   enforced by standard GitHub mechanisms — **never** `--admin`/`--no-verify`.
//!
//! All side effects go through the [`IdeaGhClient`] seam (a fake in tests). The
//! real `gh` subprocess implementation is a marked `// FUTURE:` stub (M4).
#![allow(dead_code)]

use crate::cognitive_memory::creative_idea::{CreativeIdea, IdeaStatus};
use crate::creative_ideas::{
    CREATIVE_IDEA_ISSUE_LABEL, CREATIVE_IDEA_OWNER, CREATIVE_IDEA_PR_LABEL,
};
use crate::error::{SimardError, SimardResult};
use crate::goals::{GoalRecord, GoalStatus, GoalStore, goal_slug};
use crate::improvements::EvidenceRef;
use crate::session::{SessionId, SessionPhase};
use crate::stewardship::gh_client::GhIssue;

/// Owner identity recorded on goals minted from creative ideas.
const CREATIVE_IDEA_GOAL_OWNER_IDENTITY: &str = "simard";
/// Default priority for a freshly proposed creative-idea goal.
const CREATIVE_IDEA_GOAL_PRIORITY: u8 = 3;

/// The `gh` extension seam needed by the human-review gate.
///
/// The live [`GhClient`](crate::stewardship::gh_client::GhClient) only exposes
/// `search_issues`/`create_issue`; this seam adds labeled+assigned issue
/// creation and the PR draft/label/review-request operations without mutating
/// the daemon's `gh` tooling. FUTURE (M4): a real subprocess impl.
pub trait IdeaGhClient {
    /// Create an issue with labels and assignees.
    fn create_labeled_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
        assignees: &[&str],
    ) -> SimardResult<GhIssue>;
    /// Set (or clear) a PR's draft state.
    fn set_pr_draft(&self, repo: &str, pr: u64, draft: bool) -> SimardResult<()>;
    /// Add a label to a PR.
    fn add_pr_label(&self, repo: &str, pr: u64, label: &str) -> SimardResult<()>;
    /// Request a review from `reviewer` on a PR.
    fn request_pr_review(&self, repo: &str, pr: u64, reviewer: &str) -> SimardResult<()>;
}

/// The human-review gate applied to a creative-idea PR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdeaPrGate {
    /// Always `true`: the PR is kept as a DRAFT (cannot be merged until ready).
    pub draft: bool,
    /// The merge-blocking label.
    pub blocking_label: &'static str,
    /// Reviewers whose approval is required (the repo owner).
    pub review_requested_from: Vec<String>,
    /// The originating idea's `node_id` (link back).
    pub originating_idea: String,
}

/// Accepted, not flagged → a Goal.
///
/// Produces a `Proposed` [`GoalRecord`] with `slug = goal_slug(idea.idea)`,
/// tagged in its evidence with the originating `idea.node_id` (traceability).
/// Preconditions: the idea must be in `AcceptedForImplementation`; otherwise
/// [`SimardError::InvalidIdeaTransition`] (only accepted ideas become goals).
/// `now_epoch` (unix seconds) is the injected-clock convention used by the
/// thread's `tick`, so timestamps are deterministic in tests.
pub fn route_idea_to_goal(
    idea: &CreativeIdea,
    goals: &dyn GoalStore,
    now_epoch: u64,
) -> SimardResult<GoalRecord> {
    if idea.status != IdeaStatus::AcceptedForImplementation {
        return Err(SimardError::InvalidIdeaTransition {
            from: idea.status,
            to: IdeaStatus::ImplementationStarted,
        });
    }

    let evidence = vec![EvidenceRef::raw(format!(
        "creative-idea:{}@epoch={now_epoch}",
        idea.node_id
    ))];
    let record = GoalRecord {
        slug: goal_slug(&idea.idea),
        title: idea.idea.clone(),
        rationale: creative_idea_rationale(idea),
        status: GoalStatus::Proposed,
        priority: CREATIVE_IDEA_GOAL_PRIORITY,
        owner_identity: CREATIVE_IDEA_GOAL_OWNER_IDENTITY.to_string(),
        source_session_id: creative_idea_session_id(),
        updated_in: SessionPhase::Planning,
        evidence,
        labels: vec![crate::goal_curation::labels::SOURCE_CREATIVE_IDEAS.to_string()],
    };
    goals.put(record.clone())?;
    Ok(record)
}

/// Accepted but flagged → a GitHub Issue tagging the owner.
///
/// Applies to ideas in `NeedsHumanReview`. Creates an issue labeled
/// [`CREATIVE_IDEA_ISSUE_LABEL`] and assigned to [`CREATIVE_IDEA_OWNER`]; the
/// body embeds `idea.node_id` for traceability.
pub fn route_idea_to_issue(
    idea: &CreativeIdea,
    gh: &dyn IdeaGhClient,
    repo: &str,
) -> SimardResult<GhIssue> {
    let title = format!("[creative-idea] {}", idea.idea);
    let body = format!(
        "originating-idea: {node}\n\n{rationale}\n\nRouted for human review by the Creative Ideas thread (#2419).",
        node = idea.node_id,
        rationale = creative_idea_rationale(idea),
    );
    gh.create_labeled_issue(
        repo,
        &title,
        &body,
        &[CREATIVE_IDEA_ISSUE_LABEL],
        &[CREATIVE_IDEA_OWNER],
    )
}

/// Apply the human-review gate to a PR arising from a creative-idea goal.
///
/// Enforced by three standard GitHub mechanisms — **never** `--admin`/
/// `--no-verify`: (1) keep the PR a **draft**, (2) add the merge-blocking
/// label [`CREATIVE_IDEA_PR_LABEL`], (3) **request** the owner's review
/// ([`CREATIVE_IDEA_OWNER`]). `repo` (`owner/name`) is required because each
/// `IdeaGhClient` PR method is repo-scoped.
pub fn mark_idea_pr(
    pr_number: u64,
    idea: &CreativeIdea,
    gh: &dyn IdeaGhClient,
    repo: &str,
) -> SimardResult<IdeaPrGate> {
    gh.set_pr_draft(repo, pr_number, true)?;
    gh.add_pr_label(repo, pr_number, CREATIVE_IDEA_PR_LABEL)?;
    gh.request_pr_review(repo, pr_number, CREATIVE_IDEA_OWNER)?;
    Ok(IdeaPrGate {
        draft: true,
        blocking_label: CREATIVE_IDEA_PR_LABEL,
        review_requested_from: vec![CREATIVE_IDEA_OWNER.to_string()],
        originating_idea: idea.node_id.clone(),
    })
}

/// Outcome feedback: move an idea to `ImplementationCompleted`.
///
/// Refuses (returns [`SimardError::InvalidIdeaTransition`]) unless the idea is
/// in `ImplementationStarted` **and** `metric_met` is true — so completion
/// fires only when both the PR merges through the normal gate and the idea's
/// own `success_metric` is met.
pub fn mark_completed(idea: &mut CreativeIdea, metric_met: bool) -> SimardResult<()> {
    if idea.status != IdeaStatus::ImplementationStarted || !metric_met {
        return Err(SimardError::InvalidIdeaTransition {
            from: idea.status,
            to: IdeaStatus::ImplementationCompleted,
        });
    }
    idea.try_transition(IdeaStatus::ImplementationCompleted)
}

fn creative_idea_rationale(idea: &CreativeIdea) -> String {
    if idea.context.rationale.is_empty() {
        format!(
            "Creative idea generated by the Creative Ideas thread: {}",
            idea.idea
        )
    } else {
        idea.context.rationale.clone()
    }
}

/// A fixed sentinel session id for goals minted by the (non-interactive)
/// Creative Ideas thread. The nil UUID marks "no originating interactive
/// session"; traceability is carried by the evidence tag instead.
fn creative_idea_session_id() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::nil())
}

// ── `gh` argument builders (pure; the real impl uses these at M4) ────────────
//
// Kept as pure functions so a unit test can assert the constructed argument
// vectors contain **no** `--admin` / `--no-verify` without any subprocess.

/// `gh issue create` argv for a labeled + assigned issue.
#[must_use]
pub fn gh_issue_create_argv(
    repo: &str,
    title: &str,
    body: &str,
    labels: &[&str],
    assignees: &[&str],
) -> Vec<String> {
    let mut argv = vec![
        "issue".to_string(),
        "create".to_string(),
        "-R".to_string(),
        repo.to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body".to_string(),
        body.to_string(),
    ];
    for label in labels {
        argv.push("--label".to_string());
        argv.push((*label).to_string());
    }
    for assignee in assignees {
        argv.push("--assignee".to_string());
        argv.push((*assignee).to_string());
    }
    argv
}

/// `gh pr ready --undo` argv (mark a PR back to draft).
#[must_use]
pub fn gh_pr_draft_argv(repo: &str, pr: u64) -> Vec<String> {
    vec![
        "pr".to_string(),
        "ready".to_string(),
        pr.to_string(),
        "-R".to_string(),
        repo.to_string(),
        "--undo".to_string(),
    ]
}

/// `gh pr edit --add-label` argv.
#[must_use]
pub fn gh_pr_add_label_argv(repo: &str, pr: u64, label: &str) -> Vec<String> {
    vec![
        "pr".to_string(),
        "edit".to_string(),
        pr.to_string(),
        "-R".to_string(),
        repo.to_string(),
        "--add-label".to_string(),
        label.to_string(),
    ]
}

/// `gh pr edit --add-reviewer` argv.
#[must_use]
pub fn gh_pr_add_reviewer_argv(repo: &str, pr: u64, reviewer: &str) -> Vec<String> {
    vec![
        "pr".to_string(),
        "edit".to_string(),
        pr.to_string(),
        "-R".to_string(),
        repo.to_string(),
        "--add-reviewer".to_string(),
        reviewer.to_string(),
    ]
}

/// Production [`IdeaGhClient`] that shells out to the `gh` binary using the pure
/// argv builders above. It **never** emits `--admin`/`--no-verify` (asserted by
/// a unit test over the argv builders); the human-review gate is enforced only
/// by standard GitHub mechanisms (draft + label + requested review).
#[derive(Default)]
pub struct RealIdeaGhClient;

impl RealIdeaGhClient {
    /// Construct the production client.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Run `gh` with `argv`, returning its output or an [`SimardError`].
fn run_gh(action: &str, argv: &[String]) -> SimardResult<std::process::Output> {
    let output = std::process::Command::new("gh")
        .args(argv)
        .output()
        .map_err(|e| SimardError::ActionExecutionFailed {
            action: action.to_string(),
            reason: format!("failed to spawn `{action}`: {e}"),
        })?;
    if !output.status.success() {
        return Err(SimardError::ActionExecutionFailed {
            action: action.to_string(),
            reason: format!(
                "`{action}` exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(output)
}

impl IdeaGhClient for RealIdeaGhClient {
    fn create_labeled_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
        assignees: &[&str],
    ) -> SimardResult<GhIssue> {
        let argv = gh_issue_create_argv(repo, title, body, labels, assignees);
        let output = run_gh("gh issue create", &argv)?;
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let number: u64 = url
            .rsplit('/')
            .next()
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| SimardError::ActionExecutionFailed {
                action: "gh issue create".to_string(),
                reason: format!("`gh issue create` returned non-URL output: {url:?}"),
            })?;
        Ok(GhIssue {
            number,
            url,
            title: title.to_string(),
            body: body.to_string(),
        })
    }

    fn set_pr_draft(&self, repo: &str, pr: u64, draft: bool) -> SimardResult<()> {
        // `draft = true` marks the PR back to draft (`gh pr ready --undo`);
        // `draft = false` marks it ready. Our gate only ever drafts.
        if draft {
            run_gh("gh pr ready --undo", &gh_pr_draft_argv(repo, pr))?;
        } else {
            let argv = vec![
                "pr".to_string(),
                "ready".to_string(),
                pr.to_string(),
                "-R".to_string(),
                repo.to_string(),
            ];
            run_gh("gh pr ready", &argv)?;
        }
        Ok(())
    }

    fn add_pr_label(&self, repo: &str, pr: u64, label: &str) -> SimardResult<()> {
        run_gh(
            "gh pr edit --add-label",
            &gh_pr_add_label_argv(repo, pr, label),
        )?;
        Ok(())
    }

    fn request_pr_review(&self, repo: &str, pr: u64, reviewer: &str) -> SimardResult<()> {
        run_gh(
            "gh pr edit --add-reviewer",
            &gh_pr_add_reviewer_argv(repo, pr, reviewer),
        )?;
        Ok(())
    }
}
