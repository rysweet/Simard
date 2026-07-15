# Tertiary Investigation — Architecture: recurrence-aware remediation rung & WHY-reasoner wiring guarantee

**Role:** Tertiary investigator (architect). **Date:** 2026-07-15.
**Scope:** Design (not implement) the systemic fix for the recurring
`workstream-gap` + `goal:blocked` signature. Anchors to `overseer/wiring.rs`,
the `FlagWorkstreamGaps`/`launch.rs` seam, and the `no_progress` breaker wiring.
Builds on `investigation_report.md` (primary/secondary) and does not restate it.

---

## 0. New architectural finding — the WHY-reasoner is *double-gated*, not wired

The prior report says the reasoner must be "always wired." Tracing the actual
seam (`ooda_loop/cycle.rs:582-702`) shows it is **conditionally** wired behind
two independent switches, and both failure modes are *silent*:

```
cycle.rs:582  if let Some(source) = &memories.completion_evidence {   // GATE A
cycle.rs:583      if no_progress_investigation_enabled() {             // GATE B
                      // DeterministicNoProgressReasoner + resolution_for_why ladder
                      // + reinvestigate_bare_blocked_goals   ← the ONLY self-resolving path
                  } else {
cycle.rs:685          apply_no_progress_breaker(...)                   // bare-park base ladder
                  }
              } else {
cycle.rs:700      Vec::new()   // NO breaker at all — goals blocked elsewhere stay bare
              }
```

- **Gate A** (`memories.completion_evidence.is_some()`): supplied only in the
  production daemon when `SIMARD_COMPLETION_EVIDENCE` wires an `EvidenceSource`
  (`no_progress.rs` header; mirrors `SIMARD_OUTCOME_VERIFY`). If absent, the
  **entire** breaker block collapses to `Vec::new()` — no classification, no
  ladder, no re-investigation of existing bare parks.
- **Gate B** (`no_progress_investigation_enabled()`, `no_progress.rs:203-207`):
  default `true`, but `SIMARD_NO_PROGRESS_INVESTIGATE=off` downgrades to
  `apply_no_progress_breaker` — the base verify-once ladder that authors the
  legacy `{PREFIX}{count}{SUFFIX}` bare "needs human review" block
  (`no_progress_breaker.rs:75`).

**Architectural defect:** the self-resolving ladder — the one mechanism that
prevents the recurring `goal:blocked` population — is an *opt-in that fails
open to bare-park*. There is no invariant that a `goal:blocked` reason ever
carries a `NoProgressClass`. Two env vars, a test harness that leaves
`completion_evidence = None`, or a partial daemon boot each silently reproduce
the exact recurring-park symptom under investigation. This is the precise
target of the "WHY-reasoner wiring guarantee" below.

---

## 1. The shared anti-pattern: flag-without-close

Both loops share one control-loop shape: **Observe → Orient → Decide a
terminal, non-converging action → re-Observe the unchanged world.** The Decide
table (`overseer/mod.rs:1400-1582`) already routes *four* problem kinds to a
**closing** action `Intervention::LaunchRecipe { brief }` via the
`RecipeLauncher`/`launch.rs` seam:

| ProblemKind        | Decide arm → action                        | Converges? |
|--------------------|--------------------------------------------|:----------:|
| `ProcessHealth`    | `LaunchRecipe` (mod.rs:1429)               | ✅ |
| `CrossCutting`     | `LaunchRecipe` (mod.rs:1436)               | ✅ |
| `StepFailure`      | `LaunchRecipe` (mod.rs:1565)               | ✅ |
| `DeliveryReady`    | `VerifyAndMergePr` (mod.rs:1405)           | ✅ |
| **`WorkstreamCoverage`** | **`FlagWorkstreamGaps`** (mod.rs:1543) — **notify-only** | ❌ |
| `GoalHygiene` (blocked) | `decide_blocked_goal` (mod.rs:1603)   | ⚠️ only if WHY present |

`WorkstreamCoverage` is the **only** High-priority problem whose Decide arm has
**no edge into `launch.rs`**. The launcher, the `RecipeBrief`, the per-cycle
launch cap, and the dedup key all already exist and are exercised by three
sibling arms — the seam is present; the coverage arm simply doesn't use it.
`GoalHygiene` converges *only* when `problem.why` is populated (mod.rs:1469),
which depends entirely on §0's double-gate. So both recurring families reduce
to: **a closing edge that exists but is not wired for this problem kind.**

Flag-only sinks inventoried (each should gain a closing rung or an explicit
"terminal-by-design" annotation): `act_flag_workstream_gaps` (mod.rs:884, the
subject), and — as a softer case — the `Report`/`Whisper` fall-throughs in the
Decide table (mod.rs:1411,1427,1630) which are terminal-by-design and *not* in
scope.

---

## 2. Systemic fix — a recurrence-aware remediation rung

Design goal: make persistent signals **converge**. Convert the notify-only
sink into a **three-rung ladder driven by a recurrence count that lives in
cognitive memory**, so `workstream-gap` gets the same "seen N× → act" policy
that `goal:blocked` already has via `RECURRENCE_ESCALATION_THRESHOLD`.

### 2.1 Give gaps a durable recurrence count (fix the "2× dead zone")

Today a gap's only memory is the 15-min `gap_gate` window
(`WhisperGate::new(900,200)`, mod.rs:304) — it suppresses within a window but
**forgets across windows**, so recurrence never accrues. Blocked-goal causes,
by contrast, are recalled from cognitive memory as `PriorOccurrence`s
(`root_cause.rs`; `mod.rs::recall_occurrences`).

**Design:** record each fresh gap signature as a `PriorOccurrence` keyed
`workstream-gap:{signature}` on commit (the same `commit` site,
mod.rs:931-934), and on each Act pass recall the prior count. This is a reuse,
not a new subsystem — the recall/occurrence primitive already exists for
root-cause. The `gap_gate` stays as the intra-window flood guard; cognitive
memory becomes the cross-window recurrence ledger.

### 2.2 The remediation ladder (rungs by recurrence)

Rewrite the `WorkstreamCoverage` Decide arm (mod.rs:1534) from a fixed
`FlagWorkstreamGaps` into a recurrence-partitioned decision. Per gap:

| Recurrence | Rung | Action | Rationale |
|:---:|---|---|---|
| **1× (first sight)** | **Notify** | `FlagWorkstreamGaps` (unchanged) | one-off may self-resolve; don't thrash |
| **≥2× (proven recurring)** | **Remediate** | `LaunchRecipe { brief }` via `launch.rs`, honoring the per-cycle launch cap and the board dedup key | a coverage gap has no benign "transient blip" explanation; auto-close it |
| **≥3× or launch-unsafe** | **Escalate** | `EscalateBlockedGoal`-style single operator escalation carrying the gap history | proven-recurring AND auto-launch refused → a human sees it *once*, with history |

- **Gap threshold = 2** (not the blocked-goal 3): the report's rationale holds —
  a coverage gap that recurs is definitionally under-resourced, whereas a
  blocked-goal cause can recur for benign reasons, justifying a higher bar.
- **`GoalUncovered` → auto-launch** is the high-value rung: build a
  `RecipeBrief { task_description = gap.summary, target_repo, sequence_group:
  None }` and launch exactly as `ProcessHealth` does (mod.rs:1429). The
  existing `max_launches_per_cycle` gate and the board dedup key
  (`goal_has_active_workstream`, sensor.rs) guarantee the launch never fights an
  in-flight engineer and never exceeds the concurrency ceiling.
- **`IssueUncovered` / `AnomalyUnaddressed`** rungs: same shape, briefed to open
  a PR against the issue / anomaly. Where a safe brief can't be synthesised,
  skip the Remediate rung and go straight to Escalate at ≥3×.

### 2.3 Wiring seam (where the edges attach)

- **Decide** (`overseer/mod.rs:1534-1543`): partition `gaps` by recalled
  recurrence into `notify_gaps` / `launch_briefs` / `escalate_gaps`; emit a
  small `Vec<Intervention>` (or a new `RemediateWorkstreamGaps` intervention
  that carries all three buckets) instead of the single `FlagWorkstreamGaps`.
- **Act** (`overseer/mod.rs:884`): `act_flag_workstream_gaps` keeps the notify
  bucket; add an `act_remediate_workstream_gaps` sibling that drives the launch
  bucket through the **existing** `RecipeLauncher` (the same handle
  `ActOutcome::Launched(_)` path the merge/step-failure arms use) and the
  escalate bucket through the existing operator escalation.
- **Guardrails** (`overseer/guardrails.rs:60`): `FlagWorkstreamGaps` is
  `RiskClass::Routine`; a *launch* is not. Classify the new remediation
  intervention as the same risk tier `LaunchRecipe` already carries so the
  `AutonomyGate`/budget gate governs it (no new bypass).
- **Report/telemetry** (`overseer/wiring.rs:399-423`, `activity.rs:66-68`):
  `tally_outcome` already maps `ActOutcome::Launched → recipes_launched`; add a
  `workstream_gaps_remediated` / `_escalated` counter beside
  `workstream_gaps_detected/_suppressed` so convergence is observable (§4).

---

## 3. WHY-reasoner wiring guarantee

Turn §0's opt-in-that-fails-open into an **invariant**: *every*
`GoalProgress::Blocked` reason authored by the breaker must carry a
`NoProgressClass` token, and no runtime toggle may silently remove the ladder.

### 3.1 Close the two silent gates

- **Gate A (evidence source absent → `Vec::new()`):** when
  `completion_evidence` is `None` in a **daemon** context, this is a
  mis-boot, not a valid mode. Design: fail *loud* — emit a startup-level
  `tracing::error!` + an operator escalation ("no-progress safeguard is
  DISABLED: no evidence source wired"), rather than silently skipping the
  breaker. Tests/non-daemon callers stay opt-out via an explicit
  `BreakerMode::Disabled` marker, so "None" can no longer *mean* two different
  things (absent-by-design vs absent-by-defect).
- **Gate B (`SIMARD_NO_PROGRESS_INVESTIGATE=off` → bare-park base ladder):**
  keep the kill-switch, but make its consequence explicit — when off, the base
  ladder must still stamp a `NoProgressClass::GenuinelyStuck` WHY token (via
  `no_progress_blocked_reason_with_why`) instead of the legacy bare
  `{PREFIX}{count}{SUFFIX}`. Result: **no code path authors a WHY-less block.**

### 3.2 Guarantee retroactive coverage

`reinvestigate_bare_blocked_goals` (`no_progress.rs:808`, cycle.rs:627) already
re-classifies legacy bare parks — but it runs **inside** Gate A/B. Design:
schedule it on an **independent** cadence that only needs an `EvidenceSource`,
so existing bare parks (from a pre-fix daemon or a flag-off window) always get a
WHY retroactively and route down `resolution_for_why` — even if the on-
transition breaker is disabled. This makes the guarantee *convergent for the
installed base*, not just for newly-authored blocks.

### 3.3 The invariant, stated

> **INV-WHY:** For any goal `g` with `GoalProgress::Blocked(reason)`,
> `is_bare_no_progress_block(reason) == false` within one full OODA cycle of the
> block being authored. Equivalently: `decide_blocked_goal` (mod.rs:1603) always
> receives `problem.why.is_some()`, so `GoalHygiene` always converges via the
> `resolution_for_why` ladder rather than the `Intervention::Report`
> fall-through (mod.rs:1630).

A CI assertion over the breaker + reinvestigation paths (extend
`tests_no_progress_reinvestigation.rs`) pins INV-WHY so a future refactor
cannot silently reintroduce the bare-park regression.

---

## 4. Convergence observability (the single number)

Add a **persistent-unremediated** gauge: count gap signatures whose recalled
recurrence ≥2 that did **not** produce a `LaunchRecipe`/escalation this window
(extend `overseer/activity.rs:66-68`, surfaced through `wiring.rs:349-350`).
Symmetrically, count `goal:blocked` reasons failing `is_bare_no_progress_block`
== *true* (INV-WHY violations). Both must trend to **zero** once §2–§3 land;
either rising is the leading indicator that a signature has re-entered the dead
zone. This is the operator's proof the fix is working, not just deployed.

---

## 5. Per-goal prioritized action table (P0..P2)

Priority = fastest safe automated resolution first; human-criteria work last.
Each row cites the closing seam that executes it.

| P | Goal(s) | Root class | Closing action (seam) |
|:--:|---|---|---|
| **P0** | kgpacks-rs → full parity; issues **#12/#17/#18/#23/#25** | `AlreadyComplete` / `MissingPrecondition` | Run outcome-verify/done-gate against live artifacts (`goal_curation/outcome_verify.rs`, `completion_gate.rs`); the `DeterministicNoProgressReasoner` (`no_progress.rs:990`) certifies `AlreadyComplete` → `resolution_for_why` → `MarkDone` (`no_progress_breaker.rs:390`). For any not-yet-complete, `MissingPrecondition` → `Heal` (clone via `CloneRepoHealer`, cycle.rs:595) + retry. **Never reaches a human.** Requires §3 (Gate A wired). |
| **P0** | *WHY-reasoner wiring itself* | infra defect (§0) | Land §3.1–§3.3: close both silent gates + INV-WHY assertion. This is P0 because every other blocked-goal row depends on the ladder actually running. |
| **P1** | simard-identity personas (atelier, bursar, cartographer, concierge, gastronome) | `GoalUncovered` workstream-gap | §2.2 Remediate rung: at 2× recurrence auto-`LaunchRecipe` one bounded workstream per persona through `launch.rs`, gated by `max_launches_per_cycle` + board dedup. First split the umbrella into 5 independently-verifiable sub-goals via `decompose_goal` (2..=6 fan-out, `decompose.rs`) so each carries its own done-criterion and leaves the gap set on coverage. |
| **P1** | Build local coin benchmark harness | `MissingPrecondition` / `UpstreamDependency` | If precondition is machine-establishable (benchmark data + runner in `src/coin_gym/`) → `Heal`+retry. If it depends on an unlanded upstream → `resolution_for_why` → `Defer { blocking_ref }` (`no_progress_breaker.rs:395`), auto-clears on landing. **Do not human-park.** |
| **P2** | Audit Simard test coverage → 70% | `UnclearCriteria` | The done-gate cannot certify "70% coverage" as authored → it re-parks regardless of work done. `resolution_for_why` → `SpawnEngineer` **once** (`no_progress_breaker.rs:413`) whose task is to *reformulate the criterion* into a machine-checkable artifact (e.g. `cargo llvm-cov` line-coverage ≥70% committed to CI), not to do coverage work blindly. Only after guided retry is exhausted does it `Escalate` to a human. |

**Cross-cutting sequencing:** land **P0 wiring (§3)** before running any bulk
`unblock-all`, so bare parks are cleared *with their real WHY attached* and
route down the ladder — never blindly re-unblocked (the operator's rejected
antipattern, mod.rs:1588).

---

## 6. Evidence ledger (tertiary — new/architectural citations)

| Claim | Source |
|---|---|
| WHY-reasoner double-gated (evidence source AND flag), fails open to `Vec::new()` / bare-park | `ooda_loop/cycle.rs:582-702` |
| Investigation flag default-on, `=off` kill-switch → base ladder | `ooda_loop/no_progress.rs:199-207` |
| Base ladder authors bare `{PREFIX}{count}{SUFFIX}` "needs human review" | `goal_curation/no_progress_breaker.rs:75,108` |
| Reasoner + reinvestigation both live but *inside* both gates | `ooda_loop/cycle.rs:592-636` |
| `WorkstreamCoverage` Decide arm → notify-only `FlagWorkstreamGaps` (no launch edge) | `overseer/mod.rs:1534-1543` |
| Sibling arms DO reach the launcher (`LaunchRecipe`) | `overseer/mod.rs:1429,1436,1565` |
| Act path only peeks/commits gap_gate + notifies; no `RecipeLauncher` | `overseer/mod.rs:884-948` |
| Launcher/`RecipeBrief`/`RecipeRunner` seam exists and is bounded by launch cap | `overseer/launch.rs:47-59,103-132` |
| `resolution_for_why` ladder (MarkDone/Drop/Heal/Defer/Spawn/Escalate) | `goal_curation/no_progress_breaker.rs:384-417` |
| Deterministic reasoner classifies from live artifacts (done-gate/repo/upstream) | `ooda_loop/no_progress.rs:931-1010` |
| Recurrence escalation threshold = 3 (blocked goals only) | `overseer/root_cause.rs:33`; `overseer/mod.rs:1613` |
| gap_gate = 15-min window only, no cross-window memory | `overseer/mod.rs:304,894-934` |
| `tally_outcome`/counters seam for observability | `overseer/wiring.rs:399-423`; `overseer/activity.rs:66-68` |
| Guardrails risk classification seam | `overseer/guardrails.rs:60` |
