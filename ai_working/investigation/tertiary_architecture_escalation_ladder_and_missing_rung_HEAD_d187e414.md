# Tertiary (Architect) — Escalation-Ladder Structure & Landing-Safe Remediation for the Missing Rung

**Role:** TERTIARY investigator (architecture). **Investigation-only — no production code changed.**
**HEAD:** `d187e414` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Mandate:** Assess the escalation-ladder structure gating blocked-goal resolution and workstream-gap
launch; determine whether a structural gap ("dead zone") exists between recurrence-threshold 2 and
escalation-threshold 3; and outline a **landing-safe** remediation shape for the missing remediation rung
**without implementing it**.
**Method:** VALIDATE-DON'T-RE-DERIVE. Re-read every load-bearing ladder citation at live HEAD; confirmed
drift status; did not restate the primary/secondary emission traces.

Extends (does not restart): `tertiary_architecture_LANDING_SAFE_REMEDIATION_HEAD_641f9c37.md`,
`tertiary_architecture_VALIDATION_HEAD.md`, `RECONCILIATION_LEDGER.md`, `FINAL_SYNTHESIS.md`.

---

## 0. Drift status at HEAD `d187e414` (validated, not assumed)

- `git diff --stat dea65df8..HEAD -- src/` → **only** `src/overseer/tests_root_cause.rs` (+99, additive
  two-lane decoupling tests). All other `src/overseer/*.rs` are **byte-identical** to the baseline.
- `git diff 641f9c37..HEAD` (the last tertiary artifact's HEAD → now) → **entirely under `ai_working/`**;
  **zero** source changes.
- **Consequence:** the prior tertiary landing-safe design holds byte-for-byte at `d187e414`. Every line
  citation below is live and load-bearing. This wave is a **validation + architectural confirmation**, not
  a new derivation.

---

## 1. The escalation ladder, as it exists at HEAD (re-read, exact)

### 1.1 Blocked-goal ladder — `decide_blocked_goal` (`overseer/mod.rs:1603-1631`)

Ordered decision arms (first match wins):

| Rung | Guard | Action |
|---|---|---|
| 1 | `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (**3**, `root_cause.rs:33`) | `EscalateBlockedGoal` |
| 2 | `perpetual && is_no_progress_marker(reason)` | `UnblockGoal` (self-heal false park) |
| 3 | `needs_review` | `EscalateBlockedGoal` |
| 4 | *(else)* | `Report` — **surface-only, no remediation** |

### 1.2 Recurrence signal floor — `signals_from` (`signal.rs:462-467`)

`Signal::RecurringSignature` is emitted at `occurrences >= RECURRING_SIGNATURE_THRESHOLD` (**2**,
`signal.rs:362`). This is **Lane A** (episode multiplicity, keyed on `failure_signature`) and is the counter
that produces the operator-visible `×2`.

### 1.3 Workstream-gap ladder — `decide` `WorkstreamCoverage` arm (`overseer/mod.rs:1534-1543`)

Single arm → `Intervention::FlagWorkstreamGaps` (notify-only). **There is no second rung**: no edge into
`launch.rs`, unlike the sibling `StepFailure` arm (`mod.rs:1549-1581`) which *does* return `LaunchRecipe`.
The gap ladder has exactly one step where the blocked-goal ladder has four.

---

## 2. Structural verdict: is there a "dead zone" between 2 and 3?

**Yes — but it is a two-lane *visibility/coverage* gap, not a single-axis counter dead zone.** This is the
sharpest architectural point, and it is easy to misdiagnose.

- **Lane A** (`RecurringSignature`, floor 2) and **Lane B** (`RootCause.recurrence`, floor 3) are **decoupled
  counters on different storage lanes**. `decide_blocked_goal` reads `recurrence` **only** from Lane B
  (`why.recurrence`, populated by `analyze` over recalled `PriorOccurrence`s); it never reads Lane A. The
  new `tests_root_cause.rs` additions **pin this decoupling** as an invariant
  (`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`,
  `lane_b_escalates_without_any_lane_a_signal`).
- Therefore the operator-visible `×2` (Lane A) says **nothing** about whether Lane B reached 3. A blocked
  goal can be re-observed and re-signalled at `×2` **indefinitely** while Lane B sits at 0 — because the
  Lane-B accrual is starved shut upstream by the WHY double-gate (`ooda_loop/cycle.rs:582-583`; Gate A else
  → `Vec::new()` @701, Gate B else → bare-park ladder). The symptom persists at a *low, stable* count rather
  than either escalating (rung 1) or vanishing.
- The "missing remediation rung" is therefore **Rung 4 of §1.1** (the `else → Report` no-op) *for a goal
  that is recurring on Lane A but carries neither `perpetual`+no-progress nor `needs_review`*. Such a goal is
  **visible, recurring, and terminal**: observed forever, remediated never. On the gap side, the missing rung
  is the **absent second step** of §1.3 (notify-without-launch).

**This is not fixed by moving a threshold.** Lowering `RECURRENCE_ESCALATION_THRESHOLD` to 2 would escalate
honest Lane-B transients and still would not help the double-gate-starved goals (whose Lane-B count is 0, not
2). The gap is structural (a missing rung + a starved accrual gate), not numeric. **Threshold moves are
rejected** — consistent with the prior tertiary/secondary conclusions.

---

## 3. Landing-safe remediation shape for the missing rung (architectural, not implemented)

The remediation is **additive rungs**, layered over the existing terminal arms so every currently-green
assertion is preserved. Two seams carry a missing rung; both mirror an already-proven pattern in the same
file.

### 3.1 Blocked-goal seam (Lane B accrual + Rung 1 reachability) — the atomic latch

The blocked-goal ladder already *has* an escalation rung (Rung 1); the defect is that it is **unreachable**
because Lane B never accrues. The landing-safe fix is the **D2 atomic pair** (must ship together):

1. **Close the WHY double-gate** (`cycle.rs:582-701`) so every `Blocked` reason accrues a `NoProgressClass`
   within one OODA cycle (INV-WHY). Gate A else → fail **loud** in daemon context (mis-boot), not silent
   `Vec::new()`; Gate B else → stamp a WHY token via the base ladder instead of a bare `{PREFIX}{n}{SUFFIX}`
   park.
2. **De-ratchet the counter as count-in-content upsert** (`record_occurrence`, `mod.rs:1034`): caller-key
   upsert keyed on `root_cause_signature(entry.key, primary)` carrying an in-content `occurrence_count` +
   `first_seen`/`last_seen`; escalation reads that field, not `recall.len()`.
   **Do NOT** use the literal `store_fact_with_caller_key(root_cause_signature(...))` one-liner — it
   collapses `recall.len()` to 1 forever and makes Rung 1 (`>= 3`) **dead code**
   (`RECONCILIATION_LEDGER §2`; `library_adapter.rs:885-889`).

**Why atomic:** gate + counter form a latch — fixing either alone changes nothing observable (gate-open +
ratchet → over-escalates on every ACT; gate-shut + good counter → count stays 0, `×2` persists).

### 3.2 Workstream-gap seam (the genuinely *absent* rung) — additive launch edge

Give the gap ladder the second rung the blocked-goal ladder already has:

- **Keep** `decide(WorkstreamCoverage) == FlagWorkstreamGaps` for first-observation / below-threshold
  (preserves `tests_gap_scan.rs::decide_routes_workstream_coverage_to_flag_gaps` and the `Routine`
  risk-class assertion — **do not swap the arm**).
- **Add** a second rung that fires only when a **per-gap** signature has recurred `≥ 2×`, routed through the
  **existing** `launch.rs` edge already proven by the `StepFailure` arm. Threshold 2 (not 3): a recurring
  coverage gap has no benign transient explanation.
- **INV-GAP-KEY:** key the rung on `GapItem.signature` (the Act gate already does this at `mod.rs:901,932`),
  **never** the bare `"workstream-gap"` constant (`mod.rs:1371`), or all gaps fold into one launch.
- Classify the new launch intervention at `LaunchRecipe`'s risk tier in `guardrails.rs` (not `Routine`) so
  the autonomy/budget gate governs it, bounded by `max_launches_per_cycle` + board dedup.

### 3.3 Emission hygiene (orthogonal, cheapest) — D1

In `write_back_observation` (`mod.rs:534-563` / composite at `1068-1073`), filter recall-derived
`overseer-obs:`-prefixed keys before `join("\|")`. Removes the literal `overseer-obs:…|overseer-obs:…`
nesting without touching any counter. Independent of §3.1/§3.2.

---

## 4. Landing order & regression safety (dependency-correct, additive)

1. **D2 (atomic latch)** — close WHY double-gate + count-in-content upsert. First; unlatches Rung 1 and
   drains the `goal:blocked:*` cluster at the source. Highest risk. Regression floor: the 5
   goal_health/root_cause escalation tests + the 2 new two-lane decoupling tests stay green; add
   `recurrence_counts_in_fact_content_not_node_multiplicity`, `why_gate_closed_classifies_instead_of_bare_park`.
2. **D3 (additive gap-launch rung)** — per-gap `≥2× → LaunchRecipe` keyed on `GapItem.signature`. Medium
   risk. Regression floor: `decide_routes_workstream_coverage_to_flag_gaps` +
   `flag_workstream_gaps_is_routine…` stay **unchanged**; add `workstream_gap_recurring_2x_launches_…`,
   `first_observation_still_only_flags`.
3. **D1 (pure filter)** — strip recall-derived keys. Lowest risk. Add anti-nesting + large-blob idempotency
   tests.
4. **Convergence gauges** — counters beside `workstream_gaps_detected/_suppressed`: "gap signatures ≥2× with
   no launch" and "blocked reasons failing `is_bare_no_progress_block`". Prove closure; guard regression.

No fix depends on another's *code*; the order optimizes verification legibility and signature-volume
reduction (D2 removes the largest token cluster first).

---

## 5. Verdict

- **A structural remediation gap exists**, but it is a **two-lane coverage gap**, not a numeric dead zone: a
  goal/gap recurring on Lane A (`×2`) can be **visible and terminal** because (a) the blocked-goal ladder's
  escalation rung is starved unreachable by the WHY double-gate, and (b) the workstream-gap ladder has **no**
  launch rung at all.
- **Landing-safe shape = additive rungs, no arm swaps, no threshold moves.** D2 ships atomically
  (gate + count-in-content, never `CallerKey`); D3 adds a per-gap `≥2× → launch` rung keyed on
  `GapItem.signature`; D1 is a pure hygiene filter. Order: **D2 → D3 → D1 → gauges.**
- **Validation confirms the prior tertiary design at HEAD `d187e414`** with **zero source drift**; the only
  change since baseline (`tests_root_cause.rs`, +99) *strengthens* the two-lane decoupling that this analysis
  depends on. No re-derivation was required or performed.
