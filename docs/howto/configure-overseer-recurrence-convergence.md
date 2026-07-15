---
title: Configure the Overseer's recurrence convergence (2× rung)
description: >
  How to read, act on, and tune the Overseer's 2× middle remediation rung — the
  recurrence-aware upgrade that converts a RECURRING backlog-coverage gap from a
  notify-only ping into a bounded auto-launched workstream, and escalates an
  external-repo blocker once. Covers where the launch shows up, how to respond,
  and how it rides the existing gap-scan enablement.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: howto
status: design — not yet implemented
related:
  - ../reference/overseer-recurrence-convergence-api.md
  - ../concepts/overseer-recurrence-convergence.md
  - ./review-overseer-workstream-gaps.md
  - ./configure-overseer-root-cause-why.md
  - ./watch-overseer-activity.md
  - ../design/overseer.md
---

# Configure the Overseer's recurrence convergence (2× rung)

> **Status: planned — not yet shipped.** This guide documents the designed
> behaviour of the 2× rung. The commands and surfaces below describe the intended
> end state; the feature is not yet implemented, so the auto-launch will not fire
> on a current build.

The [workstream gap-scan](./review-overseer-workstream-gaps.md) tells you, once,
about work that *should* have a workstream but does not. Before, a gap that
nobody picked up was **re-flagged every cycle** and never acted on — the same for
a blocked goal that was **re-parked every cycle**. Recurrence convergence closes
that loop: once the Overseer's cognitive memory shows a gap or block has
**recurred** (seen 2×), the Overseer stops merely pinging and **converges** it.

For the data model, thresholds, and guarantees, see the
[recurrence convergence API reference](../reference/overseer-recurrence-convergence-api.md).

## What the rung does

The Overseer already asks *why* every problem occurred and recalls prior
same-signature occurrences from memory as a **recurrence count**. Convergence
adds one action at recurrence `2` — between the "noise floor" (`2`) and the
"escalation bar" (`3`):

| Situation | Recurrence | What the Overseer does |
|---|---|---|
| Gap seen for the first time | `< 2` | **Notifies** you once (unchanged). Launches nothing. |
| **Gap has recurred** | `≥ 2` | **Launches one bounded workstream** for the single top-ranked gap. |
| Blocked goal re-parked | `≥ 2` | **Routes it down the WHY ladder** — self-heals a false park or escalates a genuine block *with* its analysis. *(pre-existing behaviour, shown for context — not changed by this feature.)* |
| External-repo blocker (e.g. kgpacks-rs issue-17) | `≥ 3` | **Files one escalation** (`gh issue create`) with reproduction context. |

Only **one** gap is auto-launched per Overseer cycle, and only behind every
budget, concurrency, and recursion gate the Overseer already enforces. The rung
opens **no** new authority — it can start investigation/coverage work, never
merge, deploy, or destructive actions.

## Where the convergence shows up

### 1. The activity feed (pull)

A cycle that converged a recurring gap reads on every Overseer surface (see
[watch what the Overseer is doing](./watch-overseer-activity.md)):

- **Dashboard → Overseer tab** (`http://localhost:8080/`)
- **TUI → Overseer pane** (`Alt+8`)
- **`simard status`** → the **OVERSEER** section

For example:

> saw 3 problems, flagged 1 workstream gap, launched 1 workstream (recurring gap, seen 2×)

The launch is attributed to the Overseer's existing `launched N workstream(s)`
clause — convergence does not invent a new counter, it feeds the one you already
watch.

### 2. The workstream itself

An auto-launched convergence workstream is a normal `smart-orchestrator` run
targeting `rysweet/Simard`, with a `task_description` templated from the gap:

```
Cover a recurring backlog gap (seen 2×): goal g-1873 — harden distill parser.
Why it matters: p1 goal with no engineer and no PR. Start an active workstream
so this stops recurring.
```

You can attach to it like any other engineer (see
[attach to a running engineer](./attach-to-a-running-engineer.md)).

### 3. The escalation issue (external blockers)

When an external-repo blocker crosses the escalation bar (3×), you get **one**
filed issue with reproduction context — not a launch, because the Overseer cannot
fix code it does not own. External text is sanitized and every `gh` call uses
argument-safe invocation.

## How to respond

- **A convergence workstream started** — let it run, or steer/close it if the gap
  is no longer worth covering. Covering the work (an engineer, a PR, or an active
  workstream referencing the item) is what stops the gap recurring.
- **A blocked goal self-healed or escalated** — if it escalated, the notification
  carries the **WHY** (the root-cause analysis), so you can fix the systemic
  defect rather than re-unblock the symptom.
- **An external escalation issue appeared** — triage it in the target repo; it
  will not be re-filed each cycle (it is deduped by signature).

## Turn the rung on or off

The rung rides the **existing** gap-scan enablement — it adds **no dedicated
config knob** (this feature changes only `mod.rs` and `root_cause.rs`, not
`config.rs`). To silence the auto-launch you disable the gap-scan itself:

| Env var | What it does | Default |
|---|---|---|
| `SIMARD_OVERSEER_GAP_SCAN` | Set to a falsey value (`0`/`false`/`no`/`off`) to disable the whole gap-scan — no `WorkstreamCoverage` problems are produced, so nothing converges or launches. | on |

```bash
# Stop the gap-scan (and therefore the convergence launch) entirely.
export SIMARD_OVERSEER_GAP_SCAN=off
```

> **Note:** there is currently **no** way to keep the recurring-gap pings while
> suppressing only the auto-launch. A dedicated opt-out
> (`SIMARD_OVERSEER_GAP_CONVERGENCE`) would require adding it to
> `src/overseer/config.rs`, which is out of scope for this design — treat it as a
> follow-up, not an existing setting.

**Precedence.** Convergence needs the gap-scan. Turning the gap-scan off
(`SIMARD_OVERSEER_GAP_SCAN=off`) or disabling the acting Overseer
(`SIMARD_OVERSEER_ENABLED=0`) turns convergence off too — there are no gaps to
converge. Changes take effect on the next daemon start.

The pre-existing blocked-goal WHY routing and the external escalation belong to
goal-board health and the existing escalation path. The two threshold constants
(`2` and `3`) are not env-tunable here; changing them would ripple across many
tests, so convergence deliberately adds an action instead of moving a boundary
(see the
[concept](../concepts/overseer-recurrence-convergence.md#why-the-constants-do-not-move)).

## FAQ

**Will a recurring gap launch a new workstream every single cycle?**
No. At most **one** gap is auto-launched per cycle, and a gap already covered by a
running workstream is suppressed by the in-flight dedup gate before it can launch.
The per-cycle launch cap and the fail-closed recursion guard apply on top.

**Could this cause an auto-launch storm?**
No. One gap per cycle × in-flight dedup × per-cycle cap × fail-closed recursion
guard — and a problem with no recall-backed WHY (`recurrence = 0`) never launches
at all. An auto-launch storm is structurally impossible.

**Can a convergence launch merge or deploy anything?**
No. The launch is classified `RiskClass::Routine` — investigation/coverage work
only. Merge, deploy, and destructive authority are never routed through this rung.

**Can hostile text from cognitive memory reach a `gh` command or the launch?**
No. Every field sourced from the multi-writer memory graph is sanitized before it
reaches a recipe brief, an issue body, or a log line, the `task_description` is
templated from structured data (never raw memory text), and every `gh issue
create` uses argv invocation (no shell string).

**Why didn't `resource:engineer_spawn` escalate?**
Because it was classified **benign membership drift** in the WHY — the admission
gate deferring a spawn under budget/concurrency pressure, a ceiling doing its
job. A genuine **spawn failure** is classified as such and surfaced.

## See also

- [Recurrence convergence API reference](../reference/overseer-recurrence-convergence-api.md)
  — thresholds, the Decide branch, bounding gates, security, and config.
- [Recurrence convergence concept](../concepts/overseer-recurrence-convergence.md)
  — the dead zone this closes and why the constants do not move.
- [Review the Overseer's workstream gaps](./review-overseer-workstream-gaps.md)
  — the notify-only baseline convergence upgrades.
- [Configure the Overseer root-cause principle](./configure-overseer-root-cause-why.md)
  — the WHY and its recurrence count this rung consumes.
- [Watch what the Overseer is doing](./watch-overseer-activity.md) — the surfaces
  a convergence launch renders on.
