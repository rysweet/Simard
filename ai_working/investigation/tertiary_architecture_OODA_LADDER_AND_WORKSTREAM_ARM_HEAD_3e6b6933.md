# TERTIARY (architect) — OODA resolution ladder, cycle.rs blocked-goal parking, and the WorkstreamCoverage decision arm

- **Role:** TERTIARY investigator (architecture / structural focus)
- **HEAD:** `3e6b6933` (validated live; prior tertiary FINAL work cited `856f854b`, `5a85317b`, `b9f99879`)
- **Assigned focus:** OODA resolution ladder / `cycle.rs` blocked-goal parking and the `WorkstreamCoverage` decision arm — whether a **remediation rung** or **loop-closing launch edge** is missing.
- **Method:** Re-grounded every load-bearing seam in live `src/` at HEAD and **validated** (did not re-derive) against the existing ledger (`FINAL_SYNTHESIS.md`, `tertiary_ooda_loop_map_and_missing_unblock_rung_HEAD_856f854b.md`, `RECONCILIATION_LEDGER.md`). Verdicts below are consistent with that ledger and extend it with two arm-specific structural findings (T1 ordering hazard, T4 cross-lane invisibility).

---

## 0. One-paragraph architectural verdict

Within my arm, the recurring `…|goal:blocked:fix-agent-kgpacks-rs-…|resource:engineer_spawn|workstream-gap` cluster is the faithful fingerprint of a **Decide→Act loop that never closes** on two of its highest-priority arms. This is **signal, not a storage/dedup defect** — but the signal points at **three genuine missing/mis-ordered edges** in the resolution ladder: (T1) a **remediation dead zone in `decide_blocked_goal` between recurrence 2 and 3, plus an ordering hazard where the escalation gate shadows the self-heal `UnblockGoal` arm**; (T2) the **`cycle.rs` blocked-goal ladder is double-gated and fails open to a bare human-review park**; (T3) the **`WorkstreamCoverage` decide arm is the only High-priority arm whose Act is notify-only** — the `route_failure`/launch/file edge that would close the loop is built but never wired. (T4) A **cross-lane threshold decoupling** (observation lane `≥2` vs. escalation lane `≥3`) makes the operator-visible "2×" invisible to the gate that would act on it.

---

## 1. Citation drift check vs. prior HEADs — CONFIRMED, not superseded

| Seam | Prior citation | Live at HEAD `3e6b6933` | Status |
|---|---|---|---|
| `observation_signature` re-wrap | `mod.rs:1068-1073` | `mod.rs:1072` `format!("overseer-obs:{}", keys.join("|"))` | ✔ trivial shift |
| `WorkstreamCoverage` decide arm | `mod.rs:1534-1543` | `mod.rs:1534-1543` `⇒ FlagWorkstreamGaps` | ✔ exact |
| `act_flag_workstream_gaps` (Act) | `mod.rs:884-948` | `mod.rs:884-948` notify-only | ✔ exact |
| `decide_blocked_goal` ladder | `mod.rs:1603-1631` | `mod.rs:1603-1631`; `UnblockGoal` `:1621` | ✔ exact |
| cycle.rs WHY double-gate | `cycle.rs:582-702` | `cycle.rs:582-702` | ✔ exact |
| `route_failure` receiver | `routing.rs:39` | `routing.rs:39`; sole prod caller `stewardship/mod.rs:75` | ✔ exact |
| Recur thresholds | 2 / 3 | `signal.rs:362` `=2`; `root_cause.rs:33` `=3` | ✔ exact |

**Drift verdict:** the prior tertiary synthesis (`856f854b`) is **confirmed at HEAD `3e6b6933`, not superseded.** No new caller edge wires the gap Act to `route_failure` — `observer.rs:542/565` reference `route_failure` only inside **test** functions (`decide_read_only` assertions), so the production caller set is still `{ stewardship/mod.rs:75 }`.

---

## 2. T1 — `decide_blocked_goal`: a [2,3) remediation dead zone + an escalation-before-unblock ordering hazard  (mod.rs:1603-1631)

The ladder evaluates guards **top-to-bottom**:

```
1603 fn decide_blocked_goal(goal_id, reason, perpetual, needs_review, recurrence, why)
1613   if recurrence >= RECURRENCE_ESCALATION_THRESHOLD (3)  -> EscalateBlockedGoal   [GATE 1]
1620   if perpetual && is_no_progress_marker(reason)         -> UnblockGoal   ★self-heal [GATE 2]
1623   if needs_review                                       -> EscalateBlockedGoal   [GATE 3]
1630   else                                                  -> Report   (passive no-op)
```

Two structural defects fall directly out of this ordering:

- **(T1a) Remediation dead zone at recurrence = 2.** The signature's honest counter is exactly **"2×"** (observation lane, `RECURRING_SIGNATURE_THRESHOLD = 2`). A `goal:blocked` that is **not** a perpetual no-progress marker and **not** `needs_review` at recurrence 2 falls through every guard to `Intervention::Report` (`:1630`) — a **passive no-op**. There is **no remediation rung between "re-observed twice" (2) and "escalate (3)."** The recurrence signal is faithfully counted but drives **no Act**. This is precisely the "missing rung between recurrence=2 and escalation=3" the strategy asked to test — it is **real and present at HEAD.**

- **(T1b) Ordering hazard: escalation shadows the self-heal rung.** `GATE 1` (escalate at `recurrence ≥ 3`) is checked **before** `GATE 2` (`UnblockGoal`). A goal that is a **genuine no-progress false-park** (the case `UnblockGoal` exists to auto-resolve) but has already recurred `≥ 3` times is captured by `GATE 1` and **can never reach `UnblockGoal`.** Because the escalation `recurrence` lives on an **append-only** root-cause fact lane (per ledger: `record_occurrence`→`store_fact`), once it latches `≥3` the false-park is **permanent**. This is the exact mechanism that keeps `goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-…` parked despite delivered PRs.

**Architectural defect (arm-scoped D-T1):** the `Blocked → active` self-heal edge is **both starved (no rung at 2) and out-ordered (unreachable past 3).** The self-heal arm should be evaluated on an **evidence-bound trigger independent of the escalation counter**, and ordered **before** the escalation gate — otherwise escalation permanently masks self-heal.

---

## 3. T2 — `cycle.rs` blocked-goal ladder is double-gated and fails open to a bare park  (cycle.rs:582-702)

The self-resolving WHY ladder — classify the stall, then **auto-complete / heal precondition / defer upstream / spawn ONE guided engineer**, plus `reinvestigate_bare_blocked_goals` for already-bare parks — is nested inside **two gates**:

```
582  if let Some(source) = &memories.completion_evidence {          [GATE A]
583    if no_progress_investigation_enabled() {                     [GATE B]
         apply_no_progress_breaker_investigated(...)   // full ladder + spawn queue
         reinvestigate_bare_blocked_goals(...)         // rescue already-bare parks
       } else { apply_no_progress_breaker(...) }        // legacy verify-once park
     } else { Vec::new() }                              // 700-702: NOTHING
```

- If **GATE A** is off (no `completion_evidence` — any non-daemon/unwired caller) → `Vec::new()` (`:700-702`): **no classification, bare park.**
- If **GATE B** is off (kill-switch) → legacy verify-once park; the WHY ladder never runs.
- The rescue pass `reinvestigate_bare_blocked_goals` (`:627-636`) — designed to rehabilitate stranded bare parks — lives **inside the same double gate (`:583`)**, so it **cannot** rescue when the gates are off.

**Architectural defect (arm-scoped D-T2):** **no invariant binds a `Blocked` reason to a resolution class.** The resolution rung has no **guaranteed** trigger; when either gate is off, all stall classes collapse to the same `[OODA-SAFEGUARD] … needs human review` park, and the very pass meant to un-strand them is gated off too. Canonical incident: the seven `kgpacks-rs` goals parked "no progress" while the work was **done** (issues closed, PRs merged) — the safeguard read *done* as *stuck* and no rung reclassified them.

---

## 4. T3 — `WorkstreamCoverage` is the only High-priority Decide arm with a notify-only Act; the loop-closing launch/file edge is missing  (mod.rs:1534-1543, 884-948; routing.rs:39)

**Decide-arm asymmetry** (all other High-priority arms terminate in a *closing* action; `WorkstreamCoverage` does not):

| ProblemKind | Intervention | Closing action? | Site |
|---|---|---|---|
| StepFailure | `LaunchRecipe` | ✔ launches corrective workstream | `mod.rs:1565` |
| ProcessHealth | `LaunchRecipe` | ✔ | `mod.rs:1429` |
| CrossCutting | `LaunchRecipe` | ✔ | `mod.rs:1436` |
| QualityRegression / CI cluster | `FileIssue` | ✔ files + routes | `mod.rs:1416` |
| **WorkstreamCoverage** | **`FlagWorkstreamGaps`** | **�’ notify-only** | **`mod.rs:1534-1543`** |

- **Act:** `act_flag_workstream_gaps` (`mod.rs:884-948`) does **exactly one** effectful thing: `notifier.notify(...)` (`:930`). It files **no** issue, launches **no** workstream, and **never calls `stewardship::route_failure`.** Its only state mutation is the dedup gate `commit` (`:933`).
- **Receiver exists, caller unwired:** `route_failure` (`routing.rs:39`) was explicitly built with a `DEFAULT_TARGET_REPO` fallback for the Overseer's `"overseer"` gap briefs, yet its **sole production caller** is `process_orchestrator_run` (`stewardship/mod.rs:75`) — a path the Overseer gap flow never touches.

**Architectural defect (arm-scoped D-T3):** a **dangling loop-closing edge.** The `WorkstreamCoverage → Act` path terminates in a notification instead of routing the gap to `route_failure`/`FileIssue`/`LaunchRecipe`. The gap is therefore **terminal** and re-emits the bare family token `workstream-gap` (`mod.rs:1371`) every window. **INV-GAP-KEY caveat (unchanged):** any wiring must key the ledger on `GapItem.signature` (used at `mod.rs:901,932`), **not** the bare `workstream-gap` dedup_key, or distinct gaps fold into one issue.

---

## 5. T4 — cross-lane threshold decoupling makes the "2×" invisible to the gate that would act  (signal.rs:362/463, root_cause.rs:33, mod.rs:1613)

- **Lane A (observation recurrence):** `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`; fires at `:463`) drives the operator-visible **"2×"** in `RecurringSignature`.
- **Lane B (root-cause escalation):** `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) is what `decide_blocked_goal` reads (`mod.rs:1613`).
- `tests_root_cause.rs:479-480` **explicitly documents these as two decoupled storage lanes** (Lane A keyed on `failure_signature`; Lane B keyed on `root_cause_signature`).

**Consequence:** the "2×" the operator sees says **nothing** about whether Lane B reached 3. This is the structural substrate under T1a: the **visible** recurrence signal and the **actionable** escalation counter live on **different axes**, so the interval `[2,3)` is a genuine dead zone where a real, twice-observed block produces no remediation.

---

## 6. Where a resolution rung should fire but does not — arm summary

| # | Loop / arm | Missing or mis-ordered edge | Should fire | Live site | Why it doesn't |
|---|---|---|---|---|---|
| T1a | Decide (blocked-goal) | remediation rung at recurrence 2 | act on the twice-observed block | `mod.rs:1603-1631` | no arm between GATE 1 (≥3) and the special-cased self-heal → falls to `Report` |
| T1b | Decide (blocked-goal) | order self-heal before escalate | reach `UnblockGoal` for a true false-park | `mod.rs:1613 vs 1620` | escalate gate precedes `UnblockGoal`; append-only latch makes it permanent |
| T2 | `cycle.rs` ladder | unconditional `Blocked → active` trigger | reclassify/self-heal bare parks | `cycle.rs:582-702` | double-gated on `completion_evidence` + kill-switch; rescue pass gated with it |
| T3 | `WorkstreamCoverage` Act | Act → `route_failure`/`FileIssue`/`LaunchRecipe` | file or launch the gap | `mod.rs:884-948`, `1534-1543`; `routing.rs:39` | Act is notify-only; receiver built, caller edge never wired |
| T4 | counter lanes | unify visible ↔ actionable recurrence | make "2×" drive the gate | `signal.rs:362`, `root_cause.rs:33`, `mod.rs:1613` | Lane A (≥2) and Lane B (≥3) are decoupled stores |

---

## 7. Recommendations (understanding-oriented; investigation only — NO code changed)

1. **Treat these as four independent edge/ordering fixes, not one bug.** Each shrinks the composite along a different token; none alone stops all recurrence.
2. **T1 fix shape:** evaluate `UnblockGoal`'s evidence-bound self-heal **before** the `recurrence ≥ 3` escalation gate, and add an explicit **remediation rung for `recurrence == 2`** (e.g., a bounded re-investigation / single guided spawn) so the twice-observed block is not a passive `Report`. Avoid the `store_fact_with_caller_key` latch trap (ledger §2); prefer count-in-content upsert so escalation can de-ratchet.
3. **T2 fix shape:** give the `Blocked → active` rung an **unconditional, evidence-bound trigger** independent of `completion_evidence`, and **hoist `reinvestigate_bare_blocked_goals` out of the double gate** so bare parks are always eligible for rescue.
4. **T3 fix shape:** wire `act_flag_workstream_gaps` to the already-built `route_failure` (→ `FileIssue`, or add a `LaunchRecipe` sibling in the `WorkstreamCoverage` decide arm), **keyed on `GapItem.signature`** (INV-GAP-KEY), so a coverage gap becomes real work rather than a repeating notification.
5. **T4 fix shape:** unify the two recurrence lanes (or read the count-in-content) so the operator-visible "2×" and the escalation bar sit on **one axis**.
6. **Out of scope (confirmed dead ends):** implementing the issue-17 int8/PQ embed fix; unblocking kgpacks-rs at goal level; exact repetition counting from the truncated blob; cognitive-memory storage-backend internals; non-overseer subsystems except where they emit tokens.

## 8. Verification performed
- Re-read live at HEAD `3e6b6933`: `decide_blocked_goal` (`mod.rs:1603-1631`), `WorkstreamCoverage` decide arm (`mod.rs:1534-1543`), `act_flag_workstream_gaps` (`mod.rs:884-948`), `observation_signature` (`mod.rs:1072`), `cycle.rs:575-710` double-gate.
- Confirmed decide-arm asymmetry by enumerating every `LaunchRecipe`/`FileIssue` emitter in `mod.rs` (`1416,1429,1436,1565`) — none on the `WorkstreamCoverage` path.
- Confirmed `route_failure` production caller set = `{ stewardship/mod.rs:75 }`; `observer.rs:542/565` are **test-only** assertions.
- Confirmed two-lane thresholds `signal.rs:362 (=2)` / `root_cause.rs:33 (=3)` and the decoupling documented in `tests_root_cause.rs:479-480`.
- Cross-checked every verdict against `tertiary_ooda_loop_map_and_missing_unblock_rung_HEAD_856f854b.md` and `FINAL_SYNTHESIS.md` — **consistent; extends without contradiction** (adds T1b ordering hazard and T4 cross-lane invisibility as arm-specific structural findings).
