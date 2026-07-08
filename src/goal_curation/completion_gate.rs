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

use super::types::{ActiveGoal, GoalBoard};

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

// ===========================================================================
// #2456 — external-signal verification outcome (extends, does not replace, the
// #2450 deploy-aware done-gate above). Classifies the gate's verdict into a
// measurable category so the false-completion rate can be tracked, and so a
// goal with **no derivable external signal** is recorded as honestly
// *unverified* rather than conflated with a verified completion.
// ===========================================================================

/// The outcome of verifying a claimed-complete goal against external signals.
///
/// Mirrors the metric vocabulary in issue #2456
/// (`goal_completion_verification ∈ {verified, unverified_no_signal, refuted,
/// error}`). Intrinsic self-report is never sufficient: a goal is `Verified`
/// only when an external postcondition held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    /// At least one external postcondition was satisfied (gate said Complete).
    Verified,
    /// No external postcondition is derivable for this goal — held as unverified
    /// rather than trusted on self-report alone.
    UnverifiedNoSignal,
    /// A derivable external signal contradicted the completion claim
    /// (e.g. PR not merged, issue still open, self-change not deployed). This is
    /// a false completion the subordinate self-reported as done.
    Refuted,
    /// Verification could not be performed this cycle (a git/gh/drift query
    /// failed). Distinct from a refutation: the truth is simply unknown.
    Error,
}

impl VerificationOutcome {
    /// Stable lowercase label for the metric `context` and logs.
    pub fn metric_label(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::UnverifiedNoSignal => "unverified_no_signal",
            Self::Refuted => "refuted",
            Self::Error => "error",
        }
    }

    /// Numeric encoding for the `f64` metric value, so a time series can be
    /// filtered/aggregated without re-parsing the context string.
    pub fn metric_code(&self) -> f64 {
        match self {
            Self::Verified => 0.0,
            Self::UnverifiedNoSignal => 1.0,
            Self::Refuted => 2.0,
            Self::Error => 3.0,
        }
    }

    /// Whether this outcome is a false completion (claimed done, signal refuted).
    /// This is the numerator of the `goal_false_completion_rate`.
    pub fn is_false_completion(&self) -> bool {
        matches!(self, Self::Refuted)
    }
}

/// Metric name under which each completion-verification outcome is emitted.
pub const COMPLETION_VERIFICATION_METRIC: &str = "goal_completion_verification";

/// Metric name under which the per-batch false-completion rate (#2456 headline
/// metric) is emitted: the share of *checkable* completions in one archival
/// pass that a derivable external signal refuted. Distinct from the per-event
/// [`COMPLETION_VERIFICATION_METRIC`] so a time series can read the rate
/// directly without re-deriving it from the event stream.
pub const FALSE_COMPLETION_RATE_METRIC: &str = "goal_false_completion_rate";

/// Whether any *external* completion signal is derivable for this goal: a
/// tracked PR, a tracked issue, or a self-affecting change (whose deploy state
/// the drift detector can resolve). When none of these exist, a blocked verdict
/// means "nothing to verify", not "verification failed".
pub fn has_derivable_signal(goal: &ActiveGoal) -> bool {
    let has_ref_of = |kind: &str| {
        goal.wip_refs
            .iter()
            .any(|r| r.kind.eq_ignore_ascii_case(kind))
    };
    has_ref_of("pr") || has_ref_of("issue") || is_self_affecting(goal)
}

/// Classify a gate verdict into a [`VerificationOutcome`] for one goal.
pub fn classify_outcome(goal: &ActiveGoal, verdict: &CompletionVerdict) -> VerificationOutcome {
    match verdict {
        CompletionVerdict::Complete(_) => VerificationOutcome::Verified,
        CompletionVerdict::Blocked { missing, .. } => classify_from_missing(goal, missing),
    }
}

/// Classify the blocked-verdict's missing-evidence list into an outcome. A
/// [`MissingEvidence::CouldNotVerify`] dominates as `Error`; otherwise a goal
/// with at least one derivable signal is `Refuted`, and a goal with none is
/// `UnverifiedNoSignal`.
pub fn classify_from_missing(
    goal: &ActiveGoal,
    missing: &[MissingEvidence],
) -> VerificationOutcome {
    if missing
        .iter()
        .any(|m| matches!(m, MissingEvidence::CouldNotVerify { .. }))
    {
        VerificationOutcome::Error
    } else if has_derivable_signal(goal) {
        VerificationOutcome::Refuted
    } else {
        VerificationOutcome::UnverifiedNoSignal
    }
}

/// Derive a normalized *error-class* key from the refuting evidence of a
/// `Refuted` completion (#2458). Each missing-evidence kind maps to a stable
/// token, joined in the gate's fixed check order (PR → issue → deploy) so the
/// same refutation always yields the same class. [`MissingEvidence::CouldNotVerify`]
/// is excluded — it routes to [`VerificationOutcome::Error`], never `Refuted`.
///
/// This is the memory from FU1's external failure signal to #2458's failure→
/// lesson loop: the returned class is the `error_class` half of the
/// `(goal_type, error_class)` recurrence key
/// ([`crate::memory_consolidation::reflection_lessons`]).
///
/// Returns `"refuted_unknown"` when no concrete refuting kind is present
/// (defensive; the classifier never routes such a list to `Refuted`).
pub fn error_class_from_missing(missing: &[MissingEvidence]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for m in missing {
        let token = match m {
            MissingEvidence::PrNotMerged => "pr_not_merged",
            MissingEvidence::IssueOpen => "issue_open",
            MissingEvidence::NotDeployed => "not_deployed",
            // `CouldNotVerify` is the `Error` outcome, not a refutation — never
            // let an unverifiable cycle masquerade as a concrete failure class.
            MissingEvidence::CouldNotVerify { .. } => continue,
        };
        if !parts.contains(&token) {
            parts.push(token);
        }
    }
    if parts.is_empty() {
        "refuted_unknown".to_string()
    } else {
        parts.join("__")
    }
}

/// Emit one completion-verification outcome via
/// [`crate::self_metrics::record_metric`]. Best-effort: a metric-write failure
/// is swallowed so it never blocks or crashes the OODA cycle.
///
/// No-op under `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl` (mirroring `record_lifecycle_decision_metric`
/// in the brain) — writing real telemetry from tests would corrupt the very
/// before/after measurement this metric exists to capture.
pub fn record_completion_verification(outcome: VerificationOutcome) {
    if cfg!(test) {
        return;
    }
    let _ = crate::self_metrics::record_metric(
        COMPLETION_VERIFICATION_METRIC,
        outcome.metric_code(),
        outcome.metric_label(),
    );
}

/// Emit the per-batch [`false_completion_rate`] over `outcomes` via
/// [`crate::self_metrics::record_metric`] under [`FALSE_COMPLETION_RATE_METRIC`].
/// A no-op when nothing in the batch was *checkable* (no verified/refuted
/// outcome) — there is no rate to report. The metric `context` carries the
/// `refuted`/`checkable` counts so a downstream reader can re-aggregate a
/// pooled rate across batches rather than averaging per-batch rates. Best-effort
/// and `cfg!(test)`-guarded for the same reason as
/// [`record_completion_verification`].
pub fn record_false_completion_rate(outcomes: &[VerificationOutcome]) {
    let Some(rate) = false_completion_rate(outcomes) else {
        return;
    };
    if cfg!(test) {
        return;
    }
    let refuted = outcomes
        .iter()
        .filter(|o| matches!(o, VerificationOutcome::Refuted))
        .count();
    let checkable = outcomes
        .iter()
        .filter(|o| {
            matches!(
                o,
                VerificationOutcome::Refuted | VerificationOutcome::Verified
            )
        })
        .count();
    let _ = crate::self_metrics::record_metric(
        FALSE_COMPLETION_RATE_METRIC,
        rate,
        &format!("refuted={refuted} checkable={checkable}"),
    );
}

/// The false-completion rate over a set of outcomes: `refuted / (verified +
/// refuted)` — the share of *checkable* completions that were wrong. Signal-less
/// (`UnverifiedNoSignal`) and `Error` outcomes are excluded from the denominator
/// so they do not dilute the rate. `None` when nothing was checkable. This is
/// the headline metric #2456 asks to trend down.
pub fn false_completion_rate(outcomes: &[VerificationOutcome]) -> Option<f64> {
    let refuted = outcomes
        .iter()
        .filter(|o| matches!(o, VerificationOutcome::Refuted))
        .count();
    let verified = outcomes
        .iter()
        .filter(|o| matches!(o, VerificationOutcome::Verified))
        .count();
    let checkable = refuted + verified;
    if checkable == 0 {
        return None;
    }
    Some(refuted as f64 / checkable as f64)
}

/// The resolution state of a goal's declared upstream dependency, as observed by
/// [`EvidenceSource::dependency_goal_state`]. Backs the `UPSTREAM-DEPENDENCY`
/// rung of the no-progress root-cause ladder (issue #16): a goal gated on a
/// still-open upstream is *deferred* (Paused with the blocking ref recorded)
/// rather than blocked, and *auto-clears* once the upstream resolves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencyState {
    /// No declared/known upstream dependency for this goal.
    None,
    /// A specific upstream (goal id / PR / issue) is still open — the goal is
    /// waiting on it. `blocking_ref` identifies the blocker for the WHY.
    Pending { blocking_ref: String },
    /// The previously-blocking upstream has landed (PR merged / issue closed /
    /// dependency goal completed) — a deferred goal may resume.
    Resolved { blocking_ref: String },
}

/// Injected evidence lookups. The production impl resolves PR/issue state via
/// `gh` and `is_deployed` via the reconciliation detector; tests inject a
/// canned source.
///
/// The two root-cause methods ([`repo_present`](Self::repo_present) and
/// [`dependency_goal_state`](Self::dependency_goal_state), issue #16) carry
/// **default bodies** so every existing implementation and test double keeps
/// compiling unchanged. Both default *conservatively* — a source that cannot
/// tell must never fabricate a missing precondition or an upstream dependency,
/// so the breaker never self-heals or self-defers on an unknown state.
pub trait EvidenceSource: Send + Sync {
    /// Is any PR for this goal merged? (`wip_refs` of kind "pr", or a merged PR
    /// referencing the goal's issue.)
    fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    /// Is the goal's linked issue closed?
    fn issue_closed(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    /// Is the merged self-change running? Backed by the Workstream A
    /// `ReconcileDetector` (`!DeployDrift::needs_deploy`).
    fn is_deployed(&self, goal: &ActiveGoal) -> SimardResult<bool>;

    /// Is the goal's governed target repository present in the workspace? Backs
    /// the `MISSING-PRECONDITION` classification (issue #16). Default `Ok(true)`:
    /// a source that cannot tell must not invent a missing precondition.
    fn repo_present(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        let _ = goal;
        Ok(true)
    }

    /// State of the goal's declared upstream dependency, if any. Backs the
    /// `UPSTREAM-DEPENDENCY` classification (issue #16). Default
    /// `Ok(DependencyState::None)`: a source that cannot tell reports no known
    /// dependency, so the breaker never defers on an unknown state.
    fn dependency_goal_state(&self, goal: &ActiveGoal) -> SimardResult<DependencyState> {
        let _ = goal;
        Ok(DependencyState::None)
    }
}

/// Blanket impl so an `&dyn EvidenceSource` (e.g. `Arc::as_ref()`) satisfies the
/// `E: EvidenceSource` bound on [`CompletionEvidenceGate`] without cloning the
/// source. Lets the daemon store one `Arc<dyn EvidenceSource>` on `OodaClients`
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
    fn repo_present(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        (**self).repo_present(goal)
    }
    fn dependency_goal_state(&self, goal: &ActiveGoal) -> SimardResult<DependencyState> {
        (**self).dependency_goal_state(goal)
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
///
/// Standing/perpetual goals (issue #2580) are never candidates: they have no
/// terminal done-state, so the gate must not archive them regardless of status.
fn is_complete_candidate(goal: &ActiveGoal) -> bool {
    !goal.is_perpetual() && goal.status.is_terminal()
}

/// True when a goal's status *looks* terminal (`Completed`, or `InProgress` at
/// ≥ 100%), independent of whether the goal is perpetual. Used to detect a
/// standing goal whose unit of work finished so it can be rolled to a fresh
/// cycle instead of stalling in a done-looking state.
fn has_dominant_progress(goal: &ActiveGoal) -> bool {
    goal.status.is_terminal()
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
        // Standing/perpetual goals never archive (issue #2580). If a unit of
        // work drove one to a terminal-looking status, roll it to a fresh cycle
        // in place instead of retaining it stuck as "done".
        if goal.is_perpetual() {
            let mut g = goal;
            if has_dominant_progress(&g) {
                g.roll_to_new_cycle();
            }
            retained.push(g);
            continue;
        }
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

    fn repo_present(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        // A goal that routes to the daemon's own repo is always present (this is
        // the checkout the daemon is running from). A repo-scoped goal is
        // present iff its governed clone exists under `$HOME/src/<repo>` — the
        // same workspace convention `ooda_actions::advance_goal`'s repo_resolver
        // uses. Absence is the MISSING-PRECONDITION signal (issue #16). When the
        // home dir cannot be resolved we report `true` (conservative: never
        // invent a missing precondition on an undeterminable path).
        let repo = match &goal.repo {
            None => return Ok(true),
            Some(r) if r.eq_ignore_ascii_case("Simard") => return Ok(true),
            Some(r) => r,
        };
        // Use only the bare repo name for the local clone path (an `owner/repo`
        // slug clones to `$HOME/src/<repo>`).
        let name = repo.rsplit('/').next().unwrap_or(repo);
        match dirs::home_dir() {
            Some(home) => Ok(home.join("src").join(name).is_dir()),
            None => Ok(true),
        }
    }
}

/// Archive completed goals through the evidence gate, honoring the
/// `SIMARD_COMPLETION_EVIDENCE` kill-switch. With the gate **on** (default),
/// only goals with hard evidence archive and blocked goals are retained with a
/// recorded blocker; with the gate **off**, this is the legacy unguarded
/// [`archive_completed`](super::archive_completed).
///
/// Returns `(archived, blocked)`; `blocked` is always empty in legacy mode.
///
/// With the gate on, every candidate's [`VerificationOutcome`] is emitted via
/// [`record_completion_verification`], and the per-batch
/// [`false_completion_rate`] is emitted via [`record_false_completion_rate`], so
/// the false-completion rate (#2456) is measurable both per-event and as a
/// ready-made rate. Recording is best-effort and never affects the return value.
pub fn archive_completed_evidence_aware(
    board: &mut GoalBoard,
    source: &dyn EvidenceSource,
) -> (Vec<ActiveGoal>, Vec<(ActiveGoal, Vec<MissingEvidence>)>) {
    if !completion_evidence_enabled() {
        let archived = super::archive_completed(board);
        return (archived, Vec::new());
    }
    let gate = CompletionEvidenceGate::new(source);
    let (archived, blocked) = archive_completed_with_evidence(board, &gate);

    // Instrument the completion-verification outcome per #2456: a goal that
    // archived was externally verified; a blocked goal is either refuted (a
    // derivable signal said "not done") or unverifiable for lack of any signal.
    let mut outcomes: Vec<VerificationOutcome> = Vec::with_capacity(archived.len() + blocked.len());
    for _ in &archived {
        outcomes.push(VerificationOutcome::Verified);
    }
    for (goal, missing) in &blocked {
        outcomes.push(classify_from_missing(goal, missing));
    }
    for outcome in &outcomes {
        record_completion_verification(*outcome);
    }
    // The headline #2456 metric: the share of *checkable* completions this pass
    // that an external signal refuted. No-op when nothing was checkable.
    record_false_completion_rate(&outcomes);

    (archived, blocked)
}

#[cfg(test)]
mod tests;
