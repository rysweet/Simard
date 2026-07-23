//! Thin Rust rail for the agentic **ecosystem-observe** chain (issue #2419).
//!
//! Simard observes her stewarded ecosystem with a DETERMINISTIC WORKFLOW OF
//! AGENTIC STEPS + PROMPTS — not a Rust "code sensor." This module is the only
//! new Rust the feature needs, and it is deliberately thin:
//!
//! 1. [`resolve_governed_roster`] reads the identity's **mutable, curated**
//!    roster of stewarded repos as pure DATA (a list of `owner/name` slugs) from
//!    the generic identity-scoped state store, seeding it from
//!    [`DEFAULT_SIMARD_GOVERNED_ROSTER`] on first run and validating each slug.
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
use crate::identity_state::{IDENTITY_STATE_SUBDIR, IdentityStateStore};

// ─────────────────────────── roster (data) ─────────────────────────────────

/// One roster entry — pure data: a `owner/name` slug plus a human-readable note.
/// The note is IGNORED by the observe rail (only slug strings reach the agent),
/// but it is preserved on disk so a human — or Simard, curating her roster
/// agentically — sees why each repo is stewarded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RosterEntry {
    /// The GitHub `owner/name` slug of a stewarded repo.
    pub slug: String,
    /// Optional human note; documentation only, never reaches `gh`.
    #[serde(default)]
    pub note: String,
}

/// The governed-repo roster: an identity's curated set of stewarded repos.
///
/// This is the CONSUMER-side payload for the generic
/// [`crate::identity_state`] store — the framework core knows nothing about
/// "rosters"; this type and its key ([`GOVERNED_ROSTER_KEY`]) live here, in the
/// overseer, where the roster is used. `schema_version` future-proofs the
/// on-disk shape; `repo` is the ordered list of stewarded repos.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GovernedRoster {
    /// On-disk schema version for forward-compatible migrations.
    #[serde(default = "default_roster_schema_version")]
    pub schema_version: u32,
    /// The stewarded repos, in curation order.
    #[serde(default)]
    pub repo: Vec<RosterEntry>,
}

impl Default for GovernedRoster {
    fn default() -> Self {
        Self {
            schema_version: default_roster_schema_version(),
            repo: Vec::new(),
        }
    }
}

fn default_roster_schema_version() -> u32 {
    1
}

/// Identity-state key under which an identity's curated governed-repo roster is
/// stored in the generic [`crate::identity_state`] store. Hardcoded const —
/// never derived from env/args/file contents.
pub const GOVERNED_ROSTER_KEY: &str = "governed_repos";

/// The DEFAULT governed roster SEEDED for the Simard identity the first time she
/// runs on a fresh state root. This is Simard's identity data — the repos she
/// stewards — NOT a framework file: after the first run the roster is owned as
/// MUTABLE identity-scoped state under the durable state root (which `install`
/// never overwrites), so Simard curates it agentically (add/remove a stewarded
/// repo) and her edits survive a self-deploy. It is embedded here (like
/// `DEFAULT_SEED_GOALS`) rather than committed under `prompt_assets/` precisely
/// so a re-deploy cannot clobber the runtime-curated copy.
///
/// `amplihack` means `rysweet/amplihack-rs`; the Python `rysweet/amplihack` is
/// deprecated and is NOT on the roster.
pub const DEFAULT_SIMARD_GOVERNED_ROSTER: &str = r#"# Simard's DEFAULT stewarded-repo roster (identity SEED).
#
# This is the one-time seed of Simard's governed roster. On first run it is
# copied into MUTABLE identity-scoped state under the durable state root
# (`<state_root>/identity_state/simard/governed_repos.toml`); after that the
# on-disk copy is authoritative and Simard curates it agentically via
# `simard roster add|remove`. A self-deploy (`simard install`) replaces
# `~/.simard/prompt_assets` but NOT the state root, so curated edits persist.
schema_version = 1

[[repo]]
slug = "rysweet/Simard"
note = "Orchestrator / self-improving engineering identity (steward of this roster)"

[[repo]]
slug = "rysweet/RustyClawd"
note = "Rust-native LLM agent SDK (base type)"

[[repo]]
slug = "rysweet/amplihack-rs"
note = "Core framework — skills, workflows, recipes, hooks, CLI, fleet"

[[repo]]
slug = "rysweet/azlin"
note = "Azure VM provisioning CLI"

[[repo]]
slug = "rysweet/amplihack-memory-lib"
note = "Graph-based 6-type cognitive memory (LadybugDB/lbug-backed)"

[[repo]]
slug = "rysweet/amplihack-agent-eval"
note = "Agent evaluation harness — L1–L12 benchmarks"

[[repo]]
slug = "rysweet/agent-kgpacks"
note = "Knowledge graph packages — GraphRAG grounding"

[[repo]]
slug = "rysweet/amplihack-recipe-runner"
note = "Code-enforced YAML workflow execution engine"

[[repo]]
slug = "rysweet/amplihack-xpia-defender"
note = "Cross-Prompt Injection Attack detection library"

[[repo]]
slug = "rysweet/gadugi-agentic-test"
note = "Multi-agent outside-in testing (Electron/CLI/web/TUI)"
"#;

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

/// Parse the governed roster from its TOML **text** into the structured
/// [`GovernedRoster`] payload. Structural parse only — slug validation happens
/// in [`validated_roster_slugs`] so a curation surface can round-trip the notes.
pub fn parse_governed_roster_toml(raw: &str) -> Result<GovernedRoster, String> {
    toml::from_str(raw).map_err(|e| format!("parse governed roster failed: {e}"))
}

/// Project a [`GovernedRoster`] to its validated `owner/name` slugs, in curation
/// order.
///
/// Each slug is checked with [`is_valid_slug`]; a malformed slug is skipped with
/// a logged warning — it never reaches the agent's `gh` calls. An empty result
/// (no valid slugs — the roster was empty or every slug was malformed) is an
/// **error**, never a silent empty pass, so a caller can fail loud instead of
/// concluding an empty fleet is healthy.
pub fn validated_roster_slugs(roster: &GovernedRoster) -> Result<Vec<String>, String> {
    let mut slugs = Vec::with_capacity(roster.repo.len());
    for entry in &roster.repo {
        let slug = entry.slug.trim();
        if is_valid_slug(slug) {
            slugs.push(slug.to_string());
        } else {
            tracing::warn!(
                target: "overseer::ecosystem_observe",
                slug = %entry.slug,
                "governed roster: skipping malformed slug (not a clean owner/name)"
            );
        }
    }
    if slugs.is_empty() {
        return Err("governed roster has no valid owner/name slugs".to_string());
    }
    Ok(slugs)
}

/// On-disk path of an identity's curated governed roster, for error messages.
fn governed_roster_path(state_root: &Path, identity: &str) -> std::path::PathBuf {
    state_root
        .join(IDENTITY_STATE_SUBDIR)
        .join(identity)
        .join(format!("{GOVERNED_ROSTER_KEY}.toml"))
}

/// Load an identity's curated governed roster as the structured
/// [`GovernedRoster`], SEEDING it from `seed_toml` on first run.
///
/// This is the single source of truth for the roster: the persisted, MUTABLE,
/// identity-scoped copy under the durable state root. On a fresh state root the
/// seed (Simard's [`DEFAULT_SIMARD_GOVERNED_ROSTER`]) is materialized once and
/// persisted; thereafter Simard's agentically-curated edits are authoritative
/// and survive a self-deploy (which never touches the state root). A malformed
/// SEED is a hard error (surfaced, never persisted) so a broken default can
/// never poison the store with an empty roster.
pub fn load_governed_roster(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
) -> SimardResult<GovernedRoster> {
    let store = IdentityStateStore::new(state_root);
    if let Some(existing) = store.load::<GovernedRoster>(identity, GOVERNED_ROSTER_KEY)? {
        return Ok(existing);
    }
    let seeded =
        parse_governed_roster_toml(seed_toml).map_err(|reason| SimardError::PromptAssetRead {
            path: governed_roster_path(state_root, identity),
            reason: format!("seed governed roster invalid: {reason}"),
        })?;
    store.save(identity, GOVERNED_ROSTER_KEY, &seeded)?;
    Ok(seeded)
}

/// Persist an identity's curated governed roster (the mutation half of the
/// curation surface — `simard roster add|remove`).
pub fn save_governed_roster(
    state_root: &Path,
    identity: &str,
    roster: &GovernedRoster,
) -> SimardResult<()> {
    IdentityStateStore::new(state_root).save(identity, GOVERNED_ROSTER_KEY, roster)
}

/// The outcome of an idempotent roster mutation — what the curation surface
/// reports back so `simard roster add|remove` (or Simard's own reasoning) can
/// tell whether the roster actually changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterMutation {
    /// The repo was newly added to the roster.
    Added,
    /// The repo was already on the roster; no change was made.
    AlreadyPresent,
    /// The repo was removed from the roster.
    Removed,
    /// The repo was not on the roster; no change was made.
    NotPresent,
}

/// Whether a slug is a clean stewarded-repo `owner/name` slug — the public
/// predicate a curation surface uses to reject a malformed repo before it is
/// ever written to the roster.
pub fn is_valid_roster_slug(slug: &str) -> bool {
    is_valid_slug(slug)
}

/// Add a stewarded repo to the ACTIVE identity's curated roster (idempotent).
///
/// The identity's roster is seeded on first use (see [`load_governed_roster`])
/// then mutated and persisted. A malformed slug is rejected with an error (it
/// never reaches `gh`); an already-present slug is a no-op ([`RosterMutation::AlreadyPresent`]).
/// This is the write half of "Simard curates her roster agentically".
pub fn add_governed_repo(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
    slug: &str,
    note: &str,
) -> SimardResult<RosterMutation> {
    let slug = slug.trim();
    if !is_valid_slug(slug) {
        return Err(SimardError::PromptAssetRead {
            path: governed_roster_path(state_root, identity),
            reason: format!("refusing to add malformed slug {slug:?} (must be clean owner/name)"),
        });
    }
    let mut roster = load_governed_roster(state_root, identity, seed_toml)?;
    if roster.repo.iter().any(|entry| entry.slug.trim() == slug) {
        return Ok(RosterMutation::AlreadyPresent);
    }
    roster.repo.push(RosterEntry {
        slug: slug.to_string(),
        note: note.trim().to_string(),
    });
    save_governed_roster(state_root, identity, &roster)?;
    Ok(RosterMutation::Added)
}

/// Remove a stewarded repo from the ACTIVE identity's curated roster
/// (idempotent). A slug that is not on the roster is a no-op
/// ([`RosterMutation::NotPresent`]). This is the other write half of Simard's
/// agentic roster curation.
pub fn remove_governed_repo(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
    slug: &str,
) -> SimardResult<RosterMutation> {
    let slug = slug.trim();
    let mut roster = load_governed_roster(state_root, identity, seed_toml)?;
    let before = roster.repo.len();
    roster.repo.retain(|entry| entry.slug.trim() != slug);
    if roster.repo.len() == before {
        return Ok(RosterMutation::NotPresent);
    }
    save_governed_roster(state_root, identity, &roster)?;
    Ok(RosterMutation::Removed)
}

/// Resolve an identity's curated governed roster to its validated `owner/name`
/// slugs — the roster the observe / merge-queue rails and the CI-health sweep
/// scan.
///
/// Seeds from `seed_toml` on first run (see [`load_governed_roster`]) and then
/// projects to validated slugs (see [`validated_roster_slugs`]). Fail-loud on an
/// empty roster so an empty scan is never mistaken for a healthy fleet.
pub fn resolve_governed_roster(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
) -> SimardResult<Vec<String>> {
    let roster = load_governed_roster(state_root, identity, seed_toml)?;
    validated_roster_slugs(&roster).map_err(|reason| SimardError::PromptAssetRead {
        path: governed_roster_path(state_root, identity),
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

    // ── governed roster: seed, parse, validate, resolve ──────────────────

    #[test]
    fn default_simard_seed_parses_to_ten_stewarded_repos() {
        let roster = parse_governed_roster_toml(DEFAULT_SIMARD_GOVERNED_ROSTER)
            .expect("the embedded Simard seed must parse");
        let slugs = validated_roster_slugs(&roster).expect("seed must yield valid slugs");
        assert_eq!(
            slugs.len(),
            10,
            "the Simard identity seed lists the 10 stewarded repos"
        );
        assert!(slugs.contains(&"rysweet/Simard".to_string()));
        assert!(slugs.contains(&"rysweet/amplihack-rs".to_string()));
        assert!(slugs.contains(&"rysweet/gadugi-agentic-test".to_string()));
        assert!(
            !slugs.iter().any(|s| s == "rysweet/amplihack"),
            "the deprecated python amplihack is not on the roster"
        );
    }

    #[test]
    fn validate_skips_malformed_slugs_but_keeps_valid_in_order() {
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
        let roster = parse_governed_roster_toml(toml).unwrap();
        let slugs = validated_roster_slugs(&roster).unwrap();
        assert_eq!(
            slugs,
            vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
            "malformed slugs are skipped; valid ones are kept in order"
        );
    }

    #[test]
    fn validate_all_invalid_is_error_not_empty_pass() {
        let roster =
            parse_governed_roster_toml("schema_version = 1\n[[repo]]\nslug = \"bad\"\n").unwrap();
        assert!(
            validated_roster_slugs(&roster).is_err(),
            "a roster with no valid slugs is an error, never a silent empty pass"
        );
    }

    #[test]
    fn validate_no_entries_is_error() {
        let roster = parse_governed_roster_toml("schema_version = 1\n").unwrap();
        assert!(validated_roster_slugs(&roster).is_err());
    }

    #[test]
    fn parse_invalid_toml_is_error() {
        assert!(parse_governed_roster_toml("this is = = not valid toml {{").is_err());
    }

    #[test]
    fn resolve_seeds_on_fresh_state_root_then_persists() {
        // A fresh state root has no curated roster; resolving SEEDS it from the
        // Simard default, persists it, and returns the validated slugs.
        let state = tempfile::tempdir().unwrap();
        let slugs = resolve_governed_roster(state.path(), "simard", DEFAULT_SIMARD_GOVERNED_ROSTER)
            .unwrap();
        assert_eq!(slugs.len(), 10);

        // The seed was persisted under the identity-scoped state path.
        let persisted = state
            .path()
            .join(crate::identity_state::IDENTITY_STATE_SUBDIR)
            .join("simard")
            .join("governed_repos.toml");
        assert!(
            persisted.is_file(),
            "resolving must persist the seed at {}",
            persisted.display()
        );
    }

    #[test]
    fn curated_edit_survives_and_seed_does_not_reapply() {
        // The deploy-durability guarantee: once curated, the on-disk roster is
        // authoritative — a later resolve (as after a re-deploy) does NOT reseed
        // back to the 10-repo default.
        let state = tempfile::tempdir().unwrap();
        let curated = GovernedRoster {
            schema_version: 1,
            repo: vec![RosterEntry {
                slug: "rysweet/only-this".to_string(),
                note: "curated down to one".to_string(),
            }],
        };
        save_governed_roster(state.path(), "simard", &curated).unwrap();

        let slugs = resolve_governed_roster(state.path(), "simard", DEFAULT_SIMARD_GOVERNED_ROSTER)
            .unwrap();
        assert_eq!(
            slugs,
            vec!["rysweet/only-this".to_string()],
            "the curated roster wins; the seed must not clobber a runtime edit"
        );
    }

    #[test]
    fn resolve_is_per_identity() {
        // Two identities on the same state root curate independent rosters.
        let state = tempfile::tempdir().unwrap();
        save_governed_roster(
            state.path(),
            "gastronome",
            &GovernedRoster {
                schema_version: 1,
                repo: vec![RosterEntry {
                    slug: "chef/menus".to_string(),
                    note: String::new(),
                }],
            },
        )
        .unwrap();

        let simard =
            resolve_governed_roster(state.path(), "simard", DEFAULT_SIMARD_GOVERNED_ROSTER)
                .unwrap();
        let gastronome =
            resolve_governed_roster(state.path(), "gastronome", DEFAULT_SIMARD_GOVERNED_ROSTER)
                .unwrap();
        assert_eq!(simard.len(), 10, "simard seeds her own default roster");
        assert_eq!(
            gastronome,
            vec!["chef/menus".to_string()],
            "gastronome keeps her own curated roster, isolated from simard"
        );
    }

    #[test]
    fn resolve_rejects_malformed_seed_without_persisting() {
        // A broken SEED is surfaced as an error and must NOT poison the store.
        let state = tempfile::tempdir().unwrap();
        assert!(resolve_governed_roster(state.path(), "simard", "not valid toml {{").is_err());
        let persisted = state
            .path()
            .join(crate::identity_state::IDENTITY_STATE_SUBDIR)
            .join("simard")
            .join("governed_repos.toml");
        assert!(
            !persisted.exists(),
            "a malformed seed must not be persisted"
        );
    }

    // ── curation mutations (add / remove) ────────────────────────────────

    #[test]
    fn add_appends_new_repo_and_persists() {
        let state = tempfile::tempdir().unwrap();
        let outcome = add_governed_repo(
            state.path(),
            "simard",
            DEFAULT_SIMARD_GOVERNED_ROSTER,
            "rysweet/new-repo",
            "a freshly stewarded sibling",
        )
        .unwrap();
        assert_eq!(outcome, RosterMutation::Added);

        // Durable: a later resolve (as after a re-deploy) sees the added repo.
        let slugs = resolve_governed_roster(state.path(), "simard", DEFAULT_SIMARD_GOVERNED_ROSTER)
            .unwrap();
        assert_eq!(slugs.len(), 11);
        assert!(slugs.contains(&"rysweet/new-repo".to_string()));
    }

    #[test]
    fn add_is_idempotent_for_existing_repo() {
        let state = tempfile::tempdir().unwrap();
        let outcome = add_governed_repo(
            state.path(),
            "simard",
            DEFAULT_SIMARD_GOVERNED_ROSTER,
            "rysweet/Simard",
            "dup",
        )
        .unwrap();
        assert_eq!(outcome, RosterMutation::AlreadyPresent);
        let slugs = resolve_governed_roster(state.path(), "simard", DEFAULT_SIMARD_GOVERNED_ROSTER)
            .unwrap();
        assert_eq!(slugs.len(), 10, "a duplicate add must not grow the roster");
    }

    #[test]
    fn add_rejects_malformed_slug_without_writing() {
        let state = tempfile::tempdir().unwrap();
        assert!(
            add_governed_repo(
                state.path(),
                "simard",
                DEFAULT_SIMARD_GOVERNED_ROSTER,
                "not a slug; rm -rf",
                "",
            )
            .is_err()
        );
        // A rejected add must not seed/persist anything for this identity.
        let persisted = state
            .path()
            .join(crate::identity_state::IDENTITY_STATE_SUBDIR)
            .join("simard")
            .join("governed_repos.toml");
        assert!(
            !persisted.exists(),
            "a malformed add must not persist a roster"
        );
    }

    #[test]
    fn remove_drops_repo_and_persists() {
        let state = tempfile::tempdir().unwrap();
        let outcome = remove_governed_repo(
            state.path(),
            "simard",
            DEFAULT_SIMARD_GOVERNED_ROSTER,
            "rysweet/azlin",
        )
        .unwrap();
        assert_eq!(outcome, RosterMutation::Removed);
        let slugs = resolve_governed_roster(state.path(), "simard", DEFAULT_SIMARD_GOVERNED_ROSTER)
            .unwrap();
        assert_eq!(slugs.len(), 9);
        assert!(!slugs.contains(&"rysweet/azlin".to_string()));
    }

    #[test]
    fn remove_is_idempotent_for_absent_repo() {
        let state = tempfile::tempdir().unwrap();
        let outcome = remove_governed_repo(
            state.path(),
            "simard",
            DEFAULT_SIMARD_GOVERNED_ROSTER,
            "rysweet/never-on-roster",
        )
        .unwrap();
        assert_eq!(outcome, RosterMutation::NotPresent);
        let slugs = resolve_governed_roster(state.path(), "simard", DEFAULT_SIMARD_GOVERNED_ROSTER)
            .unwrap();
        assert_eq!(slugs.len(), 10, "removing an absent repo is a no-op");
    }

    #[test]
    fn is_valid_roster_slug_matches_internal_validator() {
        assert!(is_valid_roster_slug("rysweet/amplihack-rs"));
        assert!(!is_valid_roster_slug("rysweet/../etc"));
        assert!(!is_valid_roster_slug("no-slash"));
    }

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
