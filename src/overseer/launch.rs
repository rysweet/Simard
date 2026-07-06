//! M2 — the [`RecipeLauncher`] adapter: launch a `smart-orchestrator`
//! workstream and poll it to its PR. The Overseer's core "drive a fix OUTSIDE
//! Simard's loop" action.
//!
//! Reuse (design doc §capability table): the exact `amplihack recipe run
//! amplifier-bundle/recipes/smart-orchestrator.yaml -c task_description=…`
//! invocation engineers use (`src/bin/simard_engineer_loop_recipe.rs:51`), with
//! `AMPLIHACK_AGENT_BINARY` preserved (`src/stewardship/recipe_merge_judge.rs:191`);
//! recipe output is parsed with the shipped noise-stripping in
//! `crate::recipe_output`.
//!
//! Concurrency is bounded by the Overseer's own per-cycle launch cap and budget
//! gate (see [`Overseer::gate`](crate::overseer::Overseer)); the launcher never
//! raises real parallelism beyond those ceilings.
//!
//! The subprocess/probe mechanics are behind an injectable [`RecipeRunner`] seam
//! so the whole launch→PR flow is unit-testable with a fake (no subprocess, no
//! network), matching the roadmap's "fake recipe runner" integration strategy.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::overseer::capabilities::{
    OverseerError, RecipeBrief, RecipeLauncher, WorkstreamHandle, WorkstreamStatus,
};

/// The recipe every Overseer fix-launch runs (`smart-orchestrator` →
/// `default-workflow`), matching the operator's manual workstreams.
pub const SMART_ORCHESTRATOR_RECIPE: &str = "amplifier-bundle/recipes/smart-orchestrator.yaml";

/// Build the `amplihack recipe run …` argument vector for a brief. Pure and
/// unit-tested so the invocation contract is pinned without spawning anything.
pub fn smart_orchestrator_args(brief: &RecipeBrief) -> Vec<String> {
    vec![
        "recipe".to_string(),
        "run".to_string(),
        SMART_ORCHESTRATOR_RECIPE.to_string(),
        "-c".to_string(),
        format!("task_description={}", brief.task_description),
        "-c".to_string(),
        format!("target_repo={}", brief.target_repo),
    ]
}

/// Extract the first `owner/repo` + PR number from recipe output. Recognises a
/// `https://github.com/<owner>/<repo>/pull/<n>` URL after the shipped
/// noise-stripping. Pure + unit-tested.
pub fn extract_pr_ref(output: &str) -> Option<(String, u32)> {
    let cleaned = crate::recipe_output::strip_ansi(output);
    for token in cleaned.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ')') {
        if let Some(rest) = token.split("github.com/").nth(1) {
            // rest = owner/repo/pull/<n>[...]
            let mut parts = rest.split('/');
            let owner = parts.next()?;
            let repo = parts.next()?;
            let kw = parts.next()?;
            if kw != "pull" && kw != "issues" {
                continue;
            }
            let num: String = parts
                .next()
                .unwrap_or_default()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if kw == "pull"
                && !owner.is_empty()
                && !repo.is_empty()
                && let Ok(pr) = num.parse::<u32>()
            {
                return Some((format!("{owner}/{repo}"), pr));
            }
        }
    }
    None
}

// ─────────────────────────── runner seam ───────────────────────────────────

/// Spawns and probes a recipe workstream. Injectable so the launch→PR flow is
/// testable with a fake; production uses [`AmplihackRecipeRunner`].
///
/// `Send + Sync` so a [`SmartOrchestratorLauncher`] can be held in a shared,
/// process-wide handle across an async server's worker threads (e.g. the
/// dashboard feedback endpoint's `OnceLock<SmartOrchestratorLauncher>`). Every
/// implementation is already thread-safe (its state lives behind a `Mutex`).
pub trait RecipeRunner: Send + Sync {
    fn spawn(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError>;
    fn probe(&self, handle: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError>;
}

/// The [`RecipeLauncher`] over a [`RecipeRunner`] seam.
pub struct SmartOrchestratorLauncher {
    runner: Box<dyn RecipeRunner>,
}

impl SmartOrchestratorLauncher {
    pub fn new(runner: Box<dyn RecipeRunner>) -> Self {
        Self { runner }
    }

    /// Production launcher: a real `amplihack recipe run` spawner.
    pub fn from_env() -> Self {
        Self::new(Box::new(AmplihackRecipeRunner::default()))
    }
}

impl RecipeLauncher for SmartOrchestratorLauncher {
    fn launch(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        self.runner.spawn(brief)
    }

    fn poll(&self, handle: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
        self.runner.probe(handle)
    }
}

// ─────────────────────────── real runner ───────────────────────────────────

struct RunEntry {
    child: std::process::Child,
    log_path: std::path::PathBuf,
}

/// Real runner: spawns `amplihack recipe run smart-orchestrator …`, capturing
/// output to a temp log so [`probe`](RecipeRunner::probe) can read the resulting
/// PR once the run finishes. `AMPLIHACK_AGENT_BINARY` is preserved from the
/// caller's environment (Copilot/Claude parity).
#[derive(Default)]
pub struct AmplihackRecipeRunner {
    runs: Mutex<HashMap<String, RunEntry>>,
}

impl RecipeRunner for AmplihackRecipeRunner {
    fn spawn(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        use std::process::{Command, Stdio};

        let log_path = std::env::temp_dir().join(format!(
            "overseer-recipe-{}.log",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let log = std::fs::File::create(&log_path).map_err(|e| OverseerError::Capability {
            what: "recipe.spawn",
            detail: format!("create log {}: {e}", log_path.display()),
        })?;
        let log_err = log.try_clone().map_err(|e| OverseerError::Capability {
            what: "recipe.spawn",
            detail: e.to_string(),
        })?;

        let mut cmd = Command::new("amplihack");
        cmd.args(smart_orchestrator_args(brief))
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        // Preserve AMPLIHACK_AGENT_BINARY if the caller set it (Copilot/Claude
        // parity) — inherited automatically; we do not override it.

        let child = cmd.spawn().map_err(|e| OverseerError::Capability {
            what: "recipe.spawn",
            detail: format!("spawn amplihack: {e}"),
        })?;
        let id = format!("recipe-{}", child.id());
        self.runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), RunEntry { child, log_path });
        Ok(WorkstreamHandle { id })
    }

    fn probe(&self, handle: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
        let mut runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = runs.get_mut(&handle.id).ok_or(OverseerError::Capability {
            what: "recipe.probe",
            detail: format!("unknown workstream {}", handle.id),
        })?;

        match entry.child.try_wait() {
            Ok(None) => Ok(WorkstreamStatus::Running),
            Ok(Some(status)) => {
                let output = std::fs::read_to_string(&entry.log_path).unwrap_or_default();
                if let Some((repo, pr)) = extract_pr_ref(&output) {
                    Ok(WorkstreamStatus::ProducedPr { repo, pr })
                } else if status.success() {
                    Ok(WorkstreamStatus::Failed {
                        reason: "recipe finished but produced no PR".to_string(),
                    })
                } else {
                    Ok(WorkstreamStatus::Failed {
                        reason: format!("recipe exited with {status}"),
                    })
                }
            }
            Err(e) => Err(OverseerError::Capability {
                what: "recipe.probe",
                detail: e.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn args_carry_recipe_and_task_description() {
        let brief = RecipeBrief {
            task_description: "fix distillation banner pollution".to_string(),
            target_repo: "rysweet/Simard".to_string(),
            sequence_group: None,
        };
        let args = smart_orchestrator_args(&brief);
        assert_eq!(args[0], "recipe");
        assert_eq!(args[1], "run");
        assert!(args.iter().any(|a| a == SMART_ORCHESTRATOR_RECIPE));
        assert!(
            args.iter()
                .any(|a| a == "task_description=fix distillation banner pollution")
        );
        assert!(args.iter().any(|a| a == "target_repo=rysweet/Simard"));
    }

    #[test]
    fn extract_pr_ref_finds_github_pull_url() {
        let out = "…work done…\nOpened https://github.com/rysweet/Simard/pull/2601 for review\n";
        assert_eq!(
            extract_pr_ref(out),
            Some(("rysweet/Simard".to_string(), 2601))
        );
    }

    #[test]
    fn extract_pr_ref_ignores_issue_urls_and_noise() {
        assert_eq!(
            extract_pr_ref("see https://github.com/rysweet/Simard/issues/9 only"),
            None
        );
        assert_eq!(extract_pr_ref("no url here"), None);
    }

    #[test]
    fn extract_pr_ref_handles_trailing_punctuation() {
        let out = "PR: (https://github.com/rysweet/amplihack/pull/42).";
        assert_eq!(
            extract_pr_ref(out),
            Some(("rysweet/amplihack".to_string(), 42))
        );
    }

    // ── launcher over a fake runner (no subprocess) ──────────────────────────

    struct FakeRunner {
        launched: Mutex<Vec<RecipeBrief>>,
        status: WorkstreamStatus,
    }
    impl RecipeRunner for FakeRunner {
        fn spawn(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
            self.launched.lock().unwrap().push(brief.clone());
            Ok(WorkstreamHandle {
                id: "ws-1".to_string(),
            })
        }
        fn probe(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
            Ok(self.status.clone())
        }
    }

    #[test]
    fn launcher_spawns_and_polls_through_the_seam() {
        let runner = FakeRunner {
            launched: Mutex::new(vec![]),
            status: WorkstreamStatus::ProducedPr {
                repo: "rysweet/Simard".to_string(),
                pr: 2601,
            },
        };
        let launcher = SmartOrchestratorLauncher::new(Box::new(runner));
        let brief = RecipeBrief {
            task_description: "fix restart churn".to_string(),
            target_repo: "rysweet/Simard".to_string(),
            sequence_group: None,
        };
        let handle = launcher.launch(&brief).unwrap();
        assert_eq!(handle.id, "ws-1");
        assert_eq!(
            launcher.poll(&handle).unwrap(),
            WorkstreamStatus::ProducedPr {
                repo: "rysweet/Simard".to_string(),
                pr: 2601,
            }
        );
    }
}
