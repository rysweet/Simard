# Secondary Investigation — Two Non-Closing Observe-and-Flag Loops + engineer_spawn Drift

**Role:** Secondary investigator (patterns focus)
**HEAD grounding:** `ad5e10606e18b162ef0f0d71edad8e38ecdf5b5f`
**Focus:** Characterize the goal-blocked WHY-gating arm and the workstream-gap
launch arm, classify `resource:engineer_spawn`, and identify the missing
resolution edge that causes problem-set persistence.
**Verdict:** Re-validated against current HEAD. Production `.rs` has NOT drifted
from prior grounding; every cited line matches. Both loops confirmed
non-closing. `engineer_spawn` confirmed benign drift.

---

## 1. The decisive finding: the `decide()` routing asymmetry

`decide()` (`src/overseer/mod.rs:1400-1580`) is the single Orient→Decide table.
Classifying every arm by whether it emits a **closing action** (one that removes
the condition) vs. an **observe-and-flag** action (notify/report only) exposes
the root pattern cleanly:

| ProblemKind | Intervention | Closing edge? | Citation |
|---|---|---|---|
| DeliveryReady | `VerifyAndMergePr` | ✅ merges the PR | mod.rs:1402-1412 |
| QualityRegression (CI) | `FileIssue` | ✅ files remediation issue | mod.rs:1413-1428 |
| ProcessHealth | `LaunchRecipe` | ✅ launches fix | mod.rs:1429-1435 |
| CrossCutting | `LaunchRecipe` | ✅ launches sweep | mod.rs:1436-1443 |
| StepFailure | `LaunchRecipe` | ✅ corrective workstream | mod.rs:1549-1575 |
| **WorkstreamCoverage** | **`FlagWorkstreamGaps`** | ❌ **notify only** | **mod.rs:1534-1544** |
| **ResourcePressure** | **`Escalate`** | ❌ **notify only** | **mod.rs:1444-1446** |
| DriftCorrection | `Whisper` | ⚠️ advisory only | mod.rs:1528-1531 |
| GoalHygiene→blocked | `decide_blocked_goal(...)` | ⚠️ conditional (see §2) | mod.rs:1447-1483 |

**Anti-pattern (PATTERNS.md "observe-and-flag without a closing action"):**
`WorkstreamCoverage` is the **only "work exists / work uncovered" problem kind
that does not route to `LaunchRecipe` or `FileIssue`.** Every sibling that
represents actionable work converges — it launches a recipe or files an issue
that (on success) removes the condition. WorkstreamCoverage merely notifies the
operator, so the gap survives every cycle. **This is the missing resolution
edge.** The convergence machinery (`Intervention::LaunchRecipe` →
`caps.recipes.launch`, mod.rs:632-633) already exists and is exercised by 4
other arms — the fix reuses it; it does not invent a mechanism.

---

## 2. Loop A — goal:blocked WHY-gating arm (the "idle" half)

**Path:** `GoalHygiene` w/ `Signal::GoalBlocked` → `decide_blocked_goal`
(mod.rs:1452-1483 → 1603-1631).

The WHY reasoner **is** wired: `root_cause::analyze` runs unconditionally for
every problem in the live cycle (mod.rs:455-458), and its recurrence/why is
threaded into `decide_blocked_goal` (mod.rs:1469-1482). So the earlier concern
"WHY reasoner unwired → permanent bare parks" does NOT hold at this HEAD — WHY
is always produced and honestly labelled (degrades to telemetry-only, never
silent).

The rung ladder (mod.rs:1613-1630):
1. `recurrence >= RECURRENCE_ESCALATION_THRESHOLD (=3)` → `EscalateBlockedGoal`
   (root_cause.rs:33). **Notify-only** — hands the root cause to the operator,
   does not remove the block.
2. `perpetual && is_no_progress_marker` → `UnblockGoal` (self-heal) — the ONLY
   truly closing rung, and it fires only for a false-parked perpetual goal.
3. `needs_review` → `EscalateBlockedGoal` — notify-only.
4. else → `Report` — surfaced in the periodic report, **left untouched**.

**Recurrence dead zone (confirmed):** a blocked goal at recurrence 1–2 that is
NOT a perpetual no-progress false-park and NOT `needs_review` falls to rung 4
(`Report`). It receives neither remediation nor escalation. The "2×" tokens in
the signature (kgpacks #12/#17/#18/#23/#25, simard-identity personas,
coverage-to-70, coin harness) sit exactly in this zone: observed, WHY-analyzed,
then parked. No convergence rung between "silently report" and "escalate at 3".

**Even the ≥3 escalation is non-closing** — `EscalateBlockedGoal` is a
notification (mod.rs:814-834, `OperatorNotification::goal_blocked_with_why`),
not an action that removes the block. So the idle arm has exactly one closing
rung (self-heal), reachable only by the narrow perpetual-no-progress case.

---

## 3. Loop B — workstream-gap launch arm (the "active" half)

**Path:** `WorkstreamCoverage` w/ `Signal::WorkstreamGap` →
`FlagWorkstreamGaps` → `act_flag_workstream_gaps` (mod.rs:1534-1543 → 671 →
884-948).

`act_flag_workstream_gaps` does exactly three things: peek/dedup each gap
against `gap_gate` (mod.rs:900-908), send ONE consolidated operator
notification for the fresh gaps (mod.rs:929-930), and `commit` each fresh gap to
the gate (mod.rs:931-934). **It never launches a recipe, files an issue, or
creates a stewardship backlog item.** The header comment is explicit:
"Routine observations never create GitHub issues or stewardship backlog items"
(mod.rs:882-883).

**Consequence:** notify → suppress-within-window → (after 900 s) notify again →
forever. The gap condition is never acted on, so it re-detects every scan. The
detector `detect_workstream_gaps` is pure/hermetic (sensor.rs), so nothing else
removes the gap. **This is the second missing resolution edge** and the direct
cause of the `workstream-gap|workstream-gap` runs in the signature.

**Lane-B durability sub-concern:** `gap_gate = WhisperGate::new(900, 200)` is an
**in-memory** per-process gate (mod.rs:201, 304). A daemon restart clears it, so
a gap re-notifies immediately on restart regardless of the 900 s window. This is
a real (but secondary) recording concern — the dedup is honest within a process
lifetime but not durable across restarts. It affects notification volume, not
the core persistence: even with a perfect durable gate, the gap would persist
because nothing closes it.

---

## 4. The two loops are ONE problem (oscillation)

An under-resourced standing goal oscillates between the two arms:
- **active** → emits `WorkstreamGap` → Loop B (notify-only)
- **idle/parked** → emits `GoalBlocked` → Loop A (report/dead-zone)

Neither arm removes the underlying condition (the goal is under-resourced /
uncovered), so the same episode re-observes indefinitely, alternating tokens.
This explains the composite signature's structure:
`goal:blocked:<slug>-<hash>` runs interleaved with `workstream-gap|
workstream-gap` runs, prefixed by `overseer-obs:` (the self-observation
write-back nesting prior observations into the next window — Lane-A, proven not
to feed Lane-B recurrence).

**Missing edge (single sentence):** there is no remediation rung that converts
an observed-and-flagged coverage gap / blocked standing goal into a launched or
filed unit of work at first *proven* recurrence — the loop stays in
observe-and-flag and never reaches the existing `LaunchRecipe`/`FileIssue`
convergence machinery.

---

## 5. `resource:engineer_spawn` — classification

- Token minted from `Signal::EngineerSpawnRate { live }` →
  `"resource:engineer_spawn"` (mod.rs:1267-1272), signature-mapped in
  capabilities.rs:562.
- ProblemKind = `ResourcePressure` → `Intervention::Escalate` (mod.rs:1444-1446):
  a global budget/spawn-pressure escalation, notify-only.

**Verdict: benign membership drift, NOT a contradicting signal.** It is a
*global* resource-pressure observation ("elevated engineer spawn, N live") that
enters/leaves the active problem set as the live-engineer count crosses its
threshold (fires at-and-above threshold only — tests_m1.rs:133). Its appearance
alongside `workstream-gap` looks contradictory (uncovered work AND too many
engineers live) but the two operate at **different seams**: workstream-gap is a
*per-goal coverage* observation; engineer_spawn is a *global admission/spawn-rate
cap*. There is **no causal edge** between them (no code path couples
`EngineerSpawnRate` to `WorkstreamGap`; grep of `engineer_spawn`/`resource:`
shows only the independent signal→token→escalate chain). The overlap is a
legitimate resource-allocation tension, not a defect. Do not build a theory
coupling them.

---

## 6. Design rationale observed

The notify-only choice for gaps/resource-pressure is a deliberate guardrail:
"Routine observations never create GitHub issues or stewardship backlog items"
(mod.rs:882-883) — an anti-spam / anti-issue-storm stance consistent with the
recurring-re-park escalation guard (root_cause.rs:28-33) that refuses to
"unblock every cycle." The rationale is sound (avoid runaway issue creation) but
it over-corrected into having **no** remediation rung for the coverage-gap and
low-recurrence-blocked cases — leaving a gap between "observe silently" and
"escalate the root cause to a human at 3×."

---

## 7. Integration points

- **Existing convergence machinery to reuse:** `Intervention::LaunchRecipe` →
  `caps.recipes.launch(brief)` (mod.rs:632-633, `RecipeLauncher` mod.rs:143) and
  `Intervention::FileIssue` (mod.rs:1416-1424). Both already carry `RecipeBrief`
  / `OrchestratorRunBrief` builders.
- **Recurrence signal already available at the decision point:**
  `problem.why.recurrence` (mod.rs:1469) — a rung keyed at first *proven*
  recurrence (2×) for no-benign-explanation gaps would slot in without new
  plumbing.
- **Gate durability:** if Lane-B durability is pursued, it lives at the
  `gap_gate` seam (mod.rs:201, 304, 900-934) — a signature-keyed idempotent
  upsert with bounded retention, not a counter change.

---

## 8. Concerns / questions for the verification phase

1. **Minimal-fix landing point (for architect/verify):** add a converging rung
   at `decide()` for `WorkstreamCoverage` — route gaps that recur ≥2× (proven,
   no benign explanation) to `LaunchRecipe`/`FileIssue` instead of notify-only,
   keeping first-sight gaps on the existing notify path. Landing point:
   mod.rs:1534-1543 (WorkstreamCoverage arm) reusing mod.rs:1429-1435 machinery.
   Symmetrically, add a rung between `Report` and `EscalateBlockedGoal(≥3)` in
   `decide_blocked_goal` (mod.rs:1613-1630) for the 2× dead zone.
2. **Do NOT change the counter / dedup key** — the "2×" is honest re-observation
   (primary's domain; consistent with prior VERDICT). Fix the closing action.
3. **Do NOT couple engineer_spawn to workstream-gap** — confirmed independent.
4. **Verify** the new rung respects the anti-issue-storm guardrail (dedup so one
   launch/issue per gap signature, not one per cycle) — the `gap_gate` or an
   equivalent signature-keyed launch gate should guard the new closing edge.
5. **Scope guard:** minimal safe fix only — do NOT redesign the OODA loop.

---

## 9. Evidence index (file:line)

- Routing table / asymmetry: `src/overseer/mod.rs:1400-1580`
- Gap arm (notify-only): `src/overseer/mod.rs:884-948`, decide route `1534-1543`
- Blocked-goal ladder + dead zone: `src/overseer/mod.rs:1447-1483, 1603-1631`
- Escalate blocked (notify): `src/overseer/mod.rs:814-834`
- WHY reasoner wired: `src/overseer/mod.rs:455-458, 1469-1474`
- Recurrence threshold: `src/overseer/root_cause.rs:28-33`
- engineer_spawn token: `src/overseer/mod.rs:1267-1272`; sig map
  `src/overseer/capabilities.rs:562`; ResourcePressure→Escalate `mod.rs:1444-1446`
- Gap gate (in-memory, non-durable): `src/overseer/mod.rs:201, 304, 900-934`
- WorkstreamGap signal/classification: `src/overseer/signal.rs:78-79, 476, 648-660`
