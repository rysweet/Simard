# Secondary Investigation — Two Non-Closing Loops (VALIDATED at HEAD 973c294b)

**Role:** Secondary investigator (patterns focus).
**Focus:** blocked-goal parking (Loop A: `root_cause.rs`, `ooda_loop/cycle.rs`)
and workstream-gap notify-only (Loop B: the `WorkstreamCoverage` Act arm).
**HEAD grounding:** `973c294b82f3d6053d8ab3abae102bd4f970e5c8`.
**Prior grounding:** `f455c06d` (secondary_two_loops_and_drift_HEAD_f455c06d.md).

**Verdict (one line):** Zero `.rs` drift since the prior secondary grounding;
every load-bearing citation re-verifies EXACTLY at current HEAD; both loops are
confirmed non-closing. **This is a control-loop convergence defect, not a
counting/dedup bug.** Extend the prior investigation — do not restart.

---

## 0. Drift verification (priority-4 deliverable)

- `git diff --name-only f455c06d..HEAD -- 'src/**/*.rs'` → **EMPTY** (production
  AND test `.rs` byte-identical). No logic changed between the prior secondary
  grounding and current HEAD.
- The only `.rs` drift in the whole prior investigation window (`6e3113bc..HEAD`)
  is `src/overseer/tests_root_cause.rs` (+test only, commit `f9cefec1`), which
  merely ENCODES the prior lane-isolation finding as a regression test
  (`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`).
- **Regression baseline (run at HEAD 973c294b):** `tests_root_cause`,
  `tests_gap_scan`, `tests_goal_health`, `ooda_loop::tests_no_progress` →
  **78 passed, 0 failed**. Advisory landing order below is anchored to these.

---

## 1. Independently re-verified citations (read directly at HEAD 973c294b)

| Claim | Location | Status |
|---|---|---|
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B escalation floor) | `overseer/root_cause.rs:33` | ✅ exact |
| `root_cause_signature = "{dedup_key}::{label}"` (per-problem Lane-B key) | `overseer/root_cause.rs:53-55` | ✅ exact |
| `WorkstreamCoverage` → `FlagWorkstreamGaps` (no launch/issue edge) | `overseer/mod.rs:1534-1543` | ✅ exact |
| `act_flag_workstream_gaps` = peek/dedup + ONE notify + commit; never launches/files | `overseer/mod.rs:884-948` | ✅ exact |
| Header: "Routine observations never create GitHub issues or stewardship backlog items" | `overseer/mod.rs:881-883` | ✅ exact |
| `decide_blocked_goal` rung ladder | `overseer/mod.rs:1603-1631` | ✅ exact |
| Escalate only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `overseer/mod.rs:1613` | ✅ exact |
| `act_escalate_blocked_goal` = dedup'd operator NOTIFICATION (not block removal) | `overseer/mod.rs:810-836` | ✅ exact |
| WHY double-gate (outer kill-switch + inner breaker threshold) | `ooda_loop/cycle.rs:582-702` | ✅ exact |
| Outer gate `no_progress_investigation_enabled()` | `ooda_loop/cycle.rs:583` | ✅ exact |
| Inner gate `INVESTIGATED_BREAKER_THRESHOLD` | `ooda_loop/cycle.rs:607,635` | ✅ exact |
| Kill-switch fallback to base verify-once ladder (never analyzes WHY) | `ooda_loop/cycle.rs:684-698` | ✅ exact |

**No stale citations.** Prior secondary artifact stands verbatim at HEAD.

---

## 2. Scope correction — `stewardship/routing.rs` is NOT the gap loop

The strategy names `stewardship/routing.rs` for the workstream-gap loop. Read in
full (52 lines): it is a **total source-module → target-repo router**
(`route_failure`, `DEFAULT_TARGET_REPO = Simard`). It never errors, never drops a
source, and is unrelated to gap emission/notification. **The real notify-only gap
loop lives entirely in `overseer/mod.rs`** (`WorkstreamCoverage` arm 1534-1543 →
`act_flag_workstream_gaps` 884-948). Record this so the verification phase does
not chase `routing.rs`.

---

## 3. Loop A — blocked-goal parking (dead zone confirmed)

`decide_blocked_goal` (`mod.rs:1603-1631`) ladder, top to bottom:

1. `recurrence >= 3` → `EscalateBlockedGoal` — a **dedup'd operator notification**
   (`act_escalate_blocked_goal`, `mod.rs:810-836`), NOT a block-removing action.
2. `perpetual && is_no_progress_marker(reason)` → `UnblockGoal` — the **only
   closing rung**, and it fires only for a perpetual false-park.
3. `needs_review` → `EscalateBlockedGoal` (notify again).
4. else → `Report` (terminal dead end).

**Dead zone (confirmed):** a genuine blocked goal at recurrence 1–2 that is
neither a perpetual false-park nor `needs_review` lands on rung 4 `Report` — no
remediation, no escalation. Every `goal:blocked:<slug>-<hash>` token in the
signature (kgpacks #12/#17/#18/#23/#25, simard-identity personas, coverage-to-70,
coin harness) sits here. The `>=3` rung is itself non-closing (notification only),
so even escalation never removes the block.

**Why the WHY-reasoner does not rescue it:** the `cycle.rs:582-702` breaker that
classifies WHY (AlreadyComplete / MissingPrecondition / UpstreamDependency /
UnclearCriteria / GoalUncovered) is **double-gated**: outer kill-switch
`no_progress_investigation_enabled()` (583) and inner `INVESTIGATED_BREAKER_
THRESHOLD` verified-no-progress floor (607/635). A goal observed 2× has not
cleared the inner floor; with the outer switch off it degrades to a verify-once
ladder (684-698) that never analyzes WHY. **Loop A is unclosed.**

---

## 4. Loop B — workstream-gap notify-only (no convergence edge)

`WorkstreamCoverage` (`mod.rs:1534-1543`) is the **only** "work exists / work
uncovered" `ProblemKind` that routes to neither `LaunchRecipe` nor `FileIssue`.
Its Act handler `act_flag_workstream_gaps` (`mod.rs:884-948`) does exactly three
things: peek+dedup each gap against `gap_gate` (900-908), send ONE consolidated
operator notification for the fresh gaps (929-930), commit each fresh gap to the
gate (931-934). No edge into `launch.rs` / `caps.recipes.launch`, no `FileIssue`.
The header comment (881-883) makes the choice explicit.

**Contrast that proves the hole:** sibling arms DeliveryReady→VerifyAndMergePr,
QualityRegression→FileIssue, ProcessHealth/CrossCutting/StepFailure→LaunchRecipe
all converge. Only `WorkstreamCoverage` (and global `ResourcePressure→Escalate`)
are notify-only. The convergence machinery already exists and is exercised by 4
sibling arms — a fix REUSES it. **Loop B is unclosed.**

Durability sub-note: `gap_gate` is an in-memory per-process `WhisperGate`; a
daemon restart clears it so a gap re-notifies regardless of the window. This
affects notification VOLUME, not closure — even a durable gate leaves the gap
terminal because nothing closes it.

---

## 5. Pattern verdict — the two loops are ONE problem

An under-resourced standing goal **oscillates**: while active it emits
`WorkstreamGap` → Loop B (notify-only); once idle/parked it emits `GoalBlocked` →
Loop A (report/dead-zone). Neither arm removes the underlying condition, so the
same episode re-observes indefinitely, alternating tokens — exactly the
interleaved `goal:blocked:<slug>-<hash>` runs and `workstream-gap|workstream-gap`
runs nested under `overseer-obs:` in the reported signature. **Treat as ONE
resourcing/convergence defect, not two counting bugs.** Anti-patterns present:

- **Observe-and-flag without a closing action** (Loop B, and Loop A rungs 1/3/4).
- **Recurrence dead zone** (signal > noise but < escalation(3) → neither
  remediation nor escalation).
- **Classify-then-route not wired** (WHY reasoner double-gated off → bare parks).

The "2×" is an HONEST re-observation count (primary's domain); the defect is the
missing convergence rung, not the counter. `engineer_spawn` is benign membership
drift — note, do not deep-dive.

---

## 6. Advisory remediation (landing-order-safe, no code changed here)

Reuses existing convergence machinery; do NOT redesign the OODA loop.

1. **D2 — close Loop A dead zone.** Insert a rung between `Report` and the `>=3`
   escalate in `decide_blocked_goal` (`mod.rs:1613-1630`) that, at first *proven*
   recurrence (2×, no benign explanation), routes to a launched/filed unit of
   work. Gate + counter MUST ship together. Use a **count-in-content upsert**
   (`occurrence_count` + first/last_seen; escalation reads the field), NOT
   `store_fact_with_caller_key` — `DedupMode::CallerKey` collapses recall to 1
   forever and makes escalation dead code (per RECONCILIATION_LEDGER §2).
2. **D3 — close Loop B.** Add a `LaunchRecipe`/`FileIssue` edge to the
   `WorkstreamCoverage` arm (`mod.rs:1534-1543`) reusing the sibling-arm
   machinery, guarded so first-sight gaps stay on the notify path and only
   proven-recurring gaps launch/file. **Key the closing-edge ledger on
   `GapItem.signature`, NOT the bare `workstream-gap` dedup_key** (avoids the
   all-gaps-fold-into-one-issue trap; note `act_flag_workstream_gaps` already
   keys its dedup on `workstream-gap:{g.signature}` at `mod.rs:901`).
3. **D1 — cut the self-feed** (primary's domain): filter recall-derived
   `overseer-obs:` meta-problems before `observation_signature`.

**Landing order:** D2 (atomic gate+counter) → D3 (closing rung + signature-keyed
guard) → D1. Each step is guarded by a green suite (`tests_root_cause`,
`tests_gap_scan`, `tests_goal_health`, `tests_no_progress` — 78 passing at HEAD).

---

## 7. Questions for the verification phase

1. Do the new D2/D3 rungs respect the anti-issue-storm guardrail — one
   launch/issue per gap/goal **signature**, not per cycle — via a
   `gap_gate`-equivalent signature-keyed launch gate?
2. Does turning `Report` (D2) / notify (D3) into a launch edge risk regressing
   the "routine observations never file issues" invariant (`mod.rs:881-883`,
   asserted by `recurring_reblock_never_files_an_issue`)? The new edge must fire
   ONLY on proven recurrence, leaving first-sight on the notify/report path.
3. Confirm the WHY reasoner's inner floor (`INVESTIGATED_BREAKER_THRESHOLD`) and
   the new 2× remediation rung do not double-fire on the same goal in one cycle.
