---
title: No-progress root-cause resolution API reference
description: Reference for the OODA no-progress breaker's root-cause resolution — the `NoProgressClass` classification and its stable tokens, the `NoProgressWhy`/`Evidence` types, the `NoProgressWhyReasoner` agentic-enrichment trait, the extended `NoProgressResolution` ladder (auto-complete / heal / defer / guided-retry / escalate-with-why), the WHY-bearing block-reason contract, the additive `EvidenceSource::repo_present` and `dependency_goal_state` methods, and the one-shot guided-retry bound.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./no-progress-breaker-api.md
  - ./completion-evidence-gate-api.md
  - ./ooda-no-progress-why-recipe.md
  - ./recipe-brain-api.md
  - ./spawn-agent-for-goal.md
  - ./self-deploy-source-prep.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../concepts/perpetual-goal-no-progress-exemption.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../../src/goal_curation/no_progress_breaker.rs
  - ../../src/goal_curation/no_progress_why.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/goal_curation/completion_gate.rs
---

# No-progress root-cause resolution API reference

> **Status: implemented (issue #16).** The classification, WHY types, and the
> pure `class → resolution` mapping live in
> `src/goal_curation/no_progress_breaker.rs` and
> `src/goal_curation/no_progress_why.rs`. The side-effecting adapter that gathers
> evidence, investigates the root cause, and performs the transition / clone /
> defer / spawn / escalate is `apply_no_progress_breaker_investigated` in
> `src/ooda_loop/no_progress.rs`. The evidence extensions live in
> `src/goal_curation/completion_gate.rs`. The rustdoc on those items is the
> canonical API; the signatures below are kept in sync with it.

This page specifies the **root-cause resolution** layer added on top of the base
breaker. For the base breaker (threshold, sentinel constants, `NoProgressTracker`,
`heal_stale_no_progress_blocks`, the perpetual exemption) see the
[no-progress breaker API reference](./no-progress-breaker-api.md). For the
rationale see
[The no-progress breaker explains WHY and self-resolves before escalating](../concepts/no-progress-root-cause-resolution.md).

## Contents

- [Design invariants](#design-invariants)
- [`NoProgressClass` and stable tokens](#noprogressclass)
- [`Evidence` and `NoProgressWhy`](#evidence-and-noprogresswhy)
- [`NoProgressWhyReasoner`](#noprogresswhyreasoner)
- [Extended `NoProgressResolution` ladder](#extended-noprogressresolution)
- [Block-reason contract](#block-reason-contract)
- [`EvidenceSource` extensions](#evidencesource-extensions)
- [Adapter: the resolution driver](#adapter-the-resolution-driver)
- [One-shot guided-retry bound](#one-shot-guided-retry-bound)
- [Fail-closed error handling](#fail-closed-error-handling)
- [What is unchanged](#what-is-unchanged)

## Design invariants

1. **Deterministic routing.** Which ladder rung a stall takes is decided by
   deterministic evidence signals, never by an LLM. The breaker fires when the
   brain is failing, so it must not delegate its own recovery to the brain.
2. **Agentic enrichment only.** The optional `NoProgressWhyReasoner` produces the
   *human-readable WHY narrative* for an escalation; it never changes routing.
3. **Additive.** Every new method on `EvidenceSource` has a default body, so
   existing implementations and test fakes compile unchanged. New
   `NoProgressResolution` variants are added; the existing `MarkDone` / `Drop` /
   `Escalate` control flow for a normal goal is preserved.
4. **Fail-closed.** No branch ever *silently* blocks or *silently* completes; an
   uncertain signal downgrades to `UNCLEAR` and the goal retries.
5. **Backward-compatible marker.** The escalation reason still begins with the
   `🔒 [OODA-SAFEGUARD]` sentinel and preserves `{PREFIX}{n}` verbatim (so
   `is_no_progress_marker` — which now keys on the **prefix alone** —
   `safeguard_marker_count`, `heal_stale_no_progress_blocks`, and the overseer
   count-parser need **zero** changes). The WHY *replaces* the bare
   `needs human review` suffix rather than appending after it.

## `NoProgressClass`

The deterministic classification of *why* a goal reached the breaker threshold.
Each variant has a stable, screaming-kebab **token** that appears in the block
reason and in tests. The tokens are exported as constants so tests assert against
them without hard-coding string literals.

```rust
/// The deterministic root-cause classification of a stalled goal, computed at
/// the breaker threshold from evidence signals (NOT from the brain).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoProgressClass {
    /// Live artifacts satisfy the done-criteria (issues closed / PRs merged /
    /// deployed). Routes to auto-complete.
    AlreadyComplete,
    /// Work is tracked elsewhere / out of scope. Routes to drop.
    Obsolete,
    /// A machine-establishable precondition is absent (e.g. a governed repo was
    /// never cloned). Routes to self-heal + retry.
    MissingPrecondition,
    /// Blocked on a specific upstream goal / PR / issue that has not landed.
    /// Routes to defer (Paused).
    UpstreamDependency,
    /// The done-criteria are not expressed as anything the done-gate can check.
    /// Routes to a guided engineer.
    UnclearCriteria,
    /// No machine-resolvable cause found. Routes to a guided engineer, then a
    /// human.
    GenuinelyStuck,
}

/// Stable classification tokens (asserted by tests; embedded in block reasons).
pub const CLASS_ALREADY_COMPLETE: &str = "ALREADY-COMPLETE";
pub const CLASS_OBSOLETE: &str = "OBSOLETE";
pub const CLASS_MISSING_PRECONDITION: &str = "MISSING-PRECONDITION";
pub const CLASS_UPSTREAM_DEPENDENCY: &str = "UPSTREAM-DEPENDENCY";
pub const CLASS_UNCLEAR_CRITERIA: &str = "UNCLEAR-CRITERIA";
pub const CLASS_GENUINELY_STUCK: &str = "GENUINELY-STUCK";

impl NoProgressClass {
    /// The stable screaming-kebab token for this class.
    pub fn token(&self) -> &'static str;
}
```

## `Evidence` and `NoProgressWhy`

Structured, human-and-machine-readable evidence gathered for a classification.
Every self-resolving action and every escalation records the evidence it acted
on.

```rust
/// One piece of structured evidence supporting a classification — a live
/// artifact reference and its observed state, e.g. `issue #16 (CLOSED)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    /// The evidence kind, e.g. `"issue"`, `"pr"`, `"repo"`, `"dependency"`.
    pub kind: String,
    /// The artifact reference, e.g. `"#16"`, `"kgpacks-rs"`, an upstream goal id.
    pub reference: String,
    /// The observed state, e.g. `"CLOSED"`, `"MERGED"`, `"absent"`, `"OPEN"`.
    pub state: String,
}

impl Evidence {
    pub fn new(
        kind: impl Into<String>,
        reference: impl Into<String>,
        state: impl Into<String>,
    ) -> Self;
    /// A human-readable reference, e.g. `issue #16 (CLOSED)`.
    pub fn render(&self) -> String;
}

/// A classified WHY plus the evidence behind it. Produced by a
/// `NoProgressWhyReasoner`; consumed by `resolution_for_why` to pick the ladder
/// rung and by `no_progress_blocked_reason_with_why` to author a WHY-bearing
/// block reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoProgressWhy {
    pub class: NoProgressClass,
    pub evidence: Vec<Evidence>,
}

impl NoProgressWhy {
    pub fn new(class: NoProgressClass, evidence: Vec<Evidence>) -> Self;
    /// Comma-joined evidence, or the explicit `(none)` sentinel when empty —
    /// never an empty string, so an escalation reason always reads coherently.
    pub fn render_evidence(&self) -> String;
    /// The single most-specific blocking reference for an `UPSTREAM-DEPENDENCY`
    /// defer: the first evidence's `reference`, falling back to its render.
    pub fn blocking_ref(&self) -> String;
}
```

## `NoProgressWhyReasoner`

The **investigation seam**: given a stalled goal, classify *why* it made no
shippable progress and gather the evidence. Injected so the breaker's
investigation is exercised hermetically (tests inject a fake). The production
implementation
(`DeterministicNoProgressReasoner`, in `src/ooda_loop/no_progress.rs`) is
**deterministic** — it routes from evidence signals the daemon gathers without
the brain, because the breaker fires precisely when the agentic loop is *failing*
on the goal. The [`ooda-no-progress-why` recipe](./ooda-no-progress-why-recipe.md)
is the optional agentic *narrative* enrichment for the escalation issue; it never
changes routing.

```rust
/// The investigation seam. On `Err`, the caller FAILS CLOSED — takes no terminal
/// action (neither blocks nor completes), surfaces the error, and lets the goal
/// retry — never silently swallowing an unknown root cause.
pub trait NoProgressWhyReasoner: Send + Sync {
    fn investigate(&self, goal: &ActiveGoal) -> SimardResult<NoProgressWhy>;
}
```

## Extended `NoProgressResolution`

The resolution the breaker selects at the threshold. The three original variants
are unchanged; three are added. Every terminal variant carries the payload its
side effect needs.

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoProgressResolution {
    /// Below the threshold — record the no-op and let the goal retry next cycle.
    Continue,

    /// ALREADY-COMPLETE — the caller marks the goal `Completed`. No block.
    MarkDone,

    /// OBSOLETE — the caller drops the goal from the active board, carrying the
    /// human-readable reason.
    Drop { reason: String },

    /// MISSING-PRECONDITION — the caller establishes the precondition (clones the
    /// repo, or spawns an engineer to), resets the no-action counter, and lets
    /// the goal retry. No block.
    Heal { why: NoProgressWhy },

    /// UPSTREAM-DEPENDENCY — the caller sets the goal `Paused`, records
    /// `blocking_ref` as a `dependency` WipRef, and lets the auto-clear pass
    /// resume it when the upstream resolves. No block.
    Defer {
        blocking_ref: String,
        evidence: Vec<Evidence>,
    },

    /// UNCLEAR-CRITERIA / GENUINELY-STUCK, first occurrence — the caller spawns
    /// ONE guided engineer (via the shared dispatch) with `task` embedding the
    /// WHY, and records that the goal has spent its guided retry. No block yet.
    SpawnEngineer { task: String, why: NoProgressWhy },

    /// Unresolvable, or a goal that stalled AGAIN after its guided retry — the
    /// caller sets `GoalProgress::Blocked` to `blocked_reason` (which carries the
    /// WHY, see below) and files `issue_title` / `issue_body`.
    Escalate {
        blocked_reason: String,
        issue_title: String,
        issue_body: String,
    },
}
```

The pure `class → resolution` map is
`resolution_for_why(consecutive, why, guided_retry_used)` (in
`src/goal_curation/no_progress_breaker.rs`). `guided_retry_used` is the goal's
persisted one-shot flag: an `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` stall yields
`SpawnEngineer` the first time and `Escalate` (WITH the WHY) only once the retry
is spent.

## Block-reason contract

When — and only when — a block is unavoidable (`Escalate`), the reason string is
rendered by a new pure function that **appends** the WHY to the existing
sentinel:

```rust
/// Render a WHY-bearing escalation reason: the safeguard sentinel with the
/// classified root cause and its evidence attached, so a human block is never
/// bare.
///
/// Shape: `{PREFIX}{consecutive} consecutive no-action cycles; why={TOKEN} evidence=[…]`
///
/// e.g.
/// ```text
/// 🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 4 consecutive no-action cycles; why=GENUINELY-STUCK evidence=[pr #7 (OPEN)]
/// ```
///
/// The legacy renderer `no_progress_blocked_reason(consecutive)` is retained for
/// existing callers/tests.
pub fn no_progress_blocked_reason_with_why(consecutive: u32, why: &NoProgressWhy) -> String;
```

Guarantees, verified by tests:

- The reason **starts with** `NO_PROGRESS_BLOCKED_PREFIX` → `is_no_progress_marker`
  is still `true` (recognition now keys on the prefix **alone**, since the WHY
  replaces the bare `needs human review` suffix).
- Stripping the prefix still leaves the leading digits `{consecutive}`, so
  `safeguard_marker_count` and `heal_stale_no_progress_blocks` are undisturbed.
- The reason **contains** the class token (`why=<TOKEN>`) and the evidence, and
  is strictly richer than the bare `no_progress_blocked_reason` → it is **never a
  bare "needs human review"**.

## `EvidenceSource` extensions

Two methods are added to the existing
[`EvidenceSource`](./completion-evidence-gate-api.md) trait, each with a
**default body** so every existing implementation and test fake keeps compiling.
`GhCliEvidenceSource` overrides `repo_present` with a real filesystem lookup.

```rust
pub trait EvidenceSource: Send + Sync {
    fn any_pr_merged(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    fn issue_closed(&self, goal: &ActiveGoal) -> SimardResult<bool>;
    fn is_deployed(&self, goal: &ActiveGoal) -> SimardResult<bool>;

    /// NEW: Is the goal's governed target repository present in the workspace?
    /// Backs MISSING-PRECONDITION. Default: `Ok(true)` (a source that cannot tell
    /// must not fabricate a missing precondition).
    fn repo_present(&self, goal: &ActiveGoal) -> SimardResult<bool> {
        let _ = goal;
        Ok(true)
    }

    /// NEW: State of the goal's declared upstream dependency, if any. Backs
    /// UPSTREAM-DEPENDENCY. Default: `Ok(DependencyState::None)`.
    fn dependency_goal_state(&self, goal: &ActiveGoal) -> SimardResult<DependencyState> {
        let _ = goal;
        Ok(DependencyState::None)
    }
}

/// The resolution state of a goal's upstream dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencyState {
    /// No declared/known upstream dependency.
    None,
    /// A specific upstream is still open — the goal is waiting on it.
    Pending { blocking_ref: String },
    /// The previously-blocking upstream has landed — a deferred goal may resume.
    Resolved { blocking_ref: String },
}
```

An `Err` from either method is treated as `GENUINELY-STUCK` by the deterministic
classifier (fail closed — never self-heal or self-complete on unknown state) and
is logged.

## Adapter: the resolution driver

The side-effecting driver in `src/ooda_loop/no_progress.rs` gains the inputs the
new rungs need. It runs in the OODA **curate** phase and performs all effects
(board mutation, clone, defer, engineer spawn, issue filing); the pure policy in
`no_progress_breaker.rs` performs none.

```rust
/// Drive the investigated no-progress breaker over one cycle's outcomes. Every
/// dependency is injected so the whole ladder is hermetically testable. Runs in
/// the OODA curate phase and performs all effects; the pure policy in
/// `no_progress_breaker.rs` performs none.
pub(crate) fn apply_no_progress_breaker_investigated(
    state: &mut OodaState,
    outcomes: &[ActionOutcome],
    evidence: &dyn EvidenceSource,
    reasoner: &dyn NoProgressWhyReasoner,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    mutation_guard: &mut GitHubMutationGuard,
    authorization: &AutonomousGitHubAuthorization,
    threshold: u32,
) -> Result<NoProgressBreakerReport, GitHubMutationError>;
```

The `SpawnEngineer` rung reuses the **same** `dispatch_spawn_engineer` the Act
phase uses: the production `QueueingEngineerDispatcher` collects the requests
during the breaker pass (which holds `&mut OodaState`), and the cycle drains them
and dispatches through `dispatch_spawn_engineer(action, state, goal_id, task,
brain, repo_root)` once the state borrow is free — not a parallel spawner. The
production reasoner is `DeterministicNoProgressReasoner` and the production healer
is `CloneRepoHealer`.

The report gains additive, default-derived fields so each rung is observable and
assertable:

```rust
pub(crate) struct NoProgressBreakerReport {
    pub marked_done: Vec<String>,           // ALREADY-COMPLETE → Completed
    pub dropped: Vec<String>,               // OBSOLETE → dropped
    pub escalated: Vec<String>,             // last rung → Blocked-with-WHY
    pub healed: Vec<String>,                // MISSING-PRECONDITION → clone+retry
    pub deferred: Vec<String>,              // UPSTREAM-DEPENDENCY → Paused
    pub engineer_spawned: Vec<String>,      // UNCLEAR/STUCK → one guided engineer
    pub auto_cleared: Vec<String>,          // Paused → NotStarted (upstream landed)
    pub investigation_errors: Vec<String>,  // reasoner Err → fail closed, no action
    pub perpetual_idled: Vec<String>,       // standing goals (unchanged)
}
```

`fired()` counts a disruptive action (`marked_done`, `dropped`, `escalated`,
`healed`, `deferred`, `engineer_spawned`); `auto_cleared`, `investigation_errors`,
and `perpetual_idled` are informational and do **not** by themselves make
`fired()` true (a self-resolving / fail-closed / exempt cycle is normal
operation, not a fault).

Reuse (no parallel machinery is built):

- **Auto-complete / drop / defer** reuse the goal-store transitions and
  `GoalProgress`.
- **Clone** reuses `gh repo clone` into the workspace `$HOME/src/<repo>` path
  (`CloneRepoHealer`).
- **Guided engineer** reuses `dispatch_spawn_engineer` — the **same** dispatch the
  OODA act phase uses (see [spawn agent for goal](./spawn-agent-for-goal.md)).
- **Issue mutation** uses the shared `GitHubMutationGuard`. The recurring goal
  and its full lineage must be eligible; `recurring_goal_reblock` never bypasses
  provenance, restart reconciliation, or the cycle budget.

## One-shot guided-retry bound

`SpawnEngineer` crosses the curate→act boundary and, unbounded, would re-create
the very livelock the breaker exists to stop. It is bounded to **exactly one**
guided retry per goal:

- A per-goal `guided_retries` set is tracked inside `NoProgressTracker`
  (persisted with the goal board, so the bound survives a daemon restart).
- The first time a goal classifies `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` at the
  threshold, `resolution_for_why` returns `SpawnEngineer`; the adapter spawns one
  engineer with the WHY in its task string, calls `mark_guided_retry`, resets the
  counter for a fresh window, and leaves the goal on the board.
- If the goal returns to the threshold **again** with the flag set — or the spawn
  itself is rejected — the breaker returns `Escalate` with the WHY. Worst case is
  bounded: **≤ 1 extra engineer session** before a human is involved.
- Genuine progress (`record_progress`) clears the flag, so a *future* stall earns
  a fresh guided retry.

## Fail-closed error handling

| Failure | Behaviour |
| --- | --- |
| Reasoner `Err` | Take NO terminal action; record in `investigation_errors`, preserve the counter, retry next cycle. Logged at `error`. Never a silent block or completion. |
| `repo_present` / `dependency_goal_state` `Err` (deterministic reasoner) | Downgrade to `GENUINELY-STUCK`; never self-heal/self-complete on unknown state. Logged. |
| Clone (`Heal`) failure | `Escalate` with the `MISSING-PRECONDITION` WHY + a `clone-error` `Evidence`. |
| `dispatch_spawn_engineer` rejected (`SpawnEngineer`) | Mark the guided retry spent; escalate with the WHY on the next stall. |
| Issue mutation failure | Return the fatal guard error and abort the owning cycle. The blocked state remains visible; no success-shaped continuation is allowed. |
| Done-gate error on `MarkDone` check | Not `ALREADY-COMPLETE`; fall through the ladder (never complete on an error). |

## What is unchanged

- Base-breaker constants and most helpers — `NO_PROGRESS_BREAKER_THRESHOLD`,
  `NO_PROGRESS_BLOCKED_PREFIX`, `NO_PROGRESS_BLOCKED_SUFFIX` (retained for the
  legacy `no_progress_blocked_reason` renderer), `no_progress_blocked_reason`,
  `NoProgressTracker`, `safeguard_marker_count` — are unchanged. See the
  [base API reference](./no-progress-breaker-api.md). `is_no_progress_marker` is
  loosened to key on the prefix **alone** (so it recognises both the legacy and
  the WHY-bearing reason); every call site is unaffected.
- `heal_stale_no_progress_blocks` and the overseer `EscalateBlockedGoal` parser
  recognise the WHY-bearing reason unchanged (they key on the prefix).
- The **perpetual exemption** runs before classification, so standing goals are
  never routed down the ladder.
- `simard goal unblock` / `unblock-all` still clear a safeguard block by hand for
  the (now rare) manual cases.

## See also

- [Concept: the breaker explains WHY and self-resolves before escalating](../concepts/no-progress-root-cause-resolution.md)
- [No-progress breaker API reference](./no-progress-breaker-api.md) — the base breaker this layer sits on.
- [The `ooda-no-progress-why` recipe reference](./ooda-no-progress-why-recipe.md) — the optional agentic narrator.
- [Completion-evidence gate API](./completion-evidence-gate-api.md) — the done-gate the classifier reuses.
- [Diagnose a no-progress block](../howto/diagnose-a-no-progress-block.md) — the operator runbook.
