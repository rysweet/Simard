//! Thin Rust rail for the agentic **ecosystem-observe** chain (issue #2419).
//!
//! Simard observes her stewarded ecosystem with a DETERMINISTIC WORKFLOW OF
//! AGENTIC STEPS + PROMPTS — not a Rust "code sensor." This module is the only
//! new Rust the feature needs, and it is deliberately thin:
//!
//! 1. [`load_governed_roster`] resolves the stewarded roster from durable,
//!    install-safe, identity-scoped mutable state — seeded once from Simard's
//!    identity default ([`SIMARD_GOVERNED_REPOS_SEED`]) — as pure DATA (a list of
//!    `owner/name` slugs), validating each slug before use.
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

/// One roster entry as parsed from the identity-scoped governed-roster TOML. Pure
/// data — a slug plus a human-readable note. The note is ignored by the loader;
/// only the slug strings reach the agent.
#[derive(Debug, serde::Deserialize)]
struct RosterEntry {
    slug: String,
    #[serde(default)]
    #[allow(dead_code)]
    note: String,
}

/// The roster file shape: `schema_version` plus a `[[repo]]` array.
#[derive(Debug, serde::Deserialize)]
struct RosterFile {
    #[serde(default)]
    #[allow(dead_code)]
    schema_version: u32,
    #[serde(default)]
    repo: Vec<RosterEntry>,
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

/// The identity that owns Simard's stewarded roster in [`crate::identity_state`].
/// The roster is part of *who Simard is* (the default identity), so it is scoped
/// to this identity name — not a framework-global concept.
pub const SIMARD_ROSTER_IDENTITY: &str = "simard";

/// The identity-scoped curated-data collection holding the governed repo roster.
/// A compile-time constant — never derived from env/args/file contents.
pub const GOVERNED_REPOS_COLLECTION: &str = "governed_repos";

/// Simard's DEFAULT governed-repo roster, embedded at build time as her identity
/// seed. It is NOT a deployed prompt-asset, so `install` never re-installs or
/// clobbers the live curated roster (which lives in durable identity-scoped
/// state). Used only to seed the collection on first use; thereafter the on-disk
/// curated copy is the single source of truth.
pub const SIMARD_GOVERNED_REPOS_SEED: &str =
    include_str!("../identity/seeds/simard_governed_repos.toml");

/// Resolve Simard's governed-repo roster from durable, install-safe,
/// identity-scoped mutable state, **seeding it from her identity default**
/// ([`SIMARD_GOVERNED_REPOS_SEED`]) on first use.
///
/// This is the SINGLE SOURCE OF TRUTH for the stewarded roster: the
/// ecosystem-observe rail, the observe-merge-queue reasoner, and the CI-health
/// sweep all resolve the fleet through here, so there is no second list to drift.
/// Because the roster is now mutable identity-scoped state under the state root,
/// Simard can curate it agentically (add/remove a stewarded repo) and a
/// self-deploy never overwrites her curation.
///
/// Returns the validated `owner/name` slugs in file order (see
/// [`parse_ecosystem_roster`]); an empty or all-malformed roster is an **error**,
/// never a silent empty pass.
pub fn load_governed_roster(state_root: &Path) -> SimardResult<Vec<String>> {
    let store = crate::identity_state::IdentityStateStore::new(state_root);
    let raw = store.load_or_seed_raw(
        SIMARD_ROSTER_IDENTITY,
        GOVERNED_REPOS_COLLECTION,
        SIMARD_GOVERNED_REPOS_SEED,
    )?;
    parse_ecosystem_roster(&raw).map_err(|reason| SimardError::PromptAssetRead {
        path: store
            .collection_path(SIMARD_ROSTER_IDENTITY, GOVERNED_REPOS_COLLECTION)
            .unwrap_or_else(|_| std::path::PathBuf::from(GOVERNED_REPOS_COLLECTION)),
        reason,
    })
}

/// Parse & validate a roster from its TOML **text** — the path-agnostic core
/// shared by every consumer of the stewarded roster. [`load_governed_roster`]
/// resolves the roster TOML from identity-scoped state (seeded from Simard's
/// identity default) and the CI-health sweep resolves the same roster through
/// that single loader, so the fleet has exactly one source of truth. Returns the
/// validated `owner/name` slugs in file order.
///
/// Each slug is checked with [`is_valid_slug`]; a malformed slug is skipped with
/// a logged warning. An empty roster (no valid slugs — the file was empty or
/// every slug was malformed) is an **error** (the `Err` reason), never a silent
/// empty pass, so a caller can fail loud instead of concluding an empty fleet is
/// healthy. The error carries only a human-readable reason; the caller wraps it
/// in whatever error type fits its context (a state path, an embed marker).
pub fn parse_ecosystem_roster(raw: &str) -> Result<Vec<String>, String> {
    let parsed: RosterFile =
        toml::from_str(raw).map_err(|e| format!("parse ecosystem roster failed: {e}"))?;

    let mut roster = Vec::with_capacity(parsed.repo.len());
    for entry in parsed.repo {
        let slug = entry.slug.trim();
        if is_valid_slug(slug) {
            roster.push(slug.to_string());
        } else {
            tracing::warn!(
                target: "overseer::ecosystem_observe",
                slug = %entry.slug,
                "ecosystem roster: skipping malformed slug (not a clean owner/name)"
            );
        }
    }

    if roster.is_empty() {
        return Err("ecosystem roster has no valid owner/name slugs".to_string());
    }
    Ok(roster)
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

    // ── governed roster (identity-scoped state) ──────────────────────────

    #[test]
    fn governed_roster_seeds_from_identity_default() {
        // A fresh state root has no curated roster yet; load seeds it from
        // Simard's identity default and returns the 10 stewarded slugs.
        let dir = tempfile::tempdir().unwrap();
        let roster = load_governed_roster(dir.path()).expect("seeded roster must load");
        assert_eq!(
            roster.len(),
            10,
            "the identity seed lists 10 stewarded repos"
        );
        assert!(roster.contains(&"rysweet/Simard".to_string()));
        assert!(roster.contains(&"rysweet/amplihack-rs".to_string()));
        assert!(roster.contains(&"rysweet/gadugi-agentic-test".to_string()));
        assert!(
            !roster.iter().any(|s| s == "rysweet/amplihack"),
            "the deprecated python amplihack is not on the roster"
        );
    }

    #[test]
    fn governed_roster_seed_is_persisted_under_state_root() {
        // Deploy-durability: the seed is written into identity-scoped state under
        // the state root (which `install` never overwrites), NOT a prompt-asset.
        let dir = tempfile::tempdir().unwrap();
        load_governed_roster(dir.path()).unwrap();
        let path = dir.path().join("identity_state/simard/governed_repos.toml");
        assert!(
            path.is_file(),
            "roster must be seeded to {}",
            path.display()
        );
    }

    #[test]
    fn governed_roster_returns_agentic_curation_not_seed() {
        // Once Simard curates the roster (add/remove a repo), a later load returns
        // the CURATED copy — a self-deploy could not clobber it, and the seed is
        // never consulted again.
        let dir = tempfile::tempdir().unwrap();
        load_governed_roster(dir.path()).unwrap(); // seed first
        let store = crate::identity_state::IdentityStateStore::new(dir.path());
        store
            .save_raw(
                SIMARD_ROSTER_IDENTITY,
                GOVERNED_REPOS_COLLECTION,
                "schema_version = 1\n[[repo]]\nslug = \"rysweet/azlin\"\n",
            )
            .unwrap();
        let roster = load_governed_roster(dir.path()).unwrap();
        assert_eq!(roster, vec!["rysweet/azlin".to_string()]);
    }

    #[test]
    fn governed_roster_all_invalid_is_error_not_empty_pass() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::identity_state::IdentityStateStore::new(dir.path());
        store
            .save_raw(
                SIMARD_ROSTER_IDENTITY,
                GOVERNED_REPOS_COLLECTION,
                "schema_version = 1\n[[repo]]\nslug = \"bad\"\n",
            )
            .unwrap();
        assert!(
            load_governed_roster(dir.path()).is_err(),
            "a curated roster with no valid slugs is an error, never a silent empty pass"
        );
    }

    #[test]
    fn embedded_identity_seed_parses_to_the_full_fleet() {
        // The baked-in identity seed is itself a valid, non-empty roster.
        let roster =
            parse_ecosystem_roster(SIMARD_GOVERNED_REPOS_SEED).expect("identity seed must parse");
        assert_eq!(roster.len(), 10);
    }

    #[test]
    fn parse_str_core_validates_and_orders_slugs() {
        // The string parser (shared by every roster consumer) validates and orders
        // slugs identically, with no filesystem involved.
        let toml = r#"
schema_version = 1
[[repo]]
slug = "rysweet/Simard"
[[repo]]
slug = "not-a-slug"
[[repo]]
slug = "rysweet//azlin"
[[repo]]
slug = "rysweet/azlin"
"#;
        let roster = parse_ecosystem_roster(toml).unwrap();
        assert_eq!(
            roster,
            vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
            "malformed slugs are skipped; valid ones are kept in order"
        );
        // Empty / all-malformed / invalid rosters are an Err reason, never a
        // silent empty pass — the fail-loud contract every consumer relies on.
        assert!(parse_ecosystem_roster("schema_version = 1\n").is_err());
        assert!(parse_ecosystem_roster("schema_version = 1\n[[repo]]\nslug = \"bad\"\n").is_err());
        assert!(parse_ecosystem_roster("this is = = not valid toml").is_err());
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
