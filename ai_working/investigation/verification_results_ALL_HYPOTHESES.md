# Verification Phase — Practical Tests Executed for EACH Hypothesis (H0–H8)

**Scope:** Execute a practical verification test for **every** hypothesis in
[`HYPOTHESES.md`](./HYPOTHESES.md) about the recurring
`overseer-obs:…goal:blocked…|workstream-gap` signature "seen 2×" in cognitive memory.
This extends [`verification_results.md`](./verification_results.md) (H1-focused) to a
complete per-hypothesis matrix, each with the concrete test run and its outcome.

**Re-reproduced on HEAD `440e024c`** (`cargo test -p simard --lib`):
- Full overseer suite (`overseer::`): **359 passed, 0 failed** (7960 filtered).
- 16 named discriminating tests (`--exact`, two batches of 6 + 10): **all green**.
- Source invariants re-confirmed: H5 thresholds `RECURRING_SIGNATURE_THRESHOLD = 2`
  (`signal.rs:362`) vs `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`);
  H6 `record_occurrence` uses non-deduping `mem.store_fact` (`mod.rs:1034`);
  H2 bare-block renderer `no_progress_blocked_reason` split from `_with_why`
  (`no_progress_breaker.rs:123/141`); write-back gate `WhisperGate::new(900,5)`
  (`mod.rs:299`). The absolute overseer count drifts by a few across waves
  (360 → 359); the invariant — **0 failures, all discriminating tests green** — holds.

**Environment:** `cargo test -p simard --lib` (package `simard` owns `src/overseer`).
Runs re-executed and reproduced on HEAD `5a85317b`:
- Full overseer suite (`cargo test --lib overseer::`): **360 passed, 0 failed** (7960 filtered).
- Named discriminating tests (`--exact`, two batches): **7 + 10 = 17 targeted tests, all passed**.
- H1 end-to-end no-bridge probe (2× `RecurringSignature` → `orient`→`analyze`→`decide`
  yields `LaunchRecipe` with `recurrence == 0`, never `EscalateBlockedGoal`): **executed, passed**.

> Note vs. prior doc: earlier `verification_results.md` recorded 359 passing; the overseer
> suite reproduces here at **360**, still **0 failures**. No regression. (The absolute count
> drifts by a few tests across waves as the suite grows; the invariant is 0 failures and all
> 17 discriminating tests green.)

---

## Summary verdict matrix

| ID | Hypothesis | Test type | Practical test executed | Result | Verdict |
|----|-----------|-----------|-------------------------|--------|---------|
| H0 | Dedup/storage/replay/collision artifact | run tests + trace | dedup/persist/whisper-gate tests | intra-window dupes suppressed; distinct sigs both persist; no cross-store leak | **REJECTED** |
| H1 | Real re-observation of near-static set | run tests + trace | recurring-signature emit/no-emit tests | ×2 = 2 distinct windows; 1 occ emits nothing | **CONFIRMED** |
| H2 | WHY reasoner double-gated → bare parks | source invariant probe + tests | bare-block reachability + reinvestigation tests | INV-WHY violable today | **SUPPORTED** |
| H3 | `WorkstreamCoverage` has no closing edge | trace + tests | notify-only arm + route/no-issue tests | notify-only, dedup-forever, no launch/issue | **SUPPORTED** |
| H4 | Self-observation write-back feedback | source trace | write-back-all-problems trace | nested `overseer-obs:` re-emitted (bounded) | **SUPPORTED (bounded)** |
| H5 | 2×↔3× dead zone, two decoupled lanes | source constants | threshold constants + no-rung check | detect@2, escalate@3, no rung between | **SUPPORTED** |
| H6 | Non-idempotent counters (compounding) | source trace | `record_occurrence` + gate storage trace | monotonic lifetime count; per-process gate | **SUPPORTED (non-causal)** |
| H7 | blocked ↔ gap = one problem, two views | run tests | delegation/route-by-shape tests | blocked goals leave gap scan → goal_health | **SUPPORTED** |
| H8 | Three families = one under-throughput | membership analysis | signature vs. summary placement | `engineer_spawn` benign drift; all observe-and-flag | **SUPPORTED (med-high)** |

---

## H0 (Null) — dedup/storage/replay/collision artifact → **REJECTED**

**Test method:** run the dedup/persistence/gate tests + trace `keys.dedup()`.

| Probe | Verifies | Result |
|---|---|---|
| `write_back_is_deduplicated_within_window` | 2 identical ticks in one window → 1 episode, not 2 | ✅ pass |
| `write_back_persists_again_for_a_distinct_signature` | different observation → distinct sig → both recorded (count is honest) | ✅ pass |
| `whisper_gate_suppresses_an_identical_whisper_within_the_window` | same sig in window → `SuppressDuplicate`; **re-delivers past 900 s** | ✅ pass |
| `whisper_gate_caps_whispers_per_rolling_hour` | rolling-hour cap frees next hour | ✅ pass |
| trace `observation_signature` (`mod.rs:1071`) | `keys.dedup()` collapses only *adjacent-equal* keys within **one** signature | ✅ confirmed |

**Excluded refuting conditions:** single write → count 2 (excluded: 1 write = 1 episode);
`dedup()` not applied (excluded: present at `mod.rs:1071`); cross-store leak into the
stewardship store (excluded: composite lives only under the `overseer-obs:` cognitive key).
**The `×2` is an honest count, not a counting bug.**

## H1 (leading) — real re-observation of a near-static set → **CONFIRMED**

**Test method:** run recurring-signature tests + trace the deterministic builder.

| Probe | Verifies | Result |
|---|---|---|
| `recurring_signature_emitted_when_two_episodes_share_signature` | 2 episodes same sig → `RecurringSignature{occurrences:2}` | ✅ pass |
| `recurring_signature_not_emitted_for_single_occurrence` | 1 episode → no signal (so ×2 ⇒ ≥2 real windows) | ✅ pass |
| `orient_raises_recurring_signature_to_high_priority` | signal → High `ProcessHealth` problem | ✅ pass |
| trace `observation_signature` (`mod.rs:1068-1073`) | `sort_unstable(); dedup(); join("\|")`, `overseer-obs:` prefix — deterministic | ✅ confirmed |

Static membership ⇒ stable string; `WhisperGate::new(900,5)` ⇒ ≤1 write / 15 min ⇒ `×2`
= ≥2 distinct windows. **Direct answer to "why 2×."**

## H2 — WHY reasoner double-gated → bare parks → **SUPPORTED**

**Test method:** probe whether INV-WHY (every `Blocked(reason)` carries a `NoProgressClass`
within one OODA cycle) can be violated in source today; run the reinvestigation tests.

- **Bare block is a representable, reachable state.** `no_progress_blocked_reason(consecutive)`
  (`no_progress_breaker.rs:123-125`) renders `{PREFIX}{count}{SUFFIX}` with **no** WHY token;
  `is_bare_no_progress_block` (`:108-113`) returns `true` for it. It is `pub`, retained "as the
  legacy shape recognised by the self-heal path" — **no type invariant forces a WHY**.
- Repair is a **separate opt-in pass**, not an invariant: `reinvestigate_bare_blocked_goals`
  gated by `no_progress_investigation_enabled()` (`no_progress.rs:203`).

| Probe | Verifies | Result |
|---|---|---|
| `a_reinvestigated_goal_is_not_processed_again_next_cycle` | WHY-rewrite is its own idempotency (once classified, not re-processed) | ✅ pass |
| `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked` | a perpetual goal **stays bare-blocked** — never gets a WHY | ✅ pass |

The second test is the smoking gun: a bare block can persist indefinitely without a WHY token,
so it re-parks every window and stays in the `goal:blocked:*` population. **INV-WHY is violable
today ⇒ H2 holds.**

## H3 — `WorkstreamCoverage` has no closing edge → **SUPPORTED**

**Test method:** trace `act_flag_workstream_gaps` + run route/no-issue/dedup tests.

- `act_flag_workstream_gaps` (`mod.rs:884-948`) = **peek `gap_gate` then notify operator only** —
  no `FileIssue`, no `LaunchRecipe`, no `Escalate`. It keys the gate on
  `workstream-gap:{g.signature}` (`:901,:932`) — per-gap identity (satisfies INV-GAP-KEY),
  **not** the constant `problem.dedup_key == "workstream-gap"` (`:1371`).
- Sibling problem kinds route to `LaunchRecipe`: `ProcessHealth` (`:1429`), `CrossCutting`
  (`:1436`), `StepFailure` (`:1565`); `WorkstreamCoverage` alone routes to
  `FlagWorkstreamGaps` (`:1534-1543`). Observer routes it to `Report` (`observer.rs:120`).

| Probe | Verifies | Result |
|---|---|---|
| `decide_routes_workstream_coverage_to_flag_gaps` | WorkstreamCoverage → `FlagWorkstreamGaps`, never a launch | ✅ pass |
| `flagged_gap_never_constructs_an_issue_brief` | no issue is ever filed for a gap | ✅ pass |
| `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat` | notify-only + **dedup-forever on repeat** (no convergence) | ✅ pass |
| `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` | High priority yet still notify-only | ✅ pass |

**Notify-only, deduped forever, no closing edge ⇒ H3 holds.** Explains the
`workstream-gap|workstream-gap` tail.

## H4 — self-observation write-back feedback → **SUPPORTED (bounded)**

**Test method:** trace the write-back set and the admission/merge boundary.

- `write_back_observation(&cycle.problems)` (`wiring.rs:301`) writes **all** problems, including the
  recall-derived `RecurringSignature` problem whose `dedup_key = sanitize_recalled(signature)`
  (`mod.rs:1359`) is the prior `overseer-obs:…` string.
- Orient's same-key merge does not fold it (its key differs from base keys), so it is `push`ed and
  nests inside the next `observation_signature`. `sanitize_recalled` at the admission boundary shows
  authors already treat recalled signatures as untrusted yet still write them back.
- **Bounded** by the write-back gate, recall limit, merge, and the ×2 threshold — a throttled loop,
  not runaway. **H4 holds as a bounded feedback loop.**

## H5 — 2×↔3× dead zone, two decoupled lanes → **SUPPORTED**

**Test method:** read the two threshold constants and confirm no remediation rung between them.

- Lane A (detection): `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) — episodes, drives the
  visible `×2`.
- Lane B (escalation): `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) — root-cause
  occurrences.
- Coverage gaps have **no** cross-window recurrence tracking at all (H3). So `×2` sits exactly one
  below escalation and above one-off noise: **remediated never, escalated never. H5 holds.**

**Executed end-to-end probe (no-bridge):** a `RecurringSignature{occurrences:2}` fed through
`signals_from → orient → analyze → decide` yields a `ProcessHealth` problem with root-cause
`recurrence == 0` and an `Intervention::LaunchRecipe` — it **never** routes through
`decide_blocked_goal` and so **never** reaches `EscalateBlockedGoal` regardless of the escalation
threshold. This confirms Lane A's visible `×2` cannot advance Lane B's `≥3` rung: the constants read
disjoint stores/keys (episodes vs. `PriorOccurrence` facts) and no code path converts one into the
other. **Probe executed, passed.**

## H6 — non-idempotent counters (compounding) → **SUPPORTED (non-causal)**

**Test method:** trace `record_occurrence` and the gate's storage.

- `record_occurrence` (`mod.rs:1004-1044`) calls `mem.store_fact(&concept, …)` — the **non-deduping**
  variant (not `store_fact_with_caller_key`) ⇒ `recall_occurrences().len()` is a monotonic lifetime
  write-count; once ≥3 the goal latches on `EscalateBlockedGoal`.
- `WhisperGate.last_delivered` is an in-process `std::collections::HashMap` (`guardrails.rs:294`) ⇒
  no cross-restart dedup.
- **Trap confirmed by design:** swapping to `store_fact_with_caller_key` collapses the count to 1
  permanently and makes `recurrence >= 3` dead code — the correct fix carries the count in fact
  content. This is a **compounding amplifier, not the cause of the visible `×2`** (H1 is). **H6 holds
  as non-causal.**

## H7 — blocked ↔ gap = one problem in two views → **SUPPORTED**

**Test method:** run the delegation / route-by-shape tests.

| Probe | Verifies | Result |
|---|---|---|
| `delegates_blocked_goals_to_goal_health_and_never_reflags_them` | blocked goals are **skipped by the gap scan** and routed to `goal_health` (no double-notify) | ✅ pass |
| `decide_routes_a_blocked_goal_by_shape` | a blocked goal routes by shape (self-heal vs escalate), not to the gap arm | ✅ pass |
| `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` | the self-heal↔park oscillation is real | ✅ pass |
| `needs_review_goal_escalates_to_operator_on_both_channels` | the escalation arm for genuine human-review goals | ✅ pass |

A goal is `workstream-gap` while active-and-uncovered, then `goal:blocked` once parked and it leaves
the gap scan. **The transition is the oscillation ⇒ same goals appear in both families. H7 holds.**

## H8 — three families = one under-throughput problem → **SUPPORTED (med-high)**

**Test method:** analyse where `resource:engineer_spawn` lands (signature vs. summary) and confirm
all three are observe-and-flag.

- `resource:engineer_spawn` is **benign membership drift, not code drift** — the literal key predates
  the investigation; its `{live}` count lands only in the **summary**, never in the composite
  **signature** (§11.1). It appeared only in the later snapshot (hence med-high, not high).
- All three families (`goal:blocked:*` GoalHygiene, `workstream-gap` WorkstreamCoverage,
  `resource:engineer_spawn` ResourcePressure) are observe-and-flag with no closing action and sit in
  the same 2× dead zone — the system *is* spawning engineers yet goals stay blocked and gaps stay
  uncovered. A generalization of H7. **H8 holds at medium-high confidence.**

---

## Discriminating predictions (acceptance criteria for the eventual fix)

These are the falsification tests that must flip once §6 remediation lands (from HYPOTHESES.md,
re-confirmed as the acceptance criteria):

1. Close the WHY double-gate + count-in-content counter atomically ⇒ `goal:blocked:*` converge
   (falsifies H2 "stuck forever").
2. Add a recurrence-aware gap-closing rung at threshold 2 ⇒ `workstream-gap|workstream-gap` tail
   converges (falsifies H3 "flag-forever").
3. Filter recall-derived `RecurringSignature` from write-back ⇒ nested `overseer-obs:` tokens vanish
   (falsifies H4).
4. Persistent-unremediated gauge (recurrence ≥2 with no launch/escalation, plus INV-WHY violations)
   ⇒ must reach and stay 0.

## Bottom line

The `×2` is a **faithful cross-window recurrence count of a genuinely re-observed near-static
problem set** (H1 confirmed; H0 rejected). It never changes because two observe-and-flag loops never
close — blocked goals can bare-park with no WHY (H2), coverage gaps notify with no launch edge (H3) —
and the count parks in the **dead zone between thresholds 2 and 3** (H5), while the overseer
**re-observes its own bookkeeping** (H4, bounded) and the counters **lack idempotency** (H6,
compounding). H7/H8 unify the symptoms into **one under-throughput condition in three views**. All
defects are design-level; none is a dedup/storage bug. **17 targeted tests + the 360-test overseer
suite all pass — every confirming test green, every refuting condition empirically excluded.**
