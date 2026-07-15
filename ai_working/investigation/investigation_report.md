# Investigation Report: Recurring `goal:blocked` states and recurring `workstream-gap` signatures

**Task type:** Investigation
**Target system:** `src/overseer/`, `src/goal_curation/`, `src/ooda_loop/`
**Date:** 2026-07-15
**Status:** Complete — root cause identified, per-goal actions and systemic fix produced.

---

## Executive summary

The overseer's recurring observation signature — several `goal:blocked` entries
(kgpacks-rs parity/issues, Simard test-coverage audit, coin benchmark harness,
simard-identity personas) alongside repeated `workstream-gap` markers — is **not
a single bug**. It is the visible symptom of **two distinct, coupled control
loops that observe-and-flag but never close**:

1. **Blocked goals** are parked by the **no-progress breaker** after 3 idle
   OODA cycles. The breaker misreads several *non-stuck* conditions (work
   already done, a missing machine-establishable precondition, an unlanded
   upstream dependency, or done-criteria the done-gate cannot check) as
   "genuinely stuck," and parks them with a bare *"needs human review"* marker.
   Because nothing then *resolves* the underlying condition, each cycle re-parks
   the same goals — a steady-state population of recurring `goal:blocked`.

2. **`workstream-gap`** is a **backlog-coverage gap**, *not* "decomposition
   produced zero workstreams" (that is a separate, loud-failing path in
   `decompose.rs`). It fires when a **p1/p2 goal has no assignee and no active
   PR/branch/session/engineer ref**, when a high-signal open issue has no PR, or
   when a live anomaly has no fix in flight. The overseer's response
   (`FlagWorkstreamGaps`) **only notifies the operator — it never launches a
   workstream to close the gap**. So the gap survives, reappears next window, and
   is re-flagged (then suppressed) indefinitely.

The **2x-recurrence** is detected by the overseer's dedup gates and root-cause
recurrence counter, but the recurrence threshold for *escalating the root cause*
is **3** (`RECURRENCE_ESCALATION_THRESHOLD`). A signature "seen 2x" therefore
sits in a **dead zone**: above one-off noise, below the escalation bar, and with
**no auto-remediation rung at all for coverage gaps**. It is observed, deduped,
and re-observed forever without being resolved.

**Systemic fix (one line):** make persistent signals *converge*. Give
`workstream-gap` a recurrence-aware remediation rung (auto-launch a bounded
workstream, or escalate, at 2x rather than only notifying), and ensure the
no-progress **WHY reasoner** is wired so blocked goals route down the
self-resolving ladder instead of parking as bare "needs human review."

---

## 1. Root cause(s) of the recurring `goal:blocked` state

### 1.1 How a goal becomes blocked

`src/goal_curation/no_progress_breaker.rs`

- `NO_PROGRESS_BREAKER_THRESHOLD = 3`: after 3 consecutive **no-action** OODA
  cycles on a goal, the breaker fires.
- Historically it parked the goal with a **bare** `GoalProgress::Blocked` reason
  `{NO_PROGRESS_BLOCKED_PREFIX}{count}{ " consecutive no-action cycles; needs
  human review" }` — it stated *what* (3 idle cycles) but never *why*.
- The documented production incident (`no_progress_why.rs` header): **seven
  `kgpacks-rs` goals were parked as "no progress" when the work was already
  done** — referenced issues `CLOSED`, workstream PRs `MERGED`. The brain kept
  returning `NO ACTION` because there was nothing left to do, but nothing marked
  the goals `Completed`, so the safeguard **misread *done* as *stuck***.

### 1.2 The real root cause: a stall has many causes, only one is "stuck"

`src/goal_curation/no_progress_why.rs` names the stable vocabulary of *why* a
goal reached the breaker (`NoProgressClass`), and `resolution_for_why`
(`no_progress_breaker.rs`) maps each to a self-resolving rung:

| Class | Meaning | Correct resolution |
|---|---|---|
| `AlreadyComplete` | live artifacts satisfy done-criteria (issues CLOSED / PRs MERGED) | **auto-complete** (`MarkDone`) |
| `Obsolete` | work tracked elsewhere / out of scope | **drop** |
| `MissingPrecondition` | machine-establishable precondition absent (e.g. governed repo never cloned) | **self-heal** (clone) + retry (`Heal`) |
| `UpstreamDependency` | blocked on a specific upstream goal/PR/issue not yet landed | **defer** (`Paused`), auto-clears |
| `UnclearCriteria` | done-criteria not expressed as anything the done-gate can check | guided engineer → human |
| `GenuinelyStuck` | no machine-resolvable cause | guided engineer → human |

**Root cause:** when the **WHY reasoner is unwired or misclassifies**, every
stall collapses to the bare *"needs human review"* park (a legacy
`{PREFIX}{count}{SUFFIX}` shape, `is_bare_no_progress_block`). That marker has
**no WHY token**, so the resolution ladder never runs. The goal stays blocked,
the breaker re-observes it next window, and it re-parks — producing the
**recurring `goal:blocked` population** the overseer sees. Only the last two
classes should ever reach a human; when the first four leak into "stuck," a
*self-resolvable* condition becomes a *permanent* block.

### 1.3 Why the specific named goals recur (per-goal cause)

Mapping each recurring goal to its most-likely `NoProgressClass`:

- **kgpacks-rs → full parity** and **kgpacks-rs issues #12/#17/#18/#23/#25**:
  `AlreadyComplete` (the canonical incident — referenced issues CLOSED / PRs
  MERGED but goals never marked `Completed`) and/or `MissingPrecondition` (the
  governed `kgpacks-rs` repo never cloned — see
  `ooda_actions/advance_goal/repo_resolver.rs` and `knowledge_client.rs`). Both
  are **auto-resolvable** (mark-done / clone+retry), so they should *never* be
  human-parked.
- **Audit Simard test coverage to 70%**: `UnclearCriteria`. "70% coverage" is
  not expressed as a done-gate-checkable artifact, so the done-gate can never
  certify completion → the goal idles → breaker parks it → human review → it is
  re-activated with the same uncheckable criterion → it re-parks. A criteria
  problem, not a work problem.
- **Build local coin benchmark harness**: `MissingPrecondition` /
  `UpstreamDependency`. The COIN harness (`src/coin_gym/mod.rs`) depends on
  benchmark data / a runner being present; absent that, the brain has no next
  action → stall → park.
- **simard-identity personas (atelier, bursar, cartographer, concierge,
  gastronome)**: primarily surface as **`workstream-gap` (GoalUncovered)** —
  p1/p2 goals with no assignee and no PR/branch — rather than as *blocked*. When
  they do block, `UnclearCriteria` (persona "done" is under-specified).

**Common infrastructure root cause across all of them:** the loop **observes and
parks but does not resolve**. The corrective machinery (`resolution_for_why`,
auto-complete, self-heal-clone, defer) exists but only helps when (a) the WHY
reasoner is wired and (b) classification is correct. When either fails, the
goal degrades to a bare human-review park that no automated rung clears.

---

## 2. What `workstream-gap` means, its trigger, and the 2x-recurrence

### 2.1 Meaning — a backlog-coverage gap (NOT zero-workstream decomposition)

`src/overseer/sensor.rs::detect_workstream_gaps` (pure, hermetic). A candidate
is a **gap** iff it has **no active workstream AND no open PR** (and, for
anomalies, no fix in flight):

- **Goals** (`GapCategory::GoalUncovered`): an **active, non-blocked** goal at
  priority **p1/p2** (`GAP_GOAL_PRIORITY_BAR = 2`) with **no assignee** and no
  `pr`/`branch`/`session`/`engineer` wip-ref (`goal_has_active_workstream`).
  Blocked goals are explicitly skipped here — they flow through `goal_health`
  instead (no double-notify).
- **Issues** (`IssueUncovered`): an open issue carrying a high-signal label
  (`bug`, `P1`, `workflow:default`) with no PR / active workstream.
- **Anomalies** (`AnomalyUnaddressed`): a live anomaly with no fix in flight.

> **Correction to the investigation hypothesis:** `workstream-gap` is *not*
> "decomposition producing zero/invalid workstreams." That condition is handled
> separately and **loudly** in `goal_curation/decompose.rs`: `decompose_goal`
> requires `MIN_SUBGOALS = 2`; a decomposer that returns `<2` sub-goals surfaces
> a **loud error and leaves the board untouched** — it does not silently emit a
> `workstream-gap`. The overseer signal is a *coverage* gap on the whole work
> picture, surfaced by the recurring gap-scan.

### 2.2 Trigger and flow

`signal.rs` → `mod.rs`:

1. **Observe** projects gaps into `ObservedState::workstream_gaps` (via the goal
   curator's `workstream_gaps` capability, `wiring.rs`).
2. It emits **one consolidated** `Signal::WorkstreamGap { gaps }` per Observe
   pass (`signal.rs:475`).
3. **Orient** classifies it to a **`WorkstreamCoverage`** problem at high
   priority (`tests_gap_scan.rs`).
4. **Act** → `Intervention::FlagWorkstreamGaps` → `act_flag_workstream_gaps`
   (`mod.rs:884`). This **only sends one consolidated operator notification**
   (email + Signal). It files **no** GitHub issue and — critically — **launches
   no workstream**. Signatures: `workstream-gap:goal:{id}` /
   `workstream-gap:issue:{repo}#{n}` / `workstream-gap:anomaly:{slug}`.

### 2.3 The recurring `workstream-gap` marker and how "2x-seen" is detected

Two dedup/recurrence mechanisms combine to produce the *recurring* signature:

- **`gap_gate: WhisperGate::new(900, 200)`** (`mod.rs:304`) — a **15-minute
  dedup window** keyed on `workstream-gap:{signature}`. On each Act pass every
  gap is `peek`ed: a signature already seen within the window is counted as
  **suppressed** (not re-notified), a fresh one is notified and `commit`ted. So
  a gap that persists across windows is re-observed and re-flagged each window —
  this is the mechanical source of the **repeating** marker.
- **Root-cause recurrence** (`root_cause.rs` + `mod.rs::recall_occurrences`):
  the overseer recalls prior same-cause occurrences from cognitive memory and
  raises `RootCause::recurrence`. `decide_blocked_goal` (`mod.rs:1603`) escalates
  the **root cause** only when `recurrence >= RECURRENCE_ESCALATION_THRESHOLD`
  (**= 3**). Rationale strings render *"RECURRING: this cause has been seen N
  time(s) before"* (`root_cause.rs:558`).

**"2x-seen" is therefore a below-threshold recurrence:** the signature has
recurred (proving it is *not* one-off noise) but `recurrence < 3`, so the
overseer neither escalates the root cause nor — for coverage gaps — takes *any*
remediating action. It re-notifies (or suppresses within the window) and moves
on. That dead zone is exactly why the same 2x signatures keep resurfacing.

### 2.4 How the gap signature relates to the blocked goals

They are two views of the **same uncovered-work problem**, deliberately split so
work is never double-notified:

- A **p1/p2 active** important goal with no workstream → `workstream-gap`
  (GoalUncovered).
- The **same goal once the breaker parks it** → leaves the gap-scan (blocked
  goals are skipped in `detect_workstream_gaps`) and reappears as a
  `goal:blocked` via `goal_health`.

So a persistently under-resourced goal **oscillates** between the two
signatures — uncovered gap while active, blocked once idle — which is why the
overseer sees *both* families of markers recurring together for the same
underlying set of goals (personas, coverage audit, coin harness, kgpacks).

---

## 3. Prioritized unblocking actions (per goal)

Priority order: fastest automated resolution first, human-criteria work last.

| # | Goal | Root class | Concrete unblocking action |
|---|---|---|---|
| **P0** | kgpacks-rs → full parity; issues #12/#17/#18/#23/#25 | `AlreadyComplete` / `MissingPrecondition` | Run the **outcome-verify / done-gate** against live artifacts (`goal_curation/outcome_verify.rs`); auto-`MarkDone` any goal whose referenced issues are CLOSED and PRs MERGED. For the rest, **self-heal clone** the governed `kgpacks-rs` repo (`repo_resolver.rs`) and retry. None of these should reach a human. |
| **P1** | Build local coin benchmark harness | `MissingPrecondition` / `UpstreamDependency` | Establish the harness precondition (benchmark data + runner in `src/coin_gym/`); if it depends on an unlanded upstream, **`Defer` (Paused)** with the blocking ref recorded so it auto-clears on landing — do not human-park. |
| **P1** | simard-identity personas (atelier, bursar, cartographer, concierge, gastronome) | `GoalUncovered` workstream-gap | These are p1/p2 goals with **no workstream** — assign an engineer / launch a bounded workstream per persona so they leave the gap set. Split the umbrella goal into 5 independently-verifiable persona sub-goals via `decompose_goal` (2..=6 fan-out) so each carries its own done-criterion. |
| **P2** | Audit Simard test coverage to 70% | `UnclearCriteria` | **Reformulate the done-criterion into a machine-checkable form** the done-gate can certify (e.g. "`cargo llvm-cov` line-coverage ≥ 70% on `cargo test`, artifact committed to CI"). Until the criterion is checkable it will re-park regardless of work done. Route once through a guided engineer to *set the metric*, not to do coverage work blindly. |

Cross-cutting immediate action: run `simard goal unblock-all` **only** after the
above so bare "needs human review" parks are cleared with their real WHY
attached, not blindly re-unblocked (the rejected antipattern).

---

## 4. Systemic fix — stop the 2x signature from recurring

The signatures recur because **persistent signals never converge**: the overseer
*observes and flags* but the two loops lack a *closing* action in the 2x dead
zone. Recommended systemic changes, in priority order:

### 4.1 Close the workstream-gap loop (highest impact)

`FlagWorkstreamGaps` today **only notifies**. Add a **recurrence-aware
remediation rung** so a gap that has been **seen ≥ 2 times** (tracked the same
way root-cause occurrences are, via cognitive memory) is *acted on*, not just
re-notified:

- For a `GoalUncovered` gap → **auto-launch a bounded workstream** via the
  existing `RecipeLauncher`, honoring `max_launches_per_cycle` (currently 2) and
  the board dedup key so it never fights an in-flight engineer.
- Where auto-launch is not safe → **escalate at 2x** instead of only at the
  root-cause threshold of 3, so a proven-recurring gap reaches a human once,
  with its history, rather than looping forever.

This converts the gap-scan from *"tell someone every 15 minutes"* into
*"tell someone once, then fix or escalate on recurrence."*

### 4.2 Unify the recurrence threshold across gaps and blocked goals

Today `RECURRENCE_ESCALATION_THRESHOLD = 3` governs **blocked-goal** root-cause
escalation but **coverage gaps have no recurrence tracking at all** — only the
15-minute `gap_gate` window. Track gap signatures in cognitive memory (as
`PriorOccurrence`s) so the same *"seen N times → escalate/remediate"* policy
applies uniformly. Consider lowering the gap threshold to **2** (a coverage gap
has no benign explanation the way a transient telemetry blip might).

### 4.3 Guarantee the no-progress WHY reasoner is wired and correct

The blocked-goal recurrence is driven by stalls degrading to bare
*"needs human review"* parks. Ensure:

- The `NoProgressWhyReasoner` production impl (`ooda_loop/no_progress.rs`) is
  **always wired** in the daemon so `resolution_for_why` runs — otherwise every
  stall falls back to a bare park with no ladder.
- `reinvestigate_bare_blocked_goals` (`ooda_loop/no_progress.rs`) periodically
  re-classifies existing **bare** parks (`is_bare_no_progress_block`) so legacy
  bare blocks get a WHY retroactively and route down the ladder instead of
  accumulating as a permanent recurring population.
- The **done-gate / outcome-verify** positively certifies `AlreadyComplete`
  (the kgpacks class) so "done" is never misread as "stuck."

### 4.4 Add convergence observability

Emit a metric/counter for **gap signatures that persist ≥ N windows without
remediation** (extend `overseer/activity.rs`
`workstream_gaps_detected/_suppressed`). A rising "persistent-unremediated" count
is the leading indicator that a signature is stuck in the 2x dead zone, and gives
the operator a single number that should trend to zero once 4.1–4.3 land.

---

## 5. Evidence ledger (grounding)

| Claim | Source |
|---|---|
| Breaker fires after 3 idle cycles; bare "needs human review" park | `goal_curation/no_progress_breaker.rs:59,70,75,123` |
| kgpacks "already done" incident; NoProgressClass vocabulary | `goal_curation/no_progress_why.rs:1-26,53-72` |
| Resolution ladder per class | `goal_curation/no_progress_breaker.rs:384-413` |
| `workstream-gap` = backlog-coverage gap; p1/p2 + no workstream | `overseer/sensor.rs:246-372,377-384` |
| Decomposition zero/invalid handled loudly & separately | `goal_curation/decompose.rs:12-15,32-37` |
| Consolidated `Signal::WorkstreamGap`; classifies to WorkstreamCoverage | `overseer/signal.rs:75-79,475-477`; `overseer/tests_gap_scan.rs:298-308` |
| Act path only notifies, never launches a workstream | `overseer/mod.rs:881-948` |
| gap_gate 15-min dedup window (900s) suppress-on-repeat | `overseer/mod.rs:301-304,894-921` |
| Recurrence escalation threshold = 3; "seen N times before" | `overseer/root_cause.rs:28-33,558`; `overseer/mod.rs:1603-1631` |
| Root-cause recall from cognitive memory | `overseer/mod.rs:456,966-1040` |

---

## 6. Success-criteria coverage

1. **Common root cause of blocked goals** — ✅ §1: stalls with self-resolvable
   causes (AlreadyComplete / MissingPrecondition / UpstreamDependency /
   UnclearCriteria) degrade to bare "needs human review" parks when the WHY
   reasoner is unwired/misclassifies; the loop parks but never resolves.
2. **Meaning/trigger of `workstream-gap` + relation to blocked goals** — ✅ §2:
   a backlog-coverage gap (p1/p2 goal with no workstream, high-signal issue with
   no PR, or unaddressed anomaly), flagged-only, deduped on a 15-min window; the
   same goals oscillate between `workstream-gap` (active) and `goal:blocked`
   (parked). 2x = below-threshold recurrence in a dead zone.
3. **Prioritized per-goal unblocking actions** — ✅ §3.
4. **Systemic fix to stop the 2x signature recurring** — ✅ §4: recurrence-aware
   remediation rung that auto-launches/escalates persistent gaps, unified
   recurrence tracking, guaranteed WHY-reasoner wiring, and convergence
   observability.
