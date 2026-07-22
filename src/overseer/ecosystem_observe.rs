//! Thin Rust rail for the agentic **ecosystem-observe** chain (issue #2419).
//!
//! Simard observes her stewarded ecosystem with a DETERMINISTIC WORKFLOW OF
//! AGENTIC STEPS + PROMPTS — not a Rust "code sensor." This module is the only
//! new Rust the feature needs, and it is deliberately thin:
//!
//! 1. [`resolve_governed_roster`] resolves the stewarded roster as pure DATA (a
//!    list of `owner/name` slugs) from Simard's **identity-scoped curated state**
//!    (see [`crate::identity_curated_state`]) — durable, mutable, deploy-durable
//!    state seeded from the identity, NOT a committed framework file. Each slug is
//!    validated before use.
//! 2. [`should_observe`] decides, on the Overseer cadence, whether an observation
//!    pass is due this tick (reusing the existing gap-scan enable / every-N gate).
//! 3. [`RecipeEcosystemObserver`] invokes the `ecosystem-observe` recipe through
//!    an injectable [`EcosystemRecipeRunner`] seam and forwards its **opaque**
//!    semantic result into the existing gated launch machinery.
//!
//! What this module deliberately does NOT do (the retired anti-pattern): it never
//! calls `gh`, never parses issue/PR/CI output, and holds NO per-repo observation
//! state (no counts, no activity structs, no problem lists). The observation
//! lives entirely in the AGENT's reasoning and is handed forward SEMANTICALLY as
//! an opaque string. See `docs/design/ecosystem-observe.md`.

use std::path::Path;

use crate::error::{SimardError, SimardResult};
use crate::identity_curated_state::{CuratedDataStore, CuratedItem, CuratedList};

// ─────────────────────────── roster (identity data) ────────────────────────

/// The identity that owns the default governed roster: Simard herself.
pub const DEFAULT_ROSTER_IDENTITY: &str = "simard";

/// The curated-state dataset name for the governed-repo roster. One identity may
/// own many datasets ([`crate::identity_curated_state`]); this is the one whose
/// items are `owner/name` repo slugs.
pub const GOVERNED_ROSTER_DATASET: &str = "governed_repos";

/// Simard's DEFAULT governed roster — the repos she stewards out of the box.
///
/// This is Simard's default-**identity** data (like `DEFAULT_SEED_GOALS`): the
/// seed that first populates her durable, mutable, deploy-durable roster. It
/// replaces the retired committed framework file `ecosystem_repos.toml`, so the
/// roster is now part of *who Simard is*, not a git-tracked file that every
/// self-deploy clobbers. `amplihack` means `rysweet/amplihack-rs`; the Python
/// `rysweet/amplihack` is deprecated and deliberately NOT on the roster.
///
/// After first use the durable curated copy is authoritative — Simard curates it
/// agentically (add/remove a stewarded repo) and her edits survive redeploys.
pub fn default_simard_roster_seed() -> CuratedList {
    CuratedList::from_items([
        CuratedItem::new(
            "rysweet/Simard",
            "Orchestrator / self-improving engineering identity (steward of this roster)",
        ),
        CuratedItem::new(
            "rysweet/RustyClawd",
            "Rust-native LLM agent SDK (base type)",
        ),
        CuratedItem::new(
            "rysweet/amplihack-rs",
            "Core framework — skills, workflows, recipes, hooks, CLI, fleet",
        ),
        CuratedItem::new("rysweet/azlin", "Azure VM provisioning CLI"),
        CuratedItem::new(
            "rysweet/amplihack-memory-lib",
            "Graph-based 6-type cognitive memory (LadybugDB/lbug-backed)",
        ),
        CuratedItem::new(
            "rysweet/amplihack-agent-eval",
            "Agent evaluation harness — L1–L12 benchmarks",
        ),
        CuratedItem::new(
            "rysweet/agent-kgpacks",
            "Knowledge graph packages — GraphRAG grounding",
        ),
        CuratedItem::new(
            "rysweet/amplihack-recipe-runner",
            "Code-enforced YAML workflow execution engine",
        ),
        CuratedItem::new(
            "rysweet/amplihack-xpia-defender",
            "Cross-Prompt Injection Attack detection library",
        ),
        CuratedItem::new(
            "rysweet/gadugi-agentic-test",
            "Multi-agent outside-in testing (Electron/CLI/web/TUI)",
        ),
    ])
}

/// Validate a stewarded-repo slug as `owner/name`. Rejects anything that is not a
/// clean two-segment slug: a missing or extra `/`, an empty segment, embedded
/// whitespace, path traversal (`..`), a leading `-`, or any shell metacharacter.
/// Only `[A-Za-z0-9._-]` is permitted per segment, so a malformed slug can never
/// reach `gh`.
fn is_valid_slug(slug: &str) -> bool {
    let mut parts = slug.split('/');
    let (owner, name) = match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) => (owner, name),
        _ => return false, // missing '/' or more than two segments
    };
    for segment in [owner, name] {
        if segment.is_empty() || segment.starts_with('-') || segment.contains("..") {
            return false;
        }
        if segment
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
        {
            return false;
        }
    }
    true
}

/// Validate a set of roster `value`s as clean `owner/name` slugs.
///
/// Returns the validated slugs in order. Each is checked with [`is_valid_slug`];
/// a malformed slug is **skipped with a logged warning** — it never reaches the
/// agent's `gh` calls. An empty result (no valid slugs, whether the dataset was
/// empty or every slug was malformed) is an **error**, never a silent empty pass:
/// the caller skips the observation tick and fabricates no Problems.
pub fn validate_roster_slugs(values: &[String]) -> Result<Vec<String>, String> {
    let mut roster = Vec::with_capacity(values.len());
    for value in values {
        let slug = value.trim();
        if is_valid_slug(slug) {
            roster.push(slug.to_string());
        } else {
            tracing::warn!(
                target: "overseer::ecosystem_observe",
                slug = %value,
                "ecosystem roster: skipping malformed slug (not a clean owner/name)"
            );
        }
    }
    if roster.is_empty() {
        return Err("ecosystem roster has no valid owner/name slugs".to_string());
    }
    Ok(roster)
}

/// Resolve the identity + seed for the governed roster from the active identity's
/// cognition.
///
/// - A **named** identity with a non-empty `target_repos` owns its OWN roster,
///   seeded from those repos — this is the generic identity-scoped mechanism at
///   work (a non-Simard identity stewards its declared target scope).
/// - Otherwise the roster belongs to Simard (the default identity) and is seeded
///   from her baked [`default_simard_roster_seed`].
pub fn governed_roster_seed_for(
    identity_name: Option<&str>,
    target_repos: &[String],
) -> (String, CuratedList) {
    match identity_name {
        Some(name) if !name.trim().is_empty() && !target_repos.is_empty() => (
            name.trim().to_string(),
            CuratedList::from_items(
                target_repos
                    .iter()
                    .map(|repo| CuratedItem::new(repo.clone(), "")),
            ),
        ),
        _ => (
            DEFAULT_ROSTER_IDENTITY.to_string(),
            default_simard_roster_seed(),
        ),
    }
}

/// Resolve the governed roster for `identity` from durable identity-scoped
/// curated state, seeding it (and persisting the seed) from `seed` on first use.
///
/// Returns the validated `owner/name` slugs, in curation order. An empty or
/// all-invalid roster is an **error** (never a silent empty pass), consistent
/// with the fail-loud contract the callers rely on. This is the single source of
/// truth: the ecosystem-observe rail, the merge-queue reasoner, and the CI-health
/// sweep all resolve the roster through this one function + durable dataset.
pub fn resolve_governed_roster(
    store: &CuratedDataStore,
    identity: &str,
    seed: &CuratedList,
) -> SimardResult<Vec<String>> {
    let list = store.load_or_seed(identity, GOVERNED_ROSTER_DATASET, seed)?;
    let path = store
        .dataset_path(identity, GOVERNED_ROSTER_DATASET)
        .unwrap_or_else(|_| store.root().to_path_buf());
    validate_roster_slugs(&list.values()).map_err(|reason| SimardError::PersistentStoreIo {
        store: "identity-curated-state".to_string(),
        action: "validate governed roster".to_string(),
        path,
        reason,
    })
}

// ─────────────────────────── cadence gate ──────────────────────────────────

/// Decide whether an ecosystem-observe pass is due this tick.
///
/// Reuses the existing gap-scan cadence semantics: observation runs only when
/// `enabled` and then only once every `every_n` ticks. `every_n` is clamped to a
/// floor of `1` (every tick) so a `0` divisor can never disable observation by
/// stealth. `tick_index` is the monotonically increasing Overseer tick counter.
pub fn should_observe(enabled: bool, every_n: u64, tick_index: u64) -> bool {
    if !enabled {
        return false;
    }
    let every_n = every_n.max(1);
    tick_index.is_multiple_of(every_n)
}

// ─────────────────────────── rail (seam) ───────────────────────────────────

/// What the rail hands the `ecosystem-observe` recipe: the validated roster, the
/// set of Simard's in-flight OODA refs (for dedup), and the (rail-owned)
/// escalation note. Pure strings — no observation state. The production
/// [`EcosystemRecipeRunner`] writes the unbounded fields to per-invocation
/// context files and passes only the `<key>_path` tokens on `argv`.
#[derive(Clone, Debug)]
pub struct EcosystemObserveRequest {
    /// Validated `owner/name` slugs the OBSERVE agent scans with `gh`.
    pub roster: Vec<String>,
    /// Simard's in-flight OODA refs, so the agent dedups against work an engineer
    /// already owns and never duplicates Simard's own OODA.
    pub inflight_refs: Vec<String>,
    /// Empty on the base pass; carries a higher-effort / repair instruction on
    /// escalation-ladder retries. Rail-owned, never a caller parameter.
    pub escalation_note: String,
}

/// Seam: invoke the `ecosystem-observe` recipe and return its **opaque** semantic
/// result. Injectable so the rail is unit-testable with a fake — no subprocess,
/// no network, no `gh`. The production impl spawns the recipe runner; the runner
/// itself never inspects, parses, or counts the returned string.
pub trait EcosystemRecipeRunner: Send + Sync {
    /// Run one observation pass and return the recipe's final opaque output.
    fn run(&self, request: &EcosystemObserveRequest) -> SimardResult<String>;
}

/// The thin rail. Schedules `ecosystem-observe` on the Overseer cadence (via
/// [`should_observe`]) and forwards its opaque semantic result. Holds NO
/// observation state and never touches a repo — the `gh` scanning and the
/// reasoning both live inside the recipe's OBSERVE agent step.
pub trait EcosystemObserver {
    /// Run one observation pass.
    ///
    /// - `Ok(Some(brief))` — the recipe produced a semantic brief string to route
    ///   into the gated launch rail. `brief` is opaque prose; the caller forwards
    ///   it and never parses it.
    /// - `Ok(None)` — nothing actionable this pass (empty roster, blank result,
    ///   or a degraded recipe run).
    /// - `Err(_)` — reserved for a caller-visible fault; the default rail is
    ///   fail-closed and prefers `Ok(None)` over fabricating work.
    fn observe(&self, roster: &[String], inflight_refs: &[String]) -> SimardResult<Option<String>>;
}

/// Recipe-runner-backed [`EcosystemObserver`] over an injectable seam.
pub struct RecipeEcosystemObserver<R: EcosystemRecipeRunner> {
    runner: R,
}

impl<R: EcosystemRecipeRunner> RecipeEcosystemObserver<R> {
    /// Build the rail over a concrete [`EcosystemRecipeRunner`].
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    /// Borrow the underlying runner (used by tests to inspect the seam).
    pub fn runner(&self) -> &R {
        &self.runner
    }
}

impl<R: EcosystemRecipeRunner> EcosystemObserver for RecipeEcosystemObserver<R> {
    fn observe(&self, roster: &[String], inflight_refs: &[String]) -> SimardResult<Option<String>> {
        // Fail-closed: an empty roster is nothing to observe. Never invoke the
        // recipe and never fabricate a Problem from an empty scan.
        if roster.is_empty() {
            tracing::warn!(
                target: "overseer::ecosystem_observe",
                "ecosystem-observe: empty roster; skipping pass (no problems fabricated)"
            );
            return Ok(None);
        }

        let request = EcosystemObserveRequest {
            roster: roster.to_vec(),
            inflight_refs: inflight_refs.to_vec(),
            escalation_note: String::new(),
        };

        match self.runner.run(&request) {
            Ok(output) => {
                // A blank recipe result is "nothing actionable", not a Problem.
                // The non-empty result is forwarded VERBATIM and never parsed.
                if output.trim().is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(output))
                }
            }
            Err(e) => {
                // Fail-closed: a recipe/infra fault degrades to "no observation",
                // logged. It never aborts the tick and never fabricates a brief.
                tracing::warn!(
                    target: "overseer::ecosystem_observe",
                    error = %e,
                    "ecosystem-observe: recipe run failed; degrading to no observation (no problems fabricated)"
                );
                Ok(None)
            }
        }
    }
}

// ─────────────────── production recipe-runner (thin) ───────────────────────

/// Adapter tag for error/telemetry attribution on the ecosystem-observe rail.
const OBSERVE_ADAPTER_TAG: &str = "ecosystem-observe";
/// The recipe this runner invokes (resolved hot-reload-first, then in-tree).
const OBSERVE_RECIPE_FILENAME: &str = "ecosystem-observe.yaml";

/// Resolve the `ecosystem-observe.yaml` recipe path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// Mirrors `recipe_merge_judge::resolve_recipe_path`. `home_override` keeps
/// tests hermetic against the ambient `~/.simard`; production passes `None`.
fn resolve_observe_recipe_path(
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

/// Production [`EcosystemRecipeRunner`]: spawns `recipe-runner-rs` on the
/// `ecosystem-observe` recipe and returns its OPAQUE final-step output.
///
/// Thin by construction. It writes the roster / in-flight refs / a writable
/// handoff placeholder to per-invocation context files (so unbounded lists ride
/// the `<key>_path` file channel, never `argv`), passes only the `_path` tokens
/// plus the empty `escalation_note`, runs the recipe in `--output-format json`,
/// and hands back the envelope's final step output via
/// [`extract_recipe_decision_output`](crate::ooda_brain::extract_recipe_decision_output).
/// It NEVER inspects, parses, or counts that string — the observation lives in
/// the agent's reasoning and is forwarded verbatim.
pub struct SpawnEcosystemRecipeRunner {
    recipe_path: std::path::PathBuf,
    agent_binary: &'static str,
}

impl SpawnEcosystemRecipeRunner {
    /// Construct if the recipe file and `recipe-runner-rs` are both available;
    /// otherwise `None` (the rail is left unwired and the pass is skipped).
    pub fn new(repo_root: &Path) -> Option<Self> {
        Self::new_with_home(repo_root, None)
    }

    fn new_with_home(repo_root: &Path, home_override: Option<&Path>) -> Option<Self> {
        let recipe_path = resolve_observe_recipe_path(repo_root, home_override)?;
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

impl EcosystemRecipeRunner for SpawnEcosystemRecipeRunner {
    fn run(&self, request: &EcosystemObserveRequest) -> SimardResult<String> {
        use crate::recipe_context_file::ContextFile;

        // Roster + in-flight refs ride the file channel as newline-joined lists;
        // the OBSERVE agent reads them with its file tool. A blank handoff file
        // is created so `observed_problems_path` names a real, writable path the
        // OBSERVE step writes and the BRIEF step reads. All guards live until
        // after `output()` so the files exist while the recipe runs.
        let roster_cf =
            ContextFile::write(OBSERVE_ADAPTER_TAG, "roster", &request.roster.join("\n")).map_err(
                |e| SimardError::AdapterInvocationFailed {
                    base_type: OBSERVE_ADAPTER_TAG.to_string(),
                    reason: format!("roster context-file write failed: {e}"),
                },
            )?;
        let inflight_cf = ContextFile::write(
            OBSERVE_ADAPTER_TAG,
            "inflight_refs",
            &request.inflight_refs.join("\n"),
        )
        .map_err(|e| SimardError::AdapterInvocationFailed {
            base_type: OBSERVE_ADAPTER_TAG.to_string(),
            reason: format!("inflight_refs context-file write failed: {e}"),
        })?;
        let problems_cf = ContextFile::write(OBSERVE_ADAPTER_TAG, "observed_problems", "")
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: OBSERVE_ADAPTER_TAG.to_string(),
                reason: format!("observed_problems context-file write failed: {e}"),
            })?;

        let output = std::process::Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(roster_cf.arg_value())
            .arg("-c")
            .arg(inflight_cf.arg_value())
            .arg("-c")
            .arg(problems_cf.arg_value())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── fake recipe-runner seam ──────────────────────────────────────────
    // Records every request and returns a scripted outcome, so the rail is
    // exercised with NO subprocess, NO network, and NO `gh`.
    enum Scripted {
        Ok(String),
        Err(String),
    }

    struct FakeRunner {
        scripted: Scripted,
        calls: Mutex<Vec<EcosystemObserveRequest>>,
    }

    impl FakeRunner {
        fn ok(output: &str) -> Self {
            Self {
                scripted: Scripted::Ok(output.to_string()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn err(reason: &str) -> Self {
            Self {
                scripted: Scripted::Err(reason.to_string()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl EcosystemRecipeRunner for FakeRunner {
        fn run(&self, request: &EcosystemObserveRequest) -> SimardResult<String> {
            self.calls.lock().unwrap().push(request.clone());
            match &self.scripted {
                Scripted::Ok(out) => Ok(out.clone()),
                Scripted::Err(reason) => Err(SimardError::AdapterInvocationFailed {
                    base_type: "ecosystem-observe".to_string(),
                    reason: reason.clone(),
                }),
            }
        }
    }

    // ── cadence gate ─────────────────────────────────────────────────────

    #[test]
    fn cadence_disabled_never_observes() {
        for tick in 0..5 {
            assert!(!should_observe(false, 1, tick));
        }
    }

    #[test]
    fn cadence_every_tick_when_n_is_one() {
        for tick in 0..5 {
            assert!(should_observe(true, 1, tick));
        }
    }

    #[test]
    fn cadence_every_n_ticks() {
        // n = 3 → observe on ticks 0, 3, 6; skip the ticks between.
        assert!(should_observe(true, 3, 0));
        assert!(!should_observe(true, 3, 1));
        assert!(!should_observe(true, 3, 2));
        assert!(should_observe(true, 3, 3));
        assert!(!should_observe(true, 3, 4));
        assert!(should_observe(true, 3, 6));
    }

    #[test]
    fn cadence_zero_divisor_clamps_to_every_tick() {
        // A 0 divisor must never disable observation by stealth.
        for tick in 0..4 {
            assert!(should_observe(true, 0, tick));
        }
    }

    // ── rail: routing + fail-closed ──────────────────────────────────────

    #[test]
    fn observe_routes_recipe_brief_forward() {
        let brief = "PROBLEM: azlin CI red on main -> brief: fix flaky provisioning test";
        let obs = RecipeEcosystemObserver::new(FakeRunner::ok(brief));
        let out = obs
            .observe(&["rysweet/azlin".to_string()], &[])
            .expect("observe must not error on a successful recipe run");
        assert_eq!(
            out.as_deref(),
            Some(brief),
            "the recipe's semantic result must be forwarded verbatim"
        );
        assert_eq!(obs.runner().call_count(), 1, "the recipe is invoked once");
    }

    #[test]
    fn observe_empty_roster_fails_closed_without_running_recipe() {
        let obs = RecipeEcosystemObserver::new(FakeRunner::ok("should never be used"));
        let out = obs.observe(&[], &[]).expect("empty roster is not an error");
        assert_eq!(out, None, "an empty roster fabricates no problems");
        assert_eq!(
            obs.runner().call_count(),
            0,
            "an empty roster must not invoke the recipe"
        );
    }

    #[test]
    fn observe_runner_error_degrades_to_none() {
        let obs = RecipeEcosystemObserver::new(FakeRunner::err("recipe-runner-rs spawn failed"));
        let out = obs
            .observe(&["rysweet/Simard".to_string()], &[])
            .expect("a recipe failure must degrade safely, not error out");
        assert_eq!(out, None, "a recipe failure must fabricate no problems");
        assert_eq!(
            obs.runner().call_count(),
            1,
            "the recipe was attempted before degrading"
        );
    }

    #[test]
    fn observe_blank_recipe_output_is_not_actionable() {
        let obs = RecipeEcosystemObserver::new(FakeRunner::ok("   \n  \t "));
        let out = obs
            .observe(&["rysweet/Simard".to_string()], &[])
            .expect("blank output is not an error");
        assert_eq!(out, None, "a blank recipe result is nothing actionable");
    }

    #[test]
    fn observe_hands_roster_and_refs_to_recipe() {
        let obs = RecipeEcosystemObserver::new(FakeRunner::ok("ok"));
        let roster = vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()];
        let refs = vec!["issue:rysweet/Simard#42".to_string()];
        obs.observe(&roster, &refs).unwrap();

        let calls = obs.runner().calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].roster, roster,
            "the roster is handed to the recipe"
        );
        assert_eq!(
            calls[0].inflight_refs, refs,
            "in-flight refs are handed to the recipe for dedup"
        );
        assert!(
            calls[0].escalation_note.is_empty(),
            "the base pass carries no escalation note"
        );
    }

    // ── roster resolution (identity-scoped curated state) ─────────────────

    fn temp_store() -> (tempfile::TempDir, CuratedDataStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CuratedDataStore::with_root(dir.path().join("identity_state"));
        (dir, store)
    }

    #[test]
    fn default_seed_lists_the_ten_stewarded_repos() {
        let seed = default_simard_roster_seed();
        let values = seed.values();
        assert_eq!(values.len(), 10, "Simard's default roster has 10 repos");
        assert!(values.contains(&"rysweet/Simard".to_string()));
        assert!(values.contains(&"rysweet/amplihack-rs".to_string()));
        assert!(values.contains(&"rysweet/gadugi-agentic-test".to_string()));
        assert!(
            !values.iter().any(|s| s == "rysweet/amplihack"),
            "the deprecated python amplihack is not on the roster"
        );
    }

    #[test]
    fn resolve_seeds_then_returns_validated_slugs() {
        let (_dir, store) = temp_store();
        let seed = default_simard_roster_seed();
        let roster = resolve_governed_roster(&store, DEFAULT_ROSTER_IDENTITY, &seed)
            .expect("first resolve seeds and returns the roster");
        assert_eq!(roster.len(), 10);
        // The dataset is now durable on disk (deploy-durable identity state).
        assert!(
            store
                .dataset_path(DEFAULT_ROSTER_IDENTITY, GOVERNED_ROSTER_DATASET)
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn resolve_returns_curated_edits_not_the_seed() {
        // The whole point of moving the roster into durable identity state: an
        // agentic curation (add/remove a stewarded repo) survives and a later
        // resolve (as on a redeploy) returns the CURATED copy, never the seed.
        let (_dir, store) = temp_store();
        let seed = default_simard_roster_seed();
        resolve_governed_roster(&store, DEFAULT_ROSTER_IDENTITY, &seed).unwrap();

        let mut curated = store
            .load(DEFAULT_ROSTER_IDENTITY, GOVERNED_ROSTER_DATASET)
            .unwrap()
            .unwrap();
        assert!(curated.add("rysweet/new-repo", "freshly stewarded"));
        assert!(curated.remove("rysweet/azlin"));
        store
            .save(DEFAULT_ROSTER_IDENTITY, GOVERNED_ROSTER_DATASET, &curated)
            .unwrap();

        let roster = resolve_governed_roster(&store, DEFAULT_ROSTER_IDENTITY, &seed).unwrap();
        assert!(roster.contains(&"rysweet/new-repo".to_string()));
        assert!(
            !roster.contains(&"rysweet/azlin".to_string()),
            "a removed repo stays removed across resolves (no re-seed clobber)"
        );
    }

    #[test]
    fn resolve_skips_malformed_slugs_but_keeps_valid() {
        let (_dir, store) = temp_store();
        let seed = CuratedList::from_items([
            CuratedItem::new("rysweet/Simard", ""),
            CuratedItem::new("not-a-slug", ""),
            CuratedItem::new("rysweet//azlin", ""),
            CuratedItem::new("rysweet/azlin", ""),
        ]);
        let roster = resolve_governed_roster(&store, DEFAULT_ROSTER_IDENTITY, &seed).unwrap();
        assert_eq!(
            roster,
            vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
            "malformed slugs are skipped; valid ones are kept in order"
        );
    }

    #[test]
    fn resolve_all_invalid_is_error_not_empty_pass() {
        let (_dir, store) = temp_store();
        let seed = CuratedList::from_items([CuratedItem::new("bad", "")]);
        assert!(
            resolve_governed_roster(&store, DEFAULT_ROSTER_IDENTITY, &seed).is_err(),
            "a roster with no valid slugs is an error, never a silent empty pass"
        );
    }

    #[test]
    fn validate_roster_slugs_empty_is_error() {
        assert!(validate_roster_slugs(&[]).is_err());
        assert!(validate_roster_slugs(&["bad".to_string()]).is_err());
    }

    #[test]
    fn seed_for_defaults_to_simard_without_identity_targets() {
        let (identity, seed) = governed_roster_seed_for(None, &[]);
        assert_eq!(identity, DEFAULT_ROSTER_IDENTITY);
        assert_eq!(seed.values().len(), 10);
        // A named identity with no target repos still falls back to Simard.
        let (identity, _) = governed_roster_seed_for(Some("gastronome"), &[]);
        assert_eq!(identity, DEFAULT_ROSTER_IDENTITY);
    }

    #[test]
    fn seed_for_named_identity_uses_its_target_repos() {
        // The generic mechanism: a non-Simard identity owns its OWN roster,
        // seeded from its declared target scope (identity.toml target_repos).
        let (identity, seed) =
            governed_roster_seed_for(Some("crocutus"), &["rysweet/hyenas".to_string()]);
        assert_eq!(identity, "crocutus");
        assert_eq!(seed.values(), vec!["rysweet/hyenas".to_string()]);
    }

    // ── slug validation ──────────────────────────────────────────────────

    #[test]
    fn slug_validation_accepts_clean_slugs() {
        assert!(is_valid_slug("rysweet/Simard"));
        assert!(is_valid_slug("rysweet/amplihack-rs"));
        assert!(is_valid_slug("rysweet/amplihack_memory_lib"));
        assert!(is_valid_slug("owner/name.with.dots"));
    }

    #[test]
    fn slug_validation_rejects_malformed() {
        assert!(!is_valid_slug("noslash"));
        assert!(!is_valid_slug("owner/name/extra"));
        assert!(!is_valid_slug("owner/"));
        assert!(!is_valid_slug("/name"));
        assert!(!is_valid_slug("owner /name")); // whitespace in owner
        assert!(!is_valid_slug("owner/na me")); // whitespace in name
        assert!(!is_valid_slug("owner/..")); // path traversal
        assert!(!is_valid_slug("-owner/name")); // leading dash
        assert!(!is_valid_slug("owner/name;rm -rf /")); // shell metachars
        assert!(!is_valid_slug("owner/name`whoami`"));
        assert!(!is_valid_slug("owner/name$(id)"));
    }
}
