# Reference: OODA Per-Goal-Cycle API

Types: `src/ooda_brain/mod.rs`
Context-gather rail: `src/ooda_brain/context.rs` (`gather_per_goal_cycle_ctx`)
Driver loop: `src/ooda_loop/cycle.rs`
Production impl: `src/ooda_brain/recipe_brain.rs`
Fallback impl: `src/ooda_brain/fallback.rs`

This is the Rust surface of the
[agentic per-goal, per-cycle decision](../concepts/agentic-per-goal-per-cycle.md).
It is a **sibling** of the engineer-lifecycle reasoner
([`decide_engineer_lifecycle`](ooda-brain-api.md)) — same shape (context-gather →
one recipe call → pure state rail → recorded judgment), different purpose. It is
invoked for **every** goal on `board.active`, every cycle.

## `PerGoalCycleCtx`

The durable, per-goal context handed to the reasoner. It carries **durable goal
state** — never a live worker's mere presence. The three demoted imperative
deciders appear here only as **read-only inputs**.

```rust
/// Durable per-goal context for one per-cycle reasoning decision.
///
/// Built by `gather_per_goal_cycle_ctx` in `context.rs`. Every field is a
/// best-effort snapshot; gathering never fails a cycle (total defaults).
pub struct PerGoalCycleCtx {
    // --- Durable goal identity & state ---
    pub goal_id: String,
    pub goal_description: String,
    pub goal_status: String,        // e.g. "Active", "Blocked(reason)"
    pub cycle_number: u32,

    // --- Durable history & in-flight work (NOT live-worktree presence) ---
    pub history_summary: String,    // recent outcomes / last N cycle results
    pub effect_jobs_in_flight: u32, // durable effect-dispatch jobs still open
    pub open_pr_refs: Vec<String>,  // PRs opened for this goal, awaiting CI/merge
    pub last_outcomes: Vec<String>, // last few ActionOutcome details
    pub wip_ref_count: u32,         // in-flight work-in-progress refs held

    // --- Worker / claim facts (facts, not verdicts) ---
    pub worker_present: bool,       // a *verified-live* engineer exists right now (liveness-checked, not bare map membership — see below)
    pub worker_log_tail: String,    // last ~8 KB of the worker log, secrets redacted

    // --- The three DEMOTED imperative deciders, now read-only INPUTS ---
    pub standing_idle_signal: bool, // from classify_standing_idle (no_progress.rs)
    pub stale_claim_secs: Option<u64>, // from the claim-reaper STALE_SECS sweep; None when no claim is expected or a live worker is present
    pub effect_board_missed: bool,  // effect-dispatch ledger board-presence check (fail-closed)
}
```

The last three fields are the crux of the design: the deciders that previously
*acted* on these signals now only *report* them. The reasoner — not a threshold —
decides what, if anything, to do about them.

### `worker_present` is liveness-verified (not bare map membership) — #4631

`worker_present` is a **verified-live-process** fact, not membership in the
in-memory `engineer_worktrees` map. `gather_per_goal_cycle_ctx` computes it as:

```rust
let worker_present = state.engineer_worktrees.contains_key(goal_id)
    && crate::ooda_actions::advance_goal::find_live_engineer_for_goal(
        &crate::goal_curation::simard_state_root(),
        goal_id,
    )
    .is_some();
```

The bare `contains_key(goal_id)` read that shipped previously was **fail-open**:
a leaked or orphaned worktree entry (engineer SIGKILLed, OOM-killed, host reboot,
daemon crash-reload) kept `worker_present == true` forever, so the goal never
re-spawned and never populated `stale_claim_secs` — it silently wedged. The
second conjunct verifies an actual live, start-time-guarded engineer (reusing the
fail-close hardening from #4608 / #4574), so a dead/leaked claim now reads
`false` and the existing reclaim path re-engages. `contains_key` is retained as a
short-circuit so the filesystem scan only runs for goals that hold a claim.
Full contract: [Worker-Presence Liveness API](worker-presence-liveness-api.md).


## `PerGoalAction`

The reasoner's output: one of six actions, each carrying a mandatory `reason`.

```rust
/// One reasoned next-action for a single active goal, for a single cycle.
///
/// Serde discriminator is `choice` (snake_case), mirroring
/// `EngineerLifecycleDecision` so the recipe JSON-envelope parser is reused.
/// (As of #4720 that parser is only exercised on non-RecipeBrain paths, e.g.
/// RustyClawdBrain; RecipeBrain reads the typed record via `read_verified`.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum PerGoalAction {
    /// Work is genuinely in flight and healthy; leave it. Never rolls/wipes.
    Continue { reason: String },

    /// No live work; start the next concrete piece. Never rolls/wipes.
    /// `task_hint` is optional model-supplied guidance for the new work.
    Spawn { reason: String, #[serde(default)] task_hint: String },

    /// The goal needs a new angle; deliberately redirect it.
    /// MUTATES refs / rolls the cycle.
    Reorient { reason: String },

    /// Something looks wrong; inspect logs/tools BEFORE any destructive step.
    /// The ONLY gate through which reclaim/reset/fault become reachable.
    Investigate { reason: String },

    /// Legitimately blocked on an external event; record why, do not churn.
    /// Never rolls/wipes.
    Wait { reason: String },

    /// The goal is done; close it and clear refs. MUTATES/clears refs.
    Complete { reason: String },
}
```

### Wire format

Each variant serializes to a single JSON object with a `choice` discriminator
and a `reason`:

```json
{"choice": "continue",   "reason": "engineer committed 2 min ago; PR #1234 open"}
{"choice": "spawn",      "reason": "no live work; seek a new source", "task_hint": "search arXiv for 2026 papers on X"}
{"choice": "reorient",   "reason": "current angle exhausted; pivot to benchmark Y"}
{"choice": "investigate","reason": "worker log tail truncated mid-tool-call; read logs before reclaim"}
{"choice": "wait",       "reason": "PR #1234 awaiting CI; nothing to do this cycle"}
{"choice": "complete",   "reason": "goal shipped and merged in PR #1234"}
```

An unknown `choice`, a missing **or empty** `reason`, or an otherwise
unparseable envelope is an **error**, never a silent default (see
[Reference: OODA Per-Goal-Cycle Recipe](ooda-per-goal-cycle-recipe.md)).

> **Changed in #4720.** On the **RecipeBrain** path this envelope is no longer
> scraped from agent stdout with `recipe_output::extract_json_payload`. The
> reasoner records its verdict by calling the
> [`simard ooda record-decision`](ooda-record-decision-cli.md) tool, which writes
> a typed `PerGoalDecisionRecord`; `RecipeBrain` reads it with `read_verified`.
> The wire shape below is identical (it is `PerGoalAction`'s serde form,
> flattened into the record), but it now travels through a validated file, not
> prose. `RustyClawdBrain` (a different, out-of-scope seam) still parses its
> agent response via `from_recipe_envelope`.

## `PerGoalDecisionRecord` and `read_verified`

`RecipeBrain` reads a typed, file-backed record instead of scraping stdout. The
record embeds the `goal_id`/`cycle_number` and a `schema` pin; `read_verified`
enforces a fail-CLOSED matrix (absent / malformed / wrong-schema / out-of-enum /
empty-reason / goal-mismatch / cycle-mismatch ⇒ `Err`). The full type,
on-disk shape, reader semantics, and security properties are documented in
[Reference: `simard ooda record-decision`](ooda-record-decision-cli.md).

On the **RecipeBrain** path the verdict no longer flows through
`PerGoalAction::from_recipe_envelope` at all — the tool validates the closed enum
at write time and `read_verified` re-validates it at read time. The
`from_recipe_envelope` parser remains the canonical path for the **other**
reasoner backends that still return an in-process envelope (notably
`RustyClawdBrain`, out of scope for #4720): `choice` is matched
case-insensitively, `reason`/`task_hint` are trimmed and bounded, and an
empty/whitespace `reason` or an unknown `choice` yields `None` (surfaced by the
caller as a no-fallback `Err`). Both paths therefore enforce the **same**
closed-enum + non-empty-`reason` contract — one via a validated file, the other
via the envelope parser — so a valid `PerGoalAction` means the same thing
regardless of backend.

## `apply_per_goal_action_to_state`

A **pure** function — no I/O, no process spawning, no filesystem access — that
applies the chosen action to in-memory `OodaState`. Keeping it pure caps the
blast radius of any single decision to memory.

```rust
/// Apply a reasoned per-goal action to in-memory OODA state.
///
/// PURE: no IO, no process, no fs. Side-effecting execution (spawning a
/// worker, filing an issue, tearing down a worktree) is performed by the
/// thin rail in cycle.rs AFTER this returns, gated by the double-spawn guard.
///
/// Returns a human-readable detail string (the chosen action's label + reason)
/// for logging and the recorded `BrainJudgmentRecord`.
pub fn apply_per_goal_action_to_state(
    action: &PerGoalAction,
    state: &mut OodaState,
    goal_id: &str,
) -> String;
```

### Per-variant state mutation table

| Action | `wip_refs` | `roll_to_new_cycle`? | Emitted follow-up effect |
|---|---|---|---|
| `Continue` | untouched | **no** | none |
| `Spawn` | untouched | **no** | spawn next piece (via double-spawn guard) |
| `Reorient` | **cleared / redirected** | **yes** | dispatch new angle |
| `Investigate` | untouched | **no** | inspect logs/tools; reclaim only as reasoned follow-up |
| `Wait` | untouched | **no** | none (records blocking reason) |
| `Complete` | **cleared** | **yes** | close goal on the active board |

The **A6 invariant** is enforced here: only `Reorient` and `Complete` mutate
`wip_refs` or roll the cycle. This is the code-level guarantee that a `continue`,
`spawn`, `wait`, or `investigate` verdict can never reproduce the `70ab8541`
idle → reset loop.

## `OodaBrain::decide_per_goal_cycle`

A new trait method on `OodaBrain` with **no default implementation** — every
brain must decide explicitly. Omitting it is a compile error, which is
intentional: it prevents a "hollow green" from an unmigrated brain silently
falling back to a no-op.

```rust
pub trait OodaBrain: Send + Sync {
    // ... existing methods (decide_engineer_lifecycle, etc.) ...

    /// Decide the single best next action for one active goal, this cycle.
    ///
    /// NO default impl: forces RecipeBrain, DeterministicLifecycleBrain, and
    /// every test double to implement it. An Err surfaces as a cycle failure
    /// (no silent fallback, #1711).
    fn decide_per_goal_cycle(
        &self,
        ctx: &PerGoalCycleCtx,
    ) -> Result<PerGoalAction, SimardError>;
}
```

### Implementations

| Impl | File | Behavior |
|---|---|---|
| `RecipeBrain` | `recipe_brain.rs` | Allocates a fresh per-cycle temp dir, passes `-c record_path` + `-c simard_bin=current_exe()`, runs the `ooda-per-goal-cycle` recipe (no timeout), then reads the typed `PerGoalDecisionRecord` via `read_verified` — **not** `extract_json_payload`. Any absent/malformed/mismatched record **surfaces `Err`** — no silent fallback. |
| `RustyClawdBrain` | `rustyclawd.rs` | Renders a compact prompt, submits it through the `LlmSubmitter`, then parses the response via the canonical `PerGoalAction::from_recipe_envelope`. **Surfaces `Err`** on an unparseable envelope — no silent fallback. Out of scope for #4720 (does not run the `ooda-per-goal-cycle` recipe). |
| `DeterministicLifecycleBrain` | `fallback.rs` | Returns `Continue` unconditionally. This preserves the no-LLM behavior of the fallback: it **never** rolls the cycle and **never** reaps, so the fallback path cannot re-introduce the idle→reset loop. |
| Test doubles (5 across 3 files) | `tests.rs`, `spawn.rs`, `recipe_brain.rs` | Return scripted `PerGoalAction`s for regression tests. |

## Driver loop (`cycle.rs`)

A new phase runs after `act()` each cycle:

1. **Snapshot** the ids in the active board (`state.active_goals.active`) into a
   `Vec`/`HashSet` first — mirroring the existing `pre_cycle_active_ids` snapshot
   at `cycle.rs:236` — to avoid a borrow conflict while `apply_*` mutates state.
2. For each id: `gather_per_goal_cycle_ctx` → `decide_per_goal_cycle` →
   `apply_per_goal_action_to_state`.
3. **Record** a `BrainJudgmentRecord` (`push_brain_judgment`) for **every** goal —
   every active goal gets an action *and* a recorded `reason` each cycle; none is
   ever left idle without both.
4. Run the thin side-effect rail for the applied action (`Spawn` reuses the
   existing double-spawn guard in `spawn.rs`).

> The helper names below (`active_ids_snapshot`, `run_side_effect_rail`) are
> **illustrative** — they sketch the shape of the driver loop, not a final API.
> The real snapshot pattern is the `pre_cycle_active_ids` `HashSet` over
> `state.active_goals.active` at `cycle.rs:236`.

```rust
// after act(), each cycle — see cycle::drive_per_goal_cycle:
let active_ids: Vec<String> = state
    .active_goals
    .active
    .iter()
    .map(|g| g.id.clone())
    .collect(); // snapshot, cf. pre_cycle_active_ids at cycle.rs:236
for goal_id in active_ids {
    let ctx = gather_per_goal_cycle_ctx(&state, &goal_id);
    let action = brain.decide_per_goal_cycle(&ctx)?; // Err => cycle failure
    let detail = apply_per_goal_action_to_state(&action, &mut state, &goal_id);
    push_brain_judgment(BrainJudgmentRecord::from_per_goal_cycle(
        &goal_id, &action, false, "",
    )); // action + reason recorded for every goal
    let _ = detail; // logged; Spawn's side-effect rail reuses the double-spawn guard
}
```

## Demoted deciders

These three call sites lose their autonomous decision authority and become pure
signal producers feeding `PerGoalCycleCtx`:

| Site | File | Before | After |
|---|---|---|---|
| `classify_standing_idle` / `apply_standing_idle` | `no_progress.rs` | called `roll_to_new_cycle` on idle | returns `standing_idle_signal: bool` |
| `reap_stale_claims` | `overseer/claim_reaper.rs` | tore down stale worker | reports `stale_claim_secs: Option<u64>` (`STALE_SECS` retained as threshold-for-input only) |
| effect board-miss (fail-closed liveness/claim handler) | `typed_ooda/ledger.rs` | board-presence signal fed reclaim (fail-closed: unproven ⇒ keep rejecting duplicates) | reports `effect_board_missed: bool` |

Reclaim now happens **only** as a reasoned follow-up to an `Investigate` verdict.
No new `SIMARD_*_SECS` config is added; `STALE_SECS` survives solely as the
threshold that populates the boolean/seconds input.

## Regression tests

Per design item **A7**, tests use a **stub `OodaBrain`** returning scripted
actions (no live recipe-runner subprocess):

| Test | Asserts |
|---|---|
| **T1 — anti-loop** | A `70ab8541`-style standing research goal, over N cycles, yields an action ∈ {`continue`, `spawn`, `investigate`} — **never** an idle → reset — and records a non-empty `reason` every cycle. |
| **T2 — investigate-first** | In a stale-worker scenario, the first destructive step is **preceded** by an `investigate` verdict. |
| serde round-trip | Every `PerGoalAction` variant serializes to / parses from its `{choice, reason}` envelope. |
| per-variant mutation table | `apply_per_goal_action_to_state` touches `wip_refs` / rolls the cycle **only** for `Reorient` and `Complete`. |
| fallback = Continue | `DeterministicLifecycleBrain::decide_per_goal_cycle` returns `Continue` and never rolls/reaps. |

## Security properties

- **Authorization = decision authority.** Destructive ref mutation
  (`roll_to_new_cycle` / clearing `wip_refs`) happens **only** for a reasoned
  `Reorient` or `Complete`; a stale/quiet-worker reclaim is reached **only**
  after an `Investigate` verdict has first inspected logs/tools — never a
  threshold crossing.
- `apply_per_goal_action_to_state` is **pure**, bounding blast radius to
  in-memory state.
- Every free-text `ctx` field is passed through `sanitize_context_var` (strips
  ANSI / C0 control chars, folds newlines) before reaching the recipe, defeating
  YAML/context/prompt injection; strings are length-capped and vectors
  count-capped (DoS / `E2BIG` guard).
- **Strict typed-record read (RecipeBrain).** The verdict travels through a
  daemon-owned, `0o600`, per-cycle temp file, not scraped prose. `read_verified`
  independently re-checks `schema`, `goal_id`, and `cycle_number`, so a stale,
  replayed, or partially written record fails CLOSED. `--record-path` is
  absolute and `..`-free (SR-VAL-8). See
  [Reference: `simard ooda record-decision`](ooda-record-decision-cli.md#security).
- Strict 6-variant envelope parse: unknown `choice` or missing `reason` → `Err`;
  model-supplied `task_hint` is sanitized.
- Command invocation stays **argv-only** (`Command::arg`, never `sh -c`) with a
  constant binary and a compile-time-constant recipe filename (no path
  traversal).

## See Also

- [Concept: Worker-Presence Liveness Verification](../concepts/worker-presence-liveness-verification.md) — the fail-open `worker_present` fix (#4631)
- [Reference: Worker-Presence Liveness API](worker-presence-liveness-api.md)
- [Concept: Agentic Per-Goal, Per-Cycle Decision](../concepts/agentic-per-goal-per-cycle.md)
- [Reference: OODA Per-Goal-Cycle Recipe & Prompt Schema](ooda-per-goal-cycle-recipe.md)
- [Reference: `simard ooda record-decision` (typed decision tool)](ooda-record-decision-cli.md) — `PerGoalDecisionRecord`, `read_verified`, fail-closed matrix
- [Reference: `OodaBrain` API](ooda-brain-api.md) — the sibling engineer-lifecycle reasoner
- [Reference: OODA Engineer-Lifecycle Recipe](ooda-engineer-lifecycle-recipe.md)
- [Reference: recipe context variable sanitization](recipe-context-var-sanitization.md)
