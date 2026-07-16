# Verification Phase — Practical Tests Executed for EACH Hypothesis (H0–H8)

**Scope:** Execute a practical verification test for **every** hypothesis in
[`HYPOTHESES.md`](./HYPOTHESES.md) about the recurring
`overseer-obs:…goal:blocked…|workstream-gap` signature "seen 2×" in cognitive memory.
This extends [`verification_results.md`](./verification_results.md) (H1-focused) to a
complete per-hypothesis matrix, each with the concrete test run and its outcome.

**Re-executed on HEAD `5c14ef03`** (twenty-first wave, `cargo test -p simard --lib`, 2026-07-16):
- Source is **byte-identical** to every prior verified HEAD: `git diff --stat cc55a6fb..HEAD -- src/`
  is **empty** and `git status --porcelain -- src/` is clean. The intervening commit (`5c14ef03`
  twentieth-wave re-execution + §28 consolidation) is `docs(investigation)/*.md`-only — **no production
  `.rs` changed, so every source citation below still holds and no fix is merged.**
- Test binary compiles clean (`cargo test -p simard --lib overseer:: --no-run` → `Finished` in 59.15s;
  `simard v0.32.1`).
- Full overseer suite (`cargo test -p simard --lib overseer::`): **361 passed, 0 failed** (7960 filtered)
  — unchanged from all prior waves.
- Source anchors re-pinned exactly at this HEAD (no drift): signature producer
  `observation_signature` (`mod.rs:1068-1073`; `keys.dedup()` at `mod.rs:1071`, `overseer-obs:` join at
  `mod.rs:1072`); `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, emit `>=` at `signal.rs:463`);
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`); `record_occurrence` (`mod.rs:1004`) uses
  non-deduping `store_fact` (`mod.rs:1034`).
- Every hypothesis's named discriminating probe re-executed **green** at this HEAD (fresh runs, not cached):
  - **H0** (null: dedup/replay/collision) + **H1** (real re-observation loop) — co-run batch of 7:
    `write_back_is_deduplicated_within_window`, `write_back_persists_again_for_a_distinct_signature`,
    `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority`,
    `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour` → **7/0** → H0 **REJECTED** (dedup gate works; count is
    honest), H1 **SUPPORTED**. Sole signature producer re-pinned at `mod.rs:1071` (`keys.dedup()`).
  - **H2** (WHY reasoner double-gated → bare park) — `no_progress_reinvestigation` broad filter
    **21 passed**, incl. smoking gun
    `ooda_loop::tests_no_progress_reinvestigation::a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`
    → INV-WHY still violable → H2 **SUPPORTED**.
  - **H3** (`WorkstreamCoverage` has no closing edge) — `decide_routes_workstream_coverage_to_flag_gaps`,
    `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` → **4/0** → H3 **SUPPORTED**
    (notify-only, no launch/file rung).
  - **H4** (self-observation write-back feedback) — source re-pinned: `keys.dedup()` collapses
    adjacent-equal only (`mod.rs:1071`); recall-derived summaries `sanitize_recalled`-cleaned then still
    written back (`observation_content` `mod.rs:1076-1085`) → H4 **SUPPORTED (bounded)**.
  - **H5** (2×↔3× dead zone) — `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
    → **1/0** + source: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362/463`) vs
    `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) → lanes decoupled → H5 **SUPPORTED**.
  - **H6** (non-idempotent counters, compounding) — source re-pinned: `record_occurrence` (`mod.rs:1004`)
    uses non-deduping `store_fact` (`mod.rs:1034`) → H6 **SUPPORTED (non-causal amplifier)**.
  - **H7** (blocked ↔ gap = one problem, two views) —
    `overseer::tests_gap_scan::delegates_blocked_goals_to_goal_health_and_never_reflags_them` (**1/0**) +
    `overseer::tests_goal_health::{decide_routes_a_blocked_goal_by_shape,`
    `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated,`
    `needs_review_goal_escalates_to_operator_on_both_channels}` (**3/0**) → **4/0 total** → H7 **SUPPORTED**.
  - **H8** (three token families = one under-throughput) — generalization of H7; no source drift on the
    benign-`engineer_spawn` finding (summary-only, never in signature) → H8 **SUPPORTED (med-high)**.
- **Verdict matrix unchanged from all prior runs; all discriminating tests green at HEAD `5c14ef03`.**
  Batch tallies this run: full overseer **361/0**; H0/H1 recurring-signature + whisper-gate batch **7/0**;
  H2 ladder **21/0**; H3 gap-closing-edge batch **4/0**; H5 dead-zone probe **1/0**; H7 blocked↔gap batch
  **4/0**. No production `.rs` changed; no remediation landed.

**Re-executed on HEAD `65cad015`** (twentieth wave, `cargo test -p simard --lib`, 2026-07-16):
- Source is **byte-identical** to every prior verified HEAD: `git diff --stat cc55a6fb..HEAD -- src/`
  is **empty** and `git status --porcelain -- src/` is clean. The intervening commits (`b312c50d`,
  `65cad015` §26 nineteenth-wave consolidation + closing convergence verdict) are
  `docs(investigation)/*.md`-only — **no production `.rs` changed, so every source citation below
  still holds and no fix is merged.**
- Test binary compiles clean (`cargo test -p simard --lib overseer:: --no-run` → `Finished` in 1m57s;
  `simard v0.32.1`).
- Full overseer suite (`cargo test -p simard --lib overseer::`): **361 passed, 0 failed** (7960 filtered)
  — unchanged from all prior waves.
- Every hypothesis's named discriminating probe re-executed **green** at this HEAD (fresh runs, not cached):
  - **H0** (null: dedup/replay/collision) + **H1** (real re-observation loop) — co-run batch of 7:
    `write_back_is_deduplicated_within_window`, `write_back_persists_again_for_a_distinct_signature`,
    `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority`,
    `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour` → **7/0** → H0 **REJECTED** (dedup gate works; count is
    honest), H1 **SUPPORTED**. Source re-pinned: sole signature producer at `mod.rs:1071` (`keys.dedup()`).
  - **H2** (WHY reasoner double-gated → bare park) — `no_progress_reinvestigation` broad filter
    **21 passed**, incl. smoking gun
    `ooda_loop::tests_no_progress_reinvestigation::a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`
    → INV-WHY still violable → H2 **SUPPORTED**.
  - **H3** (`WorkstreamCoverage` has no closing edge) — `decide_routes_workstream_coverage_to_flag_gaps`,
    `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` → **4/0** → H3 **SUPPORTED**
    (notify-only, no launch/file rung).
  - **H4** (self-observation write-back feedback) — source re-pinned: `keys.dedup()` collapses
    adjacent-equal only (`mod.rs:1071`); recall-derived summaries `sanitize_recalled`-cleaned then still
    written back (`mod.rs:1082`, admission `mod.rs:1359`) via `write_back_observation(&cycle.problems)`
    (`wiring.rs:301`) → H4 **SUPPORTED (bounded)**.
  - **H5** (2×↔3× dead zone) — `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
    → **1/0** + source: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, emit `>=` at `:463`) vs
    `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) → lanes decoupled → H5 **SUPPORTED**.
  - **H6** (non-idempotent counters, compounding) — source re-pinned: `record_occurrence` uses
    non-deduping `store_fact` (`mod.rs:1034`); `WhisperGate.last_delivered` is an in-process `HashMap`
    (`guardrails.rs:294`) → H6 **SUPPORTED (non-causal amplifier)**.
  - **H7** (blocked ↔ gap = one problem, two views) —
    `overseer::tests_gap_scan::delegates_blocked_goals_to_goal_health_and_never_reflags_them` (**1/0**) +
    `overseer::tests_goal_health::{decide_routes_a_blocked_goal_by_shape,`
    `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated,`
    `needs_review_goal_escalates_to_operator_on_both_channels}` (**3/0**) → **4/0 total** → H7 **SUPPORTED**.
  - **H8** (three token families = one under-throughput) — generalization of H7; no source drift on the
    benign-`engineer_spawn` finding (summary-only, never in signature) → H8 **SUPPORTED (med-high)**.
- **Verdict matrix unchanged from all prior runs; all discriminating tests green at HEAD `65cad015`.**
  Batch tallies this run: full overseer **361/0**; H0/H1 recurring-signature + whisper-gate batch **7/0**;
  H2 ladder **21/0**; H3 gap-closing-edge batch **4/0**; H5 dead-zone probe **1/0**; H7 blocked↔gap batch
  **4/0**. No production `.rs` changed; no remediation landed.

**Re-executed on HEAD `2191fcd2`** (nineteenth wave, `cargo test -p simard --lib`, 2026-07-16):
- Source is **byte-identical** to every prior verified HEAD: `git diff --stat cc55a6fb..HEAD -- src/`
  and `git diff --stat b47b6413..HEAD -- src/` are both **empty**, and `git status --porcelain -- src/`
  is clean. The two intervening commits (`d00e4c3f` primary emission-pipeline deep dive + re-verification,
  `2191fcd2` §25 eighteenth-wave consolidation) are `docs(investigation)/*.md`-only — **no production
  `.rs` changed, so every source citation below still holds and no fix is merged.**
- Full overseer suite (`cargo test -p simard --lib overseer::`): **361 passed, 0 failed** (7960 filtered) —
  unchanged from all prior waves.
- Every hypothesis's named discriminating probe re-run **green** at this HEAD:
  - **H0** (null: dedup/replay/collision) — `write_back_is_deduplicated_within_window`,
    `write_back_persists_again_for_a_distinct_signature`,
    `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour` (co-run with H1 batch, **7/0**) — **pass** → H0
    **REJECTED** (dedup gate works; the count is honest).
  - **H1** (real re-observation loop) — `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority` (H0+H1 batch **7/0**) — **pass** → H1 **SUPPORTED**.
    Source re-pinned: sole signature producer at `mod.rs:1070-1072` (`sort_unstable` + `dedup` + `overseer-obs:` prefix, `|`-join).
  - **H2** (WHY reasoner double-gated → bare park) — `no_progress_reinvestigation` broad filter **21 passed**,
    incl. smoking gun `ooda_loop::tests_no_progress_reinvestigation::a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`
    re-run `--exact` (**1 passed**) → INV-WHY still violable → H2 **SUPPORTED**.
  - **H3** (`WorkstreamCoverage` has no closing edge) — `decide_routes_workstream_coverage_to_flag_gaps`,
    `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` (**4/0**) — **pass** → H3
    **SUPPORTED** (notify-only, no launch/file rung).
  - **H4** (self-observation write-back feedback) — source invariant re-pinned: `keys.dedup()`
    collapses adjacent-equal only (`mod.rs:1071`); recall-derived `RecurringSignature` still admitted +
    written back → H4 **SUPPORTED (bounded)**.
  - **H5** (2×↔3× dead zone) — `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
    (**1/0**) + source: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, emit `>=` at `:463`) vs
    `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) → lanes decoupled → H5 **SUPPORTED**.
  - **H6** (non-idempotent counters, compounding) — source re-pinned: `record_occurrence` (`mod.rs:1004`)
    uses non-deduping append `store_fact` (`mod.rs:1034`) → H6 **SUPPORTED (non-causal amplifier)**.
  - **H7** (blocked ↔ gap = one problem, two views) — `delegates_blocked_goals_to_goal_health_and_never_reflags_them`,
    `decide_routes_a_blocked_goal_by_shape`, `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`,
    `needs_review_goal_escalates_to_operator_on_both_channels` (**4/0**) — **pass** → H7 **SUPPORTED**.
  - **H8** (three token families = one under-throughput) — generalization of H7; no source drift → H8 **SUPPORTED (med-high)**.
- **§25.2 drift-correction re-confirmed at HEAD:** `outcome_records_occurrence` (`wiring.rs:612-627`) still
  **excludes** `ActOutcome::Reported` from its `matches!` set — the terminal Rung-4 Report sink never records,
  so Lane-B `recurrence` stays `0` and Rung 1 (`>=3`) is unreachable for exactly the dead-zone goals. The
  minimal D2 fix (add `Reported` to that set) remains **live and unlanded**.
- **Verdict matrix unchanged across all nineteen waves; all discriminating tests green at HEAD `2191fcd2`.**
  Batch tallies this run: full overseer **361/0**; H0/H1 recurring-signature + whisper-gate batch **7/0**;
  H2 ladder (broad filter) **21/0**; H2 smoking-gun `--exact` **1/0**; H3 gap-closing-edge batch **4/0**;
  H5 dead-zone probe **1/0**; H7 blocked↔gap batch **4/0**. No production `.rs` changed; no remediation landed.

**Re-executed on HEAD `cc55a6fb`** (`cargo test -p simard --lib`, 2026-07-16):
- Source is **byte-identical** to every prior verified HEAD: `git diff --stat b47b6413..HEAD -- src/`
  is **empty**, `git diff --stat 87206fbb..HEAD -- src/` is **empty**, and `git status --porcelain
  -- src/` is clean. The intervening commits (`87206fbb` §24 seventeenth-wave consolidation, `cc55a6fb`
  emission-pipeline re-grounding + nesting-loop reproduction) are all `docs(investigation)/*.md`-only —
  **no production `.rs` changed, so every source citation below still holds and no fix is merged.**
- Test binary compiles clean (`cargo test -p simard --lib overseer:: --no-run` → `Finished` in 59s;
  `simard v0.32.1`).
- Full overseer suite (`overseer::`): **361 passed, 0 failed** (7960 filtered) — unchanged.
- Every hypothesis's named discriminating probe re-run **green** at this HEAD:
  - **H0** (null: dedup/replay/collision) — `write_back_is_deduplicated_within_window`,
    `write_back_persists_again_for_a_distinct_signature`,
    `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour` (co-run with H1 batch, **7/0**) — **pass** → H0
    **REJECTED** (dedup gate works; the count is honest).
  - **H1** (real re-observation loop) — `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority` (H0+H1 batch **7/0**) — **pass** → H1 **SUPPORTED**.
  - **H2** (WHY reasoner double-gated → bare park) — `no_progress_reinvestigation` broad filter
    **21 passed**, incl. smoking gun
    `ooda_loop::tests_no_progress_reinvestigation::a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`
    re-run `--exact` (**1 passed**) → INV-WHY still violable → H2 **SUPPORTED**.
  - **H3** (`WorkstreamCoverage` has no closing edge) — `decide_routes_workstream_coverage_to_flag_gaps`,
    `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` (**4/0**) — **pass** → H3
    **SUPPORTED** (notify-only, no launch/file rung).
  - **H4** (self-observation write-back feedback) — source invariant re-pinned: `keys.dedup()`
    collapses adjacent-equal only (`mod.rs:1071`), and the recall-derived summaries are only
    `sanitize_recalled`-cleaned then still written back (`mod.rs:1082`, admission import `mod.rs:93`) →
    H4 **SUPPORTED (bounded)**.
  - **H5** (2×↔3× dead zone) — `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
    (**1/0**) + source: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, used `signal.rs:463`) vs
    `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) → lanes decoupled → H5 **SUPPORTED**.
  - **H6** (non-idempotent counters, compounding) — source re-pinned: `record_occurrence` uses
    non-deduping `store_fact` (`mod.rs:1034`); `WhisperGate.last_delivered` is an in-process `HashMap`
    (`guardrails.rs:294`) → H6 **SUPPORTED (non-causal amplifier)**.
  - **H7** (blocked ↔ gap = one problem, two views) — `delegates_blocked_goals_to_goal_health_and_never_reflags_them`,
    `decide_routes_a_blocked_goal_by_shape`, `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`,
    `needs_review_goal_escalates_to_operator_on_both_channels` (**4/0**) — **pass** → H7 **SUPPORTED**.
  - **H8** (three token families = one under-throughput) — generalization of H7; no source drift on the
    benign-`engineer_spawn` finding (summary-only, never in signature) → H8 **SUPPORTED (med-high)**.
- **Verdict matrix unchanged from all prior runs; all discriminating tests green at HEAD `cc55a6fb`.**
  Batch tallies this run: full overseer **361/0**; H0/H1 recurring-signature + whisper-gate batch
  **7/0**; H2 ladder (broad filter) **21/0**; H2 smoking-gun `--exact` **1/0**; H3 gap-closing-edge
  batch **4/0**; H5 dead-zone probe **1/0**; H7 blocked↔gap batch **4/0**. No production `.rs` changed;
  no remediation landed.

**Re-executed on HEAD `b47b6413`** (`cargo test -p simard --lib`, 2026-07-16):
- Source is **byte-identical** to the previous verification run (`a68296c6`): `git diff --stat
  a68296c6..HEAD -- src/` is **empty** and `git status --porcelain -- src/` is clean. The intervening
  commits (`…9fd1ea0a`, `7293de99`, `3fac68a5`, `f455c06d` re-verify docs, plus `a68296c6`, `b47b6413`
  §22/§23 consolidations) are all `docs(investigation)/*.md`-only — **no production `.rs` changed, so
  every source citation below still holds and no fix is merged.**
- Test binary compiles clean (`cargo test -p simard --lib overseer:: --no-run` → `Finished` in 57s;
  `simard v0.32.1`).
- Full overseer suite (`overseer::`): **361 passed, 0 failed** (7960 filtered) — unchanged.
- Every hypothesis's named discriminating probe re-run **green** at this HEAD:
  - **H0** (null: dedup/replay/collision) — `write_back_is_deduplicated_within_window`,
    `write_back_persists_again_for_a_distinct_signature`,
    `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour` — **pass** → H0 **REJECTED** (dedup gate works; the count is honest).
  - **H1** (real re-observation loop) — `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority` — **pass** → H1 **SUPPORTED**.
  - **H2** (WHY reasoner double-gated → bare park) — `no_progress_reinvestigation` broad filter **21 passed**
    (module `ooda_loop::tests_no_progress_reinvestigation` itself = 11 `#[test]` fns, all green), incl.
    smoking gun `ooda_loop::tests_no_progress_reinvestigation::a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`
    re-run `--exact` (**1 passed**) → INV-WHY still violable → H2 **SUPPORTED**.
  - **H3** (`WorkstreamCoverage` has no closing edge) — `decide_routes_workstream_coverage_to_flag_gaps`,
    `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` — **pass** → H3 **SUPPORTED** (notify-only, no launch/file rung).
  - **H4** (self-observation write-back feedback) — source invariant re-pinned: `keys.dedup()` collapses
    adjacent-equal only (`mod.rs:1071`); recall-derived `RecurringSignature` still admitted + written back → H4 **SUPPORTED (bounded)**.
  - **H5** (2×↔3× dead zone) — `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
    (**pass**) + source: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) vs
    `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) → lanes decoupled → H5 **SUPPORTED**.
  - **H6** (non-idempotent counters, compounding) — source: `record_occurrence` uses non-deduping
    `store_fact` (`mod.rs:1034`); `WhisperGate.last_delivered` is an in-process `HashMap`
    (`guardrails.rs:294`) → H6 **SUPPORTED (non-causal amplifier)**.
  - **H7** (blocked ↔ gap = one problem, two views) — `delegates_blocked_goals_to_goal_health_and_never_reflags_them`,
    `decide_routes_a_blocked_goal_by_shape`, `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`,
    `needs_review_goal_escalates_to_operator_on_both_channels` — **pass** → H7 **SUPPORTED**.
  - **H8** (three token families = one under-throughput) — generalization of H7; no source drift on the
    benign-`engineer_spawn` finding (summary-only, never in signature) → H8 **SUPPORTED (med-high)**.
- **Verdict matrix unchanged from all prior runs; all discriminating tests green at HEAD `b47b6413`.**
  Batch tallies this run: full overseer **361/0**; H0/H1/H5 core batch **3/0**; H0/H1 recurring-signature
  + whisper-gate batch **5/0**; H3/H7 probe batch **8/0**; H2 ladder (broad filter) **21/0**; H2 smoking-gun
  `--exact` **1/0**. Env note: `NODE_OPTIONS=--max-old-space-size=32768` (saved preference) is irrelevant
  to the Rust test path — host has 503 GiB RAM, 473 GiB free; the test binary peaks well under 1 GiB, so
  the memory-pressure hypothesis is **not applicable** to this signature. No production `.rs` changed; no
  remediation landed.

**Re-executed on HEAD `a68296c6`** (`cargo test -p simard --lib`, 2026-07-16):
- Source is **byte-identical** to the previous run: `git diff --stat ad5e1060..HEAD -- src/`
  and `git diff --stat 05c08919..HEAD -- src/` are both **empty**, and `git status --porcelain -- src/`
  is clean — the intervening commits (`…7293de99`, `3fac68a5`, `9fd1ea0a`, `d6ba8b25`, `a68296c6`) are
  all `docs(investigation)/*.md`-only (fifteenth-wave §22 consolidation). **No production `.rs` changed,
  so every source citation below still holds and no fix is merged.**
- Test binary compiles clean (`cargo test -p simard --lib overseer:: --no-run` → `Finished` in 57s).
- Full overseer suite (`overseer::`): **361 passed, 0 failed** (7960 filtered).
- Every hypothesis's named discriminating probe re-run **green** at this HEAD:
  - **H0** (null: dedup/replay/collision) — `write_back_is_deduplicated_within_window`,
    `write_back_persists_again_for_a_distinct_signature`,
    `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour` — **pass** → H0 **REJECTED** (dedup gate works; the count is honest).
  - **H1** (real re-observation loop) — `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority` — **pass** → H1 **SUPPORTED**.
  - **H2** (WHY reasoner double-gated → bare park) — `no_progress_reinvestigation` ladder **21 passed**,
    incl. smoking gun `ooda_loop::tests_no_progress_reinvestigation::a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked`
    re-run `--exact` (**1 passed**) → INV-WHY still violable → H2 **SUPPORTED**.
  - **H3** (`WorkstreamCoverage` has no closing edge) — `decide_routes_workstream_coverage_to_flag_gaps`,
    `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority` — **pass** → H3 **SUPPORTED** (notify-only, no launch/file rung).
  - **H4** (self-observation write-back feedback) — source invariant re-pinned: `keys.dedup()` collapses
    adjacent-equal only (`mod.rs:1071`); recall-derived `RecurringSignature` still admitted + written back → H4 **SUPPORTED (bounded)**.
  - **H5** (2×↔3× dead zone) — `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
    (**pass**) + source: `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) vs
    `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) → lanes decoupled → H5 **SUPPORTED**.
  - **H6** (non-idempotent counters, compounding) — source: `record_occurrence` uses non-deduping
    `store_fact` (`mod.rs:1034`); `WhisperGate.last_delivered` is an in-process `HashMap`
    (`guardrails.rs:294`) → H6 **SUPPORTED (non-causal amplifier)**.
  - **H7** (blocked ↔ gap = one problem, two views) — `delegates_blocked_goals_to_goal_health_and_never_reflags_them`,
    `decide_routes_a_blocked_goal_by_shape`, `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`,
    `needs_review_goal_escalates_to_operator_on_both_channels` — **pass** → H7 **SUPPORTED**.
  - **H8** (three token families = one under-throughput) — generalization of H7; no source drift on the
    benign-`engineer_spawn` finding (summary-only, never in signature) → H8 **SUPPORTED (med-high)**.
- **Verdict matrix unchanged from all prior runs; all discriminating tests green at HEAD `a68296c6`.**
  Batch tallies this run: full overseer **361/0**; H0/H1/H5 probe batch **8/0**; H3/H7 probe batch **8/0**;
  H2 ladder **21/0**; H2 smoking-gun `--exact` **1/0**. No production `.rs` changed; no remediation landed.

**Re-executed on HEAD `ad5e1060`** (`cargo test -p simard --lib`, 2026-07-15):
- Source is **byte-identical** to the previous run: `git diff --name-only 05c08919..HEAD`
  returns solely `ai_working/investigation/verification_results_ALL_HYPOTHESES.md`
  (docs-only), and `git diff --stat 05c08919..HEAD -- src/` is empty. The working tree adds
  only three investigation deep-dive docs (`primary_emission_pipeline_trace_HEAD_05c08919.md`,
  `secondary_two_loops_and_drift_HEAD_ad5e1060.md`, `tertiary_architecture_LANDING_HEAD_ad5e1060.md`)
  — **no production `.rs` changed**, so every source citation below still holds and **no fix is merged**.
- Full overseer suite (`overseer::`): **361 passed, 0 failed** (7960 filtered).
- All named discriminating probes re-run green at this HEAD (`--exact`):
  - H0/H1 dedup+persist + H0 whisper-gate + H1 recurring-signature family + H5 lane-decoupling
    (one batch of 8, 8313 filtered): `write_back_is_deduplicated_within_window`,
    `write_back_persists_again_for_a_distinct_signature`,
    `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour`,
    `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority`,
    `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` — **8 passed**.
  - H3/H7 gap+route-by-shape (one batch of 8, 8313 filtered):
    `decide_routes_workstream_coverage_to_flag_gaps`, `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority`,
    `delegates_blocked_goals_to_goal_health_and_never_reflags_them`,
    `decide_routes_a_blocked_goal_by_shape`,
    `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`,
    `needs_review_goal_escalates_to_operator_on_both_channels` — **8 passed**.
  - H2 reinvestigation ladder (`goal_curation::tests_no_progress_reinvestigation` +
    `ooda_loop::tests_no_progress_reinvestigation`): **21 passed** (8300 filtered), including the
    smoking gun `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked` (re-run `--exact`, passed).
- Source invariants re-confirmed at this HEAD (unchanged from `05c08919`):
  H5 `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) vs
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`);
  H4 `write_back_observation(&cycle.problems)` writes ALL problems (`wiring.rs:301`);
  H6 `record_occurrence` (`mod.rs:1004`) uses non-deduping `mem.store_fact` (`mod.rs:1034`) and
  `WhisperGate.last_delivered` is an in-process `HashMap` (`guardrails.rs:294`);
  H0 `keys.dedup()` collapses adjacent-equal only (`mod.rs:1071`); H2 `is_bare_no_progress_block`
  gate present (`ooda_loop/no_progress.rs:832`), INV-WHY still violable (perpetual-goal smoking-gun
  still passes); H3 notify-only arm `act_flag_workstream_gaps` (`mod.rs:884`) + `FlagWorkstreamGaps`
  routing (`mod.rs:671,1543`). **Verdict matrix below unchanged; all tests green.**

**Previously re-executed on HEAD `05c08919`** (`cargo test -p simard --lib`, 2026-07-15):
- Full overseer suite (`overseer::`): **361 passed, 0 failed** (7960 filtered).
- All named discriminating probes re-run green at this HEAD:
  - H0/H1 dedup+persist: `write_back_is_deduplicated_within_window`,
    `write_back_persists_again_for_a_distinct_signature` (2 passed).
  - H0 whisper-gate: `whisper_gate_suppresses_an_identical_whisper_within_the_window`,
    `whisper_gate_caps_whispers_per_rolling_hour` (`overseer::tests_whisper`, 2 passed).
  - H1 recurring-signature family (`overseer::tests_memory_recall`): 8 passed incl.
    `recurring_signature_emitted_when_two_episodes_share_signature`,
    `recurring_signature_not_emitted_for_single_occurrence`,
    `orient_raises_recurring_signature_to_high_priority`.
  - H2 reinvestigation ladder: **21 passed** across `goal_curation::tests_no_progress_reinvestigation`
    (10) + `ooda_loop::tests_no_progress_reinvestigation` (11), including the smoking gun
    `a_perpetual_goal_is_never_reinvestigated_even_if_bare_blocked` and
    `a_reinvestigated_goal_is_not_processed_again_next_cycle`.
  - H3/H7 gap+delegation (`overseer::tests_gap_scan`): `decide_routes_workstream_coverage_to_flag_gaps`,
    `flagged_gap_never_constructs_an_issue_brief`,
    `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`,
    `workstream_gap_maps_to_a_workstream_coverage_problem_at_high_priority`,
    `delegates_blocked_goals_to_goal_health_and_never_reflags_them` — all green.
  - H5 lane-decoupling: `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` (passed).
  - H7 route-by-shape (`overseer::tests_goal_health`): `decide_routes_a_blocked_goal_by_shape`,
    `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated`,
    `needs_review_goal_escalates_to_operator_on_both_channels` — all green.
- Source invariants re-confirmed at this HEAD (some paths/lines drifted; invariants hold):
  H5 `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) vs
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`, applied at `mod.rs:1613`);
  H2 bare-block predicate `is_bare_no_progress_block` (`goal_curation/no_progress_breaker.rs:108`)
  + bare renderer `no_progress_blocked_reason` (`:123`) split from `_with_why` (`:141`) —
  **file relocated from `overseer/goal_curation/` to top-level `src/goal_curation/`**, INV-WHY
  still violable (perpetual-goal smoking-gun test still passes); H3 notify-only arm
  `act_flag_workstream_gaps` (`mod.rs:884`) + `FlagWorkstreamGaps` routing (`mod.rs:1543`);
  H4 `write_back_observation(&cycle.problems)` writes ALL problems (`wiring.rs:301`);
  H6 `record_occurrence` (`mod.rs:1004`) uses non-deduping `mem.store_fact` (`mod.rs:1034`) and
  `WhisperGate.last_delivered` is an in-process `HashMap` (`guardrails.rs:294`).
  **Verdict matrix below unchanged; all tests green.** Note: the H2 reinvestigation ladder has
  grown (21 tests) — the self-heal path now covers done/precondition/upstream/retry-spent variants,
  which *strengthens* remediation coverage but does **not** falsify H2: a perpetual bare block still
  persists without a WHY token, so INV-WHY remains violable.

**Previously re-executed on HEAD `bbddd23a`** (`cargo test -p simard --lib`, 2026-07-15):
- Full overseer suite (`overseer::`): **361 passed, 0 failed** (7960 filtered).
- 17 named discriminating tests (all H0–H8 probes, incl. the two
  `ooda_loop::tests_no_progress_reinvestigation` H2 tests): **all green** —
  4 H0/H1 dedup+whisper-gate probes (8317 filtered) + 13 H1–H8 route/shape probes
  (8308 filtered).
- Source invariants re-confirmed at this HEAD: H5 `RECURRING_SIGNATURE_THRESHOLD = 2`
  (`signal.rs:362`) vs `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`);
  H2 bare-block predicate `is_bare_no_progress_block` (`goal_curation/no_progress_breaker.rs:108`)
  + bare renderer `no_progress_blocked_reason` (`:123`) split from `_with_why` (`:141`) —
  INV-WHY still violable; H3 notify-only arm `act_flag_workstream_gaps` (`mod.rs:884`) +
  `FlagWorkstreamGaps` routing (`mod.rs:1543`); H4 `write_back_observation(&cycle.problems)`
  writes ALL problems (`wiring.rs:301`); H6 `record_occurrence` uses non-deduping
  `mem.store_fact` (`mod.rs:1034`) and `WhisperGate.last_delivered` is an in-process
  `HashMap` (`guardrails.rs:294`); write-back gate `WhisperGate::new(900,5)` (`mod.rs:299`).
  **Verdict matrix below unchanged; all tests green.**

**Previously re-executed on HEAD `cb8cd1dc`** (`cargo test -p simard --lib`, 2026-07-15):
- Full overseer suite (`overseer::`): **361 passed, 0 failed** (7960 filtered).
- 17 named discriminating tests (`--exact`, all H0–H8 probes incl. the two
  `ooda_loop::tests_no_progress_reinvestigation` H2 tests): **all green** (8368 filtered).
- Source invariants re-confirmed at this HEAD (line numbers drifted as the suite grew;
  the invariants themselves hold): H5 `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`)
  vs `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`); H6 `record_occurrence`
  uses non-deduping `mem.store_fact` (`mod.rs:1203`) and `WhisperGate.last_delivered` is an
  in-process `HashMap` (`guardrails.rs:294`); write-back gate `WhisperGate::new(900,5)`
  (`mod.rs:299`); H4 `write_back_observation(&cycle.problems)` writes ALL problems
  (`wiring.rs:301`); H0 `keys.dedup()` collapses adjacent-equal only (`mod.rs:1262`);
  H2 bare-block predicate `is_bare_no_progress_block` + renderer `no_progress_blocked_reason`
  present (`no_progress_breaker.rs:108/123`) — INV-WHY still violable. Note: H2 has since been
  refined in source — `is_evidenceless_no_progress_block` + `needs_reinvestigation` now also
  cover the `evidence=[(none)]` variant (the live-daemon defect verified 2026-07-15),
  strengthening H2 rather than falsifying it. **Verdict matrix below unchanged; all tests green.**

**Previously re-reproduced on HEAD `440e024c`** (`cargo test -p simard --lib`):
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
