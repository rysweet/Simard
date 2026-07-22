---
title: "Reference: Overseer PR-Reaper Policy API"
description: >
  The API contract for the reaper_policy layer: the ReaperDecision value type,
  the pure fail-closed evaluate(...) validator and its tighten-only guarantee,
  the ReaperThresholds config (stale 14d, CONFLICTING 7d, title similarity
  >=0.85 + file overlap) and its SIMARD_OVERSEER_REAPER_* resolvers with clamps,
  the intended merge_queue_observe wiring ahead of Guardrails::admit (pending),
  the deterministic
  survivor-selection rule, the fail-closed parse of mergeable/timestamps, the
  argv-safety and flag-injection assertions, the telemetry surface, and the
  regression test list.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: partial
related:
  - ../concepts/overseer-pr-reaper-policy.md
  - ./agentic-merge-queue-reasoning-api.md
  - ./claim-reaper-api.md
  - ./cross-repo-merge-authority.md
  - ./telemetry-metrics.md
  - ../howto/configure-overseer-pr-reaper.md
---

# Reference: Overseer PR-Reaper Policy API

> **Status: policy layer implemented and unit-tested; live wiring pending
> ([#4423](https://github.com/rysweet/Simard/issues/4423)).**
> The `evaluate(...)` contract and value types below are shipped and fully
> unit-tested, but they are **not yet routed into the live merge-queue path** —
> the `merge_queue_observe.rs` / `signal.rs` dispatch still hands dispositions to
> the existing `MergeAuthority`-gated interventions directly. Sections that
> describe the observe wiring and telemetry emission describe the **intended**
> integration, not current runtime behaviour.
> Primary source:
> [`src/overseer/reaper_policy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/reaper_policy.rs);
> intended wiring point:
> [`src/overseer/merge_queue_observe.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_queue_observe.rs);
> thresholds in
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs).
> Conceptual overview:
> [Overseer PR-Reaper Policy](../concepts/overseer-pr-reaper-policy.md).
> Registered as `pub mod reaper_policy;` in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> (after `claim_reaper`).

## Overview

`reaper_policy` is a **pure, deterministic, fail-closed** post-parse validation
layer. It takes an agent-proposed PR disposition plus the objective PR facts and
returns a decision that can only **tighten** the agent's proposal before it
reaches the intervention gate. It performs no I/O and issues no `gh` command.

## Data types

```rust
/// The objective, model-independent facts about one open PR, extracted from the
/// roster-trust-boundary `ReasonedPr` and the raw `gh` listing. All fields are
/// parsed once at the boundary; downstream policy is pure over this struct.
pub struct PrFacts {
    pub repo: String,
    pub number: u32,
    /// Last update time. `None` when the source timestamp was unparseable.
    /// The overseer module uses `chrono` throughout (not the `time` crate).
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Parsed mergeable state. `Unknown` when the source token was unrecognized.
    pub mergeable: MergeableState,
    /// Stopword-normalized title used for similarity scoring.
    pub normalized_title: String,
    /// Set of changed file paths (for near-duplicate file-overlap).
    pub changed_files: BTreeSet<String>,
    /// Agent-proposed duplicate-of target, when disposition == Duplicate.
    pub duplicate_of: Option<u32>,
}

/// Fail-closed parse of the `gh` `mergeable` field.
pub enum MergeableState {
    Mergeable,
    Conflicting,
    /// Unknown / unparseable ⇒ never eligible for auto-close.
    Unknown,
}

/// The tightened outcome. Never more aggressive than the agent proposal.
pub enum ReaperDecision {
    /// Facts do not meet any threshold ⇒ no intervention.
    NoAction,
    /// Post a non-destructive review comment (RiskClass::MergeAuthority,
    /// notify-only). This is the DEFAULT visible action.
    Flag(FlagStaleReason),
    /// Propose a destructive close of `number` as a duplicate of `survivor`.
    /// Still gated behind allow_verify_merge; the policy itself never closes.
    CloseDuplicate { number: u32, survivor: u32, reason: DuplicateReason },
}

/// Why a PR was flagged, for the intervention note + telemetry (scalars/ids).
/// `DuplicateNotClosable` is emitted when a proposed `CloseDuplicate` is
/// downgraded to a flag (e.g. the destructive gate is closed), so the operator
/// note and telemetry describe the *real* reason instead of mislabeling a
/// duplicate as "stale, no update".
pub enum FlagStaleReason { StaleNoUpdate, LongConflicting, DuplicateNotClosable }

pub enum DuplicateReason { TitleAndFileOverlap }
```

## The validator

```rust
/// Deterministically decide the reaper disposition for one PR.
///
/// Tighten-only contract: the returned `ReaperDecision` is never more
/// aggressive than `proposed`. Specifically:
///   * `proposed == Duplicate` may return `CloseDuplicate`, `Flag`, or `NoAction`.
///   * `proposed == Stale`     may return `Flag` or `NoAction` (never Close).
///   * any other proposal      returns `NoAction`.
///
/// Fail-closed: `MergeableState::Unknown` or `updated_at == None` makes a PR
/// ineligible for close (and for CONFLICTING-based flagging).
pub fn evaluate(
    proposed: PrDisposition,
    facts: &PrFacts,
    peers: &[PrFacts],       // candidate duplicate survivors in the same repo
    thresholds: &ReaperThresholds,
    now: chrono::DateTime<chrono::Utc>,
    destructive_allowed: bool, // mirrors Guardrails.allow_verify_merge
) -> ReaperDecision;
```

### Decision rules

| Proposed | Condition met | Result |
| --- | --- | --- |
| `Stale` | `now - updated_at > stale_days` **or** (`mergeable == Conflicting` and conflicting age `> conflicting_days`) | `Flag(StaleNoUpdate \| LongConflicting)` |
| `Stale` | timestamp `None` / condition not met | `NoAction` |
| `Duplicate` | title similarity `>= similarity` **and** `changed_files` overlap non-empty **and** a valid survivor exists **and** `destructive_allowed` | `CloseDuplicate { survivor }` |
| `Duplicate` | overlap met but `destructive_allowed == false` | `Flag(DuplicateNotClosable)` (downgraded, non-destructive) |
| `Duplicate` | similarity met but **no** file overlap | `NoAction` (never close on title alone) |
| `Duplicate` | `mergeable == Unknown` or missing survivor | `NoAction` (fail-closed) |
| anything else | — | `NoAction` |

### Survivor selection (griefing-resistant)

When a duplicate pair is confirmed, the **survivor** is chosen deterministically
from `{self} ∪ peers` that share the overlap: the PR with the **lowest
`number`**. `PrFacts` carries `number` (monotonic, assigned in creation order)
but not `created_at`, so the PR number is the age proxy — a lower number always
means the earlier PR. The *other* (higher-numbered, later) PR is the close
candidate. This makes it impossible for an attacker who opens a near-duplicate
*after* a legitimate PR to cause the legitimate (lower-numbered) PR to be closed.

### Tighten-only invariants

| # | Invariant | Enforced by |
| --- | --- | --- |
| T1 | Output is never more aggressive than `proposed`. | Match arms above; no arm upgrades. |
| T2 | `evaluate` never lowers `RiskClass`. | It emits `ReaperDecision`, not a class; wiring keeps `CloseDuplicatePr` at `MergeAuthority`. |
| T3 | No close without file overlap. | `Duplicate` close arm requires non-empty `changed_files` intersection. |
| T4 | No close without the destructive opt-in. | `destructive_allowed == false` downgrades to `Flag`. |
| T5 | Fail-closed on bad facts. | `Unknown` mergeable / `None` timestamp ⇒ `NoAction`/no-close. |
| T6 | Pure. | No I/O, no clock read (`now` injected), no globals. |

## Wiring (intended — not yet implemented)

> **This section describes the planned integration, not current behaviour.** The
> code snippet below does **not** yet exist in `merge_queue_observe.rs`; it is the
> reference design for the pending wiring step. Today `signal.rs` lifts `Stale` /
> `Duplicate` dispositions straight to `StalePrDetected` / `DuplicatePrDetected`
> signals without calling `reaper_policy::evaluate`.

Once wired, in
[`merge_queue_observe.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_queue_observe.rs),
each parsed `ReasonedPr` whose disposition is `Stale` or `Duplicate` is routed
through `reaper_policy::evaluate` **before** the corresponding intervention is
constructed and handed to `Guardrails::admit`:

```rust
let decision = reaper_policy::evaluate(
    reasoned.disposition, &facts, &peers, &thresholds, now, guardrails.allow_verify_merge,
);
// One decision counter per PR, tagged by decision (see Telemetry).
let decision_label = match &decision {
    ReaperDecision::NoAction => "no_action",
    ReaperDecision::Flag(_) => "flag",
    ReaperDecision::CloseDuplicate { .. } => "close_duplicate",
};
crate::telemetry::counter_add(
    names::OVERSEER_REAPER_DECISION, 1, &[(names::ATTR_DECISION, decision_label)],
);
match decision {
    ReaperDecision::NoAction => { /* nothing admitted */ }
    ReaperDecision::Flag(reason) => admit(Intervention::FlagStalePr { repo, pr, note: reason.note() }),
    ReaperDecision::CloseDuplicate { number, survivor, .. } =>
        admit(Intervention::CloseDuplicatePr { /* integer ids only */ }),
}
```

`Guardrails::admit` is unchanged: `FlagStalePr` and `CloseDuplicatePr` remain
`RiskClass::MergeAuthority`, admitted only when `allow_verify_merge` is set
(otherwise notify-only). The reaper policy narrows *which* proposals reach the
gate; it does not change the gate.

## Configuration

Resolvers in [`config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs)
follow the `*_from(lookup)` pattern:

```rust
pub struct ReaperThresholds {
    pub stale_days: i64,        // default 14
    pub conflicting_days: i64,  // default 7
    pub similarity: f64,        // default 0.85, clamped to [0.0, 1.0]
}

pub const SIMARD_OVERSEER_REAPER_STALE_DAYS_ENV: &str        = "SIMARD_OVERSEER_REAPER_STALE_DAYS";
pub const SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS_ENV: &str  = "SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS";
pub const SIMARD_OVERSEER_REAPER_SIMILARITY_ENV: &str        = "SIMARD_OVERSEER_REAPER_SIMILARITY";

pub const DEFAULT_REAPER_STALE_DAYS: i64       = 14;
pub const DEFAULT_REAPER_CONFLICTING_DAYS: i64 = 7;
pub const DEFAULT_REAPER_SIMILARITY: f64       = 0.85;

pub fn reaper_thresholds_from(lookup: impl Fn(&str) -> Option<String>) -> ReaperThresholds;
pub fn reaper_thresholds() -> ReaperThresholds {
    reaper_thresholds_from(|k| std::env::var(k).ok())
}
```

| Variable | Default | Clamp | Effect |
| --- | --- | --- | --- |
| `SIMARD_OVERSEER_REAPER_STALE_DAYS` | `14` | `>= 1` | Days without update before a `Stale` proposal may be flagged. |
| `SIMARD_OVERSEER_REAPER_CONFLICTING_DAYS` | `7` | `>= 1` | Days a PR must be `CONFLICTING` before it may be flagged. |
| `SIMARD_OVERSEER_REAPER_SIMILARITY` | `0.85` | `[0.0, 1.0]` | Minimum normalized-title similarity for a near-duplicate. |

Resolution is fail-safe: unset / empty / unparseable / out-of-range ⇒ the
conservative default (or clamped bound) with a `WARN`. The `>= 1` on the day
thresholds is a **clamp floor** (a hard lower bound that prevents a 0-day
threshold), not a recommended value — the *recommended* values are the 14/7
defaults. There is **no** env knob that opens destructive closes — that is only
`Guardrails.allow_verify_merge`.

## Security & argv safety

| Property | Guarantee |
| --- | --- |
| Untrusted PR text | Flows only as **positional** argv (intervention builders / `run_gh`), never a shell string. |
| Close-comment template | Interpolates **only integer PR ids**; no PR-controlled free text in destructive argv. |
| Flag-injection | Reaper path asserts argv can never contain `--admin` / `--no-verify`. |
| Destructive authority | `reaper_policy` issues no `gh pr close`; `CloseDuplicatePr` stays `MergeAuthority`, admitted only under `allow_verify_merge`. |
| Logging | Scalars + PR ids only; echoed PR text truncated (log-flood DoS); the `gh` token is never read or logged. |

## Telemetry

> **Emission pending wiring.** The counter *constants* below exist in `names.rs`,
> but nothing increments them yet — the reaper is not on the live path. The
> "When" table describes the intended emission once wired.

Metric names follow the house **dotted OTel** convention (as in `names.rs`, e.g.
`simard.daemon.cycle`). The internal constant *value* is the dotted name; the
`_total` suffix and `{decision=…}` label shown below are the **Prometheus
exporter's rendering**, not the internal name.

| Internal name (`names::`) | Constant value | Prometheus-exporter view |
| --- | --- | --- |
| `OVERSEER_REAPER_DECISION` | `simard.overseer.reaper_decision` | `simard_overseer_reaper_decision_total{decision="no_action"\|"flag"\|"close_duplicate"}` |
| `OVERSEER_REAPER_DOWNGRADED` | `simard.overseer.reaper_downgraded` | `simard_overseer_reaper_downgraded_total` |

Attributes are passed via the `counter_add(name, 1, &[(&str, &str)])` form using
`names::ATTR_*` constants — the decision counter uses a new
`names::ATTR_DECISION` (`"decision"`), matching the existing `ATTR_OUTCOME` /
`ATTR_REASON` pattern.

| Increment | When |
| --- | --- |
| `OVERSEER_REAPER_DECISION{decision=…}` | Once per PR evaluated, tagged by the resulting decision. |
| `OVERSEER_REAPER_DOWNGRADED` | When a proposed `CloseDuplicate` is downgraded to `Flag` / `NoAction`. |

Note the intended double-count: a proposed close that the closed destructive gate
downgrades increments **both** `OVERSEER_REAPER_DECISION{decision="flag"}` **and**
`OVERSEER_REAPER_DOWNGRADED`, so `downgraded > 0` with
`decision="close_duplicate" == 0` is the dry-run signature.

Plus a scalar/ID-only `DEBUG`/`INFO` `tracing` span per decision (repo, PR id,
decision, reason). No `print!` family calls.

## Regression tests

Unit tests live in
[`src/overseer/tests_merge_queue_reasoning.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_merge_queue_reasoning.rs):

| Test | Asserts |
| --- | --- |
| `evaluate_tightens_never_upgrades` | `Stale` never returns `CloseDuplicate`; unknown dispositions ⇒ `NoAction`. |
| `stale_flagged_only_past_threshold` | Update age `> stale_days` ⇒ `Flag`; within threshold ⇒ `NoAction`. |
| `long_conflicting_flagged` | `CONFLICTING` for `> conflicting_days` ⇒ `Flag(LongConflicting)`. |
| `no_close_on_title_similarity_alone` | Similarity `>= 0.85` but no file overlap ⇒ `NoAction`. |
| `duplicate_close_requires_overlap_and_optin` | Overlap + `destructive_allowed` ⇒ `CloseDuplicate`; overlap without opt-in ⇒ `Flag`. |
| `fail_closed_on_unknown_mergeable_or_timestamp` | `Unknown` mergeable / `None` timestamp ⇒ never a close. |
| `survivor_is_lowest_numbered_pr` | Duplicate survivor is the lowest-numbered (earliest) PR; the later PR is the close candidate (griefing resistance). |
| `dry_run_gate_default_is_notify_only` | With `allow_verify_merge=false`, no destructive `CloseDuplicatePr` is admitted. |
| `reaper_close_argv_never_contains_admin_or_no_verify` | Built close argv contains neither `--admin` nor `--no-verify`. |
| `thresholds_resolver_defaults_and_clamps` | Resolver returns defaults when unset and clamps out-of-range values. |
