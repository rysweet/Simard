# Secondary Investigation — Composite over-aggregation, self-feed loop & signal-vs-defect verdict

**Role:** SECONDARY investigator
**Focus:** Composite whole-board over-aggregation vs. per-problem granularity; the
non-converging "dead zone"; signal-vs-defect verdict from empirical board-advance evidence.
**HEAD:** `1de21e71` (verified — all line citations re-grounded against current source,
not trusted from prior-wave artifacts).
**Empirical grounding:** `cargo test --lib overseer::tests_memory_recall` → **32 pass, 0 fail**.

---

## Verdict (one line)

The `2×` is an **honest** re-observation, but the composite `observation_signature` is a
**whole-board fingerprint that self-feeds and self-nests**: unlike `workstream-gap`, the
composite Lane-A DOES have a closing action (`ProcessHealth → LaunchRecipe`) — but it fires
that action on a **self-referential meta-string**, and the resulting meta-problem re-enters
the next write-back, growing the signature (the nested `overseer-obs:overseer-obs:` tokens in
the question string). The defect is **over-aggregation + an unguarded write-boundary**, not
the counter.

---

## F1 — The two recurrence lanes, re-grounded at HEAD (thresholds differ)

| Lane | Emitter | Key granularity | Store | Recall count | Floor | Closing action |
|---|---|---|---|---|---|---|
| **A (composite)** | `write_back_observation` (`mod.rs:534-563`) | **whole board** `observation_signature` (`mod.rs:1068-1073`) | episodic `[sig:…]` (`wiring.rs:1084-1088`) | `signals_from` counts `failure_signature` (`signal.rs:455-470`) | **2** (`RECURRING_SIGNATURE_THRESHOLD`, `signal.rs:362`) | `RecurringSignature → ProcessHealth → LaunchRecipe` (`mod.rs:1353-1363, 1429-1435`) |
| **B (per-problem)** | `record_occurrence` (`mod.rs:1004-1043`) | single `dedup_key` | semantic fact via `store_fact` | `recall_occurrences` (`mod.rs:972-997`) → `root_cause::analyze` `recurrence` | **3** (`RECURRENCE_ESCALATION_THRESHOLD`, `root_cause.rs:33`) | `decide_blocked_goal` → `EscalateBlockedGoal` (`mod.rs:1613-1618`) |

Lanes are **decoupled** (proven green: `tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`).
The `2 vs 3` gap is real: a single blocked goal recurring exactly `2×` does **not** trip
Lane-B escalation (floor 3), while Lane-A only fires when the WHOLE board repeats.

## F2 — Over-aggregation: the composite only recurs when the ENTIRE board repeats

`observation_signature(problems)` = `"overseer-obs:" + sorted(dedup)(all problem keys).join("|")`
(`mod.rs:1068-1073`). Consequences:

- **Brittle to board churn.** ANY membership change (one goal unblocks, one new gap appears,
  spawn-rate crosses 8) mints a DIFFERENT signature. A board that oscillates never accumulates
  ≥2 identical episodes, so **individually-stuck goals can recur forever without the composite
  ever firing** — the true non-convergence risk.
- **Unbounded growth with blocked-goal count.** Confirmed intentional whole-snapshot encoding
  (`mod.rs:1064-1067`, issue #2628), bounded only by `sanitize_recalled`'s 8192-byte cap
  (`capabilities.rs:455, 468-472`).
- **Coarse actionability.** When it DOES fire, the launched recipe's `task_description` is the
  giant self-referential string `"recurring signature seen 2× in cognitive memory (overseer-obs:…)"`
  (`mod.rs:1360-1362` → `1431`) — not a specific, fixable problem.

## F3 — Lane-A SELF-FEEDS and SELF-NESTS (the mechanism behind the nested tokens) — NEW

The only episodic writer carrying a `[sig:…]` marker is the Overseer's own
`record_observation` (`wiring.rs:1076-1091`); `parse_failure_signature` reads it back
(`wiring.rs:976-986, 1025`). Therefore **every** `failure_signature` the recall counter sees is
a prior *Overseer write-back*, and it is **always the composite** — no per-problem episodic
writer exists (`record_occurrence` uses `store_fact`, i.e. Lane B, not episodic). This yields
two under-appreciated facts:

1. **The `orient` "merge into matching in-cycle problem" branch is effectively dead for this
   pipeline** (`mod.rs:1211-1221`). It only merges when the recalled signature equals a single
   problem's `dedup_key`; the composite `overseer-obs:g1|g2|…` never equals `goal:blocked:g1`.
   So the RecurringSignature **always spawns a STANDALONE `ProcessHealth` meta-problem** whose
   `dedup_key = sanitize_recalled(composite)`.
2. **That meta-problem re-enters the next write-back.** `wiring.rs:301` calls
   `write_back_observation(&cycle.problems)` over ALL problems — including the meta-problem —
   with **no write-boundary filter** excluding recall-derived problems. Next cycle:
   `observation_signature` folds the prior composite back in →
   `overseer-obs:overseer-obs:g1|…|g1|…`. **This is exactly the nested `overseer-obs:` tokens
   in the investigation question string.** `write_back_persists_again_for_a_distinct_signature`
   (green) confirms each mutated signature persists a fresh episode.

**Net:** Lane-A is isolated from Lane-B (tested) but **NOT isolated from itself** (untested).
The base composite parks at a stable `2×` (re-added every cycle while its two base episodes are
recalled), while a growing nested tail accumulates episodes and consumes memory.

## F4 — Signal-vs-defect verdict

- **The count is a faithful signal.** `observation_signature` is deterministic
  (sort+dedup); `2×` means two persisted write-back episodes shared the identical composite —
  i.e., the same SET of blocked-goal IDs + gaps was observed twice. Because `dedup_key` is
  `goal:blocked:{id}` (reason-independent, `mod.rs:1336`), identical composite = "the same
  set of things is stuck." That is a genuine *board-did-not-advance* signal, not a counting bug.
  (Within-window dedup proven green: `write_back_is_deduplicated_within_window`.)
- **The DEFECT is the response, on two counts:**
  1. **`WorkstreamCoverage` has no closing rung** — `FlagWorkstreamGaps → act_flag_workstream_gaps`
     notifies only (`mod.rs:884-948`); confirmed by the prior convergence wave
     (`secondary_reemission_and_convergence_HEAD_f9cefec1.md` F1). Re-emits every window forever.
  2. **`ProcessHealth` (composite Lane-A) HAS a closing rung, but the wrong one** — it
     `LaunchRecipe`s (cost-bearing, gated by `max_launches_per_cycle`, `mod.rs:607-611`) on a
     self-referential meta-string, and the meta-problem it acts on **re-amplifies its own
     signature**. This is self-observation noise / potential self-amplification, distinct from
     the gap dead-zone.

So the answer to "signal or defect": **the recurrence is a true signal of a non-advancing
board; the over-aggregated composite + unguarded write-boundary is the defect.** Do NOT touch
the counter (documented trap, `PATTERNS.md`).

## F5 — Is per-problem granularity redundant with the composite? Partly, and better.

A per-problem recurrence lane already exists (Lane B, `recall_occurrences`/`record_occurrence`)
and converges correctly on a specific `goal:blocked:{id}` at floor 3 → `EscalateBlockedGoal`.
It is **immune to board churn** (keyed on one problem) — exactly what the composite is not.
The composite adds little that Lane B or the per-gap path don't cover more precisely, while
adding the self-nesting pathology. Recommendation leans toward **tracking recurrence
per-problem, and demoting the composite to a pure telemetry write-back that is NOT recalled
into a `LaunchRecipe`** (or excluding recall-derived meta-problems at the write boundary).

---

## Anti-patterns present

- **Over-aggregation / whole-board fingerprint** (F2) — recurrence undetectable under board churn.
- **Self-observation feedback without write-boundary guard** (F3) — Lane-A meta-problem re-enters
  write-back; matches `PATTERNS.md` "Self-observation feedback".
- **Observe-and-flag-without-closing** (F4.1, gaps) — endorsed from prior wave.
- **Recurrence dead zone** (F1, `2 < 3` for a single goal via Lane B).
- **Doc/impl drift** on the intended gap-file rung (prior wave F2) — not re-audited here.

## Integration points

`mod.rs:301(wiring)/534-563(write-back)/972-1043(Lane B)/1068-1073(sig)/1200-1235(orient)/1353-1363,1429-1543(classify+decide)` ·
`signal.rs:362,455-470` · `wiring.rs:976-986,1013-1031,1076-1091` · `capabilities.rs:455,468-472,611-636` ·
`root_cause.rs:33`.

## Test gaps (for verification phase)

- **No test exercises the REAL composite path.** `orient_raises_recurring_signature_to_high_priority`
  and `loud_lane_a_…` both feed a **per-problem** signature (`process:distill_fail`,
  `goal:blocked:research`), never `overseer-obs:g1|g2|…`. So the standalone-meta-problem outcome
  (F3.1) and its `LaunchRecipe` are unasserted.
- **No test asserts Lane-A self-feed / nesting** is bounded or excluded. Isolation tests cover
  only A→B, not A→A. Add: a cycle that write-backs → recalls → fires RecurringSignature → asserts
  whether the meta-problem re-enters the next `observation_signature` (it currently does).

## Questions for verification phase

- **Q1:** Confirm no per-problem episodic `[sig:…]` writer exists anywhere (I found only
  `wiring.rs:1088`); if true, the `orient` merge branch is dead for production recall and the
  composite meta-problem is ALWAYS standalone.
- **Q2:** Is the composite `ProcessHealth → LaunchRecipe` actually admitted in production, or
  perpetually held by `max_launches_per_cycle`? Its admission determines whether the self-feed is
  merely noisy or actively resource-amplifying.
- **Q3:** Should the minimal fix be (a) exclude recall-derived/`ProcessHealth`-meta problems at the
  `write_back_observation` boundary (kills nesting), (b) demote composite RecurringSignature from
  `LaunchRecipe` to advisory/telemetry, or (c) both? (b)+(a) close both the self-feed and the
  non-actionable launch without touching the honest counter.
