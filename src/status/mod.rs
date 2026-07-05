//! The single typed `StatusSnapshot` that the `simard status` CLI, the dashboard
//! **Status** tab, and the TUI **Status** tab all render.
//!
//! One provider ([`provider::assemble`]) builds it from durable, process-agnostic
//! sources; [`json`] serializes it (`--json` / HTTP body); [`render`] formats the
//! canonical terminal layout. Every section is wrapped in a [`SectionEnvelope`]
//! so availability and freshness travel with the data — a missing count is
//! `absent`, never a silent `0`.
//!
//! See `docs/reference/status-snapshot-api.md` for the full contract.

pub mod json;
pub mod provider;
pub mod render;

use serde::{Deserialize, Serialize};

pub use provider::{AssembleOptions, assemble};

/// Bumped when the serialized snapshot shape changes incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// Whether a section's source could be read at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    /// Source read successfully.
    Ok,
    /// Source is absent or not reachable in this context (e.g. `gh`
    /// unauthenticated, daemon down). Not an error.
    #[default]
    Unavailable,
    /// Source errored while being read.
    Error,
}

/// How fresh a successfully-read section is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Freshness {
    /// Read within the source's freshness window.
    Live,
    /// Last-known value; source older than its window.
    Stale,
    /// Source missing entirely.
    #[default]
    Absent,
}

/// A section payload wrapped with availability + freshness metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionEnvelope<T> {
    pub availability: Availability,
    pub freshness: Freshness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default = "none", skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

fn none<T>() -> Option<T> {
    None
}

impl<T> Default for SectionEnvelope<T> {
    fn default() -> Self {
        Self {
            availability: Availability::Unavailable,
            freshness: Freshness::Absent,
            as_of: None,
            note: None,
            data: None,
        }
    }
}

impl<T> SectionEnvelope<T> {
    /// A fresh, available section carrying `data`.
    pub fn live(data: T, as_of: Option<String>) -> Self {
        Self {
            availability: Availability::Ok,
            freshness: Freshness::Live,
            as_of,
            note: None,
            data: Some(data),
        }
    }

    /// An available but stale section carrying last-known `data`.
    pub fn stale(data: T, as_of: Option<String>) -> Self {
        Self {
            availability: Availability::Ok,
            freshness: Freshness::Stale,
            as_of,
            note: None,
            data: Some(data),
        }
    }

    /// A source that is simply not present in this context (no data).
    pub fn absent(note: impl Into<String>) -> Self {
        Self {
            availability: Availability::Unavailable,
            freshness: Freshness::Absent,
            as_of: None,
            note: Some(note.into()),
            data: None,
        }
    }

    /// A source that errored while being read (no data).
    pub fn error(note: impl Into<String>) -> Self {
        Self {
            availability: Availability::Error,
            freshness: Freshness::Absent,
            as_of: None,
            note: Some(note.into()),
            data: None,
        }
    }

    /// True when the section carries usable data.
    pub fn is_present(&self) -> bool {
        self.availability == Availability::Ok && self.data.is_some()
    }
}

/// The complete status snapshot. Every section is independently degradable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub schema_version: u32,
    /// RFC3339 timestamp of when the snapshot was assembled.
    pub generated_at: String,
    #[serde(default)]
    pub daemon: SectionEnvelope<Daemon>,
    #[serde(default)]
    pub resources: SectionEnvelope<Resources>,
    #[serde(default)]
    pub llm: SectionEnvelope<LlmUsage>,
    #[serde(default)]
    pub memory: SectionEnvelope<MemoryBrain>,
    #[serde(default)]
    pub gym: SectionEnvelope<Gym>,
    #[serde(default)]
    pub goals: SectionEnvelope<GoalBoard>,
    #[serde(default)]
    pub workstreams: SectionEnvelope<Workstreams>,
    #[serde(default)]
    pub completed: SectionEnvelope<CompletedWork>,
    #[serde(default)]
    pub self_improvement: SectionEnvelope<SelfImprovement>,
    #[serde(default)]
    pub telemetry: SectionEnvelope<TelemetrySignals>,
    /// OVERSEER — the acting Overseer meta-loop's recent activity feed (#2419):
    /// last-N ticks, per-thread status, and the honest disabled/observing state.
    #[serde(default)]
    pub overseer: SectionEnvelope<crate::overseer::activity::OverseerActivity>,
}

impl StatusSnapshot {
    /// A snapshot with every section absent, stamped now.
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            generated_at: crate::telemetry::snapshot::now_rfc3339(),
            daemon: SectionEnvelope::default(),
            resources: SectionEnvelope::default(),
            llm: SectionEnvelope::default(),
            memory: SectionEnvelope::default(),
            gym: SectionEnvelope::default(),
            goals: SectionEnvelope::default(),
            workstreams: SectionEnvelope::default(),
            completed: SectionEnvelope::default(),
            self_improvement: SectionEnvelope::default(),
            telemetry: SectionEnvelope::default(),
            overseer: SectionEnvelope::default(),
        }
    }
}

impl Default for StatusSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

// ── Section payloads ────────────────────────────────────────────────────────

/// DAEMON / UPTIME.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Daemon {
    pub state: String,
    pub version: String,
    pub main_pid: Option<u32>,
    pub deployed_commit: Option<String>,
    pub instance_uptime: Option<String>,
    pub n_restarts: Option<u64>,
    pub running_since: Option<String>,
}

/// RESOURCE SNAPSHOT. Byte counts are raw bytes; `render` formats them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Resources {
    pub cpu_pct: Option<f64>,
    pub rss_bytes: Option<u64>,
    pub cgroup_mem_peak_bytes: Option<u64>,
    pub load_1: Option<f64>,
    pub load_5: Option<f64>,
    pub load_15: Option<f64>,
    pub sys_mem_used_bytes: Option<u64>,
    pub sys_mem_total_bytes: Option<u64>,
    pub sys_mem_avail_bytes: Option<u64>,
    pub disk_home: Option<DiskUsage>,
    pub disk_tmp: Option<DiskUsage>,
    pub live_engineers: Option<u32>,
}

/// Free / total bytes for one mount point.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskUsage {
    pub free_bytes: u64,
    pub total_bytes: u64,
}

/// LLM USAGE.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmUsage {
    pub copilot_turn: Option<CopilotTurn>,
    pub ledger_today: Option<LedgerWindow>,
    pub ledger_7d: Option<LedgerWindow>,
    pub ledger_all_time: Option<LedgerWindow>,
    pub daily_budget_usd: Option<f64>,
    pub reconciliation: Option<Reconciliation>,
}

/// Most-recent Copilot per-turn accounting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopilotTurn {
    pub tokens_in: u64,
    pub tokens_cached: u64,
    pub tokens_out: u64,
    pub ai_credits: u64,
}

/// Cost-ledger aggregate over a window.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LedgerWindow {
    pub cost_usd: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// The two-books reconciliation: dollar ledger vs Copilot AI-credits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reconciliation {
    pub ledger_usd: f64,
    pub credits: u64,
    /// `ok` | `under-count` | `over-count`.
    pub delta_flag: String,
}

/// MEMORY / BRAIN.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryBrain {
    pub store_path: String,
    pub store_size_bytes: Option<u64>,
    pub backend: String,
    pub nodes_total: Option<u64>,
    pub nodes: NodeCounts,
    pub edges: EdgeCounts,
    pub cognitive_processes: CognitiveHealth,
    pub brains_llm_backed: Option<String>,
    pub brain_fallbacks: Option<u64>,
    pub decide_ladder_exhausted: Option<u64>,
}

/// Per-type memory node counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeCounts {
    pub episodic: Option<u64>,
    pub semantic: Option<u64>,
    pub prospective: Option<u64>,
    pub working: Option<u64>,
    pub procedural: Option<u64>,
    pub sensory: Option<u64>,
}

/// Per-type memory edge counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeCounts {
    pub derives_from: Option<u64>,
    pub similar_to: Option<u64>,
    pub supersedes: Option<u64>,
}

/// Cognitive-process health labels.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CognitiveHealth {
    pub distillation: Option<String>,
    pub consolidation: Option<String>,
    pub introspection: Option<String>,
}

/// GYM.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gym {
    pub skip_gym: bool,
    pub configured_scenarios: Option<u32>,
    /// `idle` | `active`.
    pub self_eval_state: String,
}

/// GOAL BOARD.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GoalBoard {
    pub active: Vec<GoalItem>,
}

/// One active goal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GoalItem {
    pub short_id: String,
    /// e.g. `p0`.
    pub priority: String,
    pub status: String,
    pub summary: String,
}

/// ACTIVE WORKSTREAMS.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workstreams {
    pub operator_recipes: Vec<WorkItem>,
    pub engineer_workstreams: Vec<WorkItem>,
}

/// One workstream line: a label plus a one-line status.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkItem {
    pub label: String,
    pub status: String,
}

/// COMPLETED WORK — merged PRs grouped by repo.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompletedWork {
    pub repos: Vec<RepoPrs>,
}

/// Merged PRs for one repository.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoPrs {
    pub repo: String,
    pub prs: Vec<PrItem>,
}

/// One PR entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrItem {
    pub number: u64,
    pub summary: String,
    pub status: String,
}

/// SELF-IMPROVEMENT.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfImprovement {
    pub merged: Vec<PrItem>,
    pub running: Vec<WorkItem>,
    pub pending: Vec<WorkItem>,
}

/// TELEMETRY / UNEXPECTED SIGNALS.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetrySignals {
    /// Human label for the analysis window, e.g. `last 1h`.
    pub window: String,
    pub distill_fail_pct: Option<f64>,
    pub restart_churn: Option<u64>,
    pub gym_skipped: bool,
    /// `ok` | `over` | `unknown`.
    pub budget_flag: String,
    pub parse_fix_holding: Option<bool>,
    /// Named anomalies (panics / segv / corruption / fallback / budget).
    pub anomalies: Vec<String>,
}
