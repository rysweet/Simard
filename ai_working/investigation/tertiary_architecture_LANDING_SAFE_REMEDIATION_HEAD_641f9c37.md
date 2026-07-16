# Tertiary (Architect) — Minimal Landing-Safe Remediation, Landing Order & Regression Safety

**Role:** TERTIARY investigator (architecture / remediation design). **Investigation-only — no
production code changed.**
**HEAD:** `641f9c37` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Mandate:** Design the minimal landing-safe remediation (or justify a no-op) for the recurring
`overseer-obs:…|goal:blocked:…|workstream-gap|resource:engineer_spawn` signature, with a
dependency-correct landing order and explicit regression-safety against the existing
`tests_gap_scan.rs` / `tests_goal_health.rs` / `tests_root_cause.rs` suites.
**Method:** VALIDATE-DON'T-RE-DERIVE. Every load-bearing citation independently re-read at
`641f9c37`; the regression baseline was actually executed, not assumed.

Extends (does not restart): `FINAL_SYNTHESIS.md`, `RECONCILIATION_LEDGER.md`,
`primary_signature_emission_2x_verdict_DRIFT_RECHECK_HEAD_b47b6413.md`,
`tertiary_architecture_TWO_LANE_RECONCILIATION_AND_LANDING_HEAD_a68296c6.md`.

---

## 0. Two headline results this wave

1. **The strategy's drift warning is a FALSE ALARM — reconciled and closed.** The tasking
   asserted "drift HAS landed since `6e3113bc` in `mod.rs, observer.rs, signal.rs, wiring.rs,
   guardrails.rs`; do NOT trust stale line numbers." Measured at HEAD:
   `git diff --numstat 6e3113bc..HEAD -- src/overseer/<f>.rs` is **`NO_DIFF` for all five files**.
   The only `.rs` change under `src/overseer/` since baseline is `tests_root_cause.rs` (+99 lines,
   the two-lane decoupling tests). The newer filesystem **mtimes** (Jul 15 20:42) are a
   checkout/rebase artifact, **not** content drift. `FINAL_SYNTHESIS.md`'s original "zero source
   drift" claim therefore **holds byte-for-byte at `641f9c37`**. Every `src/overseer/*` line
   citation is live and load-bearing.

2. **Regression baseline is green and now measured, not inferred.** At HEAD:
   `cargo test -p simard --lib -- overseer::tests_gap_scan overseer::tests_goal_health
   overseer::tests_root_cause` → **53 passed; 0 failed; 0 ignored.** This is the concrete
   regression floor every remediation below must keep green (and, where it changes an asserted
   behavior, must update in the same diff).

---

## 1. Re-verified citations at HEAD `641f9c37` (I did not trust the docs' numbers)

| Claim | Source @ HEAD | Status |
|---|---|:--:|
| `observation_signature` = `sort_unstable`→`dedup`→`format!("overseer-obs:{}", keys.join("\|"))` | `overseer/mod.rs:1068-1073` | ✅ exact |
| Lane B counter is append-only `store_fact` (no upsert / ratchet) | `overseer/mod.rs:1034` | ✅ exact |
| `WorkstreamGap` mints the **bare** `"workstream-gap"` dedup_key (per-gap identity erased) | `overseer/mod.rs:1371` | ✅ exact |
| `WorkstreamCoverage` Decide arm → notify-only `Intervention::FlagWorkstreamGaps` (no launch) | `overseer/mod.rs:1538-1543` | ✅ exact |
| Sibling `StepFailure` arm → `Intervention::LaunchRecipe` (proves the hole is unique) | `overseer/mod.rs:1549-1581` | ✅ exact |
| Escalation gate `if recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `overseer/mod.rs:1613` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `overseer/root_cause.rs:33` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` | `overseer/signal.rs:362,463` | ✅ exact |
| WHY reasoner double-gated: `Some(source) = memories.completion_evidence` **&&** `no_progress_investigation_enabled()` | `ooda_loop/cycle.rs:582-583` | ✅ exact |
| Act gate already keys **per-gap** `format!("workstream-gap:{}", g.signature)` | `overseer/mod.rs:901,932` | ✅ exact |
| `gap_scan_enabled` swallows `FlagWorkstreamGaps` when off (silent-recur path) | `overseer/mod.rs:596` | ✅ exact |

**No stale citations. No de-derivation required.** The committed root-cause geometry (D1/D2/D3)
and the two-lane model are live and unchanged.

---

## 2. Remediation verdict: FIX (not no-op), but scoped and additive

A no-op is **not** justified: the signature is an honest fingerprint of a genuinely non-converging
problem set, and three design-level convergence gaps (D1/D2/D3) are live and unguarded at HEAD.
"The count is correct" is precisely why the fix must target the **missing closing actions**, not
the counter. However, the minimal landing-safe scope is narrower than a naive reading suggests —
the sharpest new constraint this wave is that **D3 must be additive, not a Decide-arm swap** (§4).

Defect recap (seams re-verified in §1):

| ID | Seam | Minimal change | Lane |
|---|---|---|---|
| **D1** | `mod.rs:1068-1073` fed by recall-derived `overseer-obs:*` problems (`mod.rs:1353-1359`, `wiring.rs:301`) | Exclude `overseer-obs:`-prefixed keys from the composite before `join("\|")` | A |
| **D2** | `cycle.rs:582-583` WHY double-gate **+** `mod.rs:1034` ratchet / `mod.rs:1613` gate | Close the gate **and** carry the count in fact content (upsert) — **atomic** | B |
| **D3** | `mod.rs:1538-1543` notify-only arm; `mod.rs:1371` bare gap key | **Add** a per-gap `≥2× → LaunchRecipe` rung keyed on `GapItem.signature`, layered over the existing notify | A |

`resource:engineer_spawn` (`mod.rs:1270`, `ProblemKind::ResourcePressure`) remains a **co-symptom,
not a fourth defect** — passive telemetry, no causal edge to `workstream-gap`. No coupling fix.

---

## 3. Atomicity constraints (what MUST ship together)

- **D2 is an atomic pair — never split.** The WHY accrual gate (`cycle.rs:582-583`) and the
  escalation counter (`mod.rs:1034` write / `mod.rs:1613` read) form a **latch**: closing the gate
  without de-ratcheting escalates on a broken counter; de-ratcheting without closing the gate
  leaves accrual starved. **The de-ratchet MUST be a count-in-content upsert, NEVER the literal
  `store_fact_with_caller_key(root_cause_signature(...))` one-liner** — `DedupMode::CallerKey`
  keeps one live fact per key, collapsing `recall.len()` to 1 and making `>= 3` dead code
  (`RECONCILIATION_LEDGER.md §2`; `cognitive_memory/library_adapter.rs:885-889`).
- **D1 and D3 are independent** of D2 and of each other (no code coupling); only their
  *verification* is cleaner in the order below.

---

## 4. Regression-safety analysis (the load-bearing new contribution)

I mapped each proposed change onto the **actually-executed** test surface (53 green at HEAD).

### D3 — the one change that touches an existing hard assertion
`tests_gap_scan.rs:852 decide_routes_workstream_coverage_to_flag_gaps` asserts **verbatim** that
`decide()` on a `WorkstreamCoverage` problem returns `Intervention::FlagWorkstreamGaps` and
`panic!`s on anything else. `tests_gap_scan.rs:872 flag_workstream_gaps_is_routine_and_admitted_by
_default_gate` further pins `classify(FlagWorkstreamGaps) == RiskClass::Routine`.

**Consequence — the landing-safe shape of D3:** the recurrence→launch rung must be **additive**,
not a replacement of the base Decide arm:
- **Keep** `decide(WorkstreamCoverage) == FlagWorkstreamGaps` for the first-observation / below-
  threshold path (both tests above stay green, unchanged).
- **Add** the launch edge as a *second* rung that fires only when a **per-gap** signature has
  recurred `≥ 2×` on Lane A — mirroring the `decide_blocked_goal` recurrence pattern
  (`mod.rs:1610-1616`), and routed through the existing `launch.rs` edge already proven by the
  `StepFailure` arm (`mod.rs:1549-1581`). New behavior ⇒ **new** tests
  (`workstream_gap_recurring_2x_launches_keyed_on_gap_signature`,
  `first_observation_still_only_flags`), not edits to the two existing assertions.
- **INV-GAP-KEY:** the rung must key on `GapItem.signature` (the Act gate *already* does this at
  `mod.rs:901,932`), never the bare `"workstream-gap"` constant (`mod.rs:1371`), or all gaps fold
  into one launch. This is why D3 lives on Lane A per-gap identity, not the composite key.

A Decide-arm *swap* (return `LaunchRecipe` instead of `FlagWorkstreamGaps`) is **rejected**: it
breaks `decide_routes_workstream_coverage_to_flag_gaps` and, worse, launches on every first-seen
gap — thrash, not convergence.

### D2 — additive escalation, existing assertions preserved
`tests_goal_health.rs` and `tests_root_cause.rs` pin both terminal shapes:
`recurring_reblock_escalates_root_cause_not_blind_unblock`,
`escalate_blocked_goal_notification_carries_the_why`,
`recurring_reblock_never_files_an_issue`, and the two-lane decoupling invariants
(`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`,
`lane_b_escalates_without_any_lane_a_signal`). The count-in-content upsert must keep
`recall`/`recurrence` semantics such that **all five stay green**, and add:
`recurrence_counts_in_fact_content_not_node_multiplicity`,
`why_gate_closed_classifies_instead_of_bare_park`. The decoupling tests are the guardrail that
proves D2 didn't accidentally couple Lane A into Lane B.

### D1 — pure/local, net-new coverage only
No existing test asserts nesting, so D1 breaks nothing; it must **add**
`recall_derived_overseer_obs_excluded_from_next_signature` (anti-nesting) and a large-blob
idempotency test (guards D1b, the 8192-byte truncation at `capabilities.rs:472`). Land last so the
diff reads against an already-shrinking signature.

---

## 5. Landing order (risk-ranked, dependency-correct)

1. **D2 (atomic latch)** — close WHY double-gate (`cycle.rs:582-701`) + count-in-content upsert
   (`mod.rs:1034` write, `:1613` read). **First:** drains the `goal:blocked:*` cluster at the
   source and unlatches escalation. Highest risk; ship + verify alone. Regression gate: the 5
   goal_health/root_cause escalation tests + 2 decoupling tests stay green; 2 new tests added.
2. **D3 (additive closing edge)** — per-gap `≥2× → LaunchRecipe` keyed on `GapItem.signature`
   (`mod.rs:1538-1543` + `mod.rs:884-948`), via existing `launch.rs`. Medium risk. Regression gate:
   `decide_routes_workstream_coverage_to_flag_gaps` + `flag_workstream_gaps_is_routine…` stay
   **unchanged and green**; 2 new tests added for the recurrence rung.
3. **D1 (pure filter)** — strip recall-derived `overseer-obs:*` before `join("\|")`
   (`mod.rs:1068-1073`). Lowest risk. Regression gate: existing signature/dedup tests green; 2 new
   hygiene tests added.
4. **Convergence gauges** — counters beside `workstream_gaps_detected/_suppressed`: "gap
   signatures ≥2× with no launch" and "blocked reasons failing `is_bare_no_progress_block`". Prove
   closure, lock regression. The optional episode-lane `(signature, floor(now/900))` idempotency
   key belongs **here only**, and only if restart-flapping is empirically confirmed the dominant
   2× source (still unmeasured — `FINAL_SYNTHESIS §5`).

No fix depends on another's *code*; the order optimizes *verification legibility* and
signature-volume reduction (D2 removes the largest token cluster first).

---

## 6. Rejected levers (re-endorsed no-change)

- **Move `RECURRING_SIGNATURE_THRESHOLD` (2) or `RECURRENCE_ESCALATION_THRESHOLD` (3).** The lanes
  are decoupled (`tests_root_cause.rs` now enforces it); the "2 vs 3" is a cross-lane visibility
  gap, not a single-axis dead zone. Moving either escalates honest transients. **Rejected.**
- **Persist the whisper `last_delivered` map** (`guardrails.rs:292-333`). It is correct *because*
  it is volatile; durability already lives on Lane B by design. Persisting it masks an open backlog
  as convergence. **Rejected.**
- **Swap the `WorkstreamCoverage` Decide arm to `LaunchRecipe`.** Breaks
  `decide_routes_workstream_coverage_to_flag_gaps` and launches on first-seen gaps. **Rejected** in
  favor of the additive rung (§4).
- **The `store_fact_with_caller_key` de-ratchet one-liner.** Makes `>= 3` dead code. **Rejected**
  in favor of count-in-content upsert.

---

## 7. Final verdict

- **FIX, not no-op.** Three live, design-level convergence gaps (D1/D2/D3) at HEAD `641f9c37`;
  the honest `×2` count indicts missing closing actions, not the counter.
- **Drift reconciliation:** the strategy's drift warning is a false alarm — `mod.rs, observer.rs,
  signal.rs, wiring.rs, guardrails.rs` are **byte-identical** to `6e3113bc`; only
  `tests_root_cause.rs` (+99, additive) differs. All citations re-verified live.
- **Regression safety (measured, not assumed):** 53 tests green at HEAD. D2 and D3 are **additive**
  — they add rungs and preserve every existing assertion, especially
  `decide_routes_workstream_coverage_to_flag_gaps` (D3 must **not** swap the arm) and the two-lane
  decoupling invariants (D2 must **not** couple the lanes). D1 is pure and breaks nothing.
- **Atomicity:** D2 ships as one change (gate + count-in-content, never `CallerKey`); D1, D3, gauges
  independent; D3 keys on `GapItem.signature`.
- **Landing order:** **D2 → D3 → D1 → gauges.** No threshold move; no durable whisper gate.
- **Scope:** investigation-only this wave — the plan is confirmed correct, dependency-ordered, and
  now backed by an executed regression baseline at HEAD.
