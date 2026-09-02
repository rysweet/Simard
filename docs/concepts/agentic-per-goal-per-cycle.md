# Agentic Per-Goal, Per-Cycle Decision

Every OODA cycle, for **every active goal**, Simard's brain runs **one** agentic
reasoning step that reads the goal's durable state, history, and in-flight work
and returns the single best next action. This is the universal, continuous
generalization of the older, special-case
[engineer-lifecycle decision](prompt-driven-ooda-brain.md): instead of only
firing when the Act phase is *about to skip* a goal with a live worktree, the
reasoner now fires for **every** goal on `board.active`, every cycle — whether
or not a worktree exists.

The reasoner replaces three imperative subsystems that previously decided a
goal's liveness independently and could fight each other; that replacement is
tracked by [#4453](https://github.com/rysweet/Simard/issues/4453).

## Why

Simard is long-running; her engineers (workers) are ephemeral. A standing goal
ships improvements in bursts, so an **idle or absent worktree is normal**, not
evidence of death. Three imperative deciders treated the *point-in-time* absence
of live work as a fault and each acted on it independently:

- **never-idle** — `classify_standing_idle` (`src/ooda_loop/no_progress.rs`) +
  `roll_to_new_cycle` reset a goal whenever it looked idle.
- **claim-reaper** — `reap_stale_claims` (`src/overseer/claim_reaper.rs`) tore
  down a worker whose heartbeat crossed a staleness threshold.
- **effect board-miss** — the durable effect-dispatch ledger
  (`src/typed_ooda/ledger.rs`) and its fail-closed liveness/claim handler, whose
  board-presence signal (does a goal have a matching in-flight effect-dispatch
  claim?) fed reclaim decisions. (The exact call site is confirmed during
  implementation; the design contract is that this check becomes a read-only
  input, not an actor.)

For the research goal `70ab8541`, these produced an **idle → reset fault-loop**:
the goal finished a burst of work, the worktree went quiet, a threshold decider
declared it idle/stale, `roll_to_new_cycle` wiped its work-in-progress refs, and
the goal restarted from zero — every cycle, forever. No counter, threshold, or
grace-window fixed this, because the fault was *deciding liveness by threshold at
all*.

The per-goal, per-cycle reasoner removes that class of bug by making the
**decision itself agentic**. A quiet worker is a fact fed to the reasoner, not a
verdict. Any worker-health concern is routed through an `investigate` action —
which inspects logs/tools **before** anything destructive — so a heartbeat or
worktree that merely *looks* stale can never reap or reset work on its own.

## The Six Actions

The reasoner returns exactly one of six actions, each with a mandatory free-text
`reason`. The **choice** among them is the agentic part; a thin deterministic
rail merely executes it.

| Action | Meaning | Touches `wip_refs` / rolls cycle? |
|---|---|---|
| `continue` | Work is genuinely in flight and healthy; leave it. | **No** |
| `spawn` | No live work; start the next concrete piece. For the research goal: seek a new source or design a new experiment — it must **never** sit idle. | **No** |
| `reorient` | The goal needs a new angle; pick one and start it. | **Yes** (deliberate redirect) |
| `investigate` | Something looks wrong (a worker went quiet); read logs/tools to find out **before** any destructive action. | **No** |
| `wait` | Legitimately blocked on an external event (a PR awaiting CI/merge); record why, do not churn. | **No** |
| `complete` | The goal is done; close it and clear refs. | **Yes** (completion) |

Only `reorient` (deliberate redirect) and `complete` (closure) mutate or clear a
goal's `wip_refs`. `continue`, `spawn`, `investigate`, and `wait` **never** roll
the cycle or wipe refs. This single invariant (design item **A6**) is the root
fix for the `70ab8541` idle → reset loop: a quiet standing goal gets `continue`
or `spawn` and keeps its in-flight refs intact, preserving Overseer dedup,
engineer-admission, and completion-gate invariants.

## Destructive actions require an `investigate` verdict first

Reclaim, reset (`roll_to_new_cycle`), and fault are **only reachable as a
reasoned follow-up to an `investigate` verdict** that first inspected the logs
and tools. A threshold crossing (stale heartbeat, missing effect job, standing
idle) can no longer trigger a destructive effect on its own — it is only surfaced
to the reasoner as an input:

- `standing_idle_signal: bool` — from the demoted `classify_standing_idle`.
- `stale_claim_secs: Option<u64>` — from the demoted claim-reaper staleness sweep
  (`STALE_SECS` threshold retained as an *input producer* only).
- `effect_board_missed: bool` — from the demoted effect-dispatch ledger
  board-presence check (fail-closed).

These three deciders are **demoted to read-only signal producers**. They no
longer call `roll_to_new_cycle`, reclaim, or fault on their own.

## What stays imperative (the thin rail)

Rust code is limited to the thin deterministic rail around the decision:

1. **Gather** durable per-goal context (`gather_per_goal_cycle_ctx`).
2. **Call** the reasoner exactly once per goal per cycle
   (`decide_per_goal_cycle`).
3. **Apply** the returned action to in-memory state
   (`apply_per_goal_action_to_state`, a pure function with no I/O) and run
   minimal safety (the existing double-spawn guard).

The rail never decides liveness by threshold. See
[Reference: `PerGoalCycle` API](../reference/ooda-per-goal-cycle-api.md) for the
types and [Reference: OODA Per-Goal-Cycle Recipe](../reference/ooda-per-goal-cycle-recipe.md)
for the prompt/recipe contract.

## What this explicitly does **not** add

Consistent with the operator directive, the feature adds **none** of the
imperative predicates that had previously crept into this issue:

- **No** attempt-ceiling counter (no "give up after N tries").
- **No** new `SIMARD_*_SECS` grace-window config.
- **No** idle-`HashSet` used **as the decision** (idle is an input, never a
  verdict).

The pre-existing `BRAIN_FAILURE_BLOCKED` consecutive-*infrastructure-Err* safety
bound (`spawn.rs`) is unchanged and is **not** extended to this reasoner as a
decision mechanism — it counts reasoner errors, not goal-liveness attempts.

## Constraints

Native Rust only (no Python, no kuzu). Nothing is named "Bridge". New log lines
use the `[simard]` tracing/`eprintln!` prefix. The reasoner surfaces an `Err`
loudly as a cycle failure with **no silent fallback**
([#1711](https://github.com/rysweet/Simard/issues/1711)).

## See Also

- [Reference: OODA Per-Goal-Cycle API](../reference/ooda-per-goal-cycle-api.md) —
  `PerGoalCycleCtx`, `PerGoalAction`, `apply_per_goal_action_to_state`,
  `OodaBrain::decide_per_goal_cycle`.
- [Reference: OODA Per-Goal-Cycle Recipe & Prompt Schema](../reference/ooda-per-goal-cycle-recipe.md) —
  prompt asset, recipe YAML, JSON envelope.
- [Concept: Prompt-Driven OODA Brain](prompt-driven-ooda-brain.md) — the
  engineer-lifecycle decision this generalizes.
- [Reference: OODA Engineer-Lifecycle Recipe](../reference/ooda-engineer-lifecycle-recipe.md) —
  the sibling reasoner reused as the template.
