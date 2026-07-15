# Verification Phase — Practical Tests Executed for EACH Hypothesis @ HEAD `f9cefec1`

**Scope:** Re-execute a practical verification test for **every** hypothesis (H0–H8) in
[`HYPOTHESES.md`](./HYPOTHESES.md) about the recurring
`overseer-obs:…goal:blocked…|workstream-gap` signature "seen 2×" in cognitive memory, on the
**current HEAD** `f9cefec1` (`test(overseer): verify Lane-A RecurringSignature does not feed
Lane-B recurrence`). This confirms the prior matrices
([`verification_results_ALL_HYPOTHESES.md`](./verification_results_ALL_HYPOTHESES.md) @ `440e024c`/
`5a85317b`, [`verification_results.md`](./verification_results.md)) still hold on the latest tree.

**Environment:** `cargo test -p simard --lib` (package `simard` owns `src/overseer`,
`src/goal_curation`, `src/ooda_loop`). Compiler: crate `simard v0.32.1`.

## Executed runs (this wave, HEAD `f9cefec1`)

| Run | Command scope | Result |
|---|---|---|
| Full overseer suite | `cargo test -p simard --lib overseer::` | **361 passed, 0 failed** (7960 filtered) |
| H0 whisper-gate probes (2) | `--exact` whisper suppress + rolling-hour cap | **2 passed, 0 failed** |
| Named discriminating tests (15) | H0/H1/H2/H3/H7 batch below | **15 passed, 0 failed** |
| Lane-A/Lane-B decoupling probes (5) | H5/H6 root-cause batch below | **5 passed, 0 failed** |

**Targeted total: 22 discriminating tests, all green** (2 + 15 + 5). Absolute overseer count
drifts a few tests across waves (359 → 360 → **361**) as the suite grows; the invariant —
**0 failures, every discriminating test green** — holds.

## Source invariants re-confirmed on HEAD `f9cefec1`

| Invariant | Location (HEAD `f9cefec1`) | Status |
|---|---|---|
| H5 detection threshold `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | ✅ unchanged |
| H5 escalation threshold `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | ✅ unchanged |
| H6 `record_occurrence` uses non-deduping `mem.store_fact(…)` | `overseer/mod.rs:1034` | ✅ unchanged |
| H2 bare-block renderer split from `_with_why` | `no_progress_breaker.rs:123` / `:141` | ✅ unchanged |
| H2 `is_bare_no_progress_block` still `pub`, no forcing invariant | `no_progress_breaker.rs:108` | ✅ unchanged |
| Write-back gate `WhisperGate::new(900, 5)` | `overseer/mod.rs:299` | ✅ unchanged |
| Completion done-gate conjunction (`missing.is_empty()`) | `completion_gate.rs:394-438` | ✅ unchanged |

The `completion_gate.rs:394` `evaluate` conjunction (§16.1 D0 reconciliation seam) confirmed: a
goal with `issue_closed == true` but `pr_merged == false` yields
`Blocked { missing: [PrNotMerged] }` (`:424-425`) — consistent with the hypotheses' D0 seam.

---

## Named discriminating tests executed (15, all ✅)

| # | Test | Module | Hypothesis |
|---|---|---|---|
| 1 | `write_back_is_deduplicated_within_window` | `overseer::tests_memory_recall` | H0 |
| 2 | `write_back_persists_again_for_a_distinct_signature` | `overseer::tests_memory_recall` | H0 |
| 3 | `recurring_signature_emitted_when_two_episodes_share_signature` | `overseer::tests_memory_recall` | H1 |
| 4 | `recurring_signature_not_emitted_for_single_occurrence` | `overseer::tests_memory_recall` | H1 |
| 5 | `orient_raises_recurring_signature_to_high_priority` | `overseer::tests_memory_recall` | H1 |
| 6 | `a_reinvestigated_goal_is_not_processed_again_next_cycle` | `ooda_loop::tests_no_progress_reinvestigation` | H2 |
| 7 | `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked` | `ooda_loop::tests_no_progress_reinvestigation` | H2 (smoking gun) |
| 8 | `decide_routes_workstream_coverage_to_flag_gaps` | `overseer::tests_gap_scan` | H3 |
| 9 | `flagged_gap_never_constructs_an_issue_brief` | `overseer::tests_gap_scan` | H3 |
| 10 | `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat` | `overseer::tests_gap_scan` | H3 |
| 11 | `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` | `overseer::tests_gap_scan` | H3 |
| 12 | `delegates_blocked_goals_to_goal_health_and_never_reflags_them` | `overseer::tests_gap_scan` | H7 |
| 13 | `decide_routes_a_blocked_goal_by_shape` | `overseer::tests_goal_health` | H7 |
| 14 | `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` | `overseer::tests_goal_health` | H7 |
| 15 | `needs_review_goal_escalates_to_operator_on_both_channels` | `overseer::tests_goal_health` | H7 |

## Lane-A/Lane-B decoupling probes executed (5, all ✅) — H5/H6

| # | Test | Verifies |
|---|---|---|
| 1 | `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` | Lane A `×2` never advances Lane B `recurrence` (the dead zone / no bridge) |
| 2 | `lane_b_escalates_without_any_lane_a_signal` | Lane B escalation keyed on root-cause facts, independent of Lane A episodes |
| 3 | `analyze_without_recall_is_telemetry_sourced_with_zero_recurrence` | fresh cause → `recurrence == 0` |
| 4 | `analyze_promotes_recall_and_records_recurrence_and_memory_source` | recall promotes + records occurrence (Lane B accrual) |
| 5 | `occurrence_recall_accumulates_recurrence_across_ticks` | non-idempotent monotonic accrual across ticks (H6 amplifier) |

---

## Per-hypothesis verdict matrix @ HEAD `f9cefec1` (reproduced)

| ID | Hypothesis | Practical test executed this wave | Result | Verdict |
|----|-----------|-----------------------------------|--------|---------|
| H0 | Dedup/storage/replay/collision artifact | tests 1–2 + whisper-gate ×2 | intra-window dupes suppressed; distinct sigs both persist; re-delivers past 900 s | **REJECTED** |
| H1 | Real re-observation of near-static set | tests 3–5 | ×2 = 2 distinct windows; 1 occ emits nothing; signal → High problem | **CONFIRMED** |
| H2 | WHY reasoner double-gated → bare parks | tests 6–7 + source probe | a perpetual goal stays bare-blocked, never gets a WHY → INV-WHY violable | **SUPPORTED** |
| H3 | `WorkstreamCoverage` has no closing edge | tests 8–11 | route to `FlagWorkstreamGaps`, no issue, notify-only + dedup-forever | **SUPPORTED** |
| H4 | Self-observation write-back feedback | write-back-all-problems trace (`wiring.rs:301`) | recall-derived `RecurringSignature` re-emitted (bounded) | **SUPPORTED (bounded)** |
| H5 | 2×↔3× dead zone, two decoupled lanes | probes 1–2 + constants (`signal.rs:362`/`root_cause.rs:33`) | detect@2, escalate@3, Lane A never feeds Lane B | **SUPPORTED** |
| H6 | Non-idempotent counters (compounding) | probes 3–5 + `mod.rs:1034` trace | monotonic lifetime accrual, non-causal amplifier | **SUPPORTED (non-causal)** |
| H7 | blocked ↔ gap = one problem, two views | tests 12–15 | blocked goals leave gap scan → goal_health; self-heal↔park oscillation real | **SUPPORTED** |
| H8 | Three families = one under-throughput | membership analysis (§11.1) | `engineer_spawn` benign drift; all observe-and-flag | **SUPPORTED (med-high)** |

---

## Bottom line

On the current HEAD `f9cefec1`, **all 22 targeted discriminating tests and the full 361-test
overseer suite pass with 0 failures**, and every source invariant the eight hypotheses depend on is
unchanged. The `×2` remains a **faithful cross-window recurrence count of a genuinely re-observed
near-static problem set** (H1 confirmed; H0 rejected); it persists because two observe-and-flag
loops never close (H2 bare-park no-WHY, H3 gap notify-only), the count parks in the **dead zone
between thresholds 2 and 3** (H5, now guarded by `loud_lane_a_recurring_signature_does_not_feed_
lane_b_recurrence`), the overseer re-observes its own bookkeeping (H4, bounded), and the counters
lack idempotency (H6, compounding). H7/H8 unify the symptoms into one under-throughput condition in
three views. All defects are design-level; none is a dedup/storage bug. Every confirming test green,
every refuting condition empirically excluded — reproduced on the latest tree.
