//! Deploy-aware done-gate: a goal becomes **complete** only with hard evidence
//! — a merged PR, a closed linked issue, and (for changes to Simard's own
//! running code) a verified deploy. Anything short keeps the goal active with a
//! recorded blocker instead of silently archiving it.
//!
//! This is the gate that prevents evidence-free done-claims like the
//! cognitive-memory backup false-completion (a goal archived as "complete" with
//! no merged PR and its linked issue still open).
//!
//! Evidence lookups are injected through [`EvidenceSource`] so the gate logic is
//! pure and runs hermetically with no network and no live `gh`. The production
//! source resolves PR/issue state through `gh` and resolves `is_deployed`
//! through the Workstream A reconciliation detector
//! (`!DeployDrift::needs_deploy`).
//!
//! See `docs/concepts/deploy-aware-done-gate.md` and
//! `docs/reference/completion-evidence-gate-api.md`.

use serde::{Deserialize, Serialize};

use crate::error::SimardResult;

use super::types::{ActiveGoal, GoalBoard, GoalProgress};

/// The verified facts the gate gathered for one goal.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionEvidence {
    /// A PR in the goal's `wip_refs` (or referencing its issue) is merged.
    pub pr_merged: bool,
    /// The goal's linked issue is closed.
    pub issue_closed: bool,
    /// The change affects Simard's own running code (see [`is_self_affecting`]).
    pub self_affecting: bool,
    /// For self-affecting goals: the merged change is running
    /// (`!DeployDrift::needs_deploy`). `true` for non-self-affecting goals.
    pub deployed: bool,
}

/// A single missing-evidence reason surfaced as a blocker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MissingEvidence {
    /// No merged PR found for this goal.
    PrNotMerged,
    /// The linked issue is still open.
    IssueOpen,
    /// Self-affecting change is merged but not yet running.
    NotDeployed,
    /// A git/gh/drift query failed; completion cannot be verified this cycle.
    CouldNotVerify { detail: String },
}

impl MissingEvidence {
    /// Short, stable human label for surfacing in `current_activity` and logs.
    pub fn label(&self) -> String {
        match self {
            Self::PrNotMerged => "PR not merged".to_string(),
            Self::IssueOpen => "issue still open".to_string(),
            Self::NotDeployed => "merged but not deployed".to_string(),
            Self::CouldNotVerify { detail } => format!("could not verify: {detail}"),
        }
    }
}

/// The gate's verdict for one goal.
#[derive(Clone, Debug, PartialEq)]
pub enum CompletionVerdict {
    /// All applicable clauses hold — completion/archive may proceed.
    Complete(CompletionEvidence),
    /// One or more clauses fail — keep the goal active, record the blocker.
    Blocked {
        evidence: CompletionEvidence,
        missing: Vec<MissingEvidence>,
    },
}

impl CompletionVerdict {
    /// `true` only for [`CompletionVerdict::Complete`].
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete(_))
    }
}

/// Injected evidence lookups. The production impl resolves PR/issue state via
/// `gh` and `is_deployed` via the reconciliation detector; tests inject a
/// canned source.
pub trait EvidenceSource: Send + Sync {
    /// Is any PR for this goal merged? (`wip_refs` of kind "pr", or a merged PR
    /// referencing the goal's issue.)
    fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    /// Is the goal's linked issue closed?
    fn issue_closed(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    /// Is the merged self-change running? Backed by the Workstream A
    /// `ReconcileDetector` (`!DeployDrift::needs_deploy`).
    fn is_deployed(&self, goal: &ActiveGoal) -> SimardResult<bool>;
}

/// Blanket impl so an `&dyn EvidenceSource` (e.g. `Arc::as_ref()`) satisfies the
/// `E: EvidenceSource` bound on [`CompletionEvidenceGate`] without cloning the
/// source. Lets the daemon store one `Arc<dyn EvidenceSource>` on `OodaBridges`
/// and pass it by reference into the gate each cycle.
impl<T: EvidenceSource + ?Sized> EvidenceSource for &T {
    fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        (**self).any_pr_merged(goal)
    }
    fn issue_closed(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        (**self).issue_closed(goal)
    }
    fn is_deployed(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        (**self).is_deployed(goal)
    }
}

/// Verifies the three-part done-gate over an injected [`EvidenceSource`].
pub struct CompletionEvidenceGate<E: EvidenceSource> {
    source: E,
}

impl<E: EvidenceSource> CompletionEvidenceGate<E> {
    pub fn new(source: E) -> Self {
        Self { source }
    }

    /// Evaluate one goal. Never panics. On a source error, returns `Blocked`
    /// with a [`MissingEvidence::CouldNotVerify`] so a goal is never completed
    /// on unverifiable evidence and the cycle is never crashed.
    pub fn evaluate(&self, goal: &ActiveGoal) -> CompletionVerdict {
        let self_affecting = is_self_affecting(goal);

        let pr_merged = match self.source.any_pr_merged(goal) {
            Ok(b) => b,
            Err(e) => return blocked_could_not_verify(self_affecting, e.to_string()),
        };
        let issue_closed = match self.source.issue_closed(goal) {
            Ok(b) => b,
            Err(e) => return blocked_could_not_verify(self_affecting, e.to_string()),
        };
        // Clause 3 only applies to self-affecting changes; otherwise deployed is
        // unconditionally true and `NotDeployed` can never appear.
        let deployed = if self_affecting {
            match self.source.is_deployed(goal) {
                Ok(b) => b,
                Err(e) => return blocked_could_not_verify(self_affecting, e.to_string()),
            }
        } else {
            true
        };

        let evidence = CompletionEvidence {
            pr_merged,
            issue_closed,
            self_affecting,
            deployed,
        };

        let mut missing = Vec::new();
        if !pr_merged {
            missing.push(MissingEvidence::PrNotMerged);
        }
        if !issue_closed {
            missing.push(MissingEvidence::IssueOpen);
        }
        if self_affecting && !deployed {
            missing.push(MissingEvidence::NotDeployed);
        }

        if missing.is_empty() {
            CompletionVerdict::Complete(evidence)
        } else {
            CompletionVerdict::Blocked { evidence, missing }
        }
    }
}

/// Build a fail-closed `Blocked` verdict when a source query errors.
fn blocked_could_not_verify(self_affecting: bool, detail: String) -> CompletionVerdict {
    CompletionVerdict::Blocked {
        evidence: CompletionEvidence {
            pr_merged: false,
            issue_closed: false,
            self_affecting,
            // Non-self-affecting goals never gate on deploy, so it stays true.
            deployed: !self_affecting,
        },
        missing: vec![MissingEvidence::CouldNotVerify { detail }],
    }
}

/// A goal affects Simard's own running code when it routes to the Simard repo
/// (the default `None`, or an explicit "Simard" slug) **or** it bumps a pinned
/// dependency rev in Simard's own `Cargo.toml`.
///
/// Docs-only goals (carrying an explicit `docs-only` / `documentation-only`
/// marker) are NOT self-affecting, so clause 3 (deployed) is skipped for them.
/// The classifier is deliberately conservative: an ambiguous Simard-repo goal
/// is treated as self-affecting (the safe, fail-closed direction — it requires
/// deploy evidence rather than skipping the clause).
pub fn is_self_affecting(goal: &ActiveGoal) -> bool {
    if is_docs_only(goal) {
        return false;
    }
    if routes_to_simard(goal) {
        return true;
    }
    bumps_simard_pin(goal)
}

/// `repo` is the default `None` (routes to Simard) or an explicit "Simard" slug.
fn routes_to_simard(goal: &ActiveGoal) -> bool {
    match &goal.repo {
        None => true,
        Some(slug) => slug.eq_ignore_ascii_case("Simard"),
    }
}

/// Explicit docs-only marker in the description.
fn is_docs_only(goal: &ActiveGoal) -> bool {
    let desc = goal.description.to_ascii_lowercase();
    desc.contains("docs-only") || desc.contains("documentation-only")
}

/// The goal bumps a pinned dependency rev in Simard's own `Cargo.toml`,
/// detected from the description or from `wip_refs` touching `Cargo.toml`.
fn bumps_simard_pin(goal: &ActiveGoal) -> bool {
    let desc = goal.description.to_ascii_lowercase();
    let mentions_cargo = desc.contains("cargo.toml");
    let mentions_bump =
        desc.contains("bump") || desc.contains("pin") || desc.contains("dependency");
    if mentions_cargo && mentions_bump {
        return true;
    }
    goal.wip_refs.iter().any(|r| {
        r.label.to_ascii_lowercase().contains("cargo.toml")
            || r.url
                .as_deref()
                .map(|u| u.to_ascii_lowercase().contains("cargo.toml"))
                .unwrap_or(false)
    })
}

/// Environment kill-switch: `SIMARD_COMPLETION_EVIDENCE=off` disables the gate
/// and restores the legacy unguarded archive behaviour. Any other value (or
/// unset) keeps the gate active. Mirrors `SIMARD_PROGRESS_EVIDENCE`.
pub fn completion_evidence_enabled() -> bool {
    std::env::var("SIMARD_COMPLETION_EVIDENCE")
        .map(|v| v.trim() != "off")
        .unwrap_or(true)
}

/// True for goals whose status makes them archive candidates under the legacy
/// rule (`Completed`, or `InProgress` at ≥ 100%).
fn is_complete_candidate(goal: &ActiveGoal) -> bool {
    matches!(goal.status, GoalProgress::Completed)
        || matches!(goal.status, GoalProgress::InProgress { percent } if percent >= 100)
}

/// Archive only goals the gate certifies `Complete`. Goals that fail the gate
/// are **retained** on the active board, their `current_activity` annotated with
/// the missing evidence so the dashboard and the next cycle see why they did not
/// archive. Returns `(archived, blocked)`.
pub fn archive_completed_with_evidence<E: EvidenceSource>(
    board: &mut GoalBoard,
    gate: &CompletionEvidenceGate<E>,
) -> (Vec<ActiveGoal>, Vec<(ActiveGoal, Vec<MissingEvidence>)>) {
    let mut archived = Vec::new();
    let mut blocked = Vec::new();
    let mut retained = Vec::new();

    for goal in std::mem::take(&mut board.active) {
        if !is_complete_candidate(&goal) {
            retained.push(goal);
            continue;
        }
        match gate.evaluate(&goal) {
            CompletionVerdict::Complete(_) => archived.push(goal),
            CompletionVerdict::Blocked { missing, .. } => {
                let mut annotated = goal;
                annotated.current_activity = Some(format!(
                    "completion blocked — missing evidence: {}",
                    render_missing(&missing)
                ));
                blocked.push((annotated.clone(), missing));
                retained.push(annotated);
            }
        }
    }

    board.active = retained;
    (archived, blocked)
}

/// Render a missing-evidence list into a compact human string.
fn render_missing(missing: &[MissingEvidence]) -> String {
    missing
        .iter()
        .map(MissingEvidence::label)
        .collect::<Vec<_>>()
        .join("; ")
}

// ===========================================================================
// Production evidence source: `gh` for PR/issue state, drift for deployed-ness
// ===========================================================================

/// Production [`EvidenceSource`]. Resolves PR/issue state through the `gh` CLI
/// and `is_deployed` through the Workstream A reconciliation detector
/// ([`GitDeploySource`](crate::self_deploy::GitDeploySource)).
///
/// Conservative by construction: a goal with no PR `wip_ref` reports
/// `any_pr_merged == false` **without** a network call, so the gate blocks an
/// evidence-free completion cheaply; a goal with no issue `wip_ref` reports
/// `issue_closed == true` (there is no open linked issue to gate on). Any `gh`
/// failure propagates as an error so the gate fails **closed**
/// ([`MissingEvidence::CouldNotVerify`]) and never archives on unverifiable
/// evidence.
pub struct GhCliEvidenceSource {
    /// Source checkout used by the drift detector (`git`).
    repo_dir: std::path::PathBuf,
    /// Default `owner/repo` slug when a goal does not carry one.
    default_repo: String,
}

impl GhCliEvidenceSource {
    /// Construct rooted at `repo_dir`, defaulting unscoped goals to
    /// `rysweet/Simard`.
    pub fn new(repo_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            repo_dir: repo_dir.into(),
            default_repo: "rysweet/Simard".to_string(),
        }
    }

    /// Resolve a goal's `owner/repo` slug. `None`/`Simard` → the default;
    /// an already-qualified `owner/repo` is used verbatim; a bare slug is
    /// scoped under the default owner.
    fn repo_slug(&self, goal: &ActiveGoal) -> String {
        match &goal.repo {
            None => self.default_repo.clone(),
            Some(r) if r.eq_ignore_ascii_case("Simard") => self.default_repo.clone(),
            Some(r) if r.contains('/') => r.clone(),
            Some(r) => {
                let owner = self.default_repo.split('/').next().unwrap_or("rysweet");
                format!("{owner}/{r}")
            }
        }
    }

    /// Run `gh <kind> view <num> --repo <repo> --json state --jq .state` and
    /// return the trimmed state string (e.g. `MERGED`, `CLOSED`, `OPEN`).
    fn gh_state(&self, kind: &str, repo: &str, num: &str) -> SimardResult<String> {
        let out = std::process::Command::new("gh")
            .args([
                kind, "view", num, "--repo", repo, "--json", "state", "--jq", ".state",
            ])
            .output()
            .map_err(|e| crate::error::SimardError::VerificationFailed {
                reason: format!("failed to spawn `gh {kind} view {num} --repo {repo}`: {e}"),
            })?;
        if !out.status.success() {
            return Err(crate::error::SimardError::VerificationFailed {
                reason: format!(
                    "`gh {kind} view {num} --repo {repo}` exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// Numeric `ref_id` of the first `wip_ref` whose kind matches `want` (e.g.
/// `"pr"`, `"issue"`).
fn first_ref_of_kind<'a>(goal: &'a ActiveGoal, want: &str) -> Option<&'a str> {
    goal.wip_refs
        .iter()
        .find(|r| r.kind.eq_ignore_ascii_case(want))
        .map(|r| r.ref_id.as_str())
}

impl EvidenceSource for GhCliEvidenceSource {
    fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        match first_ref_of_kind(goal, "pr") {
            // No tracked PR ⇒ no merge evidence (block, cheaply, no network).
            None => Ok(false),
            Some(num) => {
                let repo = self.repo_slug(goal);
                Ok(self
                    .gh_state("pr", &repo, num)?
                    .eq_ignore_ascii_case("MERGED"))
            }
        }
    }

    fn issue_closed(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        match first_ref_of_kind(goal, "issue") {
            // No tracked issue ⇒ nothing open to gate on (clause vacuously holds).
            None => Ok(true),
            Some(num) => {
                let repo = self.repo_slug(goal);
                Ok(self
                    .gh_state("issue", &repo, num)?
                    .eq_ignore_ascii_case("CLOSED"))
            }
        }
    }

    fn is_deployed(&self, _goal: &ActiveGoal) -> SimardResult<bool> {
        // Deployed-and-running ≡ the running binary is not behind merged `main`.
        // The detector is fail-safe (a git error reports "no drift"), so this
        // never spuriously blocks; it only blocks when drift is positively seen.
        let detector = crate::self_deploy::ReconcileDetector::new(
            crate::self_deploy::GitDeploySource::at(&self.repo_dir),
        );
        Ok(!detector.detect().needs_deploy)
    }
}

/// Archive completed goals through the evidence gate, honoring the
/// `SIMARD_COMPLETION_EVIDENCE` kill-switch. With the gate **on** (default),
/// only goals with hard evidence archive and blocked goals are retained with a
/// recorded blocker; with the gate **off**, this is the legacy unguarded
/// [`archive_completed`](super::archive_completed).
///
/// Returns `(archived, blocked)`; `blocked` is always empty in legacy mode.
pub fn archive_completed_evidence_aware(
    board: &mut GoalBoard,
    source: &dyn EvidenceSource,
) -> (Vec<ActiveGoal>, Vec<(ActiveGoal, Vec<MissingEvidence>)>) {
    if !completion_evidence_enabled() {
        let archived = super::archive_completed(board);
        return (archived, Vec::new());
    }
    let gate = CompletionEvidenceGate::new(source);
    archive_completed_with_evidence(board, &gate)
}

#[cfg(test)]
mod tests;
