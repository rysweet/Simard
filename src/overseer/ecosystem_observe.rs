//! Thin Rust rail for the agentic **ecosystem-observe** chain (issue #2419).
//!
//! Simard observes her stewarded ecosystem with a DETERMINISTIC WORKFLOW OF
//! AGENTIC STEPS + PROMPTS — not a Rust "code sensor." This module is the only
//! new Rust the feature needs, and it is deliberately thin:
//!
//! 1. [`resolve_stewarded_roster`] resolves the roster of stewarded repos from
//!    identity-scoped curated state (`<state_root>/identity/<id>/curated/
//!    stewarded_repos.toml`), seeding it once from the identity default. The
//!    roster is mutable, agentically-curated, deploy-durable state — part of who
//!    Simard IS, not a committed framework file. [`add_stewarded_repo`] /
//!    [`remove_stewarded_repo`] are the curation mutations (surfaced by
//!    `simard roster add|remove`).
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

/// One roster entry. Pure data — a slug plus a human-readable note. The note is
/// ignored by the loader (only the slug strings reach the agent) but is
/// preserved across curation mutations so operator context is not lost.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct RosterEntry {
    slug: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    note: String,
}

/// The roster document shape: `schema_version` plus a `[[repo]]` array.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct RosterFile {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default)]
    repo: Vec<RosterEntry>,
}

fn default_schema_version() -> u32 {
    1
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

/// Parse & validate a roster from its TOML **text** — the path-agnostic core
/// shared by every consumer (the Overseer ecosystem-observe rail, the
/// merge-queue reasoner, and the CI-health sweep) so the roster has exactly one
/// source of truth: the identity-curated document. Returns the validated
/// `owner/name` slugs in document order.
///
/// Each slug is checked with [`is_valid_slug`]; a malformed slug is skipped with
/// a logged warning. An empty roster (no valid slugs — the document was empty or
/// every slug was malformed) is an **error** (the `Err` reason), never a silent
/// empty pass, so a caller can fail loud instead of concluding an empty fleet is
/// healthy. The error carries only a human-readable reason; the caller wraps it
/// in whatever error type fits its context.
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

// ───────────────── identity-curated stewarded roster (state) ────────────────

/// The identity-curated-data key under which an identity stores her stewarded
/// roster. A fixed const — never derived from env/args (path-traversal
/// prevention in [`crate::identity_state`]).
pub const STEWARDED_ROSTER_KEY: &str = "stewarded_repos";

/// The identity slug for **Simard herself** (no `SIMARD_IDENTITY` set). Her
/// curated roster lives at `<state_root>/identity/simard/curated/stewarded_repos.toml`.
pub const DEFAULT_IDENTITY_SLUG: &str = "simard";

/// Simard's identity DEFAULT stewarded roster — the seed used ONLY to initialise
/// her durable curated store on first use. This is part of who Simard *is* (like
/// `DEFAULT_SEED_GOALS`), not a framework roster file: once seeded, the roster is
/// mutable identity-scoped state Simard curates agentically via `simard roster
/// add|remove`, and every edit survives self-deploy (the state root is never
/// overwritten by `install`).
///
/// `amplihack` means `rysweet/amplihack-rs`; the Python `rysweet/amplihack` is
/// deprecated and is NOT on the roster.
pub fn default_simard_roster_seed_toml() -> &'static str {
    r#"# Simard's DEFAULT stewarded roster — the seed for her identity-scoped,
# agentically-curated, deploy-durable roster state. This is only the initial
# seed: once written under <state_root>/identity/simard/curated/, Simard owns and
# mutates it via `simard roster add|remove`, and it survives every self-deploy.
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
note = "Graph-based 6-type cognitive memory (Kuzu-backed)"

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
"#
}

/// Load-or-seed the raw curated roster **document** (preserving notes) for
/// `(state_root, identity)`, seeding it from `seed_toml` when absent.
///
/// - If the curated document exists, its raw TOML is returned.
/// - Else, when `seed_toml` is non-blank, it is validated (fail-loud: a garbage
///   seed is never persisted), written to the curated store, and returned.
/// - Else (absent + blank seed) an `Err` is returned so the caller fails visible
///   rather than fabricating an empty roster.
fn load_or_seed_roster_doc(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
) -> Result<String, String> {
    if let Some(raw) =
        crate::identity_state::load_curated(state_root, identity, STEWARDED_ROSTER_KEY)
    {
        return Ok(raw);
    }
    let seed = seed_toml.trim();
    if seed.is_empty() {
        return Err(format!(
            "no curated roster for identity '{identity}' and no seed to initialise it"
        ));
    }
    // Validate the seed BEFORE persisting so a malformed seed is never written.
    parse_ecosystem_roster(seed_toml)?;
    crate::identity_state::store_curated(state_root, identity, STEWARDED_ROSTER_KEY, seed_toml)
        .map_err(|e| format!("failed to seed curated roster for identity '{identity}': {e}"))?;
    Ok(seed_toml.to_string())
}

/// Resolve the stewarded roster from identity-scoped curated state, seeding it
/// once from `seed_toml` when absent.
///
/// This is the single roster source of truth. It replaces the retired committed
/// `prompt_assets/simard/ecosystem_repos.toml` framework file: the roster now
/// lives as mutable, identity-scoped, deploy-durable state under
/// `<state_root>/identity/<identity>/curated/stewarded_repos.toml` (which
/// `install` never overwrites), curated agentically by Simard. Returns the
/// validated `owner/name` slugs in document order, or an `Err` reason (empty /
/// unseedable / malformed roster) the caller wraps for its context.
pub fn resolve_stewarded_roster(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
) -> Result<Vec<String>, String> {
    let raw = load_or_seed_roster_doc(state_root, identity, seed_toml)?;
    parse_ecosystem_roster(&raw)
}

/// Resolve `(identity_slug, seed_toml)` for the running daemon (and the
/// `simard roster` CLI) from the environment, mirroring how
/// [`crate::state_root`] resolves per-identity state:
///
/// - `SIMARD_IDENTITY` unset/blank → Simard herself: slug `simard`, seeded from
///   [`default_simard_roster_seed_toml`].
/// - `SIMARD_IDENTITY` set → that identity curates its OWN roster; there is no
///   baked seed (a non-Simard identity does not inherit Simard's repos), so the
///   rail wires only once the identity has curated a roster via `simard roster add`.
pub fn daemon_identity_and_seed() -> (String, String) {
    match std::env::var("SIMARD_IDENTITY") {
        Ok(name)
            if !name.trim().is_empty()
                && crate::identity_state::sanitize_component(&name).is_some() =>
        {
            let slug = crate::identity_state::sanitize_component(&name)
                .unwrap_or_else(|| DEFAULT_IDENTITY_SLUG.to_string());
            (slug, String::new())
        }
        _ => (
            DEFAULT_IDENTITY_SLUG.to_string(),
            default_simard_roster_seed_toml().to_string(),
        ),
    }
}

/// Resolve the stewarded roster for the running daemon: pick the identity +
/// seed from the environment ([`daemon_identity_and_seed`]) and resolve through
/// the identity-curated store ([`resolve_stewarded_roster`]).
pub fn resolve_daemon_stewarded_roster(state_root: &Path) -> Result<Vec<String>, String> {
    let (identity, seed) = daemon_identity_and_seed();
    resolve_stewarded_roster(state_root, &identity, &seed)
}

/// Outcome of a roster curation mutation, for CLI/operator reporting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RosterMutation {
    /// Whether the mutation changed the persisted roster (idempotent no-ops
    /// report `false`).
    pub changed: bool,
    /// The full roster (slugs, in document order) AFTER the mutation.
    pub roster: Vec<String>,
    /// A human-readable one-line summary of what happened.
    pub summary: String,
}

/// Load the curated roster **document** for mutation, defaulting to an EMPTY
/// document when neither a curated store nor a seed exists. Unlike
/// [`load_or_seed_roster_doc`] (which fails loud on an unseedable roster because
/// resolving an empty fleet would be a false-green), a *mutation* must be able to
/// bootstrap a fresh identity's roster from nothing — adding the first repo is
/// how a non-Simard identity (with no baked seed) starts curating its fleet. A
/// non-blank `seed_toml` is validated before it is adopted so a malformed seed is
/// never the mutation's starting point.
fn load_roster_doc_for_mutation(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
) -> Result<RosterFile, String> {
    if let Some(raw) =
        crate::identity_state::load_curated(state_root, identity, STEWARDED_ROSTER_KEY)
    {
        return toml::from_str(&raw).map_err(|e| format!("parse curated roster failed: {e}"));
    }
    let seed = seed_toml.trim();
    if seed.is_empty() {
        return Ok(RosterFile {
            schema_version: default_schema_version(),
            repo: Vec::new(),
        });
    }
    // Validate the seed before adopting it as the mutation's starting point.
    parse_ecosystem_roster(seed_toml)?;
    toml::from_str(seed_toml).map_err(|e| format!("parse curated roster seed failed: {e}"))
}

fn serialize_roster_doc(doc: &RosterFile) -> Result<String, String> {
    toml::to_string(doc).map_err(|e| format!("failed to serialize curated roster: {e}"))
}

/// Add `slug` (with an optional `note`) to the identity's curated stewarded
/// roster, seeding the store first when absent. Idempotent: adding a slug the
/// roster already lists is a no-op. Validates the slug as a clean `owner/name`
/// so a malformed slug can never be persisted.
pub fn add_stewarded_repo(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
    slug: &str,
    note: &str,
) -> Result<RosterMutation, String> {
    let slug = slug.trim();
    if !is_valid_slug(slug) {
        return Err(format!(
            "refusing to add malformed stewarded repo slug {slug:?} (must be a clean owner/name)"
        ));
    }
    let mut doc = load_roster_doc_for_mutation(state_root, identity, seed_toml)?;
    if doc.repo.iter().any(|e| e.slug.trim() == slug) {
        let serialized = serialize_roster_doc(&doc)?;
        let roster = parse_ecosystem_roster(&serialized)?;
        return Ok(RosterMutation {
            changed: false,
            roster,
            summary: format!("'{slug}' is already on {identity}'s stewarded roster (no change)"),
        });
    }
    doc.repo.push(RosterEntry {
        slug: slug.to_string(),
        note: note.trim().to_string(),
    });
    let serialized = serialize_roster_doc(&doc)?;
    let roster = parse_ecosystem_roster(&serialized)?;
    crate::identity_state::store_curated(state_root, identity, STEWARDED_ROSTER_KEY, &serialized)
        .map_err(|e| format!("failed to persist curated roster: {e}"))?;
    Ok(RosterMutation {
        changed: true,
        roster,
        summary: format!("added '{slug}' to {identity}'s stewarded roster"),
    })
}

/// Remove `slug` from the identity's curated stewarded roster, seeding the store
/// first when absent. Idempotent: removing a slug that is not on the roster is a
/// no-op. Refuses to remove the **last** repo — an empty roster is a fail-loud
/// error state (it would classify the whole fleet as green), never a silent
/// empty pass.
pub fn remove_stewarded_repo(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
    slug: &str,
) -> Result<RosterMutation, String> {
    let slug = slug.trim();
    let raw = load_or_seed_roster_doc(state_root, identity, seed_toml)?;
    let mut doc: RosterFile =
        toml::from_str(&raw).map_err(|e| format!("parse curated roster failed: {e}"))?;
    let before = doc.repo.len();
    let present = doc.repo.iter().any(|e| e.slug.trim() == slug);
    if !present {
        let roster = parse_ecosystem_roster(&raw)?;
        return Ok(RosterMutation {
            changed: false,
            roster,
            summary: format!("'{slug}' is not on {identity}'s stewarded roster (no change)"),
        });
    }
    let remaining_valid = doc
        .repo
        .iter()
        .filter(|e| e.slug.trim() != slug && is_valid_slug(e.slug.trim()))
        .count();
    if remaining_valid == 0 {
        return Err(format!(
            "refusing to remove '{slug}': it is {identity}'s last stewarded repo, and an empty \
             roster is a fail-loud error (it would report the whole fleet green)"
        ));
    }
    doc.repo.retain(|e| e.slug.trim() != slug);
    let removed = before - doc.repo.len();
    let serialized = serialize_roster_doc(&doc)?;
    let roster = parse_ecosystem_roster(&serialized)?;
    crate::identity_state::store_curated(state_root, identity, STEWARDED_ROSTER_KEY, &serialized)
        .map_err(|e| format!("failed to persist curated roster: {e}"))?;
    Ok(RosterMutation {
        changed: true,
        roster,
        summary: format!("removed '{slug}' from {identity}'s stewarded roster ({removed} entry)"),
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

// The stewarded roster is no longer a committed framework file resolved by
// path. It is identity-scoped curated state under
// `<state_root>/identity/<identity>/curated/stewarded_repos.toml`, resolved by
// `resolve_stewarded_roster` / `resolve_daemon_stewarded_roster`. See the module
// docs and `docs/reference/ecosystem-roster-resolution.md`.

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

    // ── roster: default seed ─────────────────────────────────────────────

    #[test]
    fn default_seed_lists_the_ten_stewarded_repos() {
        let roster = parse_ecosystem_roster(default_simard_roster_seed_toml())
            .expect("the baked default seed must parse");
        assert_eq!(
            roster.len(),
            10,
            "Simard's default stewarded seed lists the 10 stewarded repos"
        );
        assert!(roster.contains(&"rysweet/Simard".to_string()));
        assert!(roster.contains(&"rysweet/amplihack-rs".to_string()));
        assert!(roster.contains(&"rysweet/gadugi-agentic-test".to_string()));
        assert!(
            !roster.iter().any(|s| s == "rysweet/amplihack"),
            "the deprecated python amplihack is not on the roster"
        );
    }

    // ── roster: identity-curated resolution + seeding ────────────────────

    #[test]
    fn resolve_seeds_curated_store_on_first_use_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Precondition: nothing curated yet.
        assert!(!crate::identity_state::curated_exists(
            root,
            DEFAULT_IDENTITY_SLUG,
            STEWARDED_ROSTER_KEY
        ));

        let roster = resolve_stewarded_roster(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
        )
        .expect("first resolve must seed + return the default roster");
        assert_eq!(roster.len(), 10);

        // The seed is now durable state: the curated file exists and re-resolving
        // reads it (deploy-durable — install never touches the state root).
        assert!(crate::identity_state::curated_exists(
            root,
            DEFAULT_IDENTITY_SLUG,
            STEWARDED_ROSTER_KEY
        ));
    }

    #[test]
    fn resolve_reads_curated_edits_and_never_reseeds_over_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Simard curated a 2-repo roster of her own.
        let curated = "schema_version = 1\n\
            [[repo]]\nslug = \"rysweet/azlin\"\n\
            [[repo]]\nslug = \"rysweet/Simard\"\n";
        crate::identity_state::store_curated(
            root,
            DEFAULT_IDENTITY_SLUG,
            STEWARDED_ROSTER_KEY,
            curated,
        )
        .unwrap();

        // Resolving with the (10-repo) default seed present MUST honor her edit,
        // not clobber it back to the seed.
        let roster = resolve_stewarded_roster(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
        )
        .unwrap();
        assert_eq!(
            roster,
            vec!["rysweet/azlin".to_string(), "rysweet/Simard".to_string()],
            "curated identity state wins over the seed (mutable, deploy-durable)"
        );
    }

    #[test]
    fn resolve_skips_malformed_slugs_but_keeps_valid() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let curated = r#"
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
        crate::identity_state::store_curated(root, "id", STEWARDED_ROSTER_KEY, curated).unwrap();
        let roster = resolve_stewarded_roster(root, "id", "").unwrap();
        assert_eq!(
            roster,
            vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
            "malformed slugs are skipped; valid ones are kept in order"
        );
    }

    #[test]
    fn resolve_all_invalid_is_error_not_empty_pass() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        crate::identity_state::store_curated(
            root,
            "id",
            STEWARDED_ROSTER_KEY,
            "schema_version = 1\n[[repo]]\nslug = \"bad\"\n",
        )
        .unwrap();
        assert!(
            resolve_stewarded_roster(root, "id", "").is_err(),
            "a roster with no valid slugs is an error, never a silent empty pass"
        );
    }

    #[test]
    fn resolve_absent_with_blank_seed_is_error_not_empty_pass() {
        let dir = tempfile::tempdir().unwrap();
        // A named identity that has not curated a roster and has no baked seed:
        // fail-visible, never a fabricated empty roster.
        assert!(resolve_stewarded_roster(dir.path(), "gastronome", "").is_err());
        // ...and nothing was written.
        assert!(!crate::identity_state::curated_exists(
            dir.path(),
            "gastronome",
            STEWARDED_ROSTER_KEY
        ));
    }

    #[test]
    fn resolve_never_persists_a_malformed_seed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(resolve_stewarded_roster(root, "id", "this is = = not valid toml").is_err());
        assert!(
            !crate::identity_state::curated_exists(root, "id", STEWARDED_ROSTER_KEY),
            "a malformed seed must never be persisted"
        );
    }

    #[test]
    fn parse_str_core_validates_and_orders_slugs() {
        // The string parser is the single roster source-of-truth validator shared
        // by every consumer (ecosystem-observe, merge-queue, CI-health).
        let toml = r#"
schema_version = 1
[[repo]]
slug = "rysweet/Simard"
[[repo]]
slug = "not-a-slug"
[[repo]]
slug = "rysweet/azlin"
"#;
        let roster = parse_ecosystem_roster(toml).unwrap();
        assert_eq!(
            roster,
            vec!["rysweet/Simard".to_string(), "rysweet/azlin".to_string()],
        );
        // Empty / all-malformed rosters are an Err reason, never a silent empty
        // pass — the fail-loud contract every consumer relies on.
        assert!(parse_ecosystem_roster("schema_version = 1\n").is_err());
        assert!(parse_ecosystem_roster("not valid toml {{").is_err());
    }

    // ── roster: agentic curation (add / remove) ──────────────────────────

    #[test]
    fn add_appends_a_new_repo_and_persists_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Seed the default first (so add operates on a real roster).
        resolve_stewarded_roster(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
        )
        .unwrap();

        let m = add_stewarded_repo(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
            "rysweet/new-steward",
            "a fresh stewarded repo",
        )
        .unwrap();
        assert!(m.changed);
        assert!(m.roster.contains(&"rysweet/new-steward".to_string()));

        // Durable: a fresh resolve sees the added repo.
        let roster = resolve_stewarded_roster(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
        )
        .unwrap();
        assert!(roster.contains(&"rysweet/new-steward".to_string()));
    }

    #[test]
    fn add_is_idempotent_for_an_existing_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let m = add_stewarded_repo(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
            "rysweet/Simard",
            "",
        )
        .unwrap();
        assert!(!m.changed, "adding an already-listed repo is a no-op");
    }

    #[test]
    fn add_rejects_a_malformed_slug() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            add_stewarded_repo(
                dir.path(),
                DEFAULT_IDENTITY_SLUG,
                default_simard_roster_seed_toml(),
                "not-a-slug",
                "",
            )
            .is_err()
        );
    }

    #[test]
    fn add_bootstraps_a_fresh_identity_with_no_seed() {
        // The generic mechanism must let a NON-Simard identity (no baked seed)
        // start curating its fleet from nothing: adding the first repo to an
        // absent roster with a blank seed succeeds and persists, rather than
        // failing loud the way *resolution* of an empty fleet does.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(!crate::identity_state::curated_exists(
            root,
            "gastronome",
            STEWARDED_ROSTER_KEY
        ));
        let m = add_stewarded_repo(root, "gastronome", "", "rysweet/menu-repo", "first")
            .expect("adding the first repo to a fresh identity must succeed");
        assert!(m.changed);
        assert_eq!(m.roster, vec!["rysweet/menu-repo".to_string()]);
        // It is now durably resolvable for that identity (no seed needed).
        let resolved = resolve_stewarded_roster(root, "gastronome", "")
            .expect("the bootstrapped roster must resolve without a seed");
        assert_eq!(resolved, vec!["rysweet/menu-repo".to_string()]);
    }

    #[test]
    fn remove_drops_a_repo_and_persists_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let m = remove_stewarded_repo(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
            "rysweet/azlin",
        )
        .unwrap();
        assert!(m.changed);
        assert!(!m.roster.contains(&"rysweet/azlin".to_string()));

        let roster = resolve_stewarded_roster(
            root,
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
        )
        .unwrap();
        assert!(!roster.contains(&"rysweet/azlin".to_string()));
    }

    #[test]
    fn remove_is_idempotent_for_an_absent_repo() {
        let dir = tempfile::tempdir().unwrap();
        let m = remove_stewarded_repo(
            dir.path(),
            DEFAULT_IDENTITY_SLUG,
            default_simard_roster_seed_toml(),
            "rysweet/not-on-roster",
        )
        .unwrap();
        assert!(!m.changed);
    }

    #[test]
    fn remove_refuses_to_empty_the_roster() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        crate::identity_state::store_curated(
            root,
            "id",
            STEWARDED_ROSTER_KEY,
            "schema_version = 1\n[[repo]]\nslug = \"rysweet/Simard\"\n",
        )
        .unwrap();
        assert!(
            remove_stewarded_repo(root, "id", "", "rysweet/Simard").is_err(),
            "removing the last repo would report the whole fleet green — refuse it"
        );
    }

    #[test]
    #[serial_test::serial(env_simard_identity)]
    fn daemon_roster_defaults_to_simard_seed_without_identity_env() {
        let dir = tempfile::tempdir().unwrap();
        let saved = std::env::var("SIMARD_IDENTITY").ok();
        // SAFETY: guarded by the `env_simard_identity` serial key so no parallel
        // test races this env mutation; restored below.
        unsafe {
            std::env::remove_var("SIMARD_IDENTITY");
        }
        let roster = resolve_daemon_stewarded_roster(dir.path());
        // Restore before asserting so a panic never leaks the mutation.
        unsafe {
            match saved {
                Some(v) => std::env::set_var("SIMARD_IDENTITY", v),
                None => std::env::remove_var("SIMARD_IDENTITY"),
            }
        }
        let roster = roster.expect("no identity → Simard's default seed");
        assert_eq!(roster.len(), 10);
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

    // ── roster path resolution retired ───────────────────────────────────
    //
    // The stewarded roster is no longer a committed file resolved by path; it is
    // identity-scoped curated state under
    // `<state_root>/identity/<identity>/curated/stewarded_repos.toml`, exercised
    // by the `resolve_*` / `add_*` / `remove_*` tests above. The old
    // install-first `resolve_ecosystem_roster_path` (issue #2419) is obsolete:
    // the roster now lives in the state root, which is the same location on a
    // deployed daemon and never a stale source checkout, so the #2419
    // fail-closed-every-tick bug it fixed can no longer arise.
}
