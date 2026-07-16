# Secondary Investigation — Areas 3 & 4: OODA loop-closure gaps + escalation dead-zone

**Role:** Secondary (patterns) · **HEAD:** `3e6b6933` · **Date:** 2026-07-16
**Verdict:** VALIDATED — the prior corpus holds at live HEAD. **Zero production-source
drift** since the last validated baseline (`25d4c5a6`): `git diff --name-only
25d4c5a6..HEAD` touches only `ai_working/investigation/*.md`. Every load-bearing
`src/ooda_loop/*` and `src/overseer/*` citation was independently re-grounded and every
targeted test re-executed green. Investigation-only; no fixes applied.

---

## 0. Drift ledger

- `git diff --name-only 25d4c5a6..HEAD` → **10 files, all `ai_working/investigation/*.md`.**
  No `.rs` changed. All production citations below are live at HEAD `3e6b6933`.
- The visible `×2` is a **runtime cognitive-memory value**, not a source constant; source
  cannot advance it. Nothing in this drift window could have changed the count.

---

## Area 3 — OODA loop closure: why blocked-goal & workstream-gap never clear

Two textbook **observe-and-flag-without-a-closing-action** loops. Both are
Observe→Orient→Decide→**Act-that-does-not-mutate-the-observed-state**, so the next
Observe re-emits the identical dedup_key and the composite recurs.

### Loop A — Blocked-goal ladder (`decide_blocked_goal`, `mod.rs:1603-1631`) — RE-GROUNDED

| Condition | Intervention | Closes? | Live loc |
|---|---|---|---|
| `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (=3) | `EscalateBlockedGoal` | yes (operator) | `mod.rs:1613` |
| `perpetual && is_no_progress_marker(reason)` | `UnblockGoal` | yes (self-heal) | `mod.rs:1620` |
| `needs_review` | `EscalateBlockedGoal` | yes | `mod.rs:1623` |
| **else** | **`Report`** | **NO — no-op** | `mod.rs:1630` |

A plain dependency/operator block (e.g. `goal:blocked:fix-agent-kgpacks-rs-issue-17-…`)
that is **not** perpetual, carries **no** no-progress/`needs_review` marker, and sits at
recurrence **1→2** (below 3) falls into `else → Report`. `Report` →
`remediation_for` returns `Remediation::acknowledged()` (`mod.rs:1129`) — labelled
"deliberate block; nothing to fix" — while the goal state is **never touched**. This is
the missing sub-threshold remediation rung: **no `LaunchRecipe` / `FileIssue` arm** exists
for a plain block, and `Report` carries **no WHY** (WHY is consumed only by the
Escalate/Unblock arms).

### The OODA breaker CAN close Loop A — but is double-gated (the key ooda_loop coupling)

`src/ooda_loop/cycle.rs:582-583` invokes the root-cause breaker whose
`resolution_for_why` ladder (`goal_curation/no_progress_breaker.rs:384-417`) DOES map
every stall class to a **state-changing** resolution — RE-GROUNDED live:

- `AlreadyComplete → MarkDone`, `Obsolete → Drop`, `MissingPrecondition → Heal`,
  `UpstreamDependency → Defer`, `UnclearCriteria|GenuinelyStuck → SpawnEngineer` (first
  attempt) then `Escalate` WITH the concrete WHY (retry exhausted).

Per the named-goal cause map, kgpacks issue-17 classifies `AlreadyComplete`/
`MissingPrecondition` → would `MarkDone`/`Heal` and **close the loop**. It does not,
because the ladder is bypassed unless BOTH gates pass:

- **Gate A:** `memories.completion_evidence.is_some()` (`cycle.rs:582`). A `None` source
  silently disables the entire WHY ladder.
- **Gate B:** `no_progress_investigation_enabled()` (`cycle.rs:583`). If OFF in the daemon,
  the ladder never runs.

**Anti-pattern:** *Classify-then-route the stall, don't park it* — the routing ladder
exists and is correct, but is fail-open to bare-park behind a double gate. When bypassed,
the overseer's `decide_blocked_goal` dead-zone (above) is the only remaining arm, and it
no-ops sub-threshold. **The two subsystems' gaps compound: the capable ooda_loop rung is
gated off, and the overseer fallback rung is a no-op below recurrence 3.**

### Loop B — WorkstreamCoverage: notify-only terminal — RE-GROUNDED

- Emit → `ProblemKind::WorkstreamCoverage` (`mod.rs:1369`), dedup_key literal
  `"workstream-gap"` (`mod.rs:1371`).
- Decide → `FlagWorkstreamGaps { gaps }` (`mod.rs:1534-1543`) — carries evidence, nothing
  more.
- Act → `act_flag_workstream_gaps` (`mod.rs:884-948`): peeks the per-gap `gap_gate`
  WhisperGate, notifies the operator on both channels for fresh gaps, returns
  `WorkstreamGapsFlagged`. **No state mutation.**
- **No `WorkstreamCoverage → LaunchRecipe` edge anywhere** — confirmed:
  `grep 'WorkstreamCoverage.*Launch\|Launch.*Workstream' src/overseer/*.rs` → empty. The
  gap that says "important work has NO active workstream" is never converted into one.
- **No durable `FileIssue`** on the acting path; `remediation_for` routes
  `FlagWorkstreamGaps` through the `_ => Remediation::root_cause()` catch-all
  (`mod.rs:1130`) — a **notify-only** action mislabelled **root-cause-addressing**, which
  telemetrically *masks* the open loop.

**gap-spawn coupling:** the uncovered gap → an under-resourced goal idles → surfaces as
`resource:engineer_spawn` (Area 5, benign literal-key membership drift) and oscillates
`workstream-gap` (active) ↔ `goal:blocked` (idle). One under-throughput problem in three
views, not three bugs.

---

## Area 4 — Escalation thresholds & the recurrence dead-zone — RE-GROUNDED

| Fact | Live loc | Check |
|---|---|---|
| Lane A emit floor `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | ✅ |
| Lane A fires at `occurrences >= 2` | `signal.rs:463-464` | ✅ |
| Operator string verbatim the question | `mod.rs:1360-1362` | ✅ |
| Lane B escalation floor `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | ✅ |
| Escalation gate `recurrence >= 3` | `mod.rs:1613` | ✅ |

**Dead-zone CONFIRMED.** Emit floor `2` < escalation floor `3`. The `×2` string sits
**above one-off noise** (900s WhisperGate dedup) and **below** the escalation bar, with
**no remediation rung between them** in either the coverage loop or the park loop → the
signal is stuck at `2×` indefinitely.

**Two decoupled counter lanes (do NOT conflate):**
- **Lane A** — observation episodes (`store_episode`, +1 per ~900s window, threshold 2) =
  the **visible `×2`**.
- **Lane B** — root-cause occurrences (`store_fact` append-only, threshold 3) drives the
  `mod.rs:1613` escalation. The `×2` string says **nothing** about whether Lane B reached 3.

Proven green at HEAD: `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`,
`lane_b_escalates_without_any_lane_a_signal`.

**Minimal remediation rung (describe-only, per investigation scope):** place a
convergence/escalation action at the **first proven recurrence (2×)** for signals with no
benign explanation — i.e. collapse the dead zone. Two coordinated landing sites:
1. **Loop A:** add a WHY-classified, state-advancing arm in `decide_blocked_goal`
   (before recurrence 3) — launch/file rather than bare `Report`; and/or ensure Gates A/B
   in `cycle.rs:582-583` are satisfied so the existing `resolution_for_why` ladder runs.
2. **Loop B:** add a `WorkstreamCoverage → LaunchRecipe` edge and/or a deduped `FileIssue`
   in `act_flag_workstream_gaps` to honour the `FlagWorkstreamGaps` doc contract.

**§6.2b remedy TRAP (still valid):** a bare `store_fact_with_caller_key`
(`DedupMode::CallerKey`) would collapse `recurrence = recall.len()` to 1 forever, turning
`mod.rs:1613` into dead code. Correct fix = **count-in-content upsert** (persist
`occurrence_count`/`first_seen`/`last_seen`; escalation reads the field). Gate fix and
counter fix must land atomically.

---

## Empirical re-execution at HEAD `3e6b6933` (all GREEN)

- `recurring_signature` suite (8) — emit `>=2` floor, additive write-back, high-priority
  orient.
- `lane` suite (2) — Lane A/B decoupling.
- `no_progress_why` suite (13) — `resolution_for_why` class→resolution ladder
  (mark_done/heal/defer/spawn_engineer/escalate-after-retry).
- `workstream_gap` / `gap_scan` suite (5) — routine notify-only, batch counts,
  WorkstreamCoverage high-priority mapping.

---

## Reconciliation

Fully reconciles with `FINAL_SYNTHESIS.md`, `VALIDATION_VERDICT_HEAD_25d4c5a6.md`,
`secondary_areas3-5_recurrence_deadzone_and_spawn_drift_VALIDATE_HEAD_25d4c5a6.md`, and
`tertiary_architecture_NONCLOSING_LOOPS_AND_MISSING_EDGES_HEAD_25d4c5a6.md`. No divergence.
Only delta since baseline is docs. D1 (Loop A dead-zone/no-op park), D2 (recurrence
dead-zone 2-vs-3), and D3 (Loop B notify-only + remediation masking) remain **OPEN**.

## Questions for the verification phase

1. **Gate B in prod:** is `no_progress_investigation_enabled()` ON in the running daemon?
   If OFF, the `resolution_for_why` ladder never runs and Loop A can ONLY close at
   overseer recurrence 3 (dead-zone) — no counter fix helps.
2. **Gate A population:** is `memories.completion_evidence` `Some` on the ticks these goals
   park? A `None` source silently disables the whole WHY ladder.
3. **Atomic landing:** any D2 fix must land the escalation-gate read AND a count-in-content
   upsert together, or it re-introduces the §6.2b dead-code trap.
