---
title: OODA loop self-detection, reflectiveness, and proactivity
description: How Simard reasons about whether she is making real progress or spinning in a loop, breaks the loop by changing strategy, keeps open-ended goals bounded, and proactively pulls fresh work — all via prompt-asset content, no Rust changes.
last_updated: 2026-06-25
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ./prompt-driven-ooda-brain.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../howto/edit-the-ooda-brain-prompt.md
  - ../reference/ooda-decide-prompt.md
---

# OODA loop self-detection, reflectiveness, and proactivity

Simard's autonomous OODA daemon advances one goal per cycle. Without a sense of
its own recent history it can fall into a **productive-looking loop**: every
cycle it re-triages the same pull requests, finds nothing new to do, re-records
the same completion percentage, and repeats — making zero shippable progress
while *looking* busy. This is distinct from the brain-failure lockout described
in [Unblock OODA goals stuck after a brain-failure
lockout](../howto/unblock-stuck-ooda-goals.md): the brain here is "healthy" and
confidently emitting `advance_goal`; the problem is that the *content* of each
cycle never changes.

This document describes the prompt-level design that makes Simard **reflective**
(judging whether a cycle produced real progress), **loop-aware** (noticing when
she is repeating herself), and **proactive** (pulling fresh work instead of
idling). It is implemented entirely in the prompt assets under
`prompt_assets/simard/` — there are **no Rust logic changes**, and the prompt
content hot-reloads onto the live daemon.

## The loop in one picture

```
observe ─► orient ─► decide ─► advance goal ─► "triage the PRs"
   ▲                                                 │
   └─────────  nothing new shipped; 99% recorded  ◄──┘   (repeat forever)
```

The fix is to insert an explicit **"am I looping?"** judgment before the goal
action defaults to triage, and to give the supporting brains and the goal
curator the vocabulary to recognise and escalate stalled, open-ended goals.

## Progress signals vs. non-progress

Every reflective check uses the same definition of progress, so the brains agree
on what "moving" means.

| Real progress (at least one, since last cycle) | Not progress (no matter how busy it feels) |
|---|---|
| A new commit SHA on a goal branch | Re-triaging / re-reviewing the same PRs, finding nothing new |
| A PR opened, substantively updated, or merged | Re-reading the same issue or goal description |
| An issue closed | Re-reinforcing the same procedure |
| A completion-% increase **backed by a shipped artifact** | Re-recording the same completion-% (e.g. parked at 99%) with no new artifact |

## Where each behavior lives

The behaviors are spread across the prompts that already drive each OODA phase,
so each brain enforces the part it is best positioned to see. The legacy
embedded `*.md` copies (served by `PromptStore`) and the live recipe YAMLs under
`prompt_assets/simard/recipes/` are kept in lock-step.

| Behavior | Prompt asset(s) | What changed |
|---|---|---|
| **Loop self-detection + change strategy** | `goal_session_objective.md` | A leading "are you making progress, or looping?" section makes the goal-action brain reason over its recalled episodes/procedures before triaging, and pick a *different* action when stuck. |
| **Execute over triage** | `goal_session_objective.md` | The Priority Order triage is reframed as a **quick first pass, not a perpetual gate**; new implementation work is no longer deferred indefinitely. |
| **Churn vs. progress at the engineer site** | `ooda_brain.md`, `recipes/ooda-engineer-lifecycle.yaml` | The engineer-lifecycle brain treats a high `consecutive_skip_count` with no new artifact as a stuck loop and prefers `deprioritize` / `open_tracking_issue` (existing variants — no new output kinds). |
| **Surface loops while routing** | `ooda_decide.md`, `recipes/ooda-decide.yaml` | The decide brain still routes to `advance_goal` (kind unchanged) but names the suspected loop in its rationale so the goal-action re-scopes. |
| **Open-ended goal escalation** | `progress_assessment_reviewer.md`, `recipes/progress-assessment.yaml` | A re-asserted high percent with only re-triage in the plan and no new artifact is **rejected** as stalled, pushing decompose/complete/demote instead of an indefinite 99%. |
| **Open-ended goal hygiene + proactivity** | `goal_curator_system.md` | Unbounded goals must be expressed as concrete, completable sub-goals with `done-when` criteria; when the board has room and the backlog is empty, the curator proactively proposes work from Simard's own open GitHub issues rather than idling. |

## Breaking the loop: the three strategies

When the goal-action brain detects a loop it must choose a **different** next
action and express it through one of the existing two response shapes (spawn an
engineer, or `NO ACTION` with a note/`PROGRESS:` update):

1. **Decompose and execute.** Open-ended goals (no natural 100% — e.g. "increase
   test coverage across the ecosystem") are carved into one concrete, completable
   sub-goal with an explicit done-criterion, then an engineer is spawned to
   actually write the tests/code and open the PR. The bias is toward **shipping**,
   not more triage.
2. **Complete or retire.** If no further bounded progress is possible, the goal
   is recorded complete (`PROGRESS: 100` with a note) or demotion is recommended —
   never parked at 99% forever.
3. **Pull fresh concrete work.** When active goals are under the cap and the
   backlog is empty, a specific open GitHub issue Simard owns is proposed as a new
   concrete goal. Operator gating still governs promotion to active, so Simard
   **proposes** rather than silently spins.

## Output contracts are unchanged

This is deliberately a content-only change. Every wire contract the Rust parsers
depend on is preserved:

- `goal_session_objective.md` — **prose only** (no JSON, no code fences); the
  `Priority Order` section still precedes `Two response shapes`.
- `ooda_decide.md` / `recipes/ooda-decide.yaml` — the action keyword is still the
  routed kind (`advance_goal` for ordinary goals); loop observations live in the
  rationale.
- `ooda_brain.md` / `recipes/ooda-engineer-lifecycle.yaml` — the six lifecycle
  variants are unchanged; loops are broken with `deprioritize` /
  `open_tracking_issue`.
- `progress_assessment_reviewer.md` / `recipes/progress-assessment.yaml` — the
  single-line `{"verdict": …}` JSON contract is unchanged; stalled goals simply
  resolve to `reject`.

Prompt-content tests in `src/ooda_brain/prompt_store_tests.rs` pin this wording
so a future edit that drops the loop-detection guidance fails CI.

## Known follow-ups (would need Rust)

The prompt level can *reason about* and *recommend* these, but a few would be
more robust with code (filed as follow-ups, not implemented here):

- Feeding a structured per-goal **action-history digest** (last N actions +
  measured progress deltas) into the Orient/Decide context, rather than relying
  on recalled episodes.
- A first-class **`decompose_goal`** action kind so the daemon can split an
  open-ended goal automatically instead of asking the goal-action brain to do it
  in prose.
- Automatic **archival/demotion** of a goal that the progress reviewer has marked
  stalled for K consecutive cycles.

## See also

- [Prompt-driven OODA brain](./prompt-driven-ooda-brain.md)
- [How-to: unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md)
- [How-to: edit the OODA brain prompt](../howto/edit-the-ooda-brain-prompt.md)
- [Reference: OODA decide prompt schema](../reference/ooda-decide-prompt.md)
