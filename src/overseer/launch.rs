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
//!
//! # Per-signature launch idempotency (#4125)
//!
//! A blocked goal/signature recurs on every Overseer tick, so without a rail the
//! launcher would spawn a byte-identical `amplihack recipe run smart-orchestrator`
//! for the SAME work every cycle (observed 2026-07-16: three identical processes
//! 47/28/12 min apart, wasting compute). [`AmplihackRecipeRunner::spawn`] is
//! therefore **idempotent per signature**:
//!
//! 1. **Reap** — every tracked run is polled so a finished subprocess is always
//!    `wait`ed (no leaked OS zombie), but an entry is **evicted** (and its temp
//!    log unlinked) only when it is the signature being spawned *and* its child
//!    has definitively **exited**. A completed run belonging to a *different*
//!    signature is **kept** so its handle holder (e.g. the dashboard
//!    `/api/feedback/status/{id}` poller) can still read its terminal PR outcome
//!    — evicting it here would silently turn a produced PR into an "unknown
//!    workstream" (fail-visible result loss); it is freed lazily when its own
//!    signature next respawns, or on process exit. A run that is still executing
//!    — or whose state is momentarily indeterminate (a poll `Err`) — is **kept**
//!    for its own signature, so a transient error can never let a byte-identical
//!    recipe relaunch (fail-closed, matching the sibling
//!    `inflight_investigations` reconcile in [`crate::overseer`]). A genuinely
//!    completed run of the same signature is still freed here, so suppression is
//!    never permanent for a real completion.
//! 2. **Suppress** — if a still-Running run exists for the same signature, no
//!    second process is spawned; the existing run's handle is returned so the
//!    deduped caller polls the SAME run. The suppression is **fail-visible**: a
//!    `tracing::warn!(target = "overseer::recipe")` is emitted (never silent).
//! 3. **Spawn** — only a genuinely new (or newly-recurring, post-completion)
//!    signature spawns a fresh process.
//!
//! The signature ([`recipe_signature`]) folds the normalized `target_repo` and
//! `task_description` (trim + lowercase + whitespace-collapse, joined by a
//! non-whitespace `\u{1F}` unit separator) so cosmetic differences cannot defeat
//! dedup while genuinely different tasks stay distinct. The map key and the
//! caller-facing [`WorkstreamHandle::id`] are a bounded hex `sig_token`
//! (`hex(hash(signature))`) — URL-path-safe for the dashboard round-trip and
//! leaking no brief text into logs.
//!
//! # Cross-tick durability (#4125)
//!
//! The reported defect is *cross-tick*: the Overseer daemon rebuilds the entire
//! Overseer — and therefore a fresh launcher — on **every** meta-OODA tick
//! (`crate::operator_commands_ooda::daemon` calls `crate::overseer::build_overseer`
//! inside the tick thread; default cadence 900 s), and the three duplicate
//! processes were 47/28/12 min apart == three *separate* ticks. A per-instance
//! `runs` map would be empty at the start of every tick, so the rail above could
//! only ever dedup *within* one tick and would still spawn a duplicate on the
//! next. The in-flight registry is therefore **process-wide**:
//! [`AmplihackRecipeRunner::from_env`] — the only production constructor, used by
//! both the daemon tick ([`crate::overseer::build_overseer`]) and the dashboard
//! feedback endpoint — shares one `runs` map for the whole process, so
//! suppression survives the tick rebuild (and dedups across the two launch
//! paths). That process-scoped durability is what actually closes #4125.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::overseer::capabilities::{
    OverseerError, RecipeBrief, RecipeLauncher, WorkstreamHandle, WorkstreamStatus,
};

/// The recipe every Overseer fix-launch runs (`smart-orchestrator` →
/// `default-workflow`), matching the operator's manual workstreams.
pub const SMART_ORCHESTRATOR_RECIPE: &str = "amplifier-bundle/recipes/smart-orchestrator.yaml";

/// Build the `amplihack recipe run …` argument vector for a brief. Pure and
/// unit-tested so the invocation contract is pinned without spawning anything.
///
/// The free-text `task_description` is bounded with the shared context-var
/// sanitizer before it rides on argv (issues #2640/#2692): a generous 8000-char
/// ceiling defensively closes the E2BIG argv-overflow class, and the same pass
/// collapses newlines so a multi-line brief can never break the recipe's YAML
/// interpolation (#2127). `target_repo` is a short slug and stays verbatim.
///
/// A `spawn_payload::recipe_context` file channel (`task_description_path`)
/// would make this lossless, but `smart-orchestrator.yaml` is an EXTERNAL asset
/// (amplihack bundle, not this repo) that reads `{{task_description}}` inline
/// with no `{{task_description_path}}` support — filing a large value would leave
/// the orchestrator with an empty task. Bounded-inline (E2BIG-safe: 8000 chars ≪
/// ARG_MAX) stays the safe disposition until that external asset gains a `_path`
/// read.
pub fn smart_orchestrator_args(brief: &RecipeBrief) -> Vec<String> {
    let task_description =
        crate::ooda_brain::sanitize::sanitize_context_var(&brief.task_description, 8000);
    vec![
        "recipe".to_string(),
        "run".to_string(),
        SMART_ORCHESTRATOR_RECIPE.to_string(),
        "-c".to_string(),
        format!("task_description={task_description}"),
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

/// Stable per-signature dedup key for a [`RecipeBrief`]. Folds the normalized
/// `target_repo` and `task_description` so cosmetic differences (case, leading/
/// trailing/interior whitespace) cannot defeat launch dedup, while genuinely
/// different work stays distinct. A non-whitespace `\u{1F}` unit separator keeps
/// the two fields from colliding at their boundary (e.g. `("a","bc")` vs
/// `("ab","c")`). Pure + unit-tested.
fn recipe_signature(brief: &RecipeBrief) -> String {
    fn normalize(s: &str) -> String {
        // trim + lowercase + collapse any whitespace run to a single space.
        s.split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    }
    format!(
        "{}\u{1f}{}",
        normalize(&brief.target_repo),
        normalize(&brief.task_description)
    )
}

/// Bounded, URL-path-safe token for a signature: `hex(hash(signature))`. Used as
/// both the `runs` map key and the caller-facing [`WorkstreamHandle::id`] so a
/// handle can round-trip through the operator dashboard's `/api/feedback/status/
/// {id}` path without leaking brief text or carrying `/`, whitespace, or the
/// `\u{1F}` separator.
fn signature_token(signature: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    signature.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Terminal-or-running outcome of a spawned recipe child, decoupled from
/// [`std::process::ExitStatus`] (not constructible in tests) so the launch flow
/// is unit-testable through the [`ChildSpawner`] seam.
#[derive(Clone)]
struct ChildExit {
    /// Whether the child exited successfully (`ExitStatus::success`).
    success: bool,
    /// Human-readable exit description (the `ExitStatus` `Display` string),
    /// surfaced in a failure reason.
    description: String,
}

/// A spawned recipe child that can be polled without blocking. `Ok(None)` means
/// still running; `Ok(Some(exit))` means it has terminated.
trait SpawnedChild: Send {
    fn poll(&mut self) -> std::io::Result<Option<ChildExit>>;
}

/// The subprocess-spawn half of the runner seam. Injectable so tests can count
/// spawns and drive child exits without launching a real `amplihack`. Production
/// uses [`RealChildSpawner`].
trait ChildSpawner: Send + Sync {
    /// Spawn the recipe process for `brief`, returning the running child and the
    /// path its stdout/stderr are captured to (read by [`RecipeRunner::probe`]).
    fn spawn(&self, brief: &RecipeBrief) -> std::io::Result<(Box<dyn SpawnedChild>, PathBuf)>;
}

/// Production [`SpawnedChild`] wrapping a real [`std::process::Child`].
struct RealChild(std::process::Child);
impl SpawnedChild for RealChild {
    fn poll(&mut self) -> std::io::Result<Option<ChildExit>> {
        match self.0.try_wait()? {
            None => Ok(None),
            Some(status) => Ok(Some(ChildExit {
                success: status.success(),
                description: status.to_string(),
            })),
        }
    }
}

/// Production [`ChildSpawner`]: spawns `amplihack recipe run smart-orchestrator …`
/// capturing output to a temp log, with `AMPLIHACK_AGENT_BINARY` preserved from
/// the caller's environment (Copilot/Claude parity).
struct RealChildSpawner;
impl ChildSpawner for RealChildSpawner {
    fn spawn(&self, brief: &RecipeBrief) -> std::io::Result<(Box<dyn SpawnedChild>, PathBuf)> {
        use std::process::{Command, Stdio};

        let log_path = std::env::temp_dir().join(format!(
            "overseer-recipe-{}.log",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        // Owner-only (0600) on unix: the captured recipe stdout/stderr can carry
        // tokens/secrets, so the temp log must not be world-readable (security
        // review, PR #4142). The log is unlinked when its run is reaped.
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let log = opts.open(&log_path).map_err(|e| {
            std::io::Error::new(e.kind(), format!("create log {}: {e}", log_path.display()))
        })?;
        let log_err = log.try_clone()?;

        let mut cmd = Command::new("amplihack");
        cmd.args(smart_orchestrator_args(brief))
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        // Preserve AMPLIHACK_AGENT_BINARY if the caller set it (Copilot/Claude
        // parity) — inherited automatically; we do not override it.

        let child = cmd.spawn().map_err(|e| {
            // Pre-exec spawn failure (E2BIG and siblings) has no child; classify
            // + record into the Overseer sink before surfacing — no silent drop
            // (issue #2640).
            crate::spawn_payload::record_spawn_failure(&e, "overseer.recipe.spawn");
            std::io::Error::new(e.kind(), format!("spawn amplihack: {e}"))
        })?;
        Ok((Box::new(RealChild(child)), log_path))
    }
}

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

    /// Production launcher: a real `amplihack recipe run` spawner sharing the
    /// process-wide in-flight registry so launch dedup survives the daemon's
    /// per-tick Overseer rebuild (#4125 — see the module docs).
    pub fn from_env() -> Self {
        Self::new(Box::new(AmplihackRecipeRunner::from_env()))
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
    child: Box<dyn SpawnedChild>,
    log_path: std::path::PathBuf,
}

/// Cap on how much of a recipe's captured output [`probe`](RecipeRunner::probe)
/// reads. The child writes this log, so an unbounded `read_to_string` is an OOM
/// / resource-exhaustion vector (security review, PR #4142). The completion PR
/// URL the probe scans for is printed at the END of a run, so bounding to the
/// tail keeps the relevant region while capping memory.
const MAX_PROBE_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// Read at most the final [`MAX_PROBE_LOG_BYTES`] of `path`, lossily as UTF-8.
/// Any I/O error (missing/unreadable log) yields an empty string, preserving the
/// prior `unwrap_or_default()` "no PR" behaviour.
fn read_log_tail(path: &std::path::Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(MAX_PROBE_LOG_BYTES);
    if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.take(MAX_PROBE_LOG_BYTES).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Real runner: spawns `amplihack recipe run smart-orchestrator …`, capturing
/// output to a temp log so [`probe`](RecipeRunner::probe) can read the resulting
/// PR once the run finishes. `AMPLIHACK_AGENT_BINARY` is preserved from the
/// caller's environment (Copilot/Claude parity).
///
/// Launches are **idempotent per signature** (see the module docs): the `runs`
/// map is keyed by a [`signature_token`], and [`spawn`](RecipeRunner::spawn)
/// reaps finished runs then suppresses (with a visible warning) any duplicate
/// launch for a still-Running signature. In production the `runs` map is
/// **process-wide** ([`AmplihackRecipeRunner::from_env`] → [`shared_runs`]) so
/// suppression survives the daemon's per-tick Overseer rebuild (#4125).
pub struct AmplihackRecipeRunner {
    spawner: Box<dyn ChildSpawner>,
    runs: Arc<Mutex<HashMap<String, RunEntry>>>,
}

/// The process-wide in-flight run registry shared by every
/// [`AmplihackRecipeRunner::from_env`] instance. The daemon rebuilds a fresh
/// launcher every meta-OODA tick, so a per-instance map would reset each cycle
/// and defeat cross-tick dedup; one process-scoped map (see the module docs,
/// "Cross-tick durability") is what makes suppression survive the rebuild and
/// actually close #4125.
fn shared_runs() -> Arc<Mutex<HashMap<String, RunEntry>>> {
    static RUNS: OnceLock<Arc<Mutex<HashMap<String, RunEntry>>>> = OnceLock::new();
    RUNS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

impl AmplihackRecipeRunner {
    /// Production runner: a real `amplihack` spawner sharing the **process-wide**
    /// in-flight registry ([`shared_runs`]) so launch dedup survives the daemon's
    /// per-tick Overseer rebuild (#4125). Every `from_env()` runner in the
    /// process reaps and suppresses against the same map, so a duplicate launch
    /// for an in-flight signature is caught even across ticks — and across the
    /// daemon-tick and dashboard-feedback launch paths, which now share it.
    pub fn from_env() -> Self {
        Self {
            spawner: Box::new(RealChildSpawner),
            runs: shared_runs(),
        }
    }

    /// Test-only constructor injecting a fake [`ChildSpawner`] with a private,
    /// non-shared registry so the idempotency rail can be exercised without
    /// launching a real `amplihack` and without touching the process-wide map.
    #[cfg(test)]
    fn with_spawner(spawner: Box<dyn ChildSpawner>) -> Self {
        Self {
            spawner,
            runs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Test-only constructor that SHARES an explicit `runs` registry, simulating
    /// the daemon rebuilding the Overseer across ticks while the process-wide
    /// registry persists (proves cross-tick dedup — #4125).
    #[cfg(test)]
    fn with_spawner_sharing(
        spawner: Box<dyn ChildSpawner>,
        runs: Arc<Mutex<HashMap<String, RunEntry>>>,
    ) -> Self {
        Self { spawner, runs }
    }
}

impl RecipeRunner for AmplihackRecipeRunner {
    fn spawn(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
        let token = signature_token(&recipe_signature(brief));

        let mut runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Reap. The `runs` map is process-wide and SHARED across launch paths
        // (daemon tick + dashboard feedback — see the module docs), so reaping
        // has two distinct jobs that must not be conflated:
        //
        //  * Poll EVERY tracked child on every spawn so a finished subprocess is
        //    always `wait`ed — no OS zombie is ever leaked, including for entries
        //    launched by a previous, now-discarded tick's Overseer that nothing
        //    re-probes.
        //  * EVICT (and unlink the temp log of) only the entry for the signature
        //    being spawned right now. Evicting *another* path's completed run
        //    here would delete a terminal result — and its still-unread log — out
        //    from under a handle holder (e.g. the dashboard
        //    `/api/feedback/status/{id}` poller), silently turning its produced
        //    PR into an "unknown workstream" 404 (fail-visible result loss,
        //    PR #4142 audit). Other paths' completed runs are therefore KEPT so
        //    their holder can still read the outcome; they are freed lazily when
        //    their OWN signature next respawns, or on process exit.
        //
        // A still-running child — or one whose poll is momentarily `Err` — is
        // KEPT for its own signature so a duplicate launch stays suppressed.
        // Fail-CLOSED: a transient poll error can never let a byte-identical
        // recipe relaunch (#4125), matching the sibling `inflight_investigations`
        // reconcile. A genuinely-completed run of the SAME signature is still
        // freed here, so suppression is never permanent for a real completion.
        let own_exited = matches!(
            runs.get_mut(&token).map(|e| e.child.poll()),
            Some(Ok(Some(_)))
        );
        for (key, entry) in runs.iter_mut() {
            if key != &token {
                // Zombie-reap other paths' children without evicting them.
                let _ = entry.child.poll();
            }
        }
        if own_exited && let Some(entry) = runs.remove(&token) {
            let _ = std::fs::remove_file(&entry.log_path);
        }

        // Suppress: a still-Running run for this signature already exists — do
        // NOT spawn a second process; hand back the shared run (idempotent).
        if runs.contains_key(&token) {
            tracing::warn!(
                target: "overseer::recipe",
                signature = %token,
                task_repo = %brief.target_repo,
                "duplicate recipe launch suppressed; reusing in-flight run"
            );
            return Ok(WorkstreamHandle { id: token });
        }

        // Spawn: genuinely new (or newly-recurring) signature.
        let (child, log_path) =
            self.spawner
                .spawn(brief)
                .map_err(|e| OverseerError::Capability {
                    what: "recipe.spawn",
                    detail: e.to_string(),
                })?;
        runs.insert(token.clone(), RunEntry { child, log_path });
        Ok(WorkstreamHandle { id: token })
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

        match entry.child.poll() {
            Ok(None) => Ok(WorkstreamStatus::Running),
            Ok(Some(exit)) => {
                let output = read_log_tail(&entry.log_path);
                if let Some((repo, pr)) = extract_pr_ref(&output) {
                    Ok(WorkstreamStatus::ProducedPr { repo, pr })
                } else if exit.success {
                    Ok(WorkstreamStatus::Failed {
                        reason: "recipe finished but produced no PR".to_string(),
                    })
                } else {
                    Ok(WorkstreamStatus::Failed {
                        reason: format!("recipe exited with {}", exit.description),
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

    // ── per-signature idempotency (#4125) ────────────────────────────────────
    //
    // TDD contract for the launcher-level dedup rail. These exercise the
    // injectable child-spawn seam (`ChildSpawner` / `SpawnedChild` / `ChildExit`),
    // the pure `recipe_signature` normalization, and the `sig_token`-keyed
    // `AmplihackRecipeRunner::with_spawner` so no real `amplihack` is launched.

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn mk_brief(repo: &str, task: &str) -> RecipeBrief {
        RecipeBrief {
            task_description: task.to_string(),
            target_repo: repo.to_string(),
            sequence_group: None,
        }
    }

    /// Shared, test-controlled exit cell for a fake child: `None` = running,
    /// `Some(exit)` = terminated.
    type ExitCell = Arc<Mutex<Option<ChildExit>>>;

    /// A `SpawnedChild` whose exit is flipped by the test through a shared cell.
    /// `poll()` returns `Ok(None)` (running) until the test sets the cell.
    struct FakeChild {
        exit: ExitCell,
    }
    impl SpawnedChild for FakeChild {
        fn poll(&mut self) -> std::io::Result<Option<ChildExit>> {
            Ok(self
                .exit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }
    }

    /// Counts spawns and exposes each spawned child's exit cell so a test can
    /// flip a specific run to "exited". Cloned `Arc` handles let the test keep
    /// observing state after the spawner is moved into the runner.
    #[derive(Clone)]
    struct FakeChildSpawner {
        spawn_count: Arc<AtomicUsize>,
        exits: Arc<Mutex<Vec<ExitCell>>>,
    }
    impl FakeChildSpawner {
        fn new() -> Self {
            Self {
                spawn_count: Arc::new(AtomicUsize::new(0)),
                exits: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn spawns(&self) -> usize {
            self.spawn_count.load(Ordering::SeqCst)
        }
        /// Flip the Nth spawned child (0-based) to an exited state.
        fn finish(&self, idx: usize, success: bool) {
            let cells = self
                .exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *cells[idx]
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ChildExit {
                success,
                description: "exit status: 0".to_string(),
            });
        }
    }
    impl ChildSpawner for FakeChildSpawner {
        fn spawn(&self, _brief: &RecipeBrief) -> std::io::Result<(Box<dyn SpawnedChild>, PathBuf)> {
            let n = self.spawn_count.fetch_add(1, Ordering::SeqCst);
            let cell = Arc::new(Mutex::new(None));
            self.exits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(cell.clone());
            // A never-created path is fine: `probe` reads the log with
            // `unwrap_or_default()`, so a missing file yields "no PR".
            let log = std::env::temp_dir().join(format!("fake-recipe-{n}.log"));
            Ok((Box::new(FakeChild { exit: cell }), log))
        }
    }

    fn fake_runner() -> (AmplihackRecipeRunner, FakeChildSpawner) {
        let spawner = FakeChildSpawner::new();
        let runner = AmplihackRecipeRunner::with_spawner(Box::new(spawner.clone()));
        (runner, spawner)
    }

    #[test]
    fn recipe_signature_collapses_cosmetic_differences() {
        // Case + surrounding/interior whitespace must NOT defeat dedup.
        let a = recipe_signature(&mk_brief("rysweet/Simard", "Fix A"));
        let b = recipe_signature(&mk_brief("rysweet/simard", "  fix   a "));
        assert_eq!(a, b, "cosmetic-only differences must share a signature");
    }

    #[test]
    fn recipe_signature_distinguishes_real_differences() {
        let base = recipe_signature(&mk_brief("rysweet/Simard", "fix A"));
        // Different task text.
        assert_ne!(base, recipe_signature(&mk_brief("rysweet/Simard", "fix B")));
        // Same task text, different repo — genuinely different work.
        assert_ne!(
            base,
            recipe_signature(&mk_brief("rysweet/amplihack", "fix A"))
        );
    }

    #[test]
    fn recipe_signature_separator_prevents_field_collision() {
        // Without a field separator, ("a","bc") and ("ab","c") would collide.
        assert_ne!(
            recipe_signature(&mk_brief("a", "bc")),
            recipe_signature(&mk_brief("ab", "c"))
        );
    }

    #[test]
    fn same_brief_while_running_spawns_once_and_shares_handle() {
        let (runner, spawner) = fake_runner();
        let brief = mk_brief("rysweet/Simard", "fix kgpacks-rs blocked goal");

        let h1 = runner.spawn(&brief).unwrap();
        let h2 = runner.spawn(&brief).unwrap();

        assert_eq!(spawner.spawns(), 1, "duplicate launch must be suppressed");
        assert_eq!(h1.id, h2.id, "deduped caller gets the same handle id");

        // Both handles point at the SAME underlying run: flip that one child to
        // exited and both probes must observe the identical terminal status.
        spawner.finish(0, true);
        let s1 = runner.probe(&h1).unwrap();
        let s2 = runner.probe(&h2).unwrap();
        assert_eq!(s1, s2, "both handles probe the same shared run");
        assert_eq!(
            s1,
            WorkstreamStatus::Failed {
                reason: "recipe finished but produced no PR".to_string(),
            }
        );
    }

    #[test]
    fn cosmetic_variant_while_running_is_also_suppressed() {
        let (runner, spawner) = fake_runner();
        let _h1 = runner.spawn(&mk_brief("rysweet/Simard", "Fix A")).unwrap();
        let _h2 = runner
            .spawn(&mk_brief("rysweet/simard", "  fix   a "))
            .unwrap();
        assert_eq!(
            spawner.spawns(),
            1,
            "cosmetic-only variant must dedup against the running run"
        );
    }

    #[test]
    fn relaunch_after_completion_spawns_fresh() {
        let (runner, spawner) = fake_runner();
        let brief = mk_brief("rysweet/Simard", "fix kgpacks-rs blocked goal");

        let _h1 = runner.spawn(&brief).unwrap();
        assert_eq!(spawner.spawns(), 1);

        // First run completes. The next spawn must reap it, then launch fresh —
        // suppression is never permanent.
        spawner.finish(0, true);
        let _h3 = runner.spawn(&brief).unwrap();
        assert_eq!(
            spawner.spawns(),
            2,
            "a re-occurrence after completion must spawn a new process"
        );
    }

    #[test]
    fn distinct_briefs_each_spawn() {
        let (runner, spawner) = fake_runner();
        let _a = runner.spawn(&mk_brief("rysweet/Simard", "fix A")).unwrap();
        let _b = runner.spawn(&mk_brief("rysweet/Simard", "fix B")).unwrap();
        assert_eq!(
            spawner.spawns(),
            2,
            "distinct signatures must not dedup against each other"
        );
    }

    #[test]
    fn completed_run_survives_unrelated_spawn_for_its_holder() {
        // The `runs` map is shared across launch paths (daemon + dashboard). A
        // spawn for signature B must NOT evict signature A's already-completed
        // run: A's handle holder (e.g. the dashboard status poller) must still be
        // able to read A's terminal outcome. Before the fix, the global reap
        // deleted A's entry (and its log) on B's spawn, turning A's produced PR
        // into an "unknown workstream" error (PR #4142 audit).
        let (runner, spawner) = fake_runner();
        let brief_a = mk_brief("rysweet/Simard", "fix A");
        let brief_b = mk_brief("rysweet/Simard", "fix B");

        let ha = runner.spawn(&brief_a).unwrap();
        spawner.finish(0, true); // A completes.

        // An unrelated signature is launched on the SAME shared map.
        let _hb = runner.spawn(&brief_b).unwrap();
        assert_eq!(spawner.spawns(), 2, "B is a new signature and must spawn");

        // A's holder can STILL read A's terminal status — it was not evicted.
        assert_eq!(
            runner.probe(&ha).unwrap(),
            WorkstreamStatus::Failed {
                reason: "recipe finished but produced no PR".to_string(),
            },
            "an unrelated spawn must not lose a completed run's terminal result"
        );
    }

    #[test]
    fn shared_registry_dedups_across_launcher_rebuilds() {
        // #4125 core: the daemon rebuilds the whole Overseer — and a fresh
        // runner — every tick. A process-wide registry must make suppression
        // survive that rebuild, so two runners sharing one `runs` map (tick N and
        // tick N+1) spawn the SAME in-flight signature only once. Before the fix
        // the per-tick map was empty each cycle and a byte-identical duplicate
        // was spawned on the next tick.
        let spawner = FakeChildSpawner::new();
        let runs = Arc::new(Mutex::new(HashMap::new()));
        let tick1 =
            AmplihackRecipeRunner::with_spawner_sharing(Box::new(spawner.clone()), runs.clone());
        let tick2 =
            AmplihackRecipeRunner::with_spawner_sharing(Box::new(spawner.clone()), runs.clone());
        let brief = mk_brief("rysweet/Simard", "fix kgpacks-rs blocked goal");

        let h1 = tick1.spawn(&brief).unwrap(); // tick N
        let h2 = tick2.spawn(&brief).unwrap(); // tick N+1: fresh runner, shared map

        assert_eq!(
            spawner.spawns(),
            1,
            "an in-flight signature must not respawn on the next tick's fresh runner"
        );
        assert_eq!(h1.id, h2.id, "both ticks resolve to the same in-flight run");
    }

    /// A child whose `poll()` always errors, to exercise fail-closed reap.
    struct ErrChild;
    impl SpawnedChild for ErrChild {
        fn poll(&mut self) -> std::io::Result<Option<ChildExit>> {
            Err(std::io::Error::other("transient poll failure"))
        }
    }
    struct ErrChildSpawner {
        spawn_count: Arc<AtomicUsize>,
    }
    impl ChildSpawner for ErrChildSpawner {
        fn spawn(&self, _brief: &RecipeBrief) -> std::io::Result<(Box<dyn SpawnedChild>, PathBuf)> {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            Ok((
                Box::new(ErrChild),
                std::env::temp_dir().join("fake-err.log"),
            ))
        }
    }

    #[test]
    fn reap_is_fail_closed_on_poll_error() {
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let runner = AmplihackRecipeRunner::with_spawner(Box::new(ErrChildSpawner {
            spawn_count: spawn_count.clone(),
        }));
        let brief = mk_brief("rysweet/Simard", "fix kgpacks-rs blocked goal");

        let _h1 = runner.spawn(&brief).unwrap();
        assert_eq!(spawn_count.load(Ordering::SeqCst), 1);

        // Second launch: reap polls the prior child, which ERRORS. Fail-closed
        // means the entry is KEPT and the duplicate is suppressed — never a
        // second process on a transient poll error (#4125).
        let _h2 = runner.spawn(&brief).unwrap();
        assert_eq!(
            spawn_count.load(Ordering::SeqCst),
            1,
            "a poll error must not evict the in-flight entry (fail-closed)"
        );
    }

    // In-memory `tracing` writer so the suppressed-launch warning can be asserted
    // without adding a test dependency.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn suppressed_duplicate_logs_visible_warning() {
        let (runner, _spawner) = fake_runner();
        let brief = mk_brief("rysweet/Simard", "fix kgpacks-rs blocked goal");

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(BufWriter(buf.clone()))
            .with_target(true)
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let _h1 = runner.spawn(&brief).unwrap();
            let _h2 = runner.spawn(&brief).unwrap(); // suppressed → warns
        });

        let logged = String::from_utf8(
            buf.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .unwrap();
        assert!(
            logged.contains("overseer::recipe"),
            "warning must carry the overseer::recipe target; got: {logged}"
        );
        assert!(
            logged.contains("duplicate recipe launch suppressed"),
            "suppression must be fail-visible; got: {logged}"
        );
    }

    #[test]
    fn handle_id_is_url_safe_and_round_trips() {
        let (runner, _spawner) = fake_runner();
        let brief = mk_brief("rysweet/Simard", "fix kgpacks-rs blocked goal");

        let h = runner.spawn(&brief).unwrap();

        // sig_token = hex(hash(signature)): non-empty, all hex, URL-path-safe.
        assert!(!h.id.is_empty());
        assert!(
            h.id.chars().all(|c| c.is_ascii_hexdigit()),
            "handle id must be a hex digest (URL-safe, no '/', whitespace, or \\u{{1F}}); got: {}",
            h.id
        );

        // A handle rebuilt from the (URL round-tripped) id probes the SAME run,
        // as the dashboard feedback endpoint does.
        let rebuilt = WorkstreamHandle { id: h.id.clone() };
        assert_eq!(runner.probe(&rebuilt).unwrap(), runner.probe(&h).unwrap());
    }
}
