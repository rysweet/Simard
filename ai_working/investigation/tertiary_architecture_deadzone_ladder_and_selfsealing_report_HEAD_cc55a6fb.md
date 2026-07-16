# Tertiary (Architect) — Blocked-goal ladder + workstream-gap routing, the recurrence=2/escalation=3 dead zone, and the minimal landing-safe remediation

HEAD: `cc55a6fb3eb5dff9ff6d2596fa8cf3ce9a234f5a` · Scope: architecture of the two remediation lanes that keep `goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-…` (and siblings) parked. All citations re-grounded live at HEAD; drift vs prior artifacts flagged in §6.

---

## 0. Headline

The dead zone is **real and self-sealing**, but the live mechanism at HEAD is **not** the one prior artifacts blamed (caller-key upsert collapsing `recall.len()`→1). At HEAD:

- `record_occurrence` **appends** via plain `store_fact` (`mod.rs:1034`) into a **durable** `open_persistent` store (`library_adapter.rs:188-190`) — so Lane-B accrual is *not* collapsed at the store boundary.
- **The starvation is at the ACT→record boundary instead:** `ActOutcome::Reported` is **excluded** from `outcome_records_occurrence` (`wiring.rs:612-627`). A blocked goal that lands at the terminal `Report` rung is therefore **never recorded**, so its Lane-B `recurrence` stays `0` forever, so Rung 1 (`recurrence >= 3`) is **unreachable for exactly the goals stuck in the dead zone.** Park → don't record → never escalate → re-observe → park. Self-sealing.

The operator-visible `×2` is **Lane A** (episodic recall, floor 2) and is decoupled from Lane B by construction (pinned by passing tests). So the `×2` never advances the escalation ladder.

---

## 1. The two ladders (re-grounded, first-match-wins)

### 1.1 Blocked-goal ladder — `decide_blocked_goal` (`overseer/mod.rs:1603-1631`)

| Rung | Guard | Action | Recorded? (`outcome_records_occurrence`) |
|---|---|---|---|
| 1 | `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (**3**, `root_cause.rs:33`) | `EscalateBlockedGoal` → `GoalEscalated` | **yes** |
| 2 | `perpetual && is_no_progress_marker(reason)` | `UnblockGoal` → `GoalUnblocked` | **yes** |
| 3 | `needs_review` | `EscalateBlockedGoal` → `GoalEscalated` | **yes** |
| 4 | *(else — a "deliberate" block)* | `Report` → `Reported` | **NO** ← self-seal |

Rung 4 is **intentional** for a deliberate operator/dependency wait (pinned green:
`tests_root_cause.rs:648-680 deliberate_operator_block_is_acknowledged_not_symptom`, a
`perpetual:false, needs_review:false` "waiting on operator to provision infra" block is
`RemediationClass::Acknowledged`, `root_cause_addressed=true`, no unaddressed-cause alarm).
The defect is not that Rung 4 exists — it is that Rung 4 is a **terminal, non-recording** sink,
so a *genuinely-stuck* goal that merely fails to match the no-progress marker (Rung 2) or the
`needs_review` flag (Rung 3) is misfiled as "deliberate," acknowledged, and can never accrue its
way to Rung 1.

`why.recurrence` is read **only** from Lane B (`mod.rs:1469`,
`why.recurrence` populated by `root_cause::analyze` over recalled `PriorOccurrence`s,
`root_cause.rs:79-82`) — it never reads Lane A.

### 1.2 Recurrence signal floor — Lane A (`signal.rs:462-467`, threshold **2** at `signal.rs:362`)

`Signal::RecurringSignature { occurrences }` is raised when ≥2 recalled **episodes** share a
`failure_signature`. This is the operator-visible `×2`. It classifies to a **separate**
`ProblemKind::ProcessHealth` problem (`mod.rs:1353-1363`) → `Intervention::LaunchRecipe`
(`mod.rs:1429-1435`) on `rysweet/Simard` with the *signature text* as the task — a **generic**
recipe that does not unblock the specific goal. It also *merges* into a same-key in-cycle problem
to raise its priority (`orient`, `mod.rs:1211-1220`), but for a blocked goal the dedup keys differ
(`goal:blocked:<id>` vs the sanitized signature), so no merge occurs.

### 1.3 Workstream-gap ladder — `decide` `WorkstreamCoverage` arm (`overseer/mod.rs:1534-1543`)

Single arm → `Intervention::FlagWorkstreamGaps` → `act_flag_workstream_gaps`
(`mod.rs:884-948`) which **notifies the operator only** (email+Signal, deduped by
`gap_gate` on `workstream-gap:{signature}`). **No second rung, no `launch.rs` edge, no issue
file.** Its outcome `WorkstreamGapsFlagged` is also **not** in `outcome_records_occurrence`.
Contrast the sibling `StepFailure` arm (`mod.rs:1549-1581`) which *does* return `LaunchRecipe`.
The gap ladder has one step where the blocked-goal ladder has four, and the one step it has
never *closes* a gap — it only announces it, forever, across dedup windows.

---

## 2. The recurrence=2 / escalation=3 dead zone — precise structural statement

Two counters, two stores, no shared axis (invariant pinned by
`tests_root_cause.rs:490 loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` and
`lane_b_escalates_without_any_lane_a_signal`, both green):

- **Lane A** — episodic multiplicity of the `overseer-obs:goal:blocked:<slug>-<hash>` write-back
  observation; floor **2**; the number the operator sees as `×2`; drives only a generic
  ProcessHealth recipe.
- **Lane B** — root-cause occurrence facts keyed on `goal:blocked:<id>`; floor **3**; the only
  counter `decide_blocked_goal` reads.

The dead zone is the region **Lane-A ≥ 2 while Lane-B < 3** for a goal that is neither a
no-progress false-park (Rung 2) nor `needs_review` (Rung 3). Such a goal is **visible, recurring,
and terminal**: it lands at Rung 4 `Report`, which by `wiring.rs:612-627` is **not recorded**, so
Lane B **cannot advance past 0**. The dead zone is not a transient band the goal climbs out of — it
is an **absorbing state**. That is why issue-17 WS2 and its siblings are parked, not progressing.

The `overseer-obs:…|overseer-obs:…` nesting in the raw signature is Lane-A episodic recall folded
back into the composite write-back key (heavy-prefix serialization noise, per prior primary
findings) — it inflates the *string*, not the Lane-B *count*. Do not read it as a literal count.

---

## 3. Two unclosed remediation loops (why the goals stay blocked)

1. **Blocked-goal loop (absorbing `Report`).** Genuinely-stuck goal misclassified "deliberate" →
   Rung 4 `Report` → `Reported` not recorded → Lane-B `recurrence` stuck at 0 → Rung 1 unreachable
   → re-observed next cycle → `Report` again. No operator ever hears about it; no WHY-classification
   drives an action. The escalation ladder above it is *live code that is dead in practice* for
   these goals.

2. **Workstream-gap loop (notify-without-launch).** `FlagWorkstreamGaps` announces the gap, the
   `gap_gate` suppresses re-announcement within the window, the window expires, it re-announces —
   but nothing ever *launches* a workstream or *files* an issue to cover the gap. The gap persists
   as an eternal notification. `workstream-gap` appears in the recurrence set for the same reason.

---

## 4. Minimal, landing-order-safe remediation

Design constraint: preserve every green assertion (esp. `deliberate_operator_block_is_acknowledged_not_symptom`
and `decide_routes_workstream_coverage_to_flag_gaps`), and **never** turn a goal-board observation
into a per-tick operator page or a new GitHub issue.

### 4.1 Blocked-goal seam — un-starve Lane-B accrual (the atomic minimal fix)

**Root cause is the non-recording terminal sink, so fix accrual, not the threshold.** Threshold
moves are rejected (lowering `RECURRENCE_ESCALATION_THRESHOLD` to 2 escalates honest transients and
*still* does nothing while Lane-B sits at 0).

**Step 1 (one line, the load-bearing change): record the acknowledged blocked-goal park.**
Make a Rung-4 `Report` that originated from a `GoalBlocked` problem *record its occurrence* so the
`goal:blocked:<id>` signature accrues in the durable store. Smallest safe form: add
`ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612-627`). No green test pins
`Reported` as *excluded* from recording (verified), and each Report source has a distinct
`dedup_key`, so cross-source collisions cannot occur. With this alone, a genuinely-recurring
"deliberate" block accrues `1,2,3,…` and at 3 reaches the **existing** Rung 1 → `EscalateBlockedGoal`
(idempotent via `blocked_goal_gate` `escalate:{goal_id}`, `mod.rs:823-838`). First observation
(`recurrence 0`) → still `Report` → `deliberate_operator_block_is_acknowledged_not_symptom` stays
green.

  - If a narrower blast radius is wanted, scope the recording to the blocked-goal path only (record
    when `problem.kind == GoalHygiene` and evidence is `GoalBlocked`) instead of all `Reported`.
    Slightly larger diff, zero risk to non-goal Report paths (`DeliveryReady`/`QualityRegression`
    fall-through Reports).

**Step 2 (optional earlier-surface rung, fills the literal 2→3 band):** in `decide_blocked_goal`,
insert **before** the terminal `Report`:
`if recurrence >= 2 && recurrence < RECURRENCE_ESCALATION_THRESHOLD && !needs_review && !(perpetual && marker)
→ EscalateBlockedGoal` (idempotent gate ⇒ notify-once, not per-tick). This gives one operator
notification at the recurrence=2 point the recurring signature already flags, one rung *below* the
full escalation — landing safe because it reuses the existing idempotent primitive and only fires
for a *repeatedly re-observed* block, never a first-sighting deliberate wait. Step 2 depends on
Step 1 (without accrual, `recurrence` never reaches 2).

### 4.2 Workstream-gap seam — additive launch rung (the genuinely absent rung)

Give the gap ladder the second rung the blocked-goal ladder already has, mirroring the proven
`StepFailure → LaunchRecipe` pattern:
- **Keep** `decide(WorkstreamCoverage) == FlagWorkstreamGaps` for first/below-threshold (preserves
  the gap-scan tests and the `Routine` risk class).
- **Add** a rung that fires only when a **per-gap** signature (`GapItem.signature`, the key the
  `gap_gate` already uses at `mod.rs:901,932` — **never** the bare `"workstream-gap"` constant at
  `mod.rs:1371`) has recurred `≥ 2×`, routed through the existing `launch.rs` edge. Classify it at
  `LaunchRecipe`'s risk tier in `guardrails.rs` (not `Routine`) so the autonomy/budget gate and
  `max_launches_per_cycle` govern it.

**Landing order:** §4.1 Step 1 first (smallest, closes the primary loop, unblocks the escalation
ladder), then §4.1 Step 2, then §4.2 (larger, touches launch/guardrails). Emission-hygiene
de-nesting of the `overseer-obs:…` composite prefix (per primary artifacts) is orthogonal and
independent of all of the above.

---

## 5. Verification performed

- Targeted suites green at HEAD: `overseer::tests_root_cause` + `tests_goal_health` +
  `tests_gap_scan` (**53 passed, 0 failed**); `tests_memory_recall` + `tests_whisper`
  (**60 passed, 0 failed**).
- Two-lane decoupling and threshold constants confirmed live: `RECURRENCE_ESCALATION_THRESHOLD=3`
  (`root_cause.rs:33`), `RECURRING_SIGNATURE_THRESHOLD=2` (`signal.rs:362`).
- Ladder, act paths, and gates read directly at HEAD (`mod.rs:1200-1235`, `1400-1631`, `810-948`;
  `wiring.rs:255-290`, `612-627`; `root_cause.rs:64-115`; `library_adapter.rs:188-190`, `657-683`).

---

## 6. Prior-artifact reconciliation (validate, not re-derive)

| Prior claim | Cited loc | Live at HEAD `cc55a6fb`? | Note |
|---|---|---|---|
| Ladder is in `src/stewardship/routing.rs`; arms `signal_to_problem` | (strategy prompt) | **DRIFT (prompt)** | `routing.rs` is a 52-line repo-keyword router with no ladder. Ladder is `overseer/mod.rs` (`classify_signal`/`orient` + `decide`/`decide_blocked_goal`). Prior tertiary artifacts already relocated it correctly. |
| `decide_blocked_goal` 4-rung ladder, terminal `Report` no-op | `mod.rs:1603-1631` | **LIVE** | Confirmed line-for-line. |
| `RECURRENCE_ESCALATION_THRESHOLD = 3`; floor unreachable | `root_cause.rs:33` | **LIVE** | Confirmed. |
| Lane A floor 2; two lanes share no counter | `signal.rs:362,462-467`; tests | **LIVE** | Pinned by two green tests. |
| WorkstreamCoverage = notify-only, no launch rung | `mod.rs:1534-1543`, `884-948` | **LIVE** | Confirmed; StepFailure `LaunchRecipe` precedent at `1549-1581`. |
| **Lane-B starves because `record_occurrence` uses `store_fact_with_caller_key(root_cause_signature(...))` collapsing `recall.len()`→1; "de-ratchet the counter" needed** | prior `§3.1.2`, `library_adapter.rs:885-889` | **STALE / PARTIALLY OBSOLETE** | At HEAD `record_occurrence` (`mod.rs:1004-1043`) uses **append** `store_fact` (`mod.rs:1034`) into a **durable** `open_persistent` store; `recall_occurrences` uses `search_facts` (`mod.rs:972-996`). The store-boundary collapse is **not** the live mechanism. The **de-ratchet is effectively already in place.** |
| WHY double-gate at `cycle.rs:582-701` starves accrual | prior `§2`/`§3.1.1` | **OUT-OF-SCOPE / UNVERIFIED for overseer Lane B** | Overseer's Lane B (`record_occurrence`/`recall_occurrences`) is self-contained and does *not* route through `cycle.rs`. `cycle.rs`'s gate governs the engineer-loop's `MarkGoalBlocked` reason classification (which feeds `needs_review`/`reason`, i.e. Rungs 2/3), a different accrual. Not the live blocker for the Rung-4 sink. |

**Net drift correction (load-bearing):** the live self-sealing mechanism at HEAD is
`ActOutcome::Reported` being excluded from `outcome_records_occurrence` (`wiring.rs:612-627`), **not**
a store-layer counter collapse. The minimal fix therefore moves from "de-ratchet the store counter"
to "**record the acknowledged blocked-goal park so the already-correct append store can accrue.**"
