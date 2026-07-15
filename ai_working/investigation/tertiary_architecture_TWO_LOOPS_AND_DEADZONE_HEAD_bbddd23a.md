# Tertiary Investigation (architect) — The Two Non-Closing Observe-and-Flag Loops, `engineer_spawn` Classification, and the Dead-Zone Remediation Rung

**Role:** Tertiary investigator (architecture / structural).
**HEAD:** `bbddd23a` — verified. Only `src/overseer/tests_root_cause.rs` (+99 test
lines) changed since the prior tertiary pin `5a85317b`; **all non-test source is
byte-identical**, so every load-bearing line number below is exact at HEAD.
**Scope:** architecture only — component boundaries, interfaces, and the
structural reason the underlying problem set does not converge. **No code changed.**

---

## 0. Executive verdict

The composite signature persists because **three observe-and-flag interventions
terminate without a closing edge**, and one **escalation counter is architecturally
starved** so the only rung that *could* close a blocked goal is unreachable for
exactly the goals that need it. Concretely:

| # | Loop | Decide arm | Act outcome | Closing edge? |
|---|---|---|---|---|
| **L1** | Blocked-goal WHY gate | `decide_blocked_goal` → `Report` (sub-threshold) | `Reported` | **None** — re-emits `goal:blocked:<id>` every cycle |
| **L2** | Workstream-gap launch gap | `WorkstreamCoverage` → `FlagWorkstreamGaps` | `WorkstreamGapsFlagged` (notify-only) | **None** — no `launch.rs` edge, no issue |
| **L3** | Engineer-spawn pressure | `ResourcePressure` → `Escalate` | `Escalated` (bare no-op) | **None**, but **benign** (self-resolving) |

The `resource:engineer_spawn` token is **benign membership drift**, not a new
defect. The **dead zone** is real and structural: a recurring problem is *flagged*
at recall-count **2** (`RECURRING_SIGNATURE_THRESHOLD`) but *escalation* needs
**3** (`RECURRENCE_ESCALATION_THRESHOLD`), and the counter that feeds escalation
**cannot advance** for a sub-threshold blocked goal because its `Reported` outcome
is excluded from `record_occurrence`.

---

## 1. Loop L1 — Blocked-goal WHY gate never closes (the D2 latch)

### 1.1 The Decide arm

`GoalBlocked` → `ProblemKind::GoalHygiene` (`mod.rs:1324-1345`, key
`goal:blocked:{goal_id}`, `mod.rs:1336`). Decide routes it to `decide_blocked_goal`
(`mod.rs:1447-1483` → `1603-1631`):

```
recurrence >= RECURRENCE_ESCALATION_THRESHOLD (3) → EscalateBlockedGoal   (root_cause.rs:33)
perpetual && is_no_progress_marker(reason)        → UnblockGoal (self-heal)
needs_review                                      → EscalateBlockedGoal
otherwise                                         → Report            ← DEAD ZONE
```

A **plain** operator/dependency block (not perpetual+no-progress, not
needs_review, recurrence < 3) falls to `Intervention::Report` (`mod.rs:1630`) →
`ActOutcome::Reported` (`mod.rs:658`). It is observed and surfaced, **never
closed** — so the same `goal:blocked:<id>` dedup key is re-derived on the next
Observe pass, ad infinitum. This is the first non-closing loop.

### 1.2 Why the escalation rung is UNREACHABLE — the architectural latch

`decide_blocked_goal` gates escalation on `problem.why.recurrence`
(`mod.rs:1469`). `recurrence` is `recall_occurrences(dedup_key).len()` filtered to
the primary cause (`root_cause.rs:79-82`; recall at `mod.rs:972-997`). Those
occurrence facts are written **only** by `record_occurrence` (`mod.rs:1004-1043`),
which the Act loop calls **only when** `outcome_records_occurrence(&outcome)` is
true (`wiring.rs:276-280`).

**`ActOutcome::Reported` is NOT in that set** (`wiring.rs:612-627` — the list is
`Launched | Merged | Deployed | IssueFiled | Escalated | Whispered | GoalUnblocked
| GoalEscalated | ConflictResolved | GoalTransferred | Audited`). Therefore:

```
plain blocked goal → Report → Reported → (no record_occurrence)
                  → recall_occurrences(...) stays empty
                  → recurrence stays 0  ( < 3 )
                  → next cycle: Report again … forever
```

The counter that would unlock escalation can **only** advance through an outcome
(`Escalated`/`GoalEscalated`/`GoalUnblocked`) that is **itself already gated
behind having accrued the counter**. That circular dependency is the
**dead-zone latch**: escalation is structurally unreachable for precisely the
blocked-goal class it was designed to catch. (Confirmed independently; matches
`tertiary_pipeline_idempotency_RERUN_85b9398a.md §77` and the D2 line in
`tertiary_architecture_REGROUND_HEAD_5a85317b.md`.)

---

## 2. Loop L2 — Workstream-gap flagged but never launched (the D3 open edge)

`WorkstreamGap` → **one consolidated** `ProblemKind::WorkstreamCoverage` problem,
fixed key `"workstream-gap"` (`mod.rs:1368-1373`; emission `signal.rs:475-479`).
Decide's arm carries the gaps forward verbatim to
`Intervention::FlagWorkstreamGaps` (`mod.rs:1534-1543`). Act’s handler
`act_flag_workstream_gaps` (`mod.rs:884-948`) **only notifies the operator**
(email + Signal), deduping per-gap through `gap_gate`. Its own doc-comment is the
design statement of the open edge:

> "FLAG the backlog-coverage gaps… Routine observations **never create GitHub
> issues or stewardship backlog items**." (`mod.rs:881-883`)

**Interface asymmetry (the structural defect):** the `RecipeLauncher` seam
(`launch.rs` — `smart_orchestrator_args` + `SmartOrchestratorLauncher`) is wired
for `ProcessHealth`/`CrossCutting`/`StepFailure`, which all Decide to
`Intervention::LaunchRecipe` (`mod.rs:1429-1435`, `1565-1579`). `WorkstreamCoverage`
is the **only High-priority coverage problem with no edge into that seam** and no
edge into `IssueFiler`. The gap is detected, priced High, notified — and then
**left uncovered**, so the identical `workstream-gap` key recurs every scan tick.
There is no cross-window ledger that would mark a gap "already launched," so even
the notify path relies solely on the in-process `gap_gate` window.

Note this is **fixed-key**, not hashed — `"workstream-gap"` is a literal
constant. Its repetition in the corpus is genuine re-observation of the *same*
unremediated condition, not key churn.

---

## 3. `resource:engineer_spawn` — classification and verdict: benign drift

- **Emitter:** `Signal::EngineerSpawnRate { live }` fires when
  `live_engineers >= ENGINEER_SPAWN_THRESHOLD` (`signal.rs:393-397`).
- **Classification:** `ProblemKind::ResourcePressure`, `Priority::Normal`, fixed
  key `"resource:engineer_spawn"` (`mod.rs:1267-1272`). Recall keyword
  `"engineer_spawn"` (`capabilities.rs:562`).
- **Decide/Act:** `ResourcePressure` → `Intervention::Escalate { reason }`
  (`mod.rs:1444-1446`) → `ActOutcome::Escalated` (`mod.rs:663`) — a **bare no-op**
  that returns `Escalated` without dispatching a notifier. Technically a *third*
  non-closing loop (L3), but structurally **benign**: elevated live-engineer count
  is a transient resource condition that self-resolves as spawned engineers
  finish. Unlike L1/L2 there is nothing to "close."

**Verdict:** the token’s appearance/disappearance across the corpus
(`…resource:engineer_spawn|workstream-gap|resource:engineer_spawn|…`) is
**membership drift** of the per-cycle problem set — it enters the composite only
on ticks where live engineers crossed the threshold — **not a new defect**. It is
a fixed constant, so when present it is stable. (Consistent with
`tertiary_lane_isolation_and_membership_drift_HEAD_f1db90f4.md`.)

One honest asymmetry worth flagging (not a bug in scope): `Escalated` **is** in
`outcome_records_occurrence` (`wiring.rs:619`), so `resource:engineer_spawn`
*does* accrue occurrences — the opposite of the L1 `Reported` starvation. The two
resource/goal paths treat the recurrence counter inconsistently.

---

## 4. The dead-zone remediation rung — the missing tier

Two independent recurrence lanes with two thresholds create a gap with no rung
in the middle:

```
 Lane A (observation echoes)     Lane B (root-cause occurrences)
 write_back_observation           record_occurrence
 record_observation (mod.rs:534)  store_fact (mod.rs:1034)
 threshold: RECURRING_SIGNATURE   threshold: RECURRENCE_ESCALATION
   = 2  (signal.rs:362)             = 3  (root_cause.rs:33)
```

| Count | What fires | What closes | Gap |
|:--:|---|---|---|
| **1** | one-off; below both thresholds | nothing (correct — noise) | — |
| **2** | Lane A `RecurringSignature` (Priority **High**, `signal.rs:463`) | **nothing** — priority bump is inert for `GoalHygiene` (Decide ignores priority, routes by `kind`) | **DEAD ZONE** |
| **3** | Lane B escalation eligible | escalate/heal — *if Lane B ever reached 3* (it can't for L1, §1.2) | — |

At **count 2** the system has *recognized* recurrence (raised a High-priority
`RecurringSignature` problem, `mod.rs:1353-1363`) but has **no remediation rung**:
the blocked-goal path still `Report`s, and the coverage path still notify-only
flags. The recognition changes **priority**, which for `GoalHygiene`/
`WorkstreamCoverage` changes **nothing** about the action taken. So a problem sits
"flagged as recurring, still uncovered" indefinitely — this is the count-2 signal
the user observed ("seen 2×").

**Missing architectural element:** a *middle remediation tier* that, on Lane A
recognition (count ≥ 2), converts a non-closing flag into a closing action — e.g.
a `WorkstreamCoverage` edge into the `RecipeLauncher`/`IssueFiler` seam, and/or a
Lane-A-driven bump that lets a blocked goal escalate without waiting on the
starved Lane-B counter. Naming it here (not fixing it): the **"recurrence-2
closing rung."**

---

## 5. Component-boundary map (structural summary)

```
Observe (sensor/observer)  ──►  signals_from (signal.rs:366)  ──►  orient (mod.rs:1200)
                                    │  classify_signal (mod.rs:1238) → dedup_key
                                    ▼
                               decide (mod.rs:1400)
        ┌──────────────┬─────────────────────────┬──────────────────────┐
        ▼              ▼                          ▼                      ▼
  GoalHygiene    WorkstreamCoverage        ProcessHealth/           ResourcePressure
  decide_blocked FlagWorkstreamGaps        StepFailure/CrossCut     Escalate (no-op)
  _goal          (notify only)             LaunchRecipe ──► launch.rs seam
     │  sub-threshold → Report                  │  (the ONLY closing seam)
     ▼                     ▲                     ▼
  Reported            no edge here          Launched  ──► record_occurrence ✔
     │                     │                     (Lane B accrues → escalation possible)
     ✘ record_occurrence   ✘ no launch/issue edge
     (Lane B starved)      (D3 open edge)
```

- **Two "closing" seams exist** — `RecipeLauncher` (`launch.rs`) and `IssueFiler`.
- **Neither L1 (sub-threshold) nor L2 reaches either seam.** L1 dead-ends at
  `Reported`; L2 dead-ends at operator notify. Only `LaunchRecipe`-routed problems
  cross a closing seam *and* feed Lane B.
- The **write-back seam** (`write_back_observation`, `mod.rs:534-563`) is the sole
  producer of the `overseer-obs:` prefix (`mod.rs:1072`); recall of its own output
  is what re-materializes the whole composite next cycle — the outer wrapper of
  the observed signature.

---

## 6. Reconciliation with prior findings

Cross-checked against the live tree; the prior artifacts re-ground with **zero
non-test drift** at HEAD `bbddd23a`:

| Prior claim | Prior cite | Re-read @ HEAD | Status |
|---|---|---|:--:|
| `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | `signal.rs:362` | ✅ |
| `RecurringSignature` emits at `occurrences >= 2` | `signal.rs:463-464` | same | ✅ |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | same | ✅ |
| `decide_blocked_goal` escalates only `>= 3` | `mod.rs:1613` | `mod.rs:1613` | ✅ |
| `WorkstreamCoverage` → notify-only `FlagWorkstreamGaps` | `mod.rs:1534-1543` | same | ✅ |
| `resource:engineer_spawn` fixed key | `mod.rs:1270` | `mod.rs:1270` | ✅ |
| `record_occurrence` append-only `store_fact` | `mod.rs:1034` | `mod.rs:1034` | ✅ |
| `Reported` excluded from occurrence recording (**latch**) | — | `wiring.rs:612-627` | ✅ **grounded here** |

The one refinement this pass adds beyond prior tertiary docs: the D2 latch is not
merely "ACT is gated shut so `record_occurrence` rarely runs" — it is a **hard
exclusion** (`Reported ∉ outcome_records_occurrence`, `wiring.rs:612-627`), i.e.
the sub-threshold blocked goal records **zero** occurrences by construction, so
Lane B is not just slow to reach 3, it is **pinned at 0**. The circular
dependency is total, not probabilistic.

---

## 7. Residual uncertainties (explicitly marked)

- **Window-vs-restart origin of the `×2`** — `write_back_gate`/`gap_gate` are
  in-process `WhisperGate` maps (no cross-restart memory), so whether the two
  observations arose from two 900 s windows or two daemon restarts is **not
  decidable from static source**. Does not change the architectural verdict (real
  re-observation either way). Out of my tertiary scope; owned by secondary.
- **Priority-bump inertness** assumes Decide routes purely by `ProblemKind`, not
  `Priority` — confirmed for `GoalHygiene`/`WorkstreamCoverage` arms (`decide`,
  `mod.rs:1447`, `1534`), which never read `problem.priority`.

## 8. Recommendations for understanding (not fixes)

1. Read `decide` (`mod.rs:1400-1582`) and `outcome_records_occurrence`
   (`wiring.rs:612-627`) **together** — the dead-zone is only visible at their
   intersection, not in either alone.
2. Treat `launch.rs`’s `RecipeLauncher` and `IssueFiler` as the two "closing
   seams"; auditing which Decide arms reach them exposes every non-closing loop in
   one pass.
3. The minimal *closing-rung* candidates (for a future fix owner, named not
   built): (a) add a `WorkstreamCoverage → LaunchRecipe`/`FileIssue` edge; (b)
   let a Lane-A recurrence (count ≥ 2) bump a blocked goal past the Lane-B gate,
   or add `Reported` to `outcome_records_occurrence` so Lane B can actually
   accrue. Landing order and idempotency coupling are the secondary/synthesis
   owner’s call (D2 gate+counter must ship atomically; see prior D-set notes).
