---
title: Diagnose a no-progress block and read its WHY
description: Runbook for the OODA no-progress breaker's root-cause resolution — how to read the classified WHY + evidence on a block, what each classification (already-complete, obsolete, missing-precondition, upstream-dependency, unclear-criteria, genuinely-stuck) means and how it self-resolves, worked examples for each ladder rung (including the kgpacks-rs "already done" incident), and how to configure the threshold, the guided-retry bound, and the optional agentic WHY recipe.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ./unblock-stuck-ooda-goals.md
  - ./diagnose-a-reopened-goal.md
  - ./diagnose-a-rejected-goal-completion.md
  - ./run-ooda-daemon.md
  - ./edit-the-ooda-brain-prompt.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/ooda-no-progress-why-recipe.md
---

# Diagnose a no-progress block and read its WHY

> **Status: implemented (issue #16).** The root-cause-resolution upgrade is on by
> default (`SIMARD_NO_PROGRESS_INVESTIGATE=off` reverts to the base ladder). A
> no-progress block now carries a `why=<TOKEN> evidence=[…]` segment; for the
> rarer manual unblock cases see
> [Unblock OODA goals stuck after a lockout](./unblock-stuck-ooda-goals.md).

## When you need this

A goal reached the OODA **no-progress breaker** (3 consecutive no-action cycles).
As of the root-cause-resolution upgrade the breaker no longer parks such a goal
with a bare "needs human review". Instead it **classifies why** the goal stalled
and **self-resolves** the machine-fixable causes, escalating to a human only as a
last resort — and when it does, the block reason **names the cause and links the
evidence**.

Use this runbook to:

- read the WHY on a block that did reach you,
- understand which stalls resolve themselves (so you do nothing),
- reproduce or verify each ladder rung, and
- tune the threshold, the guided-retry bound, and the optional agentic narrator.

For the design and rationale see
[The no-progress breaker explains WHY and self-resolves before escalating](../concepts/no-progress-root-cause-resolution.md).
For exact types see the
[root-cause resolution API reference](../reference/no-progress-root-cause-resolution-api.md).

## Read the WHY on a block

List the board and inspect the `STATUS` column:

```bash
simard goal list
```

A breaker escalation now looks like this (single line, wrapped here for
readability):

```text
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 4 consecutive
no-action cycles; needs human review | why=GENUINELY-STUCK;
evidence=[https://github.com/rysweet/Simard/issues/123,
          https://github.com/rysweet/Simard/pull/456]
```

Parse it in three parts:

1. **Sentinel** — `🔒 [OODA-SAFEGUARD] … needs human review` — the same marker as
   before, so `simard goal unblock-all`, the overseer, and the load-time
   self-heal all still recognise it.
2. **`why=<TOKEN>`** — the classified root cause. One of
   `ALREADY-COMPLETE`, `OBSOLETE`, `MISSING-PRECONDITION`, `UPSTREAM-DEPENDENCY`,
   `UNCLEAR-CRITERIA`, `GENUINELY-STUCK`.
3. **`evidence=[…]`** — the artifact links the classifier acted on (issues, PRs,
   commits), or `[none]`.

The matching escalation issue (label `ooda-stuck`) carries the same WHY plus the
agentic narrative paragraph, if the [optional WHY
recipe](../reference/ooda-no-progress-why-recipe.md) is enabled.

> If you see a block **without** a `why=` segment, it is either a legacy block
> from before this upgrade or a block authored by a *different* path (operator
> hold, scope, dependency, or the brain-failure safeguard). Those are out of
> scope for this runbook — see
> [Unblock OODA goals stuck after a lockout](./unblock-stuck-ooda-goals.md).

## What each classification means — and what you do

| `why=` token | Root cause | Breaker action | Your action |
| --- | --- | --- | --- |
| `ALREADY-COMPLETE` | Done-criteria satisfied by live artifacts (issues closed / PRs merged / deployed) but the goal was never marked done | **Auto-completes** the goal, attaches artifacts | **Nothing** — it is `completed`, not blocked |
| `OBSOLETE` | Work is tracked elsewhere / out of scope | **Drops** it from the active board | Nothing |
| `MISSING-PRECONDITION` | A governed repo was never cloned (or similar) | **Clones**/establishes it, resets the counter, retries | Nothing — watch it resume |
| `UPSTREAM-DEPENDENCY` | Waiting on a specific blocking goal/PR/issue | **Defers** it (`paused`), records the blocking ref, **auto-clears** when the upstream lands | Nothing — or land the upstream |
| `UNCLEAR-CRITERIA` | Done-gate cannot measure the success criteria | **Spawns one guided engineer** with the WHY | Only if it re-escalates: sharpen the criteria |
| `GENUINELY-STUCK` | No machine-resolvable cause | **Spawns one guided engineer**, then **escalates** if it stalls again | Investigate using the linked evidence |

Only the last two can reach you, and only after the goal has **already spent its
one guided-engineer retry** and stalled again.

## Worked examples

### 1. The "already done" incident (ALREADY-COMPLETE → auto-complete)

The `kgpacks-rs` "already done" case: a cluster of goals whose referenced issues
were **closed** and whose workstream PRs were **merged** (the specific numbers
here are **illustrative**). The brain kept emitting `NO ACTION` because nothing
was left to do, so each hit the 3-cycle threshold.

- **Before:** all seven parked as
  `🔒 [OODA-SAFEGUARD] … needs human review` — a human closed them by hand.
- **Now:** at the threshold the classifier runs the done-gate, sees the closed
  issues + merged PRs, classifies `ALREADY-COMPLETE`, transitions each goal to
  `completed`, and attaches the artifact links as evidence. `simard goal list`
  shows `completed`, not `blocked`. No issue is filed and no human is paged.

Verify:

```bash
simard goal list | grep -i completed          # the seven goals are completed
ls -t ~/.simard/cycle_reports/ | head -1       # cycle report shows marked_done entries, escalated=0
```

### 2. Un-cloned target repo (MISSING-PRECONDITION → clone + retry)

A goal targets a governed repo that is not in the workspace, so no cycle can make
progress.

- The classifier's `repo_present` check returns `false` → `MISSING-PRECONDITION`.
- The breaker clones the repo (reusing the self-deploy source-prep clone path),
  resets the no-action counter, and lets the next cycle try for real.
- No block. The cycle report lists the goal under `healed`.

If the clone itself fails, the goal escalates with
`why=MISSING-PRECONDITION evidence=[…clone-error…]` — a concrete, actionable
block rather than a generic one.

### 3. Waiting on an upstream (UPSTREAM-DEPENDENCY → defer)

A goal declares a dependency on another goal/PR/issue that has not landed.

- `dependency_goal_state` reports an unresolved blocking ref →
  `UPSTREAM-DEPENDENCY`.
- The breaker sets the goal `paused` (a deliberate hold, **not** `blocked`) and
  records the blocking ref. `simard goal list` shows `paused`, not "needs human
  review".
- When the upstream resolves (dependency goal `completed` / PR merged / issue
  closed), the **auto-clear pass** returns the goal to `not-started` and it is
  re-selected. The cycle report lists it under `deferred`, then later
  `auto_cleared`.

### 4. Genuinely stuck (SPAWN-ENGINEER → escalate-with-WHY)

A goal with no machine-resolvable cause.

- First threshold hit: classify `GENUINELY-STUCK` (or `UNCLEAR-CRITERIA`) →
  `SpawnEngineer`. The breaker spawns **one** engineer whose task embeds the WHY
  ("prior attempts stalled: `<why>`; `<evidence>`") and marks the goal's
  guided-retry as used. No block. Cycle report lists it under `engineer_spawned`.
- If the goal stalls **again** to the threshold with the retry already spent (or
  the spawn was rejected): `Escalate`. The goal is `blocked` with
  `why=GENUINELY-STUCK evidence=[…]` and an `ooda-stuck` issue is filed.

Worst case is bounded: **at most one extra engineer session** before you are
involved, and when you are, you get the diagnosis and the evidence.

## Configuration

### Threshold

The consecutive-no-action count that trips the breaker is the compile-time
constant `NO_PROGRESS_BREAKER_THRESHOLD` (default `3`) in
`src/goal_curation/no_progress_breaker.rs`. Changing it requires a rebuild and
redeploy. It is deliberately small so a livelock is broken quickly.

### The one-shot guided-retry bound

A goal gets **exactly one** guided-engineer retry (tracked by a per-goal
`guided_retry_used` flag persisted with the goal board, so the bound survives a
daemon restart). This is not operator-tunable by design — it is the safety bound
that stops the guided retry from re-creating a livelock. See the
[bound's contract](../reference/no-progress-root-cause-resolution-api.md#one-shot-guided-retry-bound).

### The optional agentic WHY narrator

The human-readable WHY paragraph comes from the optional
[`ooda-no-progress-why` recipe](../reference/ooda-no-progress-why-recipe.md). It
hot-reloads — no rebuild needed — from:

1. `~/.simard/prompt_assets/simard/recipes/ooda-no-progress-why.yaml`, else
2. the repo copy at `prompt_assets/simard/recipes/ooda-no-progress-why.yaml`.

To edit the narrative style, sync the prompt asset:

```bash
rsync -a prompt_assets/simard/ ~/.simard/prompt_assets/simard/
```

If the recipe is absent or errors, the breaker **fails closed** to a
deterministic WHY narrative — the classification, self-resolution, and escalation
still work; only the prose is terser. The narrator **never** changes which ladder
rung is taken.

## Verify the breaker's activity

Each cycle's report records exactly which rung fired:

```bash
ls -t ~/.simard/cycle_reports/ | head -1       # newest cycle report
```

The report's no-progress summary carries additive counters — `marked_done`,
`dropped`, `escalated`, `healed`, `deferred`, `engineer_spawned`, `auto_cleared`,
`investigation_errors`, `perpetual_idled` — so you can confirm a stall
self-resolved (e.g. `healed=1`, `escalated=0`) rather than reached a human. Only
`marked_done` / `dropped` / `escalated` / `healed` / `deferred` /
`engineer_spawned` count as a firing; `auto_cleared`, `investigation_errors`, and
`perpetual_idled` are normal operation.

## Troubleshoot: the `ooda-stuck` label crash loop

If the same goal stays `Blocked` across many cycles and the journal repeats:

```text
ERROR run_ooda_cycle: simard::ooda: no-progress breaker: gh issue create failed
  (goal still Blocked) stderr=could not add label: 'ooda-stuck' not found
```

the breaker was trying to file its tracking issue with the `ooda-stuck` label
but that label does not exist in the repository, so issue creation failed and
the goal could never leave the loop.

**This is now self-healing (#4394).** `GhIssueFiler` detects the missing-label
signature and retries `gh issue create` **once without** `--label`, so the
tracking issue is always filed. In current builds you will instead see a single
WARN fallback followed by a successful file:

```text
WARN … simard::ooda: no-progress breaker: 'ooda-stuck' label missing;
  retrying gh issue create without --label stderr=could not add label: 'ooda-stuck' not found
WARN … simard::ooda: no-progress breaker: tracking issue filed for stuck goal (without label) issue=4231
```

No operator action is required — the goal is recorded and unblocked
automatically. If you *prefer* labeled tracking issues, create the label once so
the primary path succeeds and no fallback is needed:

```bash
gh label create ooda-stuck --description "OODA no-progress breaker tracking issue" --color BFD4F2
```

See the [missing-label fallback in the breaker API
reference](../reference/no-progress-breaker-api.md#tracking-issue-filer-noprogressissuefiler-and-the-missing-label-fallback)
for the exact detection rule and guarantees.

## Related
- [Concept: the breaker explains WHY and self-resolves before escalating](../concepts/no-progress-root-cause-resolution.md)
- [Root-cause resolution API reference](../reference/no-progress-root-cause-resolution-api.md)
- [No-progress breaker API reference](../reference/no-progress-breaker-api.md)
- [The `ooda-no-progress-why` recipe reference](../reference/ooda-no-progress-why-recipe.md)
- [Unblock OODA goals stuck after a lockout](./unblock-stuck-ooda-goals.md) — the manual path for non-breaker blocks.
