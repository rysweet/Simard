# Verification Phase — Practical Tests Executed for EACH Hypothesis @ HEAD `f1db90f4`

**Scope:** Re-execute a practical verification test for **every** hypothesis (H0–H8) in
[`HYPOTHESES.md`](./HYPOTHESES.md) about the recurring
`overseer-obs:…goal:blocked…|workstream-gap` signature "seen 2×" in cognitive memory, on the
**current HEAD** `f1db90f4` (`docs(investigation): fold remaining tenth-wave dives … into §17`).
`f1db90f4` is one **docs-only** commit past `f9cefec1`
([prior wave](./verification_results_HEAD_f9cefec1.md)); this confirms all matrices still hold on
the latest tree.

**Environment:** `cargo test -p simard --lib` — crate `simard v0.32.1` (owns `src/overseer`,
`src/ooda_loop`, `src/goal_curation`).

## Executed runs (this wave, HEAD `f1db90f4`)

| Run | Command scope | Result |
|---|---|---|
| Full overseer suite | `cargo test -p simard --lib overseer::` | **361 passed, 0 failed** (7960 filtered) |
| Core discriminating batch (9) | H0/H1/H2/H3/H5 `--exact` | **9 passed, 0 failed** |
| Remaining discriminating batch (11) | H3/H6/H7 `--exact` | **11 passed, 0 failed** |
| H0 whisper-gate probes (2) | within-window suppress + rolling-hour cap | **2 passed, 0 failed** |

**Targeted total: 22 discriminating tests, all green** (9 + 11 + 2). Overseer suite: **361/361**,
0 failures — identical to the prior wave.

## Source invariants re-confirmed on HEAD `f1db90f4`

| Invariant | Location | Status |
|---|---|---|
| H5 detection threshold `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | ✅ unchanged |
| H5 escalation threshold `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | ✅ unchanged |
| H6 `record_occurrence` uses non-deduping `mem.store_fact(…)` | `overseer/mod.rs:1034` | ✅ unchanged |
| Write-back gate `WhisperGate::new(900, 5)` | `overseer/mod.rs:299` | ✅ unchanged |
| Blocked-goal gate `WhisperGate::new(900, 20)` | `overseer/mod.rs:292` | ✅ unchanged |
| Gap gate `WhisperGate::new(900, 200)` | `overseer/mod.rs:304` | ✅ unchanged |

---

## Named discriminating tests executed (22, all ✅)

### H0 — dedup/storage/replay/collision artifact (NULL) — 4 tests
| Test | Module | Verifies |
|---|---|---|
| `write_back_is_deduplicated_within_window` | `overseer::tests_memory_recall` | intra-window dupes suppressed → not a double-read |
| `write_back_persists_again_for_a_distinct_signature` | `overseer::tests_memory_recall` | distinct sigs both persist → honest count |
| `whisper_gate_suppresses_an_identical_whisper_within_the_window` | `overseer::tests_whisper` | within-window gate collapses identical |
| `whisper_gate_caps_whispers_per_rolling_hour` | `overseer::tests_whisper` | rolling-hour cap re-delivers past window |

### H1 — real re-observation of near-static set (CAUSE of 2×) — 3 tests
| Test | Module | Verifies |
|---|---|---|
| `recurring_signature_emitted_when_two_episodes_share_signature` | `overseer::tests_memory_recall` | ×2 = 2 distinct windows |
| `recurring_signature_not_emitted_for_single_occurrence` | `overseer::tests_memory_recall` | 1 occ emits nothing |
| `orient_raises_recurring_signature_to_high_priority` | `overseer::tests_memory_recall` | signal → High-priority problem |

### H2 — WHY reasoner double-gated → bare parks — 2 tests
| Test | Module | Verifies |
|---|---|---|
| `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked` | `ooda_loop::tests_no_progress_reinvestigation` | smoking gun: bare-block, never a WHY |
| `a_reinvestigated_goal_is_not_processed_again_next_cycle` | `ooda_loop::tests_no_progress_reinvestigation` | single-shot reinvestigation gate |

### H3 — `WorkstreamCoverage` has no closing edge — 4 tests
| Test | Module | Verifies |
|---|---|---|
| `decide_routes_workstream_coverage_to_flag_gaps` | `overseer::tests_gap_scan` | route to notify-only `FlagWorkstreamGaps` |
| `flagged_gap_never_constructs_an_issue_brief` | `overseer::tests_gap_scan` | no issue filed |
| `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat` | `overseer::tests_gap_scan` | notify-only + dedup-forever |
| `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` | `overseer::tests_gap_scan` | High-priority mapping |

### H5 — 2×↔3× dead zone, two decoupled lanes — 2 tests
| Test | Module | Verifies |
|---|---|---|
| `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` | `overseer::tests_root_cause` | Lane A ×2 never advances Lane B |
| `lane_b_escalates_without_any_lane_a_signal` | `overseer::tests_root_cause` | Lane B keyed on facts, independent |

### H6 — non-idempotent counters (compounding, non-causal) — 3 tests
| Test | Module | Verifies |
|---|---|---|
| `analyze_without_recall_is_telemetry_sourced_with_zero_recurrence` | `overseer::tests_root_cause` | fresh cause → `recurrence == 0` |
| `analyze_promotes_recall_and_records_recurrence_and_memory_source` | `overseer::tests_root_cause` | recall promotes + records occurrence |
| `occurrence_recall_accumulates_recurrence_across_ticks` | `overseer::tests_root_cause` | monotonic accrual across ticks (amplifier) |

### H7 — blocked ↔ gap = one problem, two views — 4 tests
| Test | Module | Verifies |
|---|---|---|
| `delegates_blocked_goals_to_goal_health_and_never_reflags_them` | `overseer::tests_gap_scan` | blocked goals leave gap scan |
| `decide_routes_a_blocked_goal_by_shape` | `overseer::tests_goal_health` | shape-based routing |
| `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` | `overseer::tests_goal_health` | self-heal↔park oscillation |
| `needs_review_goal_escalates_to_operator_on_both_channels` | `overseer::tests_goal_health` | genuine review → operator escalation |

### H4 / H8 — trace + membership-analysis backed (no isolated named test)
- **H4** (self-observation write-back feedback): confirmed by the write-back-all-problems path
  (`wiring.rs:301` re-emits recall-derived `RecurringSignature`), bounded by the write-back gate,
  recall limit, same-key merge, and the ×2 threshold. All four throttles unchanged this wave.
- **H8** (three families = one under-throughput): `resource:engineer_spawn` is benign membership
  drift whose `{live}` count lands only in the summary, never in the signature — all three families
  are observe-and-flag under the same 15-min gate below escalation=3. No source change this wave.

---

## Per-hypothesis verdict matrix @ HEAD `f1db90f4`

| ID | Hypothesis | Practical test executed this wave | Result | Verdict |
|----|-----------|-----------------------------------|--------|---------|
| H0 | Dedup/storage/replay/collision artifact | 2 write-back + 2 whisper-gate probes | intra-window dupes suppressed; distinct sigs persist; re-delivers past 900 s | **REJECTED** |
| H1 | Real re-observation of near-static set | 3 recall tests | ×2 = 2 distinct windows; 1 occ emits nothing; signal → High | **CONFIRMED** |
| H2 | WHY reasoner double-gated → bare parks | 2 reinvestigation tests + source probe | perpetual goal stays bare-blocked, never a WHY → INV-WHY violable | **SUPPORTED** |
| H3 | `WorkstreamCoverage` has no closing edge | 4 gap-scan tests | route to `FlagWorkstreamGaps`, no issue, notify-only + dedup-forever | **SUPPORTED** |
| H4 | Self-observation write-back feedback | write-back-all-problems trace (`wiring.rs:301`) | recall-derived `RecurringSignature` re-emitted (bounded) | **SUPPORTED (bounded)** |
| H5 | 2×↔3× dead zone, two decoupled lanes | 2 lane probes + constants | detect@2, escalate@3, Lane A never feeds Lane B | **SUPPORTED** |
| H6 | Non-idempotent counters (compounding) | 3 root-cause probes + `mod.rs:1034` trace | monotonic lifetime accrual, non-causal amplifier | **SUPPORTED (non-causal)** |
| H7 | blocked ↔ gap = one problem, two views | 4 gap/goal-health tests | blocked goals leave gap scan → goal_health; self-heal↔park real | **SUPPORTED** |
| H8 | Three families = one under-throughput | membership analysis | `engineer_spawn` benign drift; all observe-and-flag | **SUPPORTED (med-high)** |

---

## Bottom line

On the current HEAD `f1db90f4`, **all 22 targeted discriminating tests and the full 361-test
overseer suite pass with 0 failures**, and every source invariant the eight hypotheses depend on is
unchanged from `f9cefec1`. The `×2` remains a **faithful cross-window recurrence count of a
genuinely re-observed near-static problem set** (H1 confirmed; H0 rejected). It persists because two
observe-and-flag loops never close (H2 bare-park no-WHY, H3 gap notify-only), the count parks in the
**dead zone between thresholds 2 and 3** (H5), the overseer re-observes its own bookkeeping (H4,
bounded), and the counters lack idempotency (H6, compounding). H7/H8 unify the symptoms into one
under-throughput condition in three views. All defects are design-level; none is a dedup/storage
bug. Every confirming test green, every refuting condition empirically excluded — reproduced on the
latest tree.
