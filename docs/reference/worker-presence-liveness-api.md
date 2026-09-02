---
title: "Reference: Worker-Presence Liveness API"
description: >
  The contract for the OODA per-goal reasoner's `worker_present` fact: how it is
  computed in gather_per_goal_cycle_ctx, the reused find_live_engineer_for_goal
  liveness verifier, fail-closed-on-"present" semantics, the interaction with
  stale_claim_secs, configuration (none new), and regression coverage. Fixes the
  fail-open worker-presence bug (#4631).
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/worker-presence-liveness-verification.md
  - ooda-per-goal-cycle-api.md
  - engineer-claim-release-api.md
  - engineer-worktree-isolation.md
  - state-root-resolution.md
---

# Reference: Worker-Presence Liveness API

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
> (`gather_per_goal_cycle_ctx`). Conceptual overview:
> [Worker-Presence Liveness Verification](../concepts/worker-presence-liveness-verification.md).

## The `worker_present` fact

`worker_present: bool` is a field of
[`PerGoalCycleCtx`](ooda-per-goal-cycle-api.md), gathered once per active goal
per cycle. It answers a single question:

> **Does this goal have a verified-live engineer right now?**

It is a *fact* fed to the reasoner, not a verdict. The reasoner decides what to
do (`Continue`, `Spawn`, `Investigate`, …); `worker_present` only reports
ground truth.

## Computation

```rust
// src/ooda_loop/cycle.rs — gather_per_goal_cycle_ctx
let worker_present = state.engineer_worktrees.contains_key(goal_id)
    && crate::ooda_actions::advance_goal::find_live_engineer_for_goal(
        &crate::goal_curation::simard_state_root(),
        goal_id,
    )
    .is_some();
```

Two conjuncts, evaluated left-to-right with `&&` short-circuit:

1. **`engineer_worktrees.contains_key(goal_id)`** — cheap in-memory guard. When
   `false`, the goal has no claim at all and the second conjunct is skipped
   (no filesystem IO). Preserves the old fast path for the common "no worker"
   case.
2. **`find_live_engineer_for_goal(state_root, goal_id).is_some()`** — the
   authoritative liveness verifier. Only reached when a map entry exists. Turns
   "a claim exists" into "a *live* engineer exists".

`state_root` is obtained from
[`crate::goal_curation::simard_state_root()`](state-root-resolution.md), the same
resolver already used elsewhere in `cycle.rs`. Fully-qualified paths are used so
no new `use` statements are introduced.

### Why both conjuncts

| `contains_key` | `find_live_engineer_for_goal` | `worker_present` | Meaning |
|---|---|---|---|
| `false` | *(not evaluated)* | `false` | No claim for this goal |
| `true` | `Some(path)` | `true` | Live, start-time-verified engineer |
| `true` | `None` | `false` | Claim exists but engineer is **dead / leaked** → reclaimable (bug #4631 fixed) |

The old code stopped at the first column, so the third row wrongly read `true`.

## Cost

Per goal, the added filesystem work is a single `read_dir` of the
`engineer-worktrees/` root plus a sentinel read per matching entry — reached
only when the `contains_key` short-circuit passes (i.e. the goal actually holds
a claim).

The honest **aggregate** cost is quadratic, not linear: with *N* active goals
each holding a claim, the cycle performs *N* scans, and each scan lists the
whole worktrees root (~*N* entries), i.e. **O(N²)** directory entries walked per
cycle. This is acceptable because *N* is small in practice — the worktree root
is bounded by concurrent-engineer admission limits (tens, not thousands), the
scan is plain `read_dir` on a local state directory, and it runs once per cycle
rather than per reasoning step. If concurrency limits are ever raised
substantially, this read should be lifted to a single per-cycle scan shared
across goals rather than repeated per goal.

## The liveness verifier: `find_live_engineer_for_goal`

Reused **verbatim** from the fail-close hardening (#4437 / #4608 / #4574) — no
reimplementation, no fork.

```rust
pub fn find_live_engineer_for_goal(
    state_root: &std::path::Path,
    goal_id: &str,
) -> Option<std::path::PathBuf>;
```

Paraphrased behaviour (the authoritative doc-comment lives in the source and
cites #1227 / #1213 / #1238 and its defense-in-depth role for
`dispatch_spawn_engineer`; consult the source for the canonical text): returns
the worktree of a *live* engineer for `goal_id`, else `None`. It is stateless —
relying only on the on-disk worktree dir and the per-worktree PID sentinel. It
scans `<state_root>/engineer-worktrees/{goal_id}-*` and, for each, reads
`.simard-engineer-claim`, confirms the PID is alive, and (when the sentinel
records one) verifies the process start-time still matches — so a recycled PID
is treated as dead.

Behaviour summary (source:
[`spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs)):

| Sentinel state | Result |
|---|---|
| PID alive, starttime recorded and matches | `Some(worktree)` |
| PID alive, no starttime recorded (pre-#1238 sentinel) | `Some(worktree)` (PID-only fallback) |
| PID alive, starttime recorded but **mismatches** (recycled PID) | `None` |
| PID dead | `None` |
| Sentinel missing / unparseable | `None` |
| `read_dir` of the worktrees root errors (transient IO) | `None` |
| Directory name does not match `{goal_id}-` prefix exactly | skipped |

## Fail-closed-on-"present" contract

> **`worker_present` is `true` only on positive proof of a live engineer.**

`find_live_engineer_for_goal` returns `None` for *both* proven-absent and
transient-IO-error. For the presence read this collapses safely: any non-proof
yields `worker_present = false`. This is the correct fail direction here —

- It **never fabricates** a live worker, so a dead/leaked claim can always be
  reclaimed (the #4631 fix).
- A transient false `false` at worst causes the reasoner to *examine* a goal
  (via `stale_claim_secs` → an `Investigate` verdict), which inspects logs/tools
  **before** any destructive step. A live engineer is therefore never killed by
  a momentary IO blip; the reasoner's investigate-first gate absorbs it.

This mirrors, in the opposite direction, the fail-close lease's rule that an
ambiguous liveness signal keeps a claim *present* (see
[Engineer-Claim Release & Reclaim API](engineer-claim-release-api.md)); both
choose the direction that cannot silently destroy work.

## Interaction with `stale_claim_secs`

`worker_present` gates the reclaim input in the same `gather_per_goal_cycle_ctx`:

```rust
let stale_claim_secs = if expects_worker && !worker_present {
    Some(claim_age_secs(goal))
} else {
    None
};
```

Before the fix, a leaked claim kept `worker_present == true`, so
`stale_claim_secs` stayed `None` and the reclaim signal never surfaced — the
goal was wedged. After the fix, a dead/leaked claim flips `worker_present` to
`false`, so a goal that *expects* a worker now populates `stale_claim_secs`, and
the existing reclaim path re-engages. **No new threshold or config is added;**
`STALE_SECS` is unchanged and still only feeds `claim_age_secs`.

## Configuration

**None added.** This feature introduces:

- no new environment variables,
- no new `SIMARD_*_SECS` thresholds,
- no schema change or migration,
- no new public API surface (the verifier already existed and is `pub`).

It is a pure, additive tightening of one boolean computation. Genuinely-live
workers observe **identical** behaviour to before.

## Observability

The presence decision surfaces through the reasoner's normal telemetry — the
`worker_present` value is already rendered into the per-goal reasoning prompt
(`rustyclawd.rs`, `recipe_brain.rs`) and carried on the recorded
`BrainJudgmentRecord`. Any additional signal uses structured `tracing` + OTel
only; **no `print!` / `println!`** is added, and absolute host paths are not
logged.

## Regression coverage

Tests live in the `#[cfg(test)]` module alongside the code in
[`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs),
following the hermetic, serial-safe pattern of
`tests_advance_goal.rs:465-592` (respecting the #4571 / #4575 de-flake
precedent — each test builds its own `tempdir` state root and writes its own
sentinels; no shared global state).

| Test | Setup | Asserts |
|---|---|---|
| **Leaked / idle claim → not present** | map entry present; sentinel with a **dead** PID (or missing/unparseable sentinel) | `worker_present == false` **and** `stale_claim_secs.is_some()` (goal is reclaimable) |
| **Recycled PID → not present** | map entry present; live PID but a starttime that cannot match | `worker_present == false` (reclaimable) |
| **Live engineer → present** | map entry present; sentinel with the real live PID + matching starttime | `worker_present == true`; `stale_claim_secs.is_none()` |
| **No map entry → not present (short-circuit)** | goal absent from `engineer_worktrees` | `worker_present == false`; the filesystem scan is not reached |

Required merge gates (blockers, not observed results): all required CI checks
green — coverage, pre-commit, `cargo-audit`, `cargo-deny`, `cargo-vet`,
`npm-audit`, `scripts-tests`, `install-real`, `e2e-dashboard`, GitGuardian — and
no `unwrap`/`expect`/`print!` in the changed code. The change lands under the
`ooda-core` sequence group to serialize edits to the shared `cycle.rs`.

## Related

- [Worker-Presence Liveness Verification](../concepts/worker-presence-liveness-verification.md)
- [Reference: OODA Per-Goal-Cycle API](ooda-per-goal-cycle-api.md)
- [Engineer-Claim Release & Reclaim API](engineer-claim-release-api.md) — the fail-CLOSE counterpart
- [Engineer-Worktree Isolation](engineer-worktree-isolation.md)
- [State-Root Resolution](state-root-resolution.md)
