---
title: Agentic observe/orient merge-queue + issue reasoning
description: >
  Why Simard's OODA observe/orient stage now REASONS agentically over the whole
  open-PR merge queue and open-issue backlog across the governed roster every
  cycle, instead of the brittle imperative SIMARD_AUTOMERGE_REPOS allowlist gate
  that — with the env var unset in production — produced ZERO merge reasoning
  while ~30 CI-green mergeable PRs piled up open. The thin deterministic rail
  runs an agentic recipe (idle/liveness only, no wall-clock timeout) whose
  bounded semantic brief populates new ObservedState fields; the merge ACTION
  stays behind the UNCHANGED objective + agentic gate, dual-channel notify, and
  anti-recursion author guard. Broadening REASONING never widens AUTHORIZATION,
  and disabling reasoning is LOUD, never silent.
last_updated: 2026-07-19
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./autonomous-self-merge-sensor.md
  - ./operational-autonomy-model.md
  - ./autonomous-merge-review-gate.md
  - ./enrichment-observability.md
  - ../design/agentic-observe-orient-merge-queue.md
  - ../reference/agentic-merge-queue-reasoning-api.md
  - ../reference/cross-repo-merge-authority.md
  - ../howto/configure-agentic-merge-queue-reasoning.md
  - ../howto/triage-stale-pull-requests.md
---

# Agentic observe/orient merge-queue + issue reasoning

> **Status: implemented.** This page describes the shipped observe/orient
> reasoning pass in present tense. It replaces the dead-wire allowlist sensor
> that kept Simard from reasoning about her own merge queue.

## The problem: a silent hard-OFF that produced zero reasoning

Simard's autonomous-merge chain was built and enabled, but the *only* thing that
fed it — the observe-path `ready_prs` sensor — reasoned about nothing in
production. The sensor lists only PRs whose author matches
`SIMARD_AUTOMERGE_AUTHOR` in a repo on the `SIMARD_AUTOMERGE_REPOS` allowlist. In
the live systemd unit **both env vars are unset**:

```
automerge_repos() == []      → survey_ready_prs(&[]) == []   (gh never called)
                             → ObservedState.ready_prs == []  (ALWAYS empty)
                             → the Overseer never reasons about ANY open PR
                             → prs_merged = 0 for 36h+, ~30 mergeable PRs open
```

Worse than empty, it was **silent**: an unset allowlist produced no reasoning
*and* no signal that reasoning was off. The observe/orient stage never
enumerated the open-PR queue or the issue backlog at all — the imperative
allowlist sensor was the sole path, and it was dead-wired.

That is a policy heuristic masquerading as a sensor. "Which of ~30 open PRs are
ready for action, and which issues need a workstream?" is **judgement**, not a
fixed predicate.

## The fix: reason agentically behind a thin deterministic rail

Following Simard's standing convention — *solve control-loop decisions as
agentic recipes behind a THIN deterministic rail, not imperative heuristics* —
the observe/orient stage now runs an **agentic reasoning pass** each Overseer
cycle:

1. **Survey open issues** across the governed roster + Simard and **triage** them
   (priority, readiness, next action) — pm-architect style.
2. **Survey open PRs** and **reason** about which are ready for action or merge —
   CI state, mergeable/review state, conflicts, staleness, duplication — the way
   the operator would: "check all open PRs and think about which are ready."
3. **Feed those conclusions into Decide** so ready PRs get actioned through the
   existing merge gate, stalled PRs get flagged, and duplicates get closed.

The reasoning lives in a recipe + prompts
([`observe-merge-queue`](../design/agentic-observe-orient-merge-queue.md#3-prompts--the-substance)).
Rust is a **thin rail**: it schedules the recipe (idle/liveness supervised, **no
wall-clock timeout**), parses its bounded brief fail-closed into new
`ObservedState` fields, and re-derives the *authorized* subset through the
unchanged merge gate.

## The load-bearing boundary: reasoning is broad, authorization is narrow

| | Agentic reasoning (this feature) | Authoritative action gate (unchanged) |
|---|---|---|
| **Role** | Reasons over the *whole* queue + backlog | Decides whether to merge / comment / close |
| **Scope** | Governed roster, **default-ON** | Per selected PR |
| **Produces** | A bounded semantic *proposal* | An authorized action |
| **Merge?** | **Never** — proposes only | Yes: `gh pr merge --squash --delete-branch` |
| **Gate** | none (it's reasoning) | objective gates + `MergeJudge` + author guard |
| **Fail mode** | fail-closed → empty sets + WARN | fail-closed → refuse |

The seam that keeps these apart is the **`reasoned_prs → ready_prs` re-narrowing
projection**: the agent reasons about every open PR, but a PR becomes a merge
candidate only if it independently re-passes the anti-recursion author guard, the
engineer-PR narrowing (`simard-autonomous` label OR engineer-exclusive branch
namespace), and the objective gates (base allowlist + `MERGEABLE` + all checks
green). A `ready-for-merge` *disposition* from the agent is a proposal; the
projection is the authorization. **Broadening reasoning never widens the action
gate.**

## Safety posture

- **Reasoning default-ON, disablement LOUD.** With
  `SIMARD_MERGE_REASONING_SCOPE` unset, Simard reasons over the governed roster
  (the fix for the zero-reasoning bug). Only an *explicit* `off`/`disabled`
  value turns reasoning off — and when it does, the daemon emits a `WARN`, sets
  `ObservedState.merge_reasoning_status`, and sends a **one-time** dual-channel
  `NotifyOperator` note "merge reasoning DISABLED". Unset ≠ disabled; nothing is
  ever silently off.
- **Action gate unchanged.** Every merge still passes
  [`stewardship::merge_authority`](../reference/cross-repo-merge-authority.md)
  objective gates + the `MergeJudge` (fail-closed) + the anti-recursion author
  guard + the engineer-PR narrowing. **No path uses `--admin` or
  `--no-verify`** (asserted by unit test and the repo-wide grep guard).
- **Brief is DATA, not COMMANDS.** The reasoning prompt is read-only (`gh pr
  list`/`view`, `gh issue list`/`view`, `gh pr checks` — never a write). Rust
  re-derives every action from objective state; PR/issue refs are validated
  against the governed roster; the parse is fail-closed (XPIA-hardened).
- **New interventions are gated and injection-proof.** `FlagStalePr`
  (`gh pr comment`) and `CloseDuplicatePr` (`gh pr close`) are `RiskClass::MergeAuthority`
  opt-in (notify-only when off), build **positional argv only** (no shell),
  never contain `--admin`/`--no-verify`, and respect the author guard — they act
  only on Simard's own engineer PRs, never an operator's review PR.
- **Dual-channel notify on every merge.** Autonomous or not, every merge sends a
  concise problem + PR summary to `rysweet` on **email and Signal**.
- **No wall-clock timeouts.** The agentic step is supervised by idle/liveness
  detection only.
- **Fail-visible.** Any recipe/parse fault yields empty reasoned/triaged sets
  plus a `WARN`, never a fabricated PR or a silent wrong action.

## Where it lives in the loop

The reasoning pass runs in the acting `run_cycle` enrichment path
([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)),
alongside the other enriched Observe fields. The thin rail lives in
[`src/overseer/merge_queue_observe.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_queue_observe.rs)
and mirrors the [`ecosystem-observe`](../design/ecosystem-observe.md)
`RecipeEcosystemObserver` seam. `observed_from_snapshot` stays a side-effect-free
projection (its unit tests still assert empty IO fields); the reasoned fields are
populated only in enrichment.

For the exact API — the `ObservedState` fields, the scope resolver, the seam
trait, the signals and interventions — see the
[agentic merge-queue reasoning API reference](../reference/agentic-merge-queue-reasoning-api.md).
For the full design, see
[the design spec](../design/agentic-observe-orient-merge-queue.md). To configure
and verify it, see
[how to configure agentic merge-queue reasoning](../howto/configure-agentic-merge-queue-reasoning.md).
