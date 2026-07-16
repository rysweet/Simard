---
title: The no-progress breaker explains WHY and self-resolves before escalating
description: Why the OODA no-progress safeguard no longer parks a goal with a bare "needs human review"; how it classifies the ROOT CAUSE of a stall (already-complete, obsolete, missing-precondition, upstream-dependency, unclear-criteria, genuinely-stuck), self-resolves the machine-fixable causes (auto-complete on live artifacts, clone a missing repo, defer behind an upstream, spawn a guided engineer), and only escalates to a human as a last resort with the concrete WHY + evidence attached.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./perpetual-goal-no-progress-exemption.md
  - ./no-progress-terminal-investigation.md
  - ./steerable-ooda-daemon.md
  - ./closed-loop-outcome-verification.md
  - ./overseer-root-cause-why.md
  - ../reference/no-progress-breaker-api.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ../reference/ooda-no-progress-why-recipe.md
  - ../reference/completion-evidence-gate-api.md
  - ../howto/diagnose-a-no-progress-block.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# The no-progress breaker explains WHY and self-resolves before escalating

> **Status: implemented (issue #16).** The root-cause classification, the
> resolution ladder, and the WHY-bearing block reason live in
> `src/goal_curation/no_progress_breaker.rs` and
> `src/goal_curation/no_progress_why.rs`. The side-effecting resolution
> (auto-complete, clone, defer, guided engineer spawn, escalate) lives in the
> curate-phase adapter `src/ooda_loop/no_progress.rs`
> (`apply_no_progress_breaker_investigated`). The optional agentic WHY-narrative
> reasoner is the recipe `prompt_assets/simard/recipes/ooda-no-progress-why.yaml`.
> The root-cause path is on by default; set
> `SIMARD_NO_PROGRESS_INVESTIGATE=off` to fall back to the base verify-once
> ladder. For the exact types and functions see the
> [root-cause resolution API reference](../reference/no-progress-root-cause-resolution-api.md).
>
> **Extended (issue #17):** the same investigation now also runs each cycle over
> goals **already** parked in a bare block — not only at the transition cycle — so
> a goal parked bare by a pre-#16 build (or left bare by a reasoner error) is
> re-investigated and upgraded away from bare instead of stranding forever. See
> [Re-investigating already-blocked goals](#re-investigating-already-blocked-goals-issue-17).

## The defect this fixes

The OODA daemon carries a deterministic **no-progress safeguard**: if one goal
produces `NO_PROGRESS_BREAKER_THRESHOLD` (3) consecutive *no-action* cycles, the
breaker fires. Historically, if the done-gate could not certify the goal
complete or obsolete, the breaker set `GoalProgress::Blocked` with a **bare
sentinel** reason —

```
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive no-action cycles; needs human review
```

— and filed a tracking issue. An operator arriving at that block saw *what*
(three idle cycles) but never *why*, and every non-done, non-obsolete stall went
straight to a human even when the cause was machine-resolvable.

### The motivating incident: "done" misread as "stuck"

The `kgpacks-rs` "already done" incident is the motivating case (the specific
issue/PR numbers below are **illustrative**). A cluster of goals were parked as
"no progress" when the work was **already finished**: the referenced issues were
`CLOSED` and the workstream PRs were `MERGED`. The brain kept returning
`NO ACTION` every cycle **because there was nothing left to do** — but nothing
had marked the goals `Completed`, so after three idle cycles the safeguard
misread a *done* goal as a *stuck* goal and demanded human review. The operator's
"fix" was to close the goals by hand — exactly the toil the autonomous daemon
exists to eliminate.

Other real stalls the old safeguard flattened into the same bare block:

- **Upstream-gated** — the goal is genuinely waiting on another goal/PR/issue
  that has not landed yet. Nothing is broken; it just is not this goal's turn.
- **Missing precondition** — the goal targets a governed repository that was
  never cloned into the workspace, so no cycle can make progress until the clone
  exists.
- **Unclear / unmeasurable done-criteria** — the goal's success gate is not
  expressed as anything the done-gate can check, so it can *never* certify and
  loops forever.

All four are distinct root causes with distinct correct responses. Collapsing
them into one "needs human review" both **hides the diagnosis** and **routes
machine-fixable stalls to a human**.

## The idea: classify the WHY, then act on it

Instead of a single unexplained block, the breaker now runs a **root-cause
investigation** at the threshold, classifies the stall into a
[`NoProgressClass`](../reference/no-progress-root-cause-resolution-api.md#noprogressclass),
gathers structured **evidence** for that classification, and routes down a
**resolution ladder** whose first four rungs keep the goal off the human's desk:

```text
3 consecutive no-action cycles on goal G
        │
        ▼
root-cause investigation — classify WHY + gather evidence (once)
        │
        ├─ ALREADY-COMPLETE      →  auto-complete the goal (attach live artifacts)   ── no block
        ├─ OBSOLETE              →  drop it from the active board                     ── no block
        ├─ MISSING-PRECONDITION  →  self-heal (e.g. clone the repo) + retry           ── no block
        ├─ UPSTREAM-DEPENDENCY   →  defer (Paused) + record the blocking ref          ── no block
        ├─ UNCLEAR-CRITERIA  ┐
        │                    ├─►  spawn ONE guided engineer with the WHY as guidance ── no block (yet)
        └─ GENUINELY-STUCK   ┘
                             └─  if it stalls AGAIN after its one guided retry
                                    →  ESCALATE: block WITH the concrete WHY + evidence, file an issue
```

Only the **last** rung reaches a human, and when it does the block reason is
**never bare** — it carries the classified WHY token and the evidence links.

### The four self-resolving rungs

- **ALREADY-COMPLETE → auto-complete.** When live artifacts satisfy the goal's
  done-criteria (the referenced issues are `CLOSED`, its PRs are `MERGED`, the
  self-change is deployed), the breaker transitions the goal to `Completed` and
  attaches the artifact references as evidence — no block, no issue, no human.
  This is the direct fix for the `kgpacks-rs` "already done" goals: a goal that
  is *done* is now *marked done* instead of parked.

- **MISSING-PRECONDITION → self-heal + retry.** When the only thing standing
  between the goal and progress is an absent governed repository, the breaker
  clones it (reusing the self-deploy source-prep clone path) — or spawns an
  engineer to establish the precondition — resets the no-action counter, and lets
  the next cycle try again. No block.

- **UPSTREAM-DEPENDENCY → defer.** When the goal is waiting on a specific
  blocking goal/PR/issue, the breaker moves it to `GoalProgress::Paused` (a
  deliberate hold, *not* `Blocked`) and records the blocking reference. A goal on
  hold is not a goal that failed, so it never says "needs human review"; it
  **auto-clears** back to `NotStarted` once the upstream resolves.

- **UNCLEAR-CRITERIA / GENUINELY-STUCK → one guided engineer.** When the stall
  has no machine-resolvable cause, the breaker **spawns an engineer** (through the
  same dispatch the OODA loop already uses) with the classified WHY embedded in
  the task ("prior attempts stalled: `<why>`; `<evidence>`") so the engineer
  starts from the diagnosis rather than a cold read. This is bounded to **exactly
  one** guided retry per goal.

### The last rung: escalate WITH the WHY

Escalation to a human happens only when a stall is genuinely unresolvable **or**
a goal has already burned its one guided retry and stalled again. Even then the
`Blocked` reason is required to carry the diagnosis: it starts with the existing
`🔒 [OODA-SAFEGUARD]` sentinel (so every existing consumer keeps working) and
**appends** the classified WHY token plus the evidence links. A human sees the
concrete cause and the artifacts, never a bare "needs human review". See the
[block-reason contract](../reference/no-progress-root-cause-resolution-api.md#block-reason-contract).

## Re-investigating already-blocked goals (issue #17)

The ladder above investigates a stall **only at the cycle the goal crosses the
threshold** — the block *transition*. That leaves a gap: a goal parked in the
**bare** sentinel by a daemon build that predates the root-cause investigation
(issue #16), or one left bare because the reasoner erred on the transition cycle,
sits `Blocked` with the unexplained "needs human review" reason forever and is
never re-examined. The live deploy-#41 incident surfaced exactly this — several
`kgpacks-rs` goals stranded with the bare marker while newer goals got a proper
WHY.

`reinvestigate_bare_blocked_goals` (in `src/ooda_loop/no_progress.rs`) closes the
gap. Each cycle, **after** the on-transition breaker and independent of this
cycle's action outcomes (mirroring the auto-clear scan), it:

1. selects the **bare-blocked, non-perpetual** population via the thin
   deterministic rail
   [`is_bare_no_progress_block`](../reference/no-progress-breaker-api.md) — a
   reason that carries the `🔒 [OODA-SAFEGUARD]` marker but **no**
   `NoProgressClass` WHY token;
2. runs the **same** injected reasoner and the **same**
   [`resolution_for_why`](../reference/no-progress-root-cause-resolution-api.md#extended-noprogressresolution)
   ladder over each — through the shared `apply_resolution_side_effects` driver,
   so the transition and re-investigation populations can never diverge;
3. **un-blocks** a goal handed to a fixer or healed back to `NotStarted` (an
   already-`Blocked` goal the brain would never re-select would otherwise strand
   its own fix), and rewrites every other outcome to its terminal non-bare status
   (`Completed` / dropped / `Paused` / `Blocked`-WITH-why).

The result: **no goal ever remains a bare "needs human review"** — each is
re-classified to a concrete WHY and, when the WHY is actionable, completed,
deferred behind its named upstream, healed, or handed to a spawned fixer.

**Idempotency is two-layered.** The WHY-rewrite is the *primary* guarantee: once
a goal's reason carries a class token it is no longer bare, so the rail excludes
it next cycle. A persisted `(goal, class)` dedupe set on the tracker
(`NoProgressTracker::reinvestigated`, serialized as the stable class **token**
string alongside the no-action counter) is the *belt-and-suspenders* guard: if a
crash/restart re-parks a goal bare after a fixer was already spawned, the dedupe
set short-circuits the terminal action so **at most one fixer is spawned per
`(goal, class)`** across restarts. Re-investigation is **fail-closed**: a reasoner
error leaves the bare marker exactly as-is, records nothing in the dedupe set,
and retries next cycle.

## Classification is deterministic-first, agentic-enriched

The breaker fires **precisely when the agentic loop is failing** to make
progress on a goal — so an LLM classifier asked "why am I stuck?" would be
reasoning about its own failure with the same faculties that produced it. For
that reason the **routing decision is deterministic**, driven by evidence
signals the daemon can gather without the brain:

| Signal | Source | Classifies |
| --- | --- | --- |
| done-gate verdict | [`verify_stuck_goal`](../reference/no-progress-breaker-api.md#noprogressresolution) over the [completion-evidence gate](../reference/completion-evidence-gate-api.md) | `ALREADY-COMPLETE` / `OBSOLETE` |
| governed repo present? | `EvidenceSource::repo_present` | `MISSING-PRECONDITION` |
| dependency goal / PR state | `EvidenceSource::dependency_goal_state` | `UPSTREAM-DEPENDENCY` |
| no tracked PR/issue the done-gate can ever check | — | `UNCLEAR-CRITERIA` (done-criteria structurally unmeasurable) |
| open work still referenced, none of the above | — | `GENUINELY-STUCK` (evidence = the open artifacts) |

The last two rows are the **terminal rung**, split by whether the goal still
references any artifact the done-gate could ever verify. A stall with **no**
tracked PR/issue — the synthetic `simard-identity-*` goals — has done-criteria
that are *structurally unmeasurable*, so it classifies as `UNCLEAR-CRITERIA`
with concrete evidence naming that missing criterion; a stall that still
references open work stays `GENUINELY-STUCK` with those artifacts as evidence.
**Invariant:** the deterministic reasoner never returns an empty-evidence WHY,
so the breaker can never author a bare `evidence=[(none)]` block (the
live-daemon defect that stranded the `simard-identity-*` / coverage / parity
goals with a generic, evidence-free stamp). Both terminal classes route to the
same rung (one guided engineer, then a WHY-bearing human block), so the split
sharpens the *diagnosis and evidence* without changing the action taken.

This mirrors the sibling **brain-failure** safeguard, which is likewise "a
deterministic safeguard enforced by simard, NOT a brain decision — the brain is
broken and cannot be trusted to make decisions about itself."

The **agentic** layer is *optional enrichment*, not routing. An injected
[`NoProgressWhyReasoner`](../reference/no-progress-root-cause-resolution-api.md#noprogresswhyreasoner)
— in production, the
[`ooda-no-progress-why` recipe](../reference/ooda-no-progress-why-recipe.md) run
through the shared `RecipeBrain` — turns the deterministic classification and its
evidence into a **human-readable WHY narrative** for the escalation issue and
block reason. If the reasoner errs or is absent, the breaker falls closed to the
deterministic WHY token; the reasoner **never** changes which ladder rung is
taken. This keeps the agentic "repeated structured thought" where it adds
value (explaining the stall to a human) without letting a failing brain talk
itself out of a safeguard.

## Fail-closed, always

Every uncertain branch fails **closed** — it takes no terminal action rather
than guessing:

- A **reasoner error** downgrades to the deterministic WHY (never blocks or
  completes silently on the reasoner's behalf); it is logged at `error`.
- An **evidence-source error** on `repo_present` / `dependency_goal_state` is
  treated as `UNCLEAR` (the breaker never self-completes or self-heals on an
  *unknown* state).
- A **clone failure** on the MISSING-PRECONDITION rung escalates with the clone
  error attached as evidence.
- An **auto-complete** only fires when the done-gate positively certifies the
  goal; absence of evidence is never read as completion.

The invariant: the breaker never *silently* blocks and never *silently*
completes. A stall either self-resolves with recorded evidence, or reaches a
human with the WHY attached.

## How this relates to the sibling gates

Three gates touch a stalled goal; keeping them distinct is the point.

| Gate | Question it answers | Where |
| --- | --- | --- |
| **Completion-evidence gate** | "Is this goal *actually* done, by live artifacts?" | [`completion_gate.rs`](../reference/completion-evidence-gate-api.md) |
| **No-progress root-cause resolution** (this page) | "Given the goal made no progress, *why*, and what is the least-human-cost fix?" | `no_progress_breaker.rs` + `no_progress.rs` |
| **Perpetual-goal exemption** | "Is this a bursty standing goal that is *allowed* to idle?" | [`perpetual-goal-no-progress-exemption`](./perpetual-goal-no-progress-exemption.md) |

The perpetual exemption still runs **first**: a standing/perpetual goal has its
counter reset and stays active *before* classification, so the ladder never
applies to it. Only a non-perpetual goal that has genuinely idled to the
threshold is classified and routed. And the **load-time self-heal**
(`heal_stale_no_progress_blocks`) still clears stale perpetual blocks left by
older builds; it recognises the sentinel by its unchanged `🔒 [OODA-SAFEGUARD]`
prefix, so the appended WHY does not disturb it.

## What an operator sees now

- A `kgpacks-rs`-style goal whose issues are closed and PRs merged now shows
  `completed` with the artifacts recorded — **not** a block.
- A goal targeting an un-cloned repo triggers the clone and keeps trying —
  **not** a block.
- A goal waiting on an upstream shows `paused` with the blocking ref — **not**
  "needs human review" — and resumes itself when the upstream lands.
- A genuinely stuck goal gets one guided engineer; if that fails, it blocks with
  a reason that *names the cause and links the evidence*.

See the [how-to: diagnose a no-progress block](../howto/diagnose-a-no-progress-block.md)
for reading the WHY and the per-branch examples, and the
[root-cause resolution API reference](../reference/no-progress-root-cause-resolution-api.md)
for the exact types.

## See also

- [No-progress breaker API reference](../reference/no-progress-breaker-api.md) — the base breaker, tracker, sentinel, and self-heal.
- [Root-cause resolution API reference](../reference/no-progress-root-cause-resolution-api.md) — the classification, WHY types, reasoner, and extended resolution ladder.
- [The `ooda-no-progress-why` recipe reference](../reference/ooda-no-progress-why-recipe.md) — the optional agentic WHY-narrative recipe.
- [Standing/perpetual goals are exempt from the no-progress hard-block](./perpetual-goal-no-progress-exemption.md) — the exemption that runs before this ladder.
- [Closed-loop outcome verification](./closed-loop-outcome-verification.md) — the sibling "artifact ≠ outcome" gate.
- [Diagnose a no-progress block](../howto/diagnose-a-no-progress-block.md) — the operator runbook.
