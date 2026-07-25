//! Thin Rust rail for the agentic **ecosystem-observe** chain (issue #2419).
//!
//! Simard observes her stewarded ecosystem with a DETERMINISTIC WORKFLOW OF
//! AGENTIC STEPS + PROMPTS — not a Rust "code sensor." This module is the only
//! new Rust the feature needs, and it is deliberately thin:
//!
//! 1. [`load_stewarded_roster`] reads the identity-CURATED roster of stewarded
//!    repos (seeded from committed identity data on first use, then owned as
//!    durable mutable state) as pure DATA — a list of `owner/name` slugs,
//!    validating each slug before use.
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

// ─────────────────────────── roster (data) ─────────────────────────────────
//
// The stewarded roster is NOT a framework file bound to code any more. It is
// identity-scoped mutable state (`crate::identity_curated_state`, collection
// `stewarded_repos`) that Simard OWNS and curates agentically. On first use the
// generic mechanism SEEDS the roster from committed IDENTITY DATA
// (`prompt_assets/simard/identity/stewarded_repos.seed.toml`) into the durable
// state root — which `install` never overwrites — and from then on that curated
// copy is the single source of truth. This module only *reads* it and validates
// the `owner/name` slugs before they reach the agent's `gh` calls.

/// The identity-scoped collection name that holds the stewarded roster. A
/// compile-time constant — never derived from env, argv, or file contents.
pub const STEWARDED_REPOS_COLLECTION: &str = "stewarded_repos";

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

/// Load the stewarded roster as validated `owner/name` slugs from the
/// **identity-curated** collection, seeding it on first use from committed
/// identity data.
///
/// This is the single source of truth for the roster. It routes through
/// [`crate::identity_curated_state::load_or_seed`]:
///   - if the durable `<state_root>/identity-state/<identity>/stewarded_repos.toml`
///     exists, its curated contents win (Simard's agentic edits, survived across
///     re-installs);
///   - otherwise the collection is seeded from
///     `prompt_assets/simard/identity/stewarded_repos.seed.toml` (resolved
///     install-first, then in-tree) and persisted, then returned.
///
/// Each item `key` is validated as an `owner/name` slug with [`is_valid_slug`];
/// a malformed slug is **skipped with a logged warning** — it never reaches the
/// agent's `gh` calls. An empty roster (no valid slugs) is an **error**, never a
/// silent empty pass: the caller skips the observation tick and fabricates no
/// Problems.
///
/// `state_root_override` / `home_override` are test seams; production passes
/// `None`.
pub fn load_stewarded_roster(
    repo_root: &Path,
    identity: &str,
    state_root_override: Option<&Path>,
    home_override: Option<&Path>,
) -> SimardResult<Vec<String>> {
    let repo_root = repo_root.to_path_buf();
    let collection = crate::identity_curated_state::load_or_seed(
        STEWARDED_REPOS_COLLECTION,
        identity,
        || load_roster_seed(&repo_root, home_override),
        state_root_override,
    )?;
    slugs_from_collection(&collection).map_err(|reason| SimardError::PersistentStoreIo {
        store: "stewarded_roster".into(),
        action: "validate".into(),
        path: repo_root,
        reason,
    })
}

/// Convenience wrapper for callers without an explicit `repo_root` / identity /
/// state root (e.g. the CI-health sweep CLI). Resolves the active identity via
/// [`crate::identity_curated_state::active_identity`], the seed's in-tree
/// fallback from `CARGO_MANIFEST_DIR`, and the state root from the environment.
pub fn load_stewarded_roster_from_env() -> SimardResult<Vec<String>> {
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let identity = crate::identity_curated_state::active_identity();
    load_stewarded_roster(&repo_root, &identity, None, None)
}

/// Validate a curated collection's item keys into an ordered list of `owner/name`
/// slugs. Malformed slugs are skipped with a logged warning; an empty result is
/// an `Err` reason (never a silent empty pass), so a caller fails loud instead of
/// concluding an empty fleet is healthy.
fn slugs_from_collection(
    collection: &crate::identity_curated_state::CuratedCollection,
) -> Result<Vec<String>, String> {
    let mut roster = Vec::with_capacity(collection.items.len());
    for item in &collection.items {
        let slug = item.key.trim();
        if is_valid_slug(slug) {
            roster.push(slug.to_string());
        } else {
            tracing::warn!(
                target: "overseer::ecosystem_observe",
                slug = %item.key,
                "stewarded roster: skipping malformed slug (not a clean owner/name)"
            );
        }
    }
    if roster.is_empty() {
        return Err("stewarded roster has no valid owner/name slugs".to_string());
    }
    Ok(roster)
}

/// Load the committed roster SEED (identity data) as a curated collection,
/// resolving the seed file install-first then in-tree. Used only on first use,
/// when the durable curated roster does not exist yet.
fn load_roster_seed(
    repo_root: &Path,
    home_override: Option<&Path>,
) -> SimardResult<crate::identity_curated_state::CuratedCollection> {
    let seed_path = resolve_roster_seed_path(repo_root, home_override).ok_or_else(|| {
        let in_tree = repo_root
            .join(ROSTER_SEED_RELDIR)
            .join(ROSTER_SEED_FILENAME);
        SimardError::PromptAssetMissing {
            asset_id: STEWARDED_REPOS_COLLECTION.to_string(),
            path: in_tree,
        }
    })?;
    let raw = std::fs::read_to_string(&seed_path).map_err(|e| SimardError::PromptAssetRead {
        path: seed_path.clone(),
        reason: format!("read stewarded-roster seed failed: {e}"),
    })?;
    toml::from_str(&raw).map_err(|e| SimardError::PromptAssetRead {
        path: seed_path,
        reason: format!("parse stewarded-roster seed failed: {e}"),
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

/// The stewarded-roster SEED filename and its relative directory under
/// `prompt_assets/simard`. Hardcoded consts — never derived from env/args/file
/// contents (path-traversal prevention). The seed is IDENTITY DATA consumed only
/// on first use; the durable roster lives under the state root.
pub(crate) const ROSTER_SEED_FILENAME: &str = "stewarded_repos.seed.toml";
pub(crate) const ROSTER_SEED_RELDIR: &str = "prompt_assets/simard/identity";

/// Resolve the stewarded-roster SEED path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/identity/<name>` (installed path)
///   2. `<repo_root>/prompt_assets/simard/identity/<name>` (in-tree)
///
/// Mirrors [`resolve_observe_recipe_path`]'s install-first ladder so a deployed
/// daemon (whose `repo_root` is a stale source checkout) still finds the seed
/// installed under `~/.simard`. `home_override` keeps tests hermetic against the
/// ambient `~/.simard`; production passes `None`.
pub(crate) fn resolve_roster_seed_path(
    repo_root: &Path,
    home_override: Option<&Path>,
) -> Option<std::path::PathBuf> {
    let home = home_override
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir);
    if let Some(home) = home {
        let installed = home
            .join(".simard")
            .join(ROSTER_SEED_RELDIR)
            .join(ROSTER_SEED_FILENAME);
        if installed.is_file() {
            return Some(installed);
        }
    }
    let in_tree = repo_root
        .join(ROSTER_SEED_RELDIR)
        .join(ROSTER_SEED_FILENAME);
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

    // ── roster loader (identity-curated) ─────────────────────────────────

    #[test]
    fn seed_slugs_are_validated_from_committed_seed_file() {
        // The committed identity-data SEED lists the stewarded repos as generic
        // curated items; validating its keys yields the same 10-repo roster.
        let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let seed = load_roster_seed(&repo_root, None).expect("committed seed must load");
        let roster = slugs_from_collection(&seed).expect("seed slugs validate");
        assert_eq!(roster.len(), 10, "the seed lists the 10 stewarded repos");
        assert!(roster.contains(&"rysweet/Simard".to_string()));
        assert!(roster.contains(&"rysweet/amplihack-rs".to_string()));
        assert!(roster.contains(&"rysweet/gadugi-agentic-test".to_string()));
        assert!(
            !roster.iter().any(|s| s == "rysweet/amplihack"),
            "the deprecated python amplihack is not on the roster"
        );
    }

    /// A curated collection from `(key, note)` pairs.
    fn collection(items: &[(&str, &str)]) -> crate::identity_curated_state::CuratedCollection {
        crate::identity_curated_state::CuratedCollection::from_items(
            items
                .iter()
                .map(|(k, n)| crate::identity_curated_state::CuratedItem::new(*k, *n))
                .collect(),
        )
    }

    #[test]
    fn slugs_skips_malformed_but_keeps_valid_in_order() {
        let c = collection(&[
            ("rysweet/Simard", ""),
            ("not-a-slug", ""),
            ("rysweet//azlin", ""),
            ("rysweet/azlin", ""),
        ]);
        let roster = slugs_from_collection(&c).unwrap();
        assert_eq!(
            roster,
            vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
            "malformed slugs are skipped; valid ones are kept in order"
        );
    }

    #[test]
    fn slugs_all_invalid_is_error_not_empty_pass() {
        let c = collection(&[("bad", "")]);
        assert!(
            slugs_from_collection(&c).is_err(),
            "a roster with no valid slugs is an error, never a silent empty pass"
        );
    }

    #[test]
    fn slugs_no_items_is_error() {
        let c = crate::identity_curated_state::CuratedCollection::default();
        assert!(slugs_from_collection(&c).is_err());
    }

    #[test]
    fn load_stewarded_roster_seeds_from_committed_seed_then_owns_it() {
        // Fresh state root + real repo_root (so the seed resolves in-tree). First
        // load seeds from the committed seed; a curation edit then persists to the
        // durable state; the next load returns the CURATED copy, not the seed.
        let state = tempfile::tempdir().unwrap();
        let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let first = load_stewarded_roster(&repo_root, "simard", Some(state.path()), None).unwrap();
        assert!(first.contains(&"rysweet/Simard".to_string()));
        assert_eq!(first.len(), 10);

        // Curate: add a repo to the OWNED mutable state, then drop one.
        crate::identity_curated_state::add_item(
            STEWARDED_REPOS_COLLECTION,
            "simard",
            crate::identity_curated_state::CuratedItem::new("rysweet/skwaq", "vuln research"),
            Some(state.path()),
        )
        .unwrap();
        crate::identity_curated_state::remove_item(
            STEWARDED_REPOS_COLLECTION,
            "simard",
            "rysweet/azlin",
            Some(state.path()),
        )
        .unwrap();

        let second = load_stewarded_roster(&repo_root, "simard", Some(state.path()), None).unwrap();
        assert!(
            second.contains(&"rysweet/skwaq".to_string()),
            "the agentic addition is present (curated copy wins, not re-seeded)"
        );
        assert!(
            !second.contains(&"rysweet/azlin".to_string()),
            "the agentic removal persisted across reload"
        );
    }

    #[test]
    fn load_stewarded_roster_errors_when_no_seed_and_no_state() {
        // No durable state AND no seed anywhere → a loud error, never an empty pass.
        let state = tempfile::tempdir().unwrap();
        let empty_repo = tempfile::tempdir().unwrap();
        let empty_home = tempfile::tempdir().unwrap();
        assert!(
            load_stewarded_roster(
                empty_repo.path(),
                "simard",
                Some(state.path()),
                Some(empty_home.path()),
            )
            .is_err(),
            "missing seed + empty state fails loud (no silent empty roster)"
        );
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

    // ── roster-seed path resolution (install-first) ──────────────────────
    //
    // These specify `resolve_roster_seed_path`, the install-first resolver for
    // the identity-data SEED. On a deployed daemon (WorkingDirectory ~/.simard,
    // a stale source `repo_root`) the seed must resolve from the installed
    // `~/.simard/prompt_assets/simard/identity/stewarded_repos.seed.toml` rather
    // than the (possibly stale) `repo_root`. All tests are hermetic: they use
    // `tempfile::tempdir` + an explicit `home_override` and NEVER touch the
    // ambient `~/.simard`.

    /// Write the SEED under `<base>/prompt_assets/simard/identity/…` and return
    /// the file path. `base` is a home's `.simard` dir or a repo_root.
    fn write_seed_under(base: &Path) -> std::path::PathBuf {
        let dir = base.join(ROSTER_SEED_RELDIR);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(ROSTER_SEED_FILENAME);
        std::fs::write(
            &path,
            "schema_version = 1\n[[item]]\nkey = \"rysweet/Simard\"\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn seed_filename_and_dir_are_fixed_consts() {
        // Security invariant: the names are hardcoded consts, never derived from
        // env/args/file contents (path-traversal prevention).
        assert_eq!(ROSTER_SEED_FILENAME, "stewarded_repos.seed.toml");
        assert_eq!(ROSTER_SEED_RELDIR, "prompt_assets/simard/identity");
    }

    #[test]
    fn seed_path_prefers_installed_home() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let installed = write_seed_under(&home.path().join(".simard"));
        let _in_tree = write_seed_under(repo.path());

        let resolved = resolve_roster_seed_path(repo.path(), Some(home.path()))
            .expect("resolver must find the installed seed");
        assert_eq!(
            resolved, installed,
            "install-first: the ~/.simard seed wins over the in-tree copy"
        );
    }

    #[test]
    fn seed_path_falls_back_to_repo_root() {
        let home = tempfile::tempdir().unwrap(); // empty .simard — no seed
        let repo = tempfile::tempdir().unwrap();
        let in_tree = write_seed_under(repo.path());

        let resolved = resolve_roster_seed_path(repo.path(), Some(home.path()))
            .expect("resolver must fall back to the in-tree seed");
        assert_eq!(
            resolved, in_tree,
            "fallback: the repo_root seed is used when no install copy exists"
        );
    }

    #[test]
    fn seed_path_none_when_absent() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();

        assert!(
            resolve_roster_seed_path(repo.path(), Some(home.path())).is_none(),
            "no seed anywhere → None, never a panic"
        );
    }

    #[test]
    fn seed_path_resolves_from_install_when_repo_root_lacks_seed() {
        // A deployed daemon's `repo_root` is a STALE source checkout WITHOUT the
        // seed, while the seed is installed under ~/.simard. The resolver MUST
        // resolve from the installed location.
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap(); // repo_root deliberately EMPTY
        let installed = write_seed_under(&home.path().join(".simard"));

        assert!(
            !repo
                .path()
                .join(ROSTER_SEED_RELDIR)
                .join(ROSTER_SEED_FILENAME)
                .exists(),
            "precondition: repo_root has no seed (as on the stale deploy dir)"
        );

        let resolved = resolve_roster_seed_path(repo.path(), Some(home.path()))
            .expect("must resolve the installed seed even when repo_root lacks it");
        assert_eq!(resolved, installed, "install-first resolution");
    }
}
