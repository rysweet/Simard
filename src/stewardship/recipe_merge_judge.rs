//! Recipe-runner-backed [`MergeJudge`] — a THIN, deterministic safety RAIL.
//!
//! The judgment (does this PR's real change merit merging?) is delegated to the
//! `prompt_assets/simard/recipes/merge-readiness-judge.yaml` recipe. Crucially,
//! the recipe no longer PRINTS a verdict for Simard to read back out of its
//! stdout. Instead the agent RECORDS a typed verdict by calling the
//! `simard merge record-verdict` tool (the same act-via-tool pattern as
//! `distill-episodes.yaml` → `simard memory remember`). This rail then READS the
//! typed record via [`super::merge_verdict_store`] and, before authorizing any
//! merge, INDEPENDENTLY re-verifies the hard safety gates in Rust.
//!
//! ## Why this shape (issue #4721)
//!
//! The operator forbade the fragile "recipe emits JSON → Rust reads its stdout →
//! Rust acts" transport: a single trailing comma, launcher-banner line, or ANSI
//! escape made the strict read fail and every gated merge was blocked. The
//! typed record removes that surface entirely — there is no return document to
//! misread. See `docs/reference/merge-record-verdict-cli.md`.
//!
//! ## Safety model (defense in depth — all must hold to merge)
//!
//! 1. `merge_authority::evaluate_objective_gates` — the pre-judge gate.
//! 2. This rail — reads the freshness/identity-checked record AND independently
//!    re-runs [`evaluate_objective_gates`]. It returns [`Verdict::Ready`] ONLY
//!    when the record says `merge` AND every hard gate passes. A `merge` verdict
//!    against red CI / a draft / a non-mergeable PR is REFUSED loudly.
//! 3. `merge_authority::execute_merge` — a final `gh pr view` re-check right
//!    before `gh pr merge --squash --delete-branch`.
//!
//! The agent's verdict is ADVISORY-to-merge; the deterministic rail is the
//! safety authority. Anti-replay: the rail deletes any prior record and passes
//! a fresh single-run `run_token` the recorded verdict must echo, so a stale or
//! foreign verdict can never be mistaken for this run's decision.

use std::path::PathBuf;
use std::process::Command;

use crate::error::{SimardError, SimardResult};

use super::merge_authority::{PrSnapshot, base_allowlist_from_env, evaluate_objective_gates};
use super::merge_judge::{JudgeOutcome, MergeJudge, MergeJudgeKind, Verdict};
use super::merge_verdict_store::{self, ReadOutcome, VerdictKind};

const ADAPTER_TAG: &str = "recipe-merge-judge";
const RECIPE_FILENAME: &str = "merge-readiness-judge.yaml";

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// `home_override` lets tests supply a fake home directory without mutating the
/// process-wide `HOME` env var (mirrors `disk_health::resolve_recipe_path`).
/// Production passes `None`, falling back to [`dirs::home_dir`].
fn resolve_recipe_path(
    repo_root: &std::path::Path,
    home_override: Option<&std::path::Path>,
) -> Option<PathBuf> {
    let home = home_override.map(PathBuf::from).or_else(dirs::home_dir);
    if let Some(home) = home {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Recipe-runner-backed merge-readiness judge (the thin deterministic rail).
pub struct RecipeMergeJudge {
    recipe_path: PathBuf,
    agent_binary: &'static str,
}

impl RecipeMergeJudge {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    pub fn new(repo_root: &std::path::Path) -> Option<Self> {
        Self::new_with_home(repo_root, None)
    }

    /// Like [`RecipeMergeJudge::new`], but accepts a `home_override` for the
    /// hot-reload lookup so tests stay hermetic against the ambient
    /// `~/.simard/prompt_assets` directory. Production calls `new` (`None`).
    fn new_with_home(
        repo_root: &std::path::Path,
        home_override: Option<&std::path::Path>,
    ) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root, home_override)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if Command::new("recipe-runner-rs")
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

    /// A fresh single-run token. It ties this run's recorded verdict to this
    /// exact invocation so a record left by a previous run (or a foreign
    /// process) can never be mistaken for this run's decision.
    fn fresh_run_token(pr_number: u32) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("mj-{pr_number}-{}-{nanos}", std::process::id())
    }

    /// Invoke the merge-readiness recipe once. The agent reasons over the PR and
    /// RECORDS its verdict via `simard merge record-verdict`; nothing is read
    /// back from the recipe's stdout. This is an AGENTIC step — deliberately NO
    /// timeout is imposed. The (arbitrary-size) PR body rides the shared file
    /// channel so it can never overflow `ARG_MAX`; only the absolute
    /// `pr_body_path` and small scalars ride on argv. Genuine recipe-runner
    /// failures (spawn / nonzero exit) propagate as `Err` so an infra fault is
    /// never masked by a fail-closed verdict.
    fn invoke_recipe(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
        state_root: &std::path::Path,
        run_token: &str,
    ) -> SimardResult<()> {
        let pr_body_cf =
            crate::recipe_context_file::ContextFile::write(ADAPTER_TAG, "pr_body", &snapshot.body)
                .map_err(|e| SimardError::AdapterInvocationFailed {
                    base_type: ADAPTER_TAG.to_string(),
                    reason: format!("pr_body context-file write failed: {e}"),
                })?;
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("pr_number={pr_number}"))
            .arg("-c")
            .arg(format!("repo={repo}"))
            .arg("-c")
            .arg(pr_body_cf.arg_value())
            // The rail-supplied single-run token the agent must echo to the
            // record-verdict tool so the rail can prove the record is THIS run's.
            .arg("-c")
            .arg(format!("run_token={run_token}"))
            // Where the record-verdict tool durably writes the record — the same
            // state root the rail reads from below.
            .arg("-c")
            .arg(format!("state_root={}", state_root.display()))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, 500)
                ),
            });
        }
        Ok(())
    }
}

impl MergeJudge for RecipeMergeJudge {
    fn judge(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
    ) -> SimardResult<JudgeOutcome> {
        let state_root = crate::state_root::simard_state_root();

        // Anti-replay: clear any prior record for (repo, pr) BEFORE the run so a
        // stale verdict from an earlier cycle can never leak into this decision.
        if let Err(e) = merge_verdict_store::delete_record(&state_root, repo, pr_number) {
            eprintln!(
                "[simard] {ADAPTER_TAG}: could not clear a prior verdict record for {repo}#{pr_number}: {e}"
            );
        }

        let run_token = Self::fresh_run_token(pr_number);
        self.invoke_recipe(pr_number, repo, snapshot, &state_root, &run_token)?;

        // Read the freshness/identity-checked record the agent recorded.
        let read = merge_verdict_store::read_verified(&state_root, repo, pr_number, &run_token);

        // Independently re-verify the hard safety gates against the SAME
        // allow-list the merge pipeline uses (env-driven in production).
        let allowlist = base_allowlist_from_env();
        Ok(resolve_final_verdict(&read, snapshot, &allowlist))
    }

    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Recipe
    }
}

/// The deterministic decision seam: map the typed record + independent hard-gate
/// re-verification onto a [`JudgeOutcome`]. This is where the rail's authority
/// lives, and it is unit-tested directly (no LLM, no subprocess).
///
/// Rules (issue #4721 R3/R4):
/// * [`Verdict::Ready`] **iff** the freshness-checked record says `merge` AND
///   every hard gate passes (base allow-list, MERGEABLE, CI green, NOT draft).
/// * [`Verdict::NotReady`] if the record says `hold`, OR says `merge` but any
///   hard gate fails — a LOUD refusal: the agent verdict is advisory, the rail
///   is authority, and a `merge` against red CI / draft / non-mergeable must be
///   refused.
/// * [`Verdict::Unclear`] (fail-closed) when there is no valid record for this
///   run ([`ReadOutcome::Missing`] / [`ReadOutcome::Mismatch`]); the merge
///   authority refuses on `Unclear`, so no merge proceeds.
pub fn resolve_final_verdict(
    read: &ReadOutcome,
    snapshot: &PrSnapshot,
    base_allowlist: &[String],
) -> JudgeOutcome {
    match read {
        ReadOutcome::Missing => JudgeOutcome {
            verdict: Verdict::Unclear,
            rationale: format!(
                "{ADAPTER_TAG}: no verdict record was written for this run; failing closed to \
                 unclear (the merge authority refuses on unclear)"
            ),
            blockers: vec![],
        },
        ReadOutcome::Mismatch(why) => JudgeOutcome {
            verdict: Verdict::Unclear,
            rationale: format!(
                "{ADAPTER_TAG}: the verdict record is stale/foreign/corrupt ({why}); failing \
                 closed to unclear"
            ),
            blockers: vec![],
        },
        ReadOutcome::Found(rec) => match rec.verdict {
            VerdictKind::Hold => JudgeOutcome {
                verdict: Verdict::NotReady,
                rationale: format!(
                    "{ADAPTER_TAG}: agent recorded HOLD — not merging. Reason: {}",
                    truncate(&rec.reason, 500)
                ),
                blockers: vec![],
            },
            VerdictKind::Merge => {
                // The agent's `merge` is ADVISORY. The rail independently
                // re-verifies every hard safety gate before authorizing.
                match evaluate_objective_gates(snapshot, base_allowlist) {
                    Ok(()) => JudgeOutcome {
                        verdict: Verdict::Ready,
                        rationale: format!(
                            "{ADAPTER_TAG}: agent recorded MERGE and the rail's independent \
                             hard-gate re-verification passed. Reason: {}",
                            truncate(&rec.reason, 500)
                        ),
                        blockers: vec![],
                    },
                    Err(gate) => {
                        // LOUD refusal: a `merge` verdict against a failing hard
                        // safety gate. The rail is the authority; it refuses.
                        eprintln!(
                            "[simard] {ADAPTER_TAG}: REFUSING a `merge` verdict — a hard safety \
                             gate failed on independent re-verification: {gate}"
                        );
                        JudgeOutcome {
                            verdict: Verdict::NotReady,
                            rationale: format!(
                                "{ADAPTER_TAG}: agent recorded MERGE, but the deterministic rail \
                                 REFUSES — hard safety gate failed: {gate}"
                            ),
                            blockers: vec![],
                        }
                    }
                }
            }
        },
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        prefix + "…"
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_none_when_recipe_missing() {
        let home = tempfile::tempdir().unwrap();
        let judge = RecipeMergeJudge::new_with_home(
            std::path::Path::new("/nonexistent"),
            Some(home.path()),
        );
        assert!(judge.is_none());
    }

    #[test]
    fn kind_returns_recipe() {
        let judge = RecipeMergeJudge {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
        };
        assert_eq!(judge.kind(), MergeJudgeKind::Recipe);
        assert!(judge.kind().is_configured());
    }

    #[test]
    fn fresh_run_token_is_unique_per_call() {
        let a = RecipeMergeJudge::fresh_run_token(42);
        let b = RecipeMergeJudge::fresh_run_token(42);
        assert_ne!(a, b, "run tokens must differ between calls");
        assert!(a.contains("42"), "token should carry the pr number: {a}");
    }
}
