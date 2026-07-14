---
title: Configure and observe the Overseer root-cause ("WHY") principle
description: >
  Operator guide for the Overseer's mandatory root-cause principle — how EVERY detected
  problem gets a structured WHY before the Overseer acts, how to read the WHY and the
  root-cause/symptom label in the overseer::root_cause traces, the OverseerTickReport
  counters, the activity-feed problem_entries rows, and the goal-blocked operator
  notification; how the memory-recall enrichment (amplihack-memory-lib) accumulates
  recurrence and degrades gracefully when memory is absent; how a repeatedly-re-blocked
  perpetual goal is escalated as a deduped root cause instead of re-unblocked; that the
  analysis is always-on with no opt-out; and how to verify the feature end-to-end with
  injected fakes.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/overseer-root-cause-why.md
  - ../reference/overseer-root-cause-why-api.md
  - ../design/overseer.md
  - ./watch-overseer-activity.md
  - ./configure-overseer-goal-board-health.md
  - ./unblock-stuck-ooda-goals.md
  - ./set-up-the-signal-channel.md
---

# Configure and observe the Overseer root-cause ("WHY") principle

> **Status: design — not yet implemented** (issue
> [#2635](https://github.com/rysweet/Simard/issues/2635)). This runbook specifies
> the operator surface the implementing PR will deliver for the feature we are
> about to build — the `overseer::root_cause` traces, the extended
> `OverseerTickReport` counters, the activity-feed `problem_entries` rows, the WHY
> line in the `goal-blocked` operator notification, and the `cargo test` targets
> below. Documentation and code land in the **same pull request**; until then this
> describes the specified behavior, not shipped code.

The acting **Overseer** asks **why** before it acts. On every `Problem` it
detects, it produces a structured root-cause analysis — the WHY — and picks an
action that targets the root cause when possible. A symptom-only mitigation is
explicitly labelled, with the still-live root cause surfaced (see
[Overseer root-cause ("WHY") principle](../concepts/overseer-root-cause-why.md)).
This runbook shows how to configure, read, and verify that behavior.

## There is nothing to enable — it is always-on

Root-cause analysis is **mandatory and always-on**. There is **no flag to turn it
off**, because the operator principle is "ALWAYS ask WHY, don't just patch the
symptom". You do not opt in and you cannot opt out.

Two things are still governed by existing gates:

- **The acting paths the WHY feeds** (goal-board self-heal / escalation) are
  governed, as before, by
  [`SIMARD_OVERSEER_GOAL_HEALTH`](./configure-overseer-goal-board-health.md)
  (enabled unless set to a falsey value). The **analysis itself** is not gated —
  even when an action is suppressed, the WHY is still computed and surfaced.
- **The whole Overseer** is governed by
  [`SIMARD_OVERSEER_ENABLED`](../reference/overseer-goal-board-health-api.md).

```bash
# The Overseer (and therefore its root-cause analysis) runs by default.
# Explicitly on:
export SIMARD_OVERSEER_ENABLED=1
# Goal-health acting paths on by default; disable only the ACTIONS (WHY still logged):
# export SIMARD_OVERSEER_GOAL_HEALTH=0
```

The recurrence threshold `N` — the number of recalled same-signature occurrences
at which a repeatedly-mitigated problem is escalated as a systemic root cause
instead of mitigated again — is the compile-time constant
`RECURRENCE_ESCALATION_THRESHOLD` (default **3**).

## Attach cognitive memory for recurrence (optional, recommended)

The WHY is enriched by cognitive-memory recall (amplihack-memory-lib): the
Overseer recalls prior same-signature occurrences to learn what caused this
problem before, and stores each occurrence so future recall is richer. In
production the daemon wires this automatically via `build_overseer`
(`Overseer::with_memory(mem)`).

When cognitive memory is **absent or errors**, the analysis degrades gracefully
to telemetry-only reasoning — the WHY still appears, with `source = Telemetry`
and `recurrence = 0` — and the degrade is **logged**, never silent:

```
DEBUG overseer::root_cause degrade=telemetry-only reason="no memory handle" dedup_key=...
WARN  overseer::root_cause store_failed dedup_key=... err=...
```

No configuration is required to get this behavior; it is automatic.

## Read the WHY in the traces

Each tick emits structured `tracing` events. The root-cause analysis and the
remediation label appear under `overseer::root_cause` and on the act events:

```
INFO overseer::root_cause problem=goal:continuous-research kind=GoalHygiene \
     why="perpetual goal parked by no-progress safeguard (false park) (confidence: High, source: Both, seen 4× before)" \
     confidence=High source=Both recurrence=4 candidates=3
INFO overseer::act intervention=file_issue goal_id=continuous-research \
     remediation=root_cause root_cause_addressed=true \
     why="perpetual goal parked by no-progress safeguard (false park) ..."
```

A symptom-only action is labelled explicitly, with the live root cause surfaced:

```
INFO overseer::act intervention=report \
     remediation=symptom_mitigation root_cause_addressed=false \
     unaddressed="schema/format drift in distill payload"
```

A **deliberate** operator/dependency block is a respectful no-op labelled
`acknowledged` — it is *not* a symptom mitigation and raises no alarm:

```
INFO overseer::act intervention=report \
     remediation=acknowledged root_cause_addressed=true \
     why="goal deliberately blocked by operator (confidence: High, source: Telemetry)"
```

Filter for the WHY across a running daemon:

```bash
journalctl -u simard -f | grep -E 'overseer::(root_cause|act)'
# or, if logging to a file:
tail -f ~/.simard/logs/simard.log | grep -E 'overseer::(root_cause|act)'
```

## Read the per-tick counters

Each `OverseerTickReport` (emitted as `tracing` keys and recorded to the activity
feed) gains three scalar counters:

| Counter | Meaning |
|---|---|
| `root_cause_analyses` | Problems for which a structured WHY was produced this tick |
| `root_causes_addressed` | Interventions this tick that addressed the root cause — includes deliberate blocks correctly `acknowledged` |
| `symptom_mitigations` | Interventions this tick that only mitigated the symptom (root cause left unaddressed); a deliberate `acknowledged` block is **not** counted here |

```
INFO overseer::tick problems=2 root_cause_analyses=2 root_causes_addressed=1 \
     symptom_mitigations=1 goals_unblocked=0 issues_filed=1 duration_ms=812
```

A non-zero `symptom_mitigations` is your cue that a root cause is still live —
open the activity feed (below) to see which one. A deliberate operator block is
`acknowledged` (not counted here), so an intentional block never false-alarms you.

## Read the WHY in the activity feed

The durable [Overseer activity feed](./watch-overseer-activity.md) now carries a
per-problem row for each problem handled in a tick. From the dashboard
**Overseer** tab, the TUI **Overseer** pane, `simard status`, or
`GET /api/overseer`, each tick shows:

```
Overseer tick 2026-07-06T04:20:11Z (1 symptom-mitigation, root cause unaddressed)
  • goal:continuous-research — WHY: perpetual goal parked by no-progress safeguard
      (false park) (confidence: High, source: Both, seen 4× before)
      — file_issue [root-cause]
  • process:distill-fail — WHY: schema/format drift in distill payload
      (confidence: Medium, source: Telemetry) — report [symptom]
```

Over the REST API, each record's `problem_entries` array carries the structured
`ProblemEntry { key, summary, why, action, remediation }` (see the
[API reference](../reference/overseer-root-cause-why-api.md#activity-feed-activityrs)),
so you get the machine-readable WHY, not just the rendered line:

```bash
curl -s -H "Authorization: Bearer $SIMARD_DASHBOARD_TOKEN" \
     http://127.0.0.1:8787/api/overseer | jq '.recent[0].problem_entries'
```

## Read the WHY in an operator notification

When the Overseer escalates a genuinely-blocked goal, the `goal-blocked`
notification (email + Signal) now includes the diagnosed cause, so the escalation
reaches you **with** the WHY, not just the symptom:

```
Subject: goal continuous-research needs human review

Goal `continuous-research` is blocked and needs human review.
  Reason: 🔒 [OODA-SAFEGUARD] no progress in 5 cycles — needs human review
  Why: perpetual goal starved by higher-priority work (confidence: Medium, source: Both, seen 3× before)
```

See [set up the Signal channel](./set-up-the-signal-channel.md) to make sure the
escalation actually reaches you.

## What happens to a repeatedly-blocked perpetual goal

This is the antipattern the principle eliminates. Instead of re-unblocking the
same perpetual goal every cycle, the Overseer routes on the recalled recurrence:

- **First time** it sees a false-parked perpetual goal (`recurrence < N`) → it
  self-heals with a one-off `UnblockGoal` (labelled **root-cause**), exactly as
  [goal-board health](./configure-overseer-goal-board-health.md) always did.
- **When it keeps getting re-parked** (`recurrence >= N`) -> it stops
  re-unblocking and submits a typed `recurring_goal_reblock` proposal. Eligible
  non-stewardship lineage may produce an issue only through the durable mutation
  guard. Stewardship or unknown ancestry is rejected instead of becoming
  another goal or issue.

If the guarded proposal creates such an issue, the WHY in the issue body tells you
the diagnosed root cause; fixing that (e.g. applying the perpetual tag, or the
no-progress exemption from
[#2589](../concepts/perpetual-goal-no-progress-exemption.md)) stops the recurrence
at the source.

## Verify it end-to-end

The feature is covered by hermetic tests (existing capability fakes plus an
in-memory `CognitiveMemoryOps`) — no network, no `~/.simard`, no real board.

```bash
# The root-cause suite:
cargo test -p simard overseer::root_cause

# The whole Overseer tier (root-cause + goal-health + whisper + M1/M2):
cargo test -p simard overseer::

# Lint / format gates the CI enforces:
cargo clippy --all-targets -- -D warnings
cargo fmt --check
pre-commit run --all-files
```

The suite asserts the operator-visible guarantees:

1. **Every problem gets a WHY.** A synthetic `ObservedState` with a blocked
   perpetual goal and a high `distill_fail_pct` → each `Problem` carries a
   populated `why`; `RootCause::to_string()` is a non-empty, human-readable line
   naming a primary cause.
2. **Symptom actions are labelled.** A recurring re-block (`recurrence ≥ N`) is
   **not** a blind re-`UnblockGoal`; a symptom-only action is
   `Remediation { class: SymptomMitigation, root_cause_addressed: false,
   unaddressed_note: Some(_) }`, and an eligible recurring perpetual re-block
   routes through `GitHubMutationGuard`. A **deliberate** operator/dependency block is
   instead `Remediation { class: Acknowledged, root_cause_addressed: true,
   unaddressed_note: None }` and leaves `symptom_mitigations` unincremented — no
   false alarm.
3. **The WHY is in the feed.** After a tick,
   `OverseerActivityRecord.problem_entries` contains the entry with its `why`,
   `action`, and `remediation`; `humanize_tick` mentions the symptom-mitigation
   count when > 0.
4. **First-time false-park still self-heals** (recurrence 0 → `UnblockGoal`,
   root-cause) — no regression of the #2609 goal-health behavior.
5. **Graceful memory degrade.** With no memory handle, the analysis still yields
   a `RootCause` (`source = Telemetry`, `recurrence = 0`); no panic, no silent
   drop.
6. **Notifications carry the WHY.** The escalation notification body contains the
   `Why:` line.
7. **Recurrence accumulates.** Two ticks on the same signature → the second
   tick's recall observes `recurrence ≥ 1`.

## Guarantees

- **Always-on:** every detected problem gets a WHY; there is no opt-out.
- **Never silent:** a symptom-only action is always labelled and its live root
  cause surfaced in the feed, counters, and (for goal blocks) the notification.
- **No false alarms:** a deliberate operator/dependency block is labelled
  `acknowledged` (addressed), never `symptom-mitigation`, so an intentional block
  never inflates `symptom_mitigations` nor raises the "root cause unaddressed" alarm.
- **Graceful degrade, logged:** missing / erroring memory drops to telemetry-only
  reasoning with a `tracing` log — never a silent fallback.
- **No fighting itself:** recurring root causes are escalated via a **deduped**
  issue on the root-cause signature, not re-patched every cycle.
- **Additive:** every field is additive and defaulted; existing activity-feed and
  report JSON deserialize unchanged.

## See also

- [Overseer root-cause ("WHY") principle](../concepts/overseer-root-cause-why.md)
  — the design rationale and the antipattern it eliminates.
- [Overseer root-cause ("WHY") API reference](../reference/overseer-root-cause-why-api.md)
  — the exact types, fields, and functions.
- [Configure and observe Overseer goal-board health](./configure-overseer-goal-board-health.md)
  — the self-heal / escalate actions the WHY routes.
- [How to watch what the Overseer is doing](./watch-overseer-activity.md)
  — the activity-feed surfaces the WHY appears in.
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md)
  — the operator counterpart when a root cause needs manual intervention.
