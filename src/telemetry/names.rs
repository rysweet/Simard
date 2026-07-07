//! Canonical metric-name and attribute-key constants for the unified telemetry
//! facade.
//!
//! Every metric name is dotted `simard.<area>.<name>`; every attribute value is
//! a fixed low-cardinality enum (see `docs/reference/telemetry-metrics.md`).
//! Emitters MUST reference these constants rather than string literals so the
//! catalog stays single-sourced and greppable.

// ── Distillation — simard.distill.* ─────────────────────────────────────────

/// One distillation run completed. Attribute `result` = `ok` | `parse_fail`.
pub const DISTILL_RUNS: &str = "simard.distill.runs";
/// Facts produced by a distillation run.
pub const DISTILL_FACTS: &str = "simard.distill.facts";
/// Procedures produced by a distillation run.
pub const DISTILL_PROCEDURES: &str = "simard.distill.procedures";
/// Episodes marked processed by a distillation run.
pub const DISTILL_EPISODES_MARKED: &str = "simard.distill.episodes_marked";

// ── Brain — simard.brain.* ──────────────────────────────────────────────────

/// One brain decision. Attributes `phase` = `decide` | `orient` | `lifecycle`;
/// `result` = `parsed` | `default_empty` | `default_malformed` | `error`.
pub const BRAIN_DECISION: &str = "simard.brain.decision";
/// The `decide` ladder was exhausted with no keyword match.
pub const BRAIN_LADDER_EXHAUSTED: &str = "simard.brain.ladder_exhausted";
/// A reasoner's bounded escalation ladder ended (exhausted / invoke-error) with
/// no parseable decision, so the phase surfaced an EXPLICIT hard parse error to
/// its caller instead of a silent deterministic default (issue #2580 —
/// operator zero-fallback contract). Fires only on a genuine, post-sanitization,
/// post-bounded-retry parse failure — never on a first-try parse or a ladder
/// recovery — so it is the honest "current fallback rate" signal.
pub const BRAIN_PARSE_ERROR: &str = "simard.brain.parse_error";
/// A decision escalated (degraded / quarantine / SIGTERM path).
pub const BRAIN_ESCALATIONS: &str = "simard.brain.escalations";

// ── Engineer — simard.engineer.* ────────────────────────────────────────────

/// An engineer subprocess was spawned.
pub const ENGINEER_SPAWNED: &str = "simard.engineer.spawned";
/// An engineer subprocess exited. Attribute `outcome` = `success` | `failure`
/// | `killed` | `timeout`.
pub const ENGINEER_EXITED: &str = "simard.engineer.exited";
/// Live engineer subprocess count (gauge).
pub const ENGINEER_ACTIVE: &str = "simard.engineer.active";

// ── Daemon — simard.daemon.* ────────────────────────────────────────────────

/// Incremented on the daemon's self-restart / recovery-restart path.
pub const DAEMON_RESTART: &str = "simard.daemon.restart";
/// One OODA cycle completed.
pub const DAEMON_CYCLE: &str = "simard.daemon.cycle";
/// Wall-clock seconds per OODA cycle (histogram).
pub const DAEMON_CYCLE_DURATION_SECONDS: &str = "simard.daemon.cycle_duration_seconds";

/// Explicit histogram bucket boundaries (seconds) for
/// [`DAEMON_CYCLE_DURATION_SECONDS`].
pub const DAEMON_CYCLE_DURATION_BUCKETS: &[f64] =
    &[0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0];

// ── Memory graph — simard.memory.* ──────────────────────────────────────────

/// Node count per memory type (gauge). Attribute `type` = `episodic` |
/// `semantic` | `prospective` | `working` | `procedural` | `sensory`.
pub const MEMORY_NODES: &str = "simard.memory.nodes";
/// Edge count per relationship type (gauge). Attribute `type` = `DERIVES_FROM`
/// | `SIMILAR_TO` | `SUPERSEDES`.
pub const MEMORY_EDGES: &str = "simard.memory.edges";

// ── LLM usage — simard.llm.* ────────────────────────────────────────────────

/// Token throughput (counter). Attributes `dir` = `in` | `out`; `cached` =
/// `true` | `false`.
pub const LLM_TOKENS: &str = "simard.llm.tokens";
/// Dollar cost from the cost ledger (counter).
pub const LLM_COST_USD: &str = "simard.llm.cost_usd";
/// Copilot AI-credits consumed (counter).
pub const LLM_CREDITS: &str = "simard.llm.credits";

// ── Goals — simard.goal.* ───────────────────────────────────────────────────

/// Active goals on the board (gauge).
pub const GOAL_ACTIVE: &str = "simard.goal.active";
/// Goals marked completed (counter).
pub const GOAL_COMPLETED: &str = "simard.goal.completed";
/// Aggregate progress signal 0–100 (gauge).
pub const GOAL_PROGRESS: &str = "simard.goal.progress";

// ── Disk reclaim — simard.disk.reclaim.* ────────────────────────────────────

/// Bytes actually reclaimed this run (counter). `0` on a dry-run / no-op.
pub const DISK_RECLAIM_BYTES_FREED: &str = "simard.disk.reclaim.bytes_freed";
/// Paths actually removed this run (counter), tagged by [`ATTR_KIND`].
pub const DISK_RECLAIM_PATHS_REMOVED: &str = "simard.disk.reclaim.paths_removed";
/// Candidates a hard rail refused (counter), tagged by [`ATTR_REASON`]. Every
/// increment is a path that was **not** deleted (the human-review list).
pub const DISK_RECLAIM_CANDIDATES_SKIPPED: &str = "simard.disk.reclaim.candidates_skipped";
/// Home-partition `%-used` measured at the start of the run (gauge, 0–100).
pub const DISK_RECLAIM_USED_PCT_BEFORE: &str = "simard.disk.reclaim.used_pct_before";
/// Home-partition `%-used` after the run (gauge, 0–100).
pub const DISK_RECLAIM_USED_PCT_AFTER: &str = "simard.disk.reclaim.used_pct_after";

// ── Attribute keys ──────────────────────────────────────────────────────────

/// Attribute key: outcome/result discriminator (`ok`/`parse_fail`, parse
/// outcome, etc.).
pub const ATTR_RESULT: &str = "result";
/// Attribute key: brain decision phase.
pub const ATTR_PHASE: &str = "phase";
/// Attribute key: engineer exit outcome.
pub const ATTR_OUTCOME: &str = "outcome";
/// Attribute key: memory node/edge type.
pub const ATTR_TYPE: &str = "type";
/// Attribute key: token direction.
pub const ATTR_DIR: &str = "dir";
/// Attribute key: token cache status.
pub const ATTR_CACHED: &str = "cached";
/// Attribute key: disk-reclaim run source (`daemon` \| `cli`).
pub const ATTR_SOURCE: &str = "source";
/// Attribute key: reclamation candidate kind (`tracked_worktree` \|
/// `orphan_dir` \| `stale_build_cache`).
pub const ATTR_KIND: &str = "kind";
/// Attribute key: reclamation reject reason (mirrors `RejectReason`).
pub const ATTR_REASON: &str = "reason";

/// Sentinel bucket an out-of-catalog attribute value is folded into.
pub const OTHER_BUCKET: &str = "other";
