# Tertiary (Architect) — D3 blocked-goal / workstream-gap coupling & notify-only `WorkstreamCoverage` routing

**Role:** Tertiary investigator (amplihack:architect). **Understanding-only — no fix landed.**
**HEAD:** `ea6ec55455732120c127b369f7b6c5b2d36fa4bb`
**Focus:** Validate D3 — is the `WorkstreamCoverage → FlagWorkstreamGaps` route (`mod.rs:1534-1543`)
a structural open loop (notify-only, no corrective/park action) that keeps re-feeding the
composite `goal:blocked:…|workstream-gap` signature?

**Method:** Validate-don't-re-derive. Every citation re-read live at HEAD. Remediation status
verified by `git diff`.

---

## 0. Verdict

**D3 CONFIRMED OPEN.** The `WorkstreamCoverage` arm is a **notify-only terminal sink with no
closing edge**. It emits exactly one operator notification (email + Signal), deduped by an
in-memory `WhisperGate`, and returns an outcome (`WorkstreamGapsFlagged`) that is **excluded from
occurrence recording**. There is **no launch edge, no FileIssue, no park/transfer** — nothing that
changes the underlying board so the gap stops being observed next tick. The gap therefore
re-observes every cycle, and its `dedup_key` (`"workstream-gap"`, a **constant token**) is folded
into `observation_signature()` alongside each still-blocked `goal:blocked:<id>`, re-feeding the
composite signature indefinitely. The `×2` recurrence is an **honest re-observation count of a
static, un-actioned problem set** — not a counting or write defect.

---

## 1. The routing chain (re-grounded live at HEAD)

| Stage | Location | Behavior |
|---|---|---|
| Detect | `sensor.rs:288-372` `detect_workstream_gaps` | Surveys board/issues/anomalies → `Vec<GapItem>` |
| Observe | `signal.rs:475-478` | Folds gaps into ONE consolidated `Signal::WorkstreamGap { gaps }` |
| Orient | `mod.rs:1368-1371` | Classifies to `ProblemKind::WorkstreamCoverage`, `dedup_key = "workstream-gap"` (constant) |
| Decide | `mod.rs:1534-1543` | `Intervention::FlagWorkstreamGaps { gaps }` — carries evidence verbatim |
| Act | `mod.rs:884-948` `act_flag_workstream_gaps` | **Notify-only**: one `OperatorNotification::workstream_gap`, deduped via `gap_gate` |
| Record | `wiring.rs:612-627` `outcome_records_occurrence` | `WorkstreamGapsFlagged` is **NOT** in the match arm → not recorded |

`gap_gate` is a `WhisperGate` (`mod.rs:201`, `WhisperGate::new(900, 200)`) — a **notification
rate-limit/dedup gate**, not a launch. Its `peek`/`commit` (`mod.rs:902,933`) only decide whether
to re-notify. It never mutates the goal board or files work.

### The missing closing edge (architectural)
The sibling `StepFailure` arm (`mod.rs:1549-1580`) returns `Intervention::LaunchRecipe` — a real
corrective workstream. **The launch edge exists in the codebase and is simply not wired to the
gap arm.** `WorkstreamCoverage` has exactly one rung (notify) where `StepFailure` has a
remediation rung. That asymmetry is the structural hole.

---

## 2. The blocked-goal ↔ workstream-gap coupling (nuance corrected)

The strategy framing ("blocked-goal ↔ workstream-gap coupling") is **half right**. At the detector
they are *deliberately decoupled*:

- `sensor.rs:299-302`: blocked goals are **explicitly skipped** by gap-scan
  (`if matches!(g.status, GoalProgress::Blocked(_)) { continue; }` — "Blocked goals flow through
  goal_health; never re-flag them here.").
- Blocked goals instead route `Signal::GoalBlocked` → `ProblemKind::GoalHygiene`
  (`mod.rs:1324-1345`), `dedup_key = "goal:blocked:<id>"` → `decide_blocked_goal`
  (`mod.rs:1603-1631`).

So the two token families in the composite signature come from **two independent, non-closing
loops**, not one coupled detector:

1. **Blocked-goal loop** — `decide_blocked_goal` first-match ladder. Rung 4 (`else`) returns
   `Intervention::Report` (`mod.rs:1630`). `ActOutcome::Reported` is **absent** from
   `outcome_records_occurrence` (`wiring.rs:612-627`). A genuinely-stuck goal that misses the
   no-progress marker (Rung 2, `perpetual && is_no_progress_marker`) and `needs_review` (Rung 3)
   is misfiled "deliberate," acknowledged, and **can never accrue toward Rung 1**
   (`recurrence >= RECURRENCE_ESCALATION_THRESHOLD`, **3**, `root_cause.rs:33`). Park → don't
   record → never escalate → re-observe → re-park.
2. **Workstream-gap loop** — notify-only, as in §1.

They are **coupled dynamically, not structurally**: an under-resourced goal oscillates between
`workstream-gap` (active/uncovered, priority ≤ `GAP_GOAL_PRIORITY_BAR`=2, `sensor.rs:249,303`) and
`goal:blocked` (parked). Neither side has a terminal state, so **both** recurring families are fed
each cycle. This is why the composite carries both token types simultaneously.

---

## 3. Why the `×2` is signal, not defect (D3 contribution)

`observation_signature()` (`mod.rs:1068-1073`) is `format!("overseer-obs:{}", sorted_deduped_keys.join("|"))`.
`"workstream-gap"` is a **constant `dedup_key`** (`mod.rs:1371`) — every tick with any uncovered
gap emits the identical token. Because Act (notify-only) never removes the gap, the *same*
constant token reappears every cycle, joined with whichever `goal:blocked:<id>` keys are still
parked. The episodic recall (`RECURRING_SIGNATURE_THRESHOLD`=2, `signal.rs:362`) then honestly
reports the write-back has been seen ≥2×. The duplication/nesting in the observed blob is the
constant-token + self-observation write-back artifact already excluded as a hashing/count bug by
the primary trace — **D3 corroborates: the loop that produces the repetition is the open Act edge,
not a storage defect.**

---

## 4. Remediation status (verified)

- `git diff --name-only d00e4c3f..HEAD` (prior tertiary baseline → HEAD): **zero non-test `.rs`
  changes.**
- `git diff --name-only e5257a33..HEAD` (referenced baseline → HEAD): **only `ai_working/
  investigation/*.md`** (docs). No production, no test `.rs`.

No self-exclusion, no idempotent-upsert gate, and **no corrective routing on the gap arm** landed.
The `WorkstreamCoverage → FlagWorkstreamGaps` notify-only route at `mod.rs:1534-1543` is byte-for-byte
the prior baseline. **D3 open.**

---

## 5. Reconciliation with prior verdicts

Consistent with `tertiary_architecture_NONCLOSING_LOOPS_DEADZONE_D0_HEAD_d00e4c3f.md` (§1 Loop 2)
and the consolidated findings: the workstream-gap ladder is "notify-without-launch (missing closing
edge)." **No drift.** One clarification recorded here: the coupling is *dynamic oscillation*
between two decoupled detectors (blocked goals are skipped by gap-scan at `sensor.rs:299-302`), not
a single coupled path — but the net structural effect (both families perpetually re-fed) is
identical to the baseline verdict.

---

## 6. The missing gate (for understanding, not to implement)

To close D3, the `WorkstreamCoverage` arm would need a **second rung with a state-changing edge**
symmetric to `StepFailure`'s `LaunchRecipe` — e.g., launch a bounded resourcing workstream or file
one tracking issue for a gap that persists beyond N notify cycles — **and** its outcome would need
to enter `outcome_records_occurrence` so recurrence can climb. Absent both, notify-only leaves the
board unchanged and the composite signature self-perpetuates.

**Success criteria met:** each D3 token classified signal-vs-defect with `file:line` evidence at
HEAD; the missing corrective-routing gate identified; remediation status verified via `git diff`
(docs-only); verdict reconciled against the baseline with the single coupling nuance recorded.
