# Tertiary (Architect) — Non-closing OODA loops, the 2↔3 dead-zone occurrence gate, the D0 reconciliation-seam/completion-gate root cause, and a dependency-ordered remediation plan

**Role:** Tertiary investigator (amplihack:architect). **Understanding-only** (no fix landed).
**HEAD:** `d00e4c3f43a5627448b26db76c17678701dfc691`
**Method:** Validate-don't-re-derive. Every load-bearing citation re-read live at HEAD; the
two commits since the last full re-ground (`cc55a6fb`) are **docs-only** (`git diff --name-only
cc55a6fb..HEAD` has zero non-test `.rs` changes), so the settled corpus applies unchanged.

---

## 0. Headline (verdict)

The recurring `goal:blocked:…-<hash>` / `workstream-gap` signature is **not a counting or
write defect** — it is the string fingerprint of **three coupled OODA loops that never close**.
The operator-visible `×2` (Lane A, episodic floor 2) is decoupled by construction from the
escalation counter (Lane B, floor 3). The dead zone is the **absorbing region Lane-A ≥ 2 while
Lane-B < 3**, and it is absorbing because the genuinely-stuck goal lands at a **terminal,
non-recording sink**. The load-bearing root cause is a **conjunction at D0** (decide/observe):
a goal parked with **no WHY-class** (D0 completion-gate) is routed to `Report` (terminal), and
`Report`'s outcome is **excluded from occurrence recording**, so Lane-B `recurrence` can never
leave 0 → Rung 1 (`>=3`) is unreachable → re-observe → re-park. Self-sealing.

---

## 1. The three non-closing loops (re-grounded live at HEAD)

### Loop 1 — Blocked-goal ladder terminating in a non-recording `Report`
`decide_blocked_goal` (`src/overseer/mod.rs:1603-1631`), first-match-wins:

| Rung | Guard | Action | Recorded? (`outcome_records_occurrence`) |
|---|---|---|---|
| 1 | `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (**3**, `root_cause.rs:33`) | `EscalateBlockedGoal` | yes |
| 2 | `perpetual && is_no_progress_marker(reason)` | `UnblockGoal` (self-heal) | yes |
| 3 | `needs_review` | `EscalateBlockedGoal` | yes |
| 4 | *else ("deliberate" block)* | `Report` → `ActOutcome::Reported` | **NO** |

`ActOutcome::Reported` is **absent** from `outcome_records_occurrence`
(`src/overseer/wiring.rs:612-627` — re-read line-for-line at HEAD; the `matches!` arm lists
`Launched|Merged|Deployed|IssueFiled|Escalated|Whispered|GoalUnblocked|GoalEscalated|
ConflictResolved|GoalTransferred|Audited`, **no `Reported`**). Rung 4 is *intentional* for a real
operator/dependency wait (pinned green: `tests_root_cause.rs:648-680
deliberate_operator_block_is_acknowledged_not_symptom`). The defect is that Rung 4 is a
**terminal non-recording sink**, so a genuinely-stuck goal that merely misses the no-progress
marker (Rung 2) and the `needs_review` flag (Rung 3) is misfiled "deliberate," acknowledged, and
**can never accrue toward Rung 1**. The loop does not close: park → don't record → never
escalate → re-observe → park.

### Loop 2 — Workstream-gap ladder: notify-without-launch (missing closing edge)
`WorkstreamCoverage` Decide arm (`src/overseer/mod.rs:1534-1543`) → `Intervention::
FlagWorkstreamGaps` → `act_flag_workstream_gaps` (`mod.rs:884-948`) **notifies the operator
only** (email+Signal, deduped by `gap_gate` on `workstream-gap:{signature}`). There is **no
second rung, no `launch.rs` edge, no `FileIssue`**, and its outcome `WorkstreamGapsFlagged` is
**also not** in `outcome_records_occurrence`. The sibling `StepFailure` arm
(`mod.rs:1549-1580`) *does* return `LaunchRecipe` — proving the launch edge exists and is
simply not wired to the gap arm. Blocked goals are additionally skipped by gap-scan
(`delegates_blocked_goals_to_goal_health_and_never_reflags_them`, green), so an under-resourced
goal **oscillates** between `workstream-gap` (active/uncovered) and `goal:blocked` (parked),
feeding both recurring families with **no terminal state on either side**.

### Loop 3 — Lane A ProcessHealth recipe that does not unblock the specific goal
`Signal::RecurringSignature { occurrences }` fires at `>= RECURRING_SIGNATURE_THRESHOLD` (**2**,
`signal.rs:362,463`) → classifies to a **separate** `ProblemKind::ProcessHealth`
(`mod.rs:1353-1363`) → `Intervention::LaunchRecipe` with the *signature text* as the task — a
**generic** recipe that never touches the specific `goal:blocked:<id>`. Its dedup key
(sanitized signature) differs from `goal:blocked:<id>`, so it never merges into or advances the
blocked-goal ladder. This is the `×2` the operator sees; it spins without closing the goal.

---

## 2. The 2↔3 dead-zone occurrence gate (precise structural statement)

Two counters, two stores, **no shared axis** (invariant pinned green:
`tests_root_cause.rs loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` and
`lane_b_escalates_without_any_lane_a_signal`):

- **Lane A** — episodic multiplicity of the `overseer-obs:goal:blocked:<slug>-<hash>` write-back
  observation; floor **2**; the operator-visible `×2`; drives only the generic ProcessHealth
  recipe (Loop 3).
- **Lane B** — durable root-cause occurrence facts keyed on `goal:blocked:<id>`; floor **3**;
  the **only** counter `decide_blocked_goal` reads (`why.recurrence`, populated from Lane B
  recall at `mod.rs:1469`, `root_cause.rs:79-82`).

**The gate:** Lane B is fed by `record_occurrence` (`mod.rs:1004-1043`), which uses **append**
`store_fact` (`mod.rs:1034`, durable `open_persistent` — *not* a caller-key upsert; the prior
"store-layer collapse" theory is STALE/obsolete at HEAD). Accrual is therefore correct **when it
runs** — but it only runs for outcomes in `outcome_records_occurrence`. A goal at Rung 4
`Report` is **never recorded**, so Lane B stays at 0 while Lane A reads 2. The dead zone
`Lane-A ≥ 2 ∧ Lane-B < 3` is thus an **absorbing state**, not a transient band — the exact
reason issue-17 WS2 and its siblings are parked, not progressing.

The `overseer-obs:…|overseer-obs:…` / `|workstream-gap|workstream-gap|` doubling in the raw
signature is **Lane-A episodic recall folded into the composite write-back key** (heavy-prefix
serialization noise, per settled primary/secondary findings) — it inflates the *string*, not the
Lane-B *count*. Do not read the doubling as a literal occurrence tally.

---

## 3. The D0 reconciliation-seam / completion-gate root cause (load-bearing)

The latch is produced **upstream of** `decide_blocked_goal`, at the D0 decide/observe seam where
a stalled goal is parked. The **conjunction** (`src/ooda_loop/cycle.rs:582-583`):

```
Gate A:  if let Some(source) = &memories.completion_evidence   // cycle.rs:582
Gate B:      if no_progress_investigation_enabled()            // cycle.rs:583 → no_progress.rs:203-207
```

- **Gate A (reconciliation-seam / completion-gate).** When `completion_evidence == None`, the
  entire investigated-breaker block is skipped and the goal is parked with a **bare marker and no
  WHY class**. This is exactly the reconciliation case the focus names: an anchor whose
  completion cannot be *reconciled* to a linked merged-PR evidence source (issue-closed-without-
  linked-merged-PR, or any non-daemon/absent evidence path) yields `completion_evidence == None`.
- **Gate B.** If `SIMARD_NO_PROGRESS_INVESTIGATE=off`, it falls back to the legacy verify-once
  ladder — again no WHY class authored.

Either gate failing ⇒ **no WHY classification** ⇒ the goal misses Rung 2 (`perpetual &&
marker`) and Rung 3 (`needs_review`) ⇒ falls through to Rung 4 `Report` ⇒ not recorded ⇒ Lane B
pinned at 0. **This is the single load-bearing conjunction** that manufactures the "deliberate"
misclassification and hands it to the non-recording sink. (Note: `cycle.rs`'s gate governs the
engineer-loop's WHY/`needs_review`/`reason` inputs — Rungs 2/3 — while Lane B accrual is starved
independently at the ACT→record boundary. Both must be addressed; see §4.)

---

## 4. Dependency-ordered remediation plan (landing-safe, preserves all green tests)

Design constraints: preserve every green assertion (esp.
`deliberate_operator_block_is_acknowledged_not_symptom`,
`decide_routes_workstream_coverage_to_flag_gaps`,
`flagged_gap_never_constructs_an_issue_brief`, the two two-lane-decoupling tests); **never** turn
a goal-board observation into a per-tick operator page or a new GitHub issue.

**R1 — Un-starve Lane-B accrual (atomic, load-bearing, smallest diff). [no deps]**
Record the acknowledged blocked-goal park so the already-correct append store can accrue.
Smallest safe form: add `ActOutcome::Reported` to `outcome_records_occurrence`
(`wiring.rs:612-627`). No green test pins `Reported` as *excluded*; each Report source carries a
distinct `dedup_key`, so cross-source collisions cannot occur. Effect: a genuinely-recurring
"deliberate" block accrues `1,2,3,…` and at 3 reaches the **existing** Rung 1 →
`EscalateBlockedGoal` (idempotent via `blocked_goal_gate` `escalate:{goal_id}`). First sighting
(`recurrence 0`) still → `Report`, so the deliberate-block test stays green.
*Narrower blast radius option:* record only when `problem.kind == GoalHygiene` and evidence is
`GoalBlocked`, leaving `DeliveryReady`/`QualityRegression` fall-through Reports untouched.

**R2 — Close the D0 completion-gate WHY gap (root-cause of the misclassification). [dep: none;
complements R1]**
At the Gate-A/`completion_evidence == None` and Gate-B-disabled paths (`cycle.rs:582-583`),
author a **conservative WHY class** instead of a bare marker for reconciliation stalls
(issue-closed-without-linked-merged-PR ⇒ a `DEPENDENCY`/`UNCLEAR-CRITERIA`-style class, not
"deliberate"). This routes the goal to Rung 2/3 *on its own merits* rather than falling to the
Rung-4 sink. R1 makes recurrence *observable*; R2 stops the misclassification that sends
honest-stuck goals to the sink in the first place. Land R1 first (it is strictly smaller and
unblocks the existing ladder); R2 reduces how often the sink is reached at all.

**R3 — Optional earlier-surface rung (fills the literal 2→3 band). [dep: R1]**
In `decide_blocked_goal`, before the terminal `Report`, insert
`if recurrence >= 2 && recurrence < RECURRENCE_ESCALATION_THRESHOLD && !needs_review &&
!(perpetual && marker) → EscalateBlockedGoal`. Reuses the idempotent gate ⇒ one notification at
the recurrence=2 point the recurring signature already flags. Depends on R1 (without accrual,
`recurrence` never reaches 2). Do **not** lower `RECURRENCE_ESCALATION_THRESHOLD` — that
escalates honest transients and *still* does nothing while Lane B sits at 0.

**R4 — Add the missing workstream-gap closing edge. [dep: none; largest, land last]**
Give the gap ladder the second rung the blocked-goal ladder already has, mirroring the proven
`StepFailure → LaunchRecipe` pattern (`mod.rs:1549-1580`):
- **Keep** `WorkstreamCoverage → FlagWorkstreamGaps` for first/below-threshold (preserves
  gap-scan tests + `Routine` risk class).
- **Add** a rung firing only when a **per-gap** signature (`GapItem.signature`, the key
  `gap_gate` already uses at `mod.rs:901,932` — **never** the bare `"workstream-gap"` constant at
  `mod.rs:1371`) has recurred `≥ 2×`, routed through the existing `launch.rs` edge and classified
  at `LaunchRecipe`'s risk tier in `guardrails.rs` (not `Routine`) so autonomy/budget and
  `max_launches_per_cycle` govern it.

**Orthogonal (independent of R1-R4):** emission-hygiene de-nesting of the `overseer-obs:…`
composite prefix (per settled primary artifacts) — string cleanup only, not a loop-closing fix.

**Landing order:** R1 → R2 → R3 → R4. R1 is the minimal load-bearing change (closes Loop 1's
accrual, unblocks the existing escalation ladder); R2 addresses the D0 misclassification root
cause; R3 adds the earlier surface; R4 (largest, touches launch/guardrails) closes Loop 2.

---

## 5. Ancillary verdicts (re-grounded)

- **`resource:engineer_spawn` token:** benign membership drift from re-observation, **not** a
  coupled admission/spawn cap breach — no explicit ceiling/`max_launches` error signal is emitted
  on this path (consistent with settled secondary/specialist findings; the growth tracks
  re-observation membership, not a cap error).
- **2× recurrence verdict:** BENIGN honest re-observation (distinct cycles, Lane-A floor-2
  episodic multiplicity), **not** a dedup/replay/collision write defect — pinned by the two-lane
  decoupling tests.
- **Doubling in the signature:** D1 self-observation nesting fingerprint (Lane-A recall folded
  into the composite key), **not** a per-token duplication bug.

---

## 6. Verification performed (this wave)

- `git diff --name-only cc55a6fb..HEAD | grep .rs (non-test)` → **NONE** (docs-only; prior green
  suites and citations carry forward).
- Re-read live at HEAD `d00e4c3f`: `wiring.rs:612-627` (`Reported` absent — confirmed exact);
  `mod.rs:1603-1631` (ladder — exact); `mod.rs:1534-1543` + `1549-1580` (gap vs StepFailure arms —
  exact); `mod.rs:1004-1043` (append `store_fact` at 1034 — exact); `root_cause.rs:33`
  (`=3`); `signal.rs:362,463` (`=2`, fire `>=2`); `cycle.rs:582-583` + `no_progress.rs:203-207`
  (D0 double-gate — exact, file-split from the corpus's cited `cycle.rs:582-702`, behavior
  identical).

**Net:** the settled tertiary corpus is valid unchanged at HEAD `d00e4c3f`. The load-bearing
mechanism is the D0 completion-gate misclassification (§3) feeding the non-recording `Report`
sink (§1 Loop 1 / §2 gate); remediation is dependency-ordered R1→R2→R3→R4 (§4).
