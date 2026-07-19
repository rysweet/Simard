//! Thin Rust rail for the agentic **observe-merge-queue** chain (issue #4097).
//!
//! ROOT CAUSE this module fixes: the Overseer's observe/orient stage populated
//! `ObservedState.ready_prs` from a single imperative allowlist sensor
//! (`survey_ready_prs(&automerge_repos())`). With `SIMARD_AUTOMERGE_REPOS` /
//! `SIMARD_AUTOMERGE_AUTHOR` unset in production, the allowlist was empty, the
//! sensor returned nothing, and the Overseer reasoned about ZERO open PRs while a
//! CI-green merge queue piled up. Unset silently meant OFF.
//!
//! The fix moves the observe/orient merge-queue + issue REASONING into a
//! DETERMINISTIC WORKFLOW OF AGENTIC STEPS behind a THIN rail, mirroring
//! [`crate::overseer::ecosystem_observe`]:
//!
//! 1. An agent (the `observe-merge-queue` recipe) surveys open PRs + issues
//!    across the governed roster with `gh` and REASONS to a bounded brief.
//! 2. [`RecipeMergeQueueReasoner`] invokes that recipe through an injectable
//!    [`MergeQueueRecipeRunner`] seam and forwards the recipe's **opaque** result
//!    string VERBATIM (never parsing it inside the seam).
//! 3. [`parse_merge_queue_brief`] parses that brief FAIL-CLOSED against the
//!    roster trust boundary into typed `ReasonedPr` / `TriagedIssue` values.
//!
//! What this module deliberately does NOT do: it never calls `gh`, never merges,
//! and its `ReasonedPr::ReadyForMerge` disposition is a PROPOSAL only — the
//! re-narrowing [`project_ready_prs`](crate::overseer::project_ready_prs)
//! projection (author guard + engineer-PR narrowing + objective gates) is the
//! ONLY thing that authorizes a merge. Reasoning is broad; authorization is
//! narrow. See `docs/design/agentic-observe-orient-merge-queue.md`.

use std::path::Path;

use serde_json::Value;

use crate::error::{SimardError, SimardResult};
use crate::overseer::capabilities::{
    IssuePriority, IssueReadiness, PrDisposition, ReasonedPr, TriagedIssue,
};

/// Adapter tag for error/telemetry attribution on the observe-merge-queue rail.
const OBSERVE_ADAPTER_TAG: &str = "observe-merge-queue";
/// The recipe this runner invokes (resolved install-first, then in-tree).
const OBSERVE_RECIPE_FILENAME: &str = "observe-merge-queue.yaml";
/// Upper bound on any free-text agent field (`rationale`, `next_action`) copied
/// out of the brief. Prevents a hostile/oversized brief inflating a downstream
/// notification, comment body, or log line.
const MAX_FIELD_LEN: usize = 500;

// ─────────────────────────── rail (seam) ───────────────────────────────────

/// What the rail hands the `observe-merge-queue` recipe: the resolved reasoning
/// scope (governed roster or an operator-narrowed subset), Simard's in-flight
/// OODA refs (for dedup), and the (rail-owned) escalation note. Pure strings —
/// no observation state. Mirrors [`crate::overseer::ecosystem_observe::EcosystemObserveRequest`].
#[derive(Clone, Debug)]
pub struct MergeQueueObserveRequest {
    /// Validated `owner/name` slugs the reasoning agent scans with `gh`.
    pub scope: Vec<String>,
    /// Simard's in-flight OODA refs, so the agent dedups against work an engineer
    /// already owns and never re-proposes it.
    pub inflight_refs: Vec<String>,
    /// Empty on the base pass; carries a higher-effort / repair instruction on
    /// escalation-ladder retries. Rail-owned, never a caller parameter.
    pub escalation_note: String,
}

/// Seam: invoke the `observe-merge-queue` recipe and return its **opaque**
/// result. Injectable so the rail is unit-testable with a fake — no subprocess,
/// no network, no `gh`. The production impl spawns the recipe runner; the runner
/// itself never inspects, parses, or counts the returned string.
pub trait MergeQueueRecipeRunner: Send + Sync {
    /// Run one reasoning pass and return the recipe's final opaque output.
    fn run(&self, request: &MergeQueueObserveRequest) -> SimardResult<String>;
}

/// The thin rail. Forwards the recipe's opaque brief string. Holds NO observation
/// state and never touches a repo — the `gh` scanning and the reasoning both live
/// inside the recipe's agent step.
pub trait MergeQueueReasoner {
    /// Run one reasoning pass.
    ///
    /// - `Ok(Some(brief))` — the recipe produced a semantic brief string to parse
    ///   FAIL-CLOSED via [`parse_merge_queue_brief`]. `brief` is opaque; the rail
    ///   forwards it and never parses it.
    /// - `Ok(None)` — nothing actionable this pass (empty scope, blank result, or
    ///   a degraded recipe run).
    /// - `Err(_)` — reserved for a caller-visible fault; the default rail is
    ///   fail-closed and prefers `Ok(None)` over fabricating reasoning.
    fn observe(&self, request: MergeQueueObserveRequest) -> SimardResult<Option<String>>;
}

/// Recipe-runner-backed [`MergeQueueReasoner`] over an injectable seam.
pub struct RecipeMergeQueueReasoner<R: MergeQueueRecipeRunner> {
    runner: R,
}

impl<R: MergeQueueRecipeRunner> RecipeMergeQueueReasoner<R> {
    /// Build the rail over a concrete [`MergeQueueRecipeRunner`].
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    /// Borrow the underlying runner (used by tests to inspect the seam).
    pub fn runner(&self) -> &R {
        &self.runner
    }
}

impl<R: MergeQueueRecipeRunner> MergeQueueReasoner for RecipeMergeQueueReasoner<R> {
    fn observe(&self, request: MergeQueueObserveRequest) -> SimardResult<Option<String>> {
        // Fail-closed: an empty scope is nothing to reason about. Never invoke the
        // recipe and never fabricate reasoning from an empty scan.
        if request.scope.is_empty() {
            tracing::warn!(
                target: "overseer::merge",
                "observe-merge-queue: empty scope; skipping pass (no reasoning fabricated)"
            );
            return Ok(None);
        }

        match self.runner.run(&request) {
            Ok(output) => {
                // A blank recipe result is "nothing actionable", not reasoning.
                // The non-empty result is forwarded VERBATIM and never parsed here.
                if output.trim().is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(output))
                }
            }
            Err(e) => {
                // Fail-closed: a recipe/infra fault degrades to "no reasoning",
                // logged. It never aborts the tick and never fabricates a brief.
                tracing::warn!(
                    target: "overseer::merge",
                    error = %e,
                    "observe-merge-queue: recipe run failed; degrading to no reasoning \
                     (no PRs/issues fabricated)"
                );
                Ok(None)
            }
        }
    }
}

// ─────────────────── brief parse — bounded + FAIL-CLOSED ────────────────────

/// The parsed brief: typed, roster-bounded PR and issue conclusions. Every entry
/// survived the FAIL-CLOSED parse ([`parse_merge_queue_brief`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeQueueBriefOutcome {
    pub reasoned_prs: Vec<ReasonedPr>,
    pub triaged_issues: Vec<TriagedIssue>,
}

/// Parse the agent's opaque merge-queue brief into typed, roster-bounded values,
/// FAIL-CLOSED (#4097).
///
/// Invariants (each pinned by a test):
/// - Whole-brief garbage (empty, non-JSON, wrong top-level shape) ⇒ empty
///   outcome; never a panic, never fabricated work.
/// - The `scope` (governed roster) is the TRUST BOUNDARY: any entry whose `repo`
///   is not on `scope` is DROPPED. An agent (or an XPIA-injected brief) can never
///   widen reasoning to an off-roster repo.
/// - An unknown `disposition` / `priority` / `readiness`, or a missing required
///   field (`pr`/`issue` number), drops just that entry — the valid siblings
///   survive.
/// - A `Duplicate` disposition with no `duplicate_of` is incoherent ⇒ dropped.
pub fn parse_merge_queue_brief(brief: &str, scope: &[String]) -> MergeQueueBriefOutcome {
    let Ok(value) = serde_json::from_str::<Value>(brief) else {
        return MergeQueueBriefOutcome::default();
    };

    let reasoned_prs = value
        .get("reasoned_prs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| parse_reasoned_pr(entry, scope))
                .collect()
        })
        .unwrap_or_default();

    let triaged_issues = value
        .get("triaged_issues")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| parse_triaged_issue(entry, scope))
                .collect()
        })
        .unwrap_or_default();

    MergeQueueBriefOutcome {
        reasoned_prs,
        triaged_issues,
    }
}

/// True iff `repo` is on the roster trust boundary.
fn on_roster(repo: &str, scope: &[String]) -> bool {
    scope.iter().any(|r| r == repo)
}

/// Copy a bounded free-text field out of the brief. Trims and caps to
/// [`MAX_FIELD_LEN`] so a hostile/oversized brief cannot inflate a downstream
/// notification, comment, or log line.
fn bounded_text(entry: &Value, key: &str) -> String {
    let raw = entry.get(key).and_then(Value::as_str).unwrap_or("").trim();
    raw.chars().take(MAX_FIELD_LEN).collect()
}

fn parse_reasoned_pr(entry: &Value, scope: &[String]) -> Option<ReasonedPr> {
    let repo = entry
        .get("repo")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if !on_roster(&repo, scope) {
        return None;
    }
    let pr = u32::try_from(entry.get("pr").and_then(Value::as_u64)?).ok()?;
    let disposition = parse_disposition(entry.get("disposition").and_then(Value::as_str)?)?;
    let duplicate_of = entry
        .get("duplicate_of")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    // A Duplicate with no original to point at is incoherent — drop it.
    if disposition == PrDisposition::Duplicate && duplicate_of.is_none() {
        return None;
    }
    // A PR cannot be a duplicate of ITSELF. A self-referential pointer (agent
    // hallucination or an injected brief) would otherwise drive a
    // `CloseDuplicatePr` that closes a legitimate PR "as a duplicate of itself".
    // Fail closed: drop it.
    if duplicate_of == Some(pr) {
        return None;
    }
    Some(ReasonedPr {
        repo,
        pr,
        disposition,
        rationale: bounded_text(entry, "rationale"),
        duplicate_of,
    })
}

fn parse_triaged_issue(entry: &Value, scope: &[String]) -> Option<TriagedIssue> {
    let repo = entry
        .get("repo")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if !on_roster(&repo, scope) {
        return None;
    }
    let issue = u32::try_from(entry.get("issue").and_then(Value::as_u64)?).ok()?;
    let priority = parse_priority(entry.get("priority").and_then(Value::as_str)?)?;
    let readiness = parse_readiness(entry.get("readiness").and_then(Value::as_str)?)?;
    Some(TriagedIssue {
        repo,
        issue,
        priority,
        readiness,
        next_action: bounded_text(entry, "next_action"),
    })
}

/// Map a disposition token (case-insensitive) to [`PrDisposition`]. Unknown ⇒
/// `None` (dropped, fail-closed) so a novel/hostile token never reaches Decide.
fn parse_disposition(raw: &str) -> Option<PrDisposition> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ready-for-merge" | "ready_for_merge" => Some(PrDisposition::ReadyForMerge),
        "needs-work" | "needs_work" => Some(PrDisposition::NeedsWork),
        "stale" => Some(PrDisposition::Stale),
        "duplicate" => Some(PrDisposition::Duplicate),
        _ => None,
    }
}

fn parse_priority(raw: &str) -> Option<IssuePriority> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "high" | "critical" => Some(IssuePriority::High),
        "medium" | "normal" => Some(IssuePriority::Medium),
        "low" => Some(IssuePriority::Low),
        _ => None,
    }
}

fn parse_readiness(raw: &str) -> Option<IssueReadiness> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ready" => Some(IssueReadiness::Ready),
        "blocked" => Some(IssueReadiness::Blocked),
        "needs-info" | "needs_info" => Some(IssueReadiness::NeedsInfo),
        _ => None,
    }
}

// ─────────────────── recipe resolution (install-first) ──────────────────────

/// Resolve the `observe-merge-queue.yaml` recipe path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (installed / hot-reload)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// Mirrors [`crate::overseer::ecosystem_observe`]'s resolver. `home_override`
/// keeps tests hermetic against the ambient `~/.simard`; production passes `None`.
pub fn resolve_merge_queue_recipe_path(
    repo_root: &Path,
    home_override: Option<&Path>,
) -> Option<std::path::PathBuf> {
    let home = home_override
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir);
    if let Some(home) = home {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(OBSERVE_RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(OBSERVE_RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

// ─────────────────── production recipe-runner (thin) ───────────────────────

/// Production [`MergeQueueRecipeRunner`]: spawns `recipe-runner-rs` on the
/// `observe-merge-queue` recipe and returns its OPAQUE final-step output.
///
/// Thin by construction (mirrors
/// [`crate::overseer::ecosystem_observe::SpawnEcosystemRecipeRunner`]). It writes
/// the scope / in-flight refs / a writable handoff placeholder to per-invocation
/// context files (so unbounded lists ride the `<key>_path` file channel, never
/// `argv`), passes only the `_path` tokens plus the empty `escalation_note`, runs
/// the recipe in `--output-format json`, and hands back the envelope's final step
/// output. It NEVER inspects, parses, or counts that string — the reasoning lives
/// in the agent and is forwarded verbatim for [`parse_merge_queue_brief`].
pub struct SpawnMergeQueueRecipeRunner {
    recipe_path: std::path::PathBuf,
    agent_binary: &'static str,
}

impl SpawnMergeQueueRecipeRunner {
    /// Construct if the recipe file and `recipe-runner-rs` are both available;
    /// otherwise `None` (the rail is left unwired and the pass is skipped).
    pub fn new(repo_root: &Path) -> Option<Self> {
        Self::new_with_home(repo_root, None)
    }

    fn new_with_home(repo_root: &Path, home_override: Option<&Path>) -> Option<Self> {
        let recipe_path = resolve_merge_queue_recipe_path(repo_root, home_override)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if std::process::Command::new("recipe-runner-rs")
            .arg("--version")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self {
            recipe_path,
            agent_binary,
        })
    }
}

impl MergeQueueRecipeRunner for SpawnMergeQueueRecipeRunner {
    fn run(&self, request: &MergeQueueObserveRequest) -> SimardResult<String> {
        use crate::recipe_context_file::ContextFile;

        let scope_cf = ContextFile::write(OBSERVE_ADAPTER_TAG, "scope", &request.scope.join("\n"))
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: OBSERVE_ADAPTER_TAG.to_string(),
                reason: format!("scope context-file write failed: {e}"),
            })?;
        let inflight_cf = ContextFile::write(
            OBSERVE_ADAPTER_TAG,
            "inflight_refs",
            &request.inflight_refs.join("\n"),
        )
        .map_err(|e| SimardError::AdapterInvocationFailed {
            base_type: OBSERVE_ADAPTER_TAG.to_string(),
            reason: format!("inflight_refs context-file write failed: {e}"),
        })?;
        let brief_cf =
            ContextFile::write(OBSERVE_ADAPTER_TAG, "merge_queue_brief", "").map_err(|e| {
                SimardError::AdapterInvocationFailed {
                    base_type: OBSERVE_ADAPTER_TAG.to_string(),
                    reason: format!("merge_queue_brief context-file write failed: {e}"),
                }
            })?;

        let output = std::process::Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(scope_cf.arg_value())
            .arg("-c")
            .arg(inflight_cf.arg_value())
            .arg("-c")
            .arg(brief_cf.arg_value())
            .arg("-c")
            .arg(format!("escalation_note={}", request.escalation_note))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: OBSERVE_ADAPTER_TAG.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated: String = stderr.chars().take(500).collect();
            return Err(SimardError::AdapterInvocationFailed {
                base_type: OBSERVE_ADAPTER_TAG.to_string(),
                reason: format!("recipe exited with {}: {}", output.status, truncated),
            });
        }

        // Opaque forward: the final BRIEF step's output, never parsed here.
        crate::ooda_brain::extract_recipe_decision_output(&output.stdout, OBSERVE_ADAPTER_TAG)
    }
}
