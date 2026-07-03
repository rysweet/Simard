//! Periodic **brain self-examination + memory-hygiene** pass (issue #2419).
//!
//! A higher-level introspection layer the OODA daemon runs on its own env-gated
//! interval (default daily). It *uses* the existing per-cycle infrastructure
//! (distillation, statistics, expired-sensory prune) rather than duplicating it,
//! mirroring the [`crate::disk_health`] periodic-recipe hook pattern.
//!
//! ## Split of labor
//!
//! * The **Rust hook** ([`run_memory_hygiene`] / [`run_brain_introspection`])
//!   owns the verified, RPC-backed memory operations (`get_statistics`,
//!   `prune_expired_sensory`, `consolidate_episodes`), the deterministic prune
//!   cap ([`enforce_prune_cap`]), and metric writes.
//! * The **agentic recipe** (a `recipe-runner-rs` subprocess) owns LLM judgment
//!   — brain-health analysis, pattern mining, prune-candidate identification —
//!   and the GitHub-issue output.
//!
//! ## Safety model (first increment)
//!
//! The first increment is **read + safe-consolidate + recommend**. The only
//! daemon-side deletion is the non-discretionary [`CognitiveMemoryOps::
//! prune_expired_sensory`] cleanup of already-expired transient rows — which is
//! TTL cleanup and therefore **not** clamped by the cap. Value-bearing pruning
//! (superseded / low-value / duplicate memories) is **recommendation-only**:
//! the recipe emits capped `PRUNE_CANDIDATE` lines for human review.
//!
//! The hook deliberately never calls [`CognitiveMemoryOps::prune_superseded`]
//! daemon-side: over the daemon's IPC adapter that call is an `Ok(0)` no-op (only
//! the in-process `LibraryCognitiveMemory` reclaims), so invoking it would be a
//! silent-degradation hazard. Backed-up, bounded destructive prune is a
//! documented follow-up that adds the RPC on the adapter **server**.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::runtime_config::RuntimeConfig;

/// Stable adapter / issue-dedup label for this pass.
const ADAPTER_TAG: &str = "brain-introspection";
/// Recipe asset filename (resolved hot-reload-first, then in-tree).
const RECIPE_FILENAME: &str = "brain-introspection.yaml";
/// Episodic batch size handed to `consolidate_episodes` per run.
const CONSOLIDATE_BATCH: u32 = 50;

/// Default cadence: run the pass once every 24 hours.
pub const DEFAULT_INTERVAL_SECS: u64 = 86_400;
/// Default ceiling on the number of value-bearing prune *recommendations* a
/// single run may surface (absolute count; does NOT throttle expired-sensory).
pub const DEFAULT_MAX_PRUNE: usize = 25;
/// Default rolling-baseline window (number of prior runs) for regressions.
pub const DEFAULT_BASELINE_RUNS: u32 = 7;

// ───────────────────────────────────────────────────────────────────────────
// Config knobs — env parsing
// ───────────────────────────────────────────────────────────────────────────

/// Parse `SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS`. A valid `0` is honored as
/// "disabled" (does NOT fall back to the default); garbage/empty → default.
pub fn interval_secs_from_env(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS)
}

/// Parse `SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE`; garbage/unset → default.
pub fn max_prune_from_env(raw: Option<&str>) -> usize {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_PRUNE)
}

/// Parse `SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS`; garbage/unset → default.
pub fn baseline_runs_from_env(raw: Option<&str>) -> u32 {
    raw.and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_BASELINE_RUNS)
}

/// Daemon interval gate. `interval_secs == 0` disables the pass entirely;
/// otherwise the pass is due once `elapsed >= interval`.
pub fn should_run_introspection(elapsed: Duration, interval_secs: u64) -> bool {
    interval_secs > 0 && elapsed >= Duration::from_secs(interval_secs)
}

/// The pure, deterministic safety bound: `min(requested, cap)`.
///
/// Bounds the recipe's value-bearing prune *recommendation* count (passed as
/// `-c max_prune=<cap>`). A `cap` of 0 always yields 0 (introspection performs
/// no value-bearing prune recommendations when disabled). It does **not**
/// throttle [`CognitiveMemoryOps::prune_expired_sensory`].
pub fn enforce_prune_cap(requested: usize, cap: usize) -> usize {
    requested.min(cap)
}

// ───────────────────────────────────────────────────────────────────────────
// Safe, RPC-backed memory hygiene (deterministic, no subprocess)
// ───────────────────────────────────────────────────────────────────────────

/// Outcome of the deterministic, daemon-side memory-hygiene pass.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryHygieneOutcome {
    /// Non-sensory live memory count plus the facts consolidation added.
    pub live_memories: u64,
    /// Already-expired transient sensory rows removed (non-discretionary TTL
    /// cleanup — NOT clamped by the prune cap).
    pub sensory_pruned: usize,
    /// Facts/procedures added by consolidation, measured as the post−pre delta
    /// of `(semantic + procedural)` counts from `get_statistics`.
    pub consolidated_facts: u64,
}

/// Run the safe, bounded, RPC-backed memory-hygiene core.
///
/// Sequence (each a memory RPC whose failure propagates):
/// 1. `get_statistics()` → pre-snapshot (non-sensory live count).
/// 2. `prune_expired_sensory()` → remove already-expired transient rows
///    (uncapped, non-discretionary TTL cleanup).
/// 3. `consolidate_episodes(batch)` → additive distillation (episodic →
///    semantic/procedural), reusing the per-cycle pipeline.
/// 4. `get_statistics()` → post-snapshot; `consolidated_facts` is the post−pre
///    `(semantic + procedural)` delta.
///
/// Never calls [`CognitiveMemoryOps::prune_superseded`] (no destructive
/// value-prune daemon-side; see the module-level safety model).
pub fn run_memory_hygiene(
    mem: &dyn CognitiveMemoryOps,
    batch_size: u32,
) -> SimardResult<MemoryHygieneOutcome> {
    let before = mem.get_statistics()?;

    // Non-discretionary TTL cleanup of already-expired transient rows. NOT
    // throttled by the prune cap — these rows are already past their TTL.
    let sensory_pruned = mem.prune_expired_sensory()?;

    // Additive distillation (episodic → semantic/procedural). Reuses the same
    // pipeline the per-cycle scheduler drives; we do NOT re-implement it.
    let _ = mem.consolidate_episodes(batch_size)?;

    let after = mem.get_statistics()?;

    let sem_proc_before = before.semantic_count + before.procedural_count;
    let sem_proc_after = after.semantic_count + after.procedural_count;
    let consolidated_facts = sem_proc_after.saturating_sub(sem_proc_before);

    // Non-sensory live memories at run start, plus the facts consolidation
    // promoted this run (distillation is additive, so the count never shrinks).
    let non_sensory_before = before.working_count
        + before.episodic_count
        + before.semantic_count
        + before.procedural_count
        + before.prospective_count;
    let live_memories = non_sensory_before + consolidated_facts;

    Ok(MemoryHygieneOutcome {
        live_memories,
        sensory_pruned,
        consolidated_facts,
    })
}

// ───────────────────────────────────────────────────────────────────────────
// Report + marker grammar
// ───────────────────────────────────────────────────────────────────────────

/// Structured result of one brain-introspection run.
///
/// The memory-count fields (`live_memories`, `sensory_pruned`,
/// `consolidated_facts`) are **hook-measured**; the narrative fields
/// (`brain_health`, `patterns`, `regressions`, `issue_url`) and the clamped
/// `prune_requested` come from the agentic recipe's text markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrainIntrospectionReport {
    /// Non-sensory live memory count (hook-measured; includes consolidated).
    pub live_memories: u64,
    /// Already-expired transient sensory rows removed daemon-side (uncapped).
    pub sensory_pruned: usize,
    /// Hook-measured `(semantic + procedural)` consolidation delta. The recipe's
    /// `CONSOLIDATED_FACTS=` marker is an advisory echo only.
    pub consolidated_facts: u64,
    /// Value-bearing prune *candidates* recommended, clamped to the cap. Never
    /// auto-deleted in this increment.
    pub prune_requested: usize,
    /// Brain-health findings (≥1 from a successful recipe run).
    pub brain_health: Vec<String>,
    /// Recurring patterns mined from recent episodes / cycle reports.
    pub patterns: Vec<String>,
    /// Regressions detected against the rolling baseline.
    pub regressions: Vec<String>,
    /// URL of the created/updated brain-introspection issue, if emitted.
    pub issue_url: Option<String>,
}

impl BrainIntrospectionReport {
    /// Whether the run produced actionable work or signal.
    pub fn actionable(&self) -> bool {
        self.prune_requested > 0 || !self.regressions.is_empty() || self.consolidated_facts > 0
    }

    /// One-line summary suitable for the daemon log.
    pub fn summary(&self) -> String {
        format!(
            "brain introspection: {} live memories, {} health findings, {} patterns, \
             {} prune candidates, {} sensory pruned, {} consolidated, issue={}",
            self.live_memories,
            self.brain_health.len(),
            self.patterns.len(),
            self.prune_requested,
            self.sensory_pruned,
            self.consolidated_facts,
            self.issue_url.as_deref().unwrap_or("none"),
        )
    }
}

/// Parse the recipe's plain-text markers from the agent step output.
///
/// Grammar (one marker per line; unknown lines silently ignored):
/// ```text
/// BRAIN_HEALTH: <line>        # >=1 required; missing → parse error
/// PATTERN: <line>            # 0..n
/// REGRESSION: <line>         # 0..n
/// PRUNE_CANDIDATE: <line>    # 0..n (human-readable; not stored as a field)
/// PRUNE_REQUESTED=<usize>    # count of candidates (defaults to 0)
/// CONSOLIDATED_FACTS=<u64>   # advisory echo only; hook delta is authoritative
/// ISSUE_URL=<url>            # optional
/// ```
///
/// The hook-measured fields are left at `0`/`None`; the caller fills them in.
pub fn parse_brain_introspection_text(stdout: &str) -> Result<BrainIntrospectionReport, String> {
    let mut brain_health: Vec<String> = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    let mut regressions: Vec<String> = Vec::new();
    let mut prune_requested: usize = 0;
    let mut consolidated_facts: u64 = 0;
    let mut issue_url: Option<String> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(v) = trimmed.strip_prefix("BRAIN_HEALTH:") {
            let v = v.trim();
            if !v.is_empty() {
                brain_health.push(v.to_string());
            }
        } else if let Some(v) = trimmed.strip_prefix("PATTERN:") {
            let v = v.trim();
            if !v.is_empty() {
                patterns.push(v.to_string());
            }
        } else if let Some(v) = trimmed.strip_prefix("REGRESSION:") {
            let v = v.trim();
            if !v.is_empty() {
                regressions.push(v.to_string());
            }
        } else if trimmed.strip_prefix("PRUNE_CANDIDATE:").is_some() {
            // Human-readable candidate description for the issue body; the count
            // is carried by PRUNE_REQUESTED, so nothing to store here.
        } else if let Some(v) = trimmed.strip_prefix("PRUNE_REQUESTED=") {
            prune_requested = v
                .trim()
                .parse::<usize>()
                .map_err(|e| format!("invalid PRUNE_REQUESTED value '{}': {e}", v.trim()))?;
        } else if let Some(v) = trimmed.strip_prefix("CONSOLIDATED_FACTS=") {
            // Advisory echo: the hook's measured delta is authoritative, so a
            // malformed value is tolerated rather than failing the parse.
            if let Ok(n) = v.trim().parse::<u64>() {
                consolidated_facts = n;
            }
        } else if let Some(v) = trimmed.strip_prefix("ISSUE_URL=") {
            let v = v.trim();
            if !v.is_empty() {
                issue_url = Some(v.to_string());
            }
        }
        // Unknown lines are silently ignored (forward-compatible with LLM noise).
    }

    if brain_health.is_empty() {
        return Err("missing required BRAIN_HEALTH line in recipe output".to_string());
    }

    Ok(BrainIntrospectionReport {
        live_memories: 0,
        sensory_pruned: 0,
        consolidated_facts,
        prune_requested,
        brain_health,
        patterns,
        regressions,
        issue_url,
    })
}

// ───────────────────────────────────────────────────────────────────────────
// Recipe resolution + invocation
// ───────────────────────────────────────────────────────────────────────────

/// JSON envelope returned by `recipe-runner-rs --output-format json`.
#[derive(Debug, Deserialize)]
struct RecipeOutput {
    success: bool,
    step_results: Vec<StepResult>,
}

/// A single step's result inside the [`RecipeOutput`] envelope.
#[derive(Debug, Deserialize)]
struct StepResult {
    #[allow(dead_code)] // Part of the JSON contract; not consumed by the hook.
    step_id: String,
    output: String,
}

/// Resolve the recipe YAML path. Checks, in order:
///   1. `<home>/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// `home_override` lets tests supply a fake home directory without mutating the
/// process-wide `HOME`. Returns `None` if neither path exists.
pub(crate) fn resolve_recipe_path(
    repo_root: &Path,
    home_override: Option<&Path>,
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

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        prefix + "…"
    } else {
        prefix
    }
}

fn record_metric_best_effort(name: &str, value: f64) {
    if let Err(e) = crate::self_metrics::record_metric(name, value, ADAPTER_TAG) {
        warn!("brain introspection: failed to record metric {name}: {e}");
    }
}

/// Spawn the agentic `brain-introspection` recipe and parse its markers.
///
/// Returns `Err(SimardError::AdapterInvocationFailed)` on every failure mode
/// (recipe missing, spawn failure, non-zero exit, bad JSON, missing markers);
/// [`run_brain_introspection`] treats those as best-effort and degrades.
fn run_agentic_recipe(
    repo_root: &Path,
    state_root: &Path,
    home_override: Option<&Path>,
    cap: usize,
    baseline_runs: u32,
    hygiene: &MemoryHygieneOutcome,
) -> SimardResult<BrainIntrospectionReport> {
    let recipe_path = resolve_recipe_path(repo_root, home_override).ok_or_else(|| {
        SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!(
                "recipe file {RECIPE_FILENAME} not found in hot-reload or in-tree paths"
            ),
        }
    })?;

    let agent_binary = RuntimeConfig::load()?.llm_provider.agent_binary_value();
    let stats_json = serde_json::to_string(hygiene).unwrap_or_else(|_| "{}".to_string());

    let output = Command::new("recipe-runner-rs")
        .arg(recipe_path.as_os_str())
        .arg("--output-format")
        .arg("json")
        .env("AMPLIHACK_AGENT_BINARY", agent_binary)
        .arg("-c")
        .arg(format!("state_root={}", state_root.display()))
        .arg("-c")
        .arg(format!("repo_path={}", repo_root.display()))
        .arg("-c")
        .arg(format!("max_prune={cap}"))
        .arg("-c")
        .arg(format!("baseline_runs={baseline_runs}"))
        .arg("-c")
        .arg(format!("stats={stats_json}"))
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

    let envelope: RecipeOutput = serde_json::from_slice(&output.stdout).map_err(|e| {
        SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: format!("failed to deserialize recipe JSON output: {e}"),
        }
    })?;

    if !envelope.success {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: "recipe reported success=false in JSON output".to_string(),
        });
    }

    if envelope.step_results.is_empty() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason: "no step results in recipe JSON output".to_string(),
        });
    }

    // Markers may be emitted across several steps, so parse the concatenation.
    let combined = envelope
        .step_results
        .iter()
        .map(|s| s.output.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    parse_brain_introspection_text(&combined).map_err(|e| SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason: format!("failed to parse recipe text output: {e}"),
    })
}

/// Full periodic brain-introspection hook, called from the daemon loop.
///
/// Runs the deterministic, safe [`run_memory_hygiene`] core first (its memory
/// RPC failures propagate), records baseline metrics, then spawns the agentic
/// recipe **best-effort**: a recipe failure is logged at `WARN` and the run
/// still returns the hygiene outcomes (the agentic layer must never block safe
/// memory hygiene). The recipe's value-bearing prune recommendation count is
/// clamped via [`enforce_prune_cap`].
pub fn run_brain_introspection(
    mem: &dyn CognitiveMemoryOps,
    repo_root: &Path,
    state_root: &Path,
    home_override: Option<&Path>,
) -> SimardResult<BrainIntrospectionReport> {
    // Steps 1–3: deterministic, safe hygiene. Memory RPC failures propagate
    // before any subprocess is spawned.
    let hygiene = run_memory_hygiene(mem, CONSOLIDATE_BATCH)?;

    record_metric_best_effort(
        "brain_introspection_live_memories",
        hygiene.live_memories as f64,
    );
    record_metric_best_effort(
        "brain_introspection_sensory_pruned",
        hygiene.sensory_pruned as f64,
    );
    record_metric_best_effort(
        "brain_introspection_consolidated",
        hygiene.consolidated_facts as f64,
    );

    let cap = max_prune_from_env(
        std::env::var("SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE")
            .ok()
            .as_deref(),
    );
    let baseline_runs = baseline_runs_from_env(
        std::env::var("SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS")
            .ok()
            .as_deref(),
    );

    let mut report = BrainIntrospectionReport {
        live_memories: hygiene.live_memories,
        sensory_pruned: hygiene.sensory_pruned,
        consolidated_facts: hygiene.consolidated_facts,
        prune_requested: 0,
        brain_health: Vec::new(),
        patterns: Vec::new(),
        regressions: Vec::new(),
        issue_url: None,
    };

    // Steps 4–5: agentic recipe (best-effort). Any failure → WARN + graceful.
    match run_agentic_recipe(
        repo_root,
        state_root,
        home_override,
        cap,
        baseline_runs,
        &hygiene,
    ) {
        Ok(parsed) => {
            report.brain_health = parsed.brain_health;
            report.patterns = parsed.patterns;
            report.regressions = parsed.regressions;
            report.prune_requested = enforce_prune_cap(parsed.prune_requested, cap);
            report.issue_url = parsed.issue_url;
            // `consolidated_facts` stays hook-measured (recipe marker advisory).
        }
        Err(e) => {
            warn!("brain introspection: agentic recipe degraded (safe hygiene preserved): {e}");
        }
    }

    record_metric_best_effort(
        "brain_introspection_prune_requested",
        report.prune_requested as f64,
    );

    Ok(report)
}
