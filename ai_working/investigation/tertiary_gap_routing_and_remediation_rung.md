# Tertiary Investigation (deep dive) — Workstream-gap detection semantics, routing origin, and a recurrence-aware gap-remediation rung

**Role:** Tertiary investigator (architect). **Date:** 2026-07-15.
**HEAD:** `dea65df8` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Focus:** Workstream-gap detection semantics + routing origin; design a
recurrence-aware gap-remediation rung *below* the escalation threshold.
**Relationship to prior artifacts:** VALIDATES and EXTENDS
[`tertiary_architecture_design.md`](./tertiary_architecture_design.md) and
[`CONSOLIDATED_FINDINGS.md`](./CONSOLIDATED_FINDINGS.md). It does not restate
them. Three new architectural findings (§1–§3) sharpen *where* the recurrence
key must live and expose a dual-path quarantine the prior design under-stated.

---

## 0. Re-validation at HEAD (all prior claims hold)

Re-grounded to current source; every prior tertiary claim is confirmed:

| Claim | Source (re-verified) | Verdict |
|---|---|---|
| Composite signature = `overseer-obs:{sorted∪dedup(dedup_key)joined-by-\|}` | `mod.rs:1068-1072` | ✅ |
| Blocked-goal problem `dedup_key` = `goal:blocked:{goal_id}` | `mod.rs:1336` | ✅ |
| Coverage problem `dedup_key` = bare `"workstream-gap"` | `mod.rs:1371` | ✅ |
| Acting Decide arm `WorkstreamCoverage → FlagWorkstreamGaps` (notify-only) | `mod.rs:1534-1543` | ✅ |
| Act path notifies + commits `gap_gate` only; no launcher | `mod.rs:884-948` | ✅ |
| gap_gate = `WhisperGate::new(900, 200)` (15-min window, intra-window only) | `mod.rs:304` | ✅ |
| Per-gap identity is `GapItem.signature` (`goal:<id>`/`issue:<repo>#<n>`/`anomaly:<slug>`) | `signal.rs:135-138`; built `sensor.rs:306,335,357` | ✅ |
| `RecipeLauncher`/`RecipeBrief`/`smart_orchestrator_args` seam exists, bounded | `launch.rs:47-59,103-132` | ✅ |
| Escalation threshold = 3, blocked goals only | `root_cause.rs:33`; `mod.rs:1613` | ✅ |
| No remediation landed since design doc (last 2 commits are the docs) | `git log` | ✅ |

**Verdict:** the `×2` is a genuine re-observation loop of an unchanged problem
set, not a storage/replay artifact. The system is in the design phase; the
remediation rung is unbuilt.

---

## 1. NEW — Coverage is quarantined from *every* closing seam (dual-path, not one)

The prior design named the acting Decide arm as the single notify-only sink.
Tracing the **read-only observer** path shows the quarantine is **architecturally
doubled** — `WorkstreamCoverage` is the *only* High-priority `ProblemKind`
excluded from BOTH closing mechanisms the codebase already owns:

| Path | Closing mechanisms available | Where `WorkstreamCoverage` lands |
|---|---|---|
| **M1 read-only observer** (`observer.rs:98-121`) | 5 kinds → `FileIssue` (ProcessHealth, QualityRegression, GoalHygiene, StepFailure, CrossCutting; `observer.rs:105`) | `Intervention::Report` (`observer.rs:120`) — never files |
| **M2 acting overseer** (`mod.rs:1400-1582`) | 4 kinds → `LaunchRecipe`/`VerifyAndMergePr` (`mod.rs:1405,1429,1436,1565`) | `FlagWorkstreamGaps` (`mod.rs:1543`) — notify-only |

So a coverage gap can reach **neither** a filed tracking issue **nor** a launched
workstream, on **either** operating mode. Every *other* High-priority finding
converges through at least one of these. This is the precise *routing origin* of
the recurrence: not a bug in gap detection (detection is correct and dedup-clean),
but a **Decide-table routing hole** — the gap is detected accurately, then routed
to a terminal, non-converging action on both paths. The observer comment even
calls the path "unreachable in M1" (`observer.rs:118-119`), which is only true
today because M1 does not survey gaps; the moment gap survey is added to M1, the
gap silently degrades to `Report`.

---

## 2. NEW — The bare `workstream-gap` dedup_key destroys per-gap identity, blocking any recurrence ledger keyed on it

`goal:blocked:{goal_id}` (`mod.rs:1336`) carries goal identity into the
observation signature; the coverage problem's dedup_key is the **fixed constant**
`"workstream-gap"` (`mod.rs:1371`). Architectural consequences:

1. **Signature opacity.** `observation_signature` cannot distinguish "2 gaps"
   from "20 different gaps" — every coverage problem contributes the same opaque
   token. The repeated `workstream-gap|workstream-gap` fragments in the recall are
   distinct *problems/episodes* all collapsing to one label. Per-gap recurrence
   is therefore **invisible at the observation layer**.
2. **Issue-fold hazard.** `problem_to_run_brief` sets
   `failure_kind = problem.dedup_key` (`observer.rs:133`), and stewardship dedups
   on `failure_signature(failure_kind, error_text)`. If one naively wired the
   observer's `FileIssue` arm for `WorkstreamCoverage` (the obvious "fix"),
   **every distinct gap would fold into ONE issue** — under-reporting, not
   remediation. This is a trap the remediation rung must avoid.
3. **Correct key already exists one level down.** `GapItem.signature`
   (`signal.rs:135-138`) is the stable per-gap identity (`goal:<id>` /
   `issue:<repo>#<n>` / `anomaly:<slug>`), and `act_flag_workstream_gaps` already
   keys `gap_gate` on `format!("workstream-gap:{}", g.signature)` (`mod.rs:901`).

> **Design constraint (INV-GAP-KEY):** the recurrence ledger, the launch dedup
> key, and any per-gap issue MUST key on `GapItem.signature`, **never** on
> `problem.dedup_key`. The consolidated `WorkstreamGap` signal must be
> **fanned back out** to per-gap identities at the remediation seam — the inverse
> of the consolidation done at emission (`signal.rs:475-478`).

This refines the prior design's §2.1 ("record each fresh gap signature as a
`PriorOccurrence`"): the correct source of that signature is `GapItem.signature`,
and the reason it *must* be that field (not the dedup_key) is finding §2 above.

---

## 3. NEW — The routing seam is built and dangling, waiting for a coverage brief

`stewardship::route_failure` (`routing.rs:39-52`) is **total** and its doc
comment (`routing.rs:11-15`) *explicitly anticipates* "the Overseer's `overseer`
workstream-gap briefs" landing in a real repo via `DEFAULT_TARGET_REPO`
(`rysweet/Simard`). Yet **no code path currently produces a coverage brief into
stewardship** — the acting arm emits `FlagWorkstreamGaps`, the observer emits
`Report`. The routing target the remediation rung needs is therefore already
implemented and reachable; the gap-remediation rung only has to *produce the
brief* and hand it to the existing `RecipeLauncher` (in-loop launch) or the
existing `StewardshipIssueFiler` (out-of-loop file). No new routing subsystem is
required — the seam is present and dangling.

---

## 4. Recurrence-aware gap-remediation rung (design, below the escalation threshold)

Design goal unchanged from prior §2 — make the persistent gap signal
**converge** — but sharpened by §1–§3. The rung lives **below**
`RECURRENCE_ESCALATION_THRESHOLD = 3` (blocked-goal escalation) and fills the
2× dead zone for gaps specifically.

### 4.1 Durable per-gap recurrence ledger (the missing memory)

Today gaps have no cross-window memory (§0, `gap_gate` is intra-window only).
Add a cognitive-memory `PriorOccurrence` keyed on **`GapItem.signature`**
(per INV-GAP-KEY), recorded at the existing commit site (`mod.rs:931-934`) and
recalled on each Act pass — reusing the `recall_occurrences`/`PriorOccurrence`
primitive that already backs root-cause recurrence (`root_cause.rs:35+`;
`mod.rs::recall_occurrences`). `gap_gate` stays the intra-window flood guard;
cognitive memory becomes the cross-window ledger. **No new subsystem.**

### 4.2 The three-rung ladder (recurrence partitions the consolidated signal)

Rewrite the acting arm (`mod.rs:1534-1543`) to **fan the consolidated
`Vec<GapItem>` back out** and partition by recalled per-gap recurrence:

| Per-gap recurrence | Rung | Action | Seam |
|:---:|---|---|---|
| **1× (first sight)** | **Notify** | keep `FlagWorkstreamGaps` (may self-resolve; don't thrash) | `act_flag_workstream_gaps` (`mod.rs:884`) unchanged |
| **2× (proven recurring — the dead zone)** | **Remediate** | build `RecipeBrief{task_description = gap.why_it_matters/title, target_repo = route_failure("simard::overseer")}` and `LaunchRecipe`, honoring `max_launches_per_cycle` + board dedup (`goal_has_active_workstream`) | `RecipeLauncher` (`launch.rs:124-132`), same path `ProcessHealth` uses (`mod.rs:1429`) |
| **≥3× or launch-unsafe** | **Escalate** | ONE operator escalation carrying the per-gap history (mirrors `EscalateBlockedGoal`) | existing operator escalation |

- **Gap threshold = 2**, deliberately *below* the blocked-goal 3: a coverage gap
  that recurs is definitionally under-resourced (no benign "transient blip"
  explanation), whereas a blocked-goal cause can recur benignly — justifying the
  higher bar there. This asymmetry is the whole point of a *rung below the
  escalation threshold*.
- **`GoalUncovered` gaps are the high-value auto-launch rung** — a `goal:<id>`
  gap maps 1:1 to a `smart-orchestrator` task. `IssueUncovered` /
  `AnomalyUnaddressed` use the same brief shape (open a PR against the
  issue/anomaly); where no safe brief can be synthesised, skip Remediate and go
  straight to Escalate at ≥3×.

### 4.3 Wiring seam (edges attach to existing code)

- **Decide** (`mod.rs:1534`): emit `Vec<Intervention>` (or a new
  `RemediateWorkstreamGaps` carrying notify/launch/escalate buckets) instead of a
  single `FlagWorkstreamGaps`.
- **Act** (`mod.rs:884`): keep `act_flag_workstream_gaps` for the notify bucket;
  add `act_remediate_workstream_gaps` driving the launch bucket through the
  existing `RecipeLauncher` (`ActOutcome::Launched` path) and the escalate bucket
  through existing operator escalation.
- **Guardrails** (`guardrails.rs`): `FlagWorkstreamGaps` is `RiskClass::Routine`;
  a *launch* is not — classify the remediation intervention at the same risk tier
  `LaunchRecipe` already carries so the AutonomyGate/budget gate governs it (no
  new bypass).
- **Telemetry** (`activity.rs:66-68`; `wiring.rs`): add
  `workstream_gaps_remediated` / `_escalated` beside the existing
  `_detected`/`_suppressed` counters, plus a **persistent-unremediated gauge**
  (per-gap recurrence ≥2 that produced no launch/escalation this window) that
  must trend to zero — the operator's proof the rung works, not just ships.

---

## 5. Component-boundary summary (routing origin, one picture)

```
Observe  detect_workstream_gaps (sensor.rs:288)  ──► Vec<GapItem>{signature=goal:/issue:/anomaly:}
            │  (detection is correct + dedup-clean; NOT the defect)
            ▼
Signal   Signal::WorkstreamGap{gaps}  (signal.rs:476)   ── consolidates N gaps into ONE signal
            ▼
Orient   ProblemKind::WorkstreamCoverage, dedup_key="workstream-gap"  (mod.rs:1371)
            │  ◄── DEFECT A (§2): bare dedup_key discards per-gap identity
            ▼
Decide   ┌ M1 observer  → Intervention::Report        (observer.rs:120)  ── never files
         └ M2 acting    → FlagWorkstreamGaps           (mod.rs:1543)      ── notify-only
            │  ◄── DEFECT B (§1): only High-priority kind with NO edge to FileIssue OR LaunchRecipe
            ▼
Act      notify + gap_gate.commit (intra-window only)  (mod.rs:929-933)
            │  ◄── DEFECT C (§0): no cross-window memory → recurrence never accrues → 2× dead zone
            ▼
         re-Observe the unchanged world  ──►  overseer-obs:…|workstream-gap|…  recorded ×2
```

Dangling-but-ready seams the rung reuses: `RecipeLauncher`/`RecipeBrief`
(`launch.rs`), total `route_failure` anticipating coverage briefs
(`routing.rs:11-15`), `PriorOccurrence`/recall (`root_cause.rs`), `GapItem.signature`
per-gap key (`signal.rs:135`).

---

## 6. Reconciliation & deltas vs. prior artifacts

- **Confirms** the prior tertiary 3-rung ladder, cognitive-memory recurrence
  count, gap-threshold=2, and launch.rs reuse — all still valid at HEAD.
- **Extends** with three findings the prior docs did not isolate:
  1. §1 the quarantine is **dual-path** (observer `Report` + acting `FlagWorkstreamGaps`), making `WorkstreamCoverage` the sole High-priority kind cut off from *both* `FileIssue` and `LaunchRecipe`.
  2. §2 the bare `"workstream-gap"` dedup_key **destroys per-gap identity** and would fold all gaps into one issue if naively filed → the recurrence ledger MUST key on `GapItem.signature` (INV-GAP-KEY), and the consolidated signal must be **fanned back out** at the remediation seam.
  3. §3 `route_failure` is **built and dangling**, already anticipating coverage briefs — no new routing subsystem needed.
- **No contradictions** with `CONSOLIDATED_FINDINGS.md`, `investigation_report.md`,
  or the secondary dedup/write-back findings.

**Reconciled root-cause statement (tertiary/architecture):** the recurring
`overseer-obs:…|workstream-gap` signature is produced by a **Decide-table routing
hole**, not a detection defect. `WorkstreamCoverage` is routed to a terminal
notify/report on both operating modes, has **no cross-window recurrence memory**
(only a 15-min gap_gate), and carries a **bare, identity-less dedup_key** that
hides per-gap recurrence. The fix is a recurrence-aware remediation rung that
(a) keys a durable per-gap ledger on `GapItem.signature`, (b) at 2× recurrence
fans the consolidated signal back out and launches a bounded workstream through
the **already-present** `RecipeLauncher`/`route_failure` seams, and (c) escalates
once at ≥3× — closing the loop below the blocked-goal escalation threshold.
