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

// ── Enrichment observability — simard.enrichment.* (#2942) ──────────────────

/// One instrumented enrichment decision. Attribute `attached` = `true` | `false`
/// (the memory reader resolved to `Some` vs degraded to `None`). The
/// `attached` split is the attach-rate numerator/denominator
/// (`attach_rate = decisions{attached=true} / decisions{*}`), recorded only for
/// the *expected* population (turns where enrichment was configured).
pub const ENRICHMENT_DECISIONS: &str = "simard.enrichment.decisions";
/// One reader-launch degrade. Attribute `reason` = `memory_ipc` |
/// `knowledge_launch` — the concrete cause so an operator sees which reader is
/// down.
pub const ENRICHMENT_DEGRADED: &str = "simard.enrichment.degraded";
/// Rendered enrichment-block size injected per decision (histogram: count+sum →
/// average bytes/decision at zero attribute cardinality).
pub const ENRICHMENT_PREAMBLE_BYTES: &str = "simard.enrichment.preamble_bytes";
/// Facts rendered into the preamble per decision (histogram: count+sum → avg
/// facts/decision).
pub const ENRICHMENT_FACTS_INJECTED: &str = "simard.enrichment.facts_injected";
/// Procedures rendered into the preamble per decision (histogram: count+sum →
/// avg procedures/decision).
pub const ENRICHMENT_PROCEDURES_INJECTED: &str = "simard.enrichment.procedures_injected";

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

// ── Cognitive threads — simard.thread.<id>.* (#4786) ────────────────────────
//
// Per-thread observability. Thread identity is embedded in the metric NAME
// (`simard.thread.<id>.<suffix>`) rather than an attribute value: the scheduler
// hosts ~15 threads, which on a single attribute key would breach the registry's
// `MAX_VALUES_PER_KEY` (16) cardinality cliff and fold into `other`; embedding in
// the name yields one clean series per (thread, suffix). Every series is emitted
// with an EMPTY attribute set. Build a full name with [`thread_metric_name`].

/// Metric-name prefix for every per-thread series (`simard.thread.`).
pub const THREAD_METRIC_PREFIX: &str = "simard.thread.";

/// Build a per-thread metric/span name: `simard.thread.<id>.<suffix>`.
///
/// The single source of truth for the per-thread naming scheme, shared by the
/// emitting telemetry seam (`cognitive_threads::telemetry`) and the reading
/// oversight rail (`overseer::thread_oversight`) so the two can never drift.
/// `id` and `suffix` are compile-time constants at every call site (SR-11).
pub fn thread_metric_name(id: &str, suffix: &str) -> String {
    format!("{THREAD_METRIC_PREFIX}{id}.{suffix}")
}
/// Suffix: every scheduler attempt to run the thread (counter).
pub const THREAD_SUFFIX_RUNS: &str = "runs";
/// Suffix: successful runs (counter). Every scheduled run terminates as either
/// a success or a failure, so `successes + failures == runs`; the success rate
/// is `successes / runs`.
pub const THREAD_SUFFIX_SUCCESSES: &str = "successes";
/// Suffix: failed/errored runs (counter). Success rate is derivable.
pub const THREAD_SUFFIX_FAILURES: &str = "failures";
/// Suffix: per-run wall-clock duration in seconds (histogram).
pub const THREAD_SUFFIX_DURATION_SECONDS: &str = "duration_seconds";
/// Suffix: Unix epoch (seconds) of the last completed run (gauge). Liveness:
/// `now - last_run_epoch` is the last-run age the Overseer derives.
pub const THREAD_SUFFIX_LAST_RUN_EPOCH: &str = "last_run_epoch";
/// Suffix: Unix epoch (seconds) of the next scheduled run (gauge). The cadence /
/// staleness seam the Overseer reads to detect a stalled thread.
pub const THREAD_SUFFIX_NEXT_RUN_EPOCH: &str = "next_run_epoch";
/// Suffix: `1` while a tick is in flight, `0` otherwise (gauge).
pub const THREAD_SUFFIX_ACTIVE: &str = "active";

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
/// Attribute key: enrichment memory-reader attach state (`true` | `false`).
pub const ATTR_ATTACHED: &str = "attached";
/// Attribute key: disk-reclaim run source (`daemon` \| `cli`).
pub const ATTR_SOURCE: &str = "source";
/// Attribute key: reclamation candidate kind (`tracked_worktree` \|
/// `orphan_dir` \| `stale_build_cache`).
pub const ATTR_KIND: &str = "kind";
/// Attribute key: shared reason discriminator — enrichment degrade reason
/// (`memory_ipc` | `knowledge_launch`) or reclamation reject reason
/// (mirrors `RejectReason`).
pub const ATTR_REASON: &str = "reason";

/// Sentinel bucket an out-of-catalog attribute value is folded into.
pub const OTHER_BUCKET: &str = "other";
