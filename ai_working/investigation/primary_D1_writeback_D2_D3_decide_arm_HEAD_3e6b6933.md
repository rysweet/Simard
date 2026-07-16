# Primary Deep Dive — D1 signature write-back + D2/D3 Decide-arm gap

**Role:** PRIMARY investigator (this wave)
**HEAD:** `3e6b6933`  **Date:** 2026-07-16
**Focus:** D1 self-observation write-back; D2/D3 Decide-arm closing edge
**Drift precondition:** `git diff --name-only d71c2410..HEAD -- src/` → **zero non-test
`.rs` drift**. Every load-bearing citation below re-grounded byte-for-byte at HEAD.

Verdict: **EXTEND, do not restart.** Prior 23 waves reached a fixpoint. This wave
re-verifies all D1/D2/D3 citations live and reaffirms: the defect is a *missing
convergence rung*, not a counting/dedup bug. Actionable next step is **remediation**.

---

## D1 — Self-observation write-back (nesting `overseer-obs:` fragments)

**Data flow (self-ingestion loop, no write-boundary self-provenance filter):**

1. `write_back_observation(&cycle.problems)` — `mod.rs:534-563`
   - Gathers `observation_signature(problems)` and stores an `ObservationEpisode`
     via the ephemeral `write_back_gate` (in-memory `WhisperGate`, no persisted state).
2. `observation_signature` — `mod.rs:1068-1073`
   ```rust
   fn observation_signature(problems: &[Problem]) -> String {
       let mut keys: Vec<&str> = problems.iter().map(|p| p.dedup_key.as_str()).collect();
       keys.sort_unstable();
       keys.dedup();                       // collapses ADJACENT equals only
       format!("overseer-obs:{}", keys.join("|"))
   }
   ```
3. `record_observation` — `wiring.rs:1076-1091`: embeds `... [sig:{signature}]` +
   `{"signature": …}` metadata; provenance FIXED (`source_label = "overseer"`, `wiring.rs:1081`).
4. `run_cycle` dispatch — `wiring.rs:301-312`: write-back after the act loop; a store
   bumps `memory_writes`, an error is surfaced (never swallowed).

**Key finding (live):** there is **no self-provenance filter at the WRITE boundary**
of `write_back_observation` (`mod.rs:534-546`). Recall promotes a prior
`RecurringSignature` (whose signature already begins `overseer-obs:`) into a fresh
`ProcessHealth` problem that re-enters `write_back_observation`, so its `dedup_key`
is re-wrapped: `overseer-obs:…|overseer-obs:…`. Because `keys.dedup()` only collapses
**adjacent** equals and each family key appears at most once per snapshot, the literal
`…|workstream-gap|workstream-gap|…` doubling is a **positive fingerprint of D1 nesting**,
impossible from true per-token duplication.

- Egress hardening exists (`sanitize_recalled`, `capabilities.rs:468`, applied at
  `mod.rs:1082/1084/1359-1360`) but it sanitizes *text*, not *provenance* — it does
  not drop self-authored recalled problems.
- **Only G1 dedup gate** sits on the self-feed edge and is **defeated by signature
  mutation** (the signature changes each time a fragment nests, so the gate sees a new key).

**Minimal D1 fix (no cross-file plumbing):** in `write_back_observation` (`mod.rs:534-546`),
drop `overseer-obs:`-prefixed / recall-derived problems from `problems` *before*
computing `observation_signature`. Order-independent defence-in-depth with the
recall-side `source_label == "overseer"` exclusion.

---

## D2 — Blocked-goal escalation dead zone (2×↔3×, two decoupled lanes)

**Two independent counters, no rung between them:**

- **Lane A** (observation episodes): `RECURRING_SIGNATURE_THRESHOLD = 2`
  (`signal.rs:362`), emitted at `signal.rs:463` when `occurrences >= 2`.
- **Lane B** (root-cause occurrences): `RECURRENCE_ESCALATION_THRESHOLD = 3`
  (`root_cause.rs:33`), consumed by `decide_blocked_goal` at `mod.rs:1613`.

**Structural latch (the load-bearing D2 root cause):** occurrences are only recorded
for outcomes in `outcome_records_occurrence` (`wiring.rs:612-627`). That set is:
`Launched | Merged | Deployed | IssueFiled | Escalated | Whispered | GoalUnblocked |
GoalEscalated | ConflictResolved | GoalTransferred | Audited`.

- **`ActOutcome::Reported` is NOT in the set.** A sub-threshold blocked goal routed to
  `Intervention::Report` (`decide_blocked_goal`, `mod.rs:1630`) accrues **exactly zero**
  Lane-B occurrences → its `recurrence` can **never** climb to 3 → the
  `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` escalation at `mod.rs:1613` is
  **structurally unreachable** for this path. (Note `Reported` *is* in the sibling
  `outcome_takes_effect` predicate, `wiring.rs:420` — the two predicates diverge exactly here.)
- Recording is append-only via `store_fact` (`mod.rs:1034`), keyed by
  `root_cause_signature = "{dedup_key}::{label}"` (`root_cause.rs:53-55`).

**Minimal D2 fix (must ship ATOMICALLY — gate + counter together):** add
`ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612-627`) so a
re-observed bare-parked blocked goal accrues occurrences and can reach the escalation
rung. Landing the gate without the counter (or vice-versa) changes nothing.

---

## D3 — WorkstreamCoverage Decide arm has no closing edge (notify-only)

**Decide → Act path (observe-and-flag, never launches/files):**

1. Decide arm — `mod.rs:1534-1543`: `ProblemKind::WorkstreamCoverage =>
   Intervention::FlagWorkstreamGaps { gaps }`.
2. Risk class — `guardrails.rs:60`: `FlagWorkstreamGaps => RiskClass::Routine` (never
   HIGH-RISK, so never escalated by autonomy gating).
3. Act dispatch — `mod.rs:671`: `FlagWorkstreamGaps { gaps } => act_flag_workstream_gaps(gaps)`.
4. `act_flag_workstream_gaps` — `mod.rs:884-948`: peeks each gap on `gap_gate`
   (`mod.rs:901-908`), **notifies the operator on both channels** (`mod.rs:929-930`),
   commits the dedup slot (`mod.rs:931-934`), returns `WorkstreamGapsFlagged`.
   **No `LaunchRecipe`, no `IssueFiled`, no stewardship backlog item.**

**Key finding (live):** unlike sibling HIGH-priority arms — `StepFailure` reaches
`Intervention::LaunchRecipe` (`mod.rs:1565-1579`) — the WorkstreamCoverage arm has
**no launch/file edge**. And `ActOutcome::WorkstreamGapsFlagged` is **absent** from
`outcome_records_occurrence` (`wiring.rs:612-627`), so gap flags also accrue zero
Lane-B occurrences → the gap loop can never escalate either. Contract-verified by
`tests_gap_scan.rs` (`flagged_gap_never_constructs_an_issue_brief`,
`flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`).

**Minimal D3 fix:** add a closing rung at first proven recurrence (Lane-A threshold 2)
that, for a gap that re-flags, escalates/files ONE issue.
**INV-GAP-KEY:** the rung MUST key on `GapItem.signature` (as `act_flag_workstream_gaps`
already does, `mod.rs:901`), NOT the bare `"workstream-gap"` dedup_key, or all gaps
fold into a single issue.

---

## Convergence-dynamics summary (why the composite never converges)

The composite `overseer-obs:…|goal:blocked:…|workstream-gap|workstream-gap` is one
under-throughput problem viewed in two states — `workstream-gap` (active) ↔
`goal:blocked` (idle) — re-observed faithfully each window. `×2` is HONEST
(Lane-A threshold 2, H1-confirmed / H0-rejected). It recurs forever because:

- **D3** flags but never launches/files → the gap is never closed.
- **D2** bare-parks the blocked goal as `Reported` → zero Lane-B occurrences → the
  escalate-at-3 rung is unreachable; it sits permanently in the **2↔3 dead zone**.
- **D1** re-wraps recalled `overseer-obs:` fragments at an unfiltered write boundary →
  the signature mutates and self-nests → G1 dedup is defeated → the loop self-feeds.

`resource:engineer_spawn` and the PR-ID roster (`#3063…#4162`) are **NOISE / benign
membership drift** (spawn neutralized to no-action pre-dispatch, PR #3611). `spawn=false`
is the lead; do not chase individual PRs.

---

## Landing order (dependency-safe) & traps

1. **D2** gate+counter atomically (`wiring.rs:612-627` add `Reported`).
2. **D3** closing rung at Lane-A recurrence 2, keyed on `GapItem.signature`.
3. **D1** write-boundary self-provenance filter (`mod.rs:534-546`).
4. Convergence gauges + re-run the H0–H8 matrix as a regression gate.

**TRAP (do not repeat):** the committed §6.2b remedy — replacing `store_fact` with a
literal `store_fact_with_caller_key(root_cause_signature)` — collapses recall to 1
forever, making the recurrence≥3 rung DEAD CODE. Use a **count-in-content upsert**
(incremented `occurrence_count` + first/last_seen), not caller-key overwrite.

## Open questions (for synthesis)
- D2 empirical: does a live daemon restart clearing the ephemeral `write_back_gate`
  re-open the 900s window and re-persist the same `[sig:…]` episode (the mechanism that
  pins `occurrences == 2`)? (H1 mechanism — supported by design read, not yet runtime-traced.)
- Should the D2 `Reported`-records-occurrence change be scoped ONLY to blocked-goal
  Reports, to avoid inflating recurrence for benign deliberate-block Reports?
