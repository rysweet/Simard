# Tertiary Investigation (architect) — Lane-A vs Lane-B recurrence isolation & "intended signal vs recording defect"

**Role:** TERTIARY investigator (architect).
**HEAD:** `f9cefec1` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
**Focus:** Whether Lane-A `RecurringSignature` feeds Lane-B recurrence (commit
`f9cefec1`), and whether the architecture treats the blocked-cluster `×2`
recurrence as **intended signal** or a **recording defect**. Reconciled against
prior artifacts (`tertiary_orchestration_synthesis_…_f9cefec1`,
`secondary_reemission_and_convergence_…_f9cefec1`, `RECONCILIATION_LEDGER`,
`secondary_dedup_recurrence_VALIDATION_HEAD`).
**Method:** Re-read every load-bearing line in `src/`; did not trust doc
citations. Ran the guard suite.
**Empirical check:** `cargo test --lib overseer::tests_root_cause` →
**21 passed, 0 failed** at HEAD, including both lane-isolation tests.

---

## 0. One-line verdict

The `×2` is **intended signal, not a recording defect.** The two recurrence
lanes are **architecturally isolated by construction** — different memory
projections, different counters, different thresholds, different decisions — and
commit `f9cefec1` adds a regression guard proving Lane-A cannot bleed into
Lane-B. The genuine defect is *elsewhere*: a **missing closing action** so the
observed board never changes, plus a **2↔3 escalation dead-zone** that is a
*consequence* of the (correct) isolation. Do **not** "fix" the counter or the
isolation.

---

## 1. The two lanes are separate subsystems (cited)

| Aspect | **Lane A — episodic recall** | **Lane B — root-cause escalation** |
|---|---|---|
| Type | `Signal::RecurringSignature{signature, occurrences}` (`signal.rs:70`) | `RootCause.recurrence: u32` (`root_cause.rs`, via `analyze`) |
| Input data | `state.recall.episodes[].failure_signature` — raw failure-signature strings from recalled episodes (`signal.rs:455-470`) | `recall: &[PriorOccurrence]` filtered by `o.cause_label == primary_label` (`root_cause.rs:79-85`) |
| Memory source | `state.recall` snapshot (episode stream) | `recall_occurrences(dedup_key)` → `StoredOccurrence` facts keyed on `occurrence_concept(dedup_key)` (`mod.rs:456,972-997`) |
| Threshold | `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`) | `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) |
| Emission site | appended in `signals_from` when `occurrences ≥ 2` (`signal.rs:463`) | computed in `analyze` per problem (`mod.rs:457`) |
| Decision consumed by | `orient` → merges into matching problem / High-priority `ProcessHealth` advisory; **raises priority only** (`mod.rs:1353-1363`) | `decide_blocked_goal` → `EscalateBlockedGoal` at `recurrence ≥ 3` (`mod.rs:1613`) |

**They never read each other.** Lane A counts `failure_signature` strings off the
episode snapshot; Lane B counts `PriorOccurrence.cause_label` matches off
independently-stored `StoredOccurrence` facts. The only place both are visible is
as **sibling fields of one `Problem`** (`evidence: Vec<Signal>` vs `why:
Option<RootCause>`); no code path derives one from the other.

---

## 2. Commit `f9cefec1` — what it guards (cited test)

`tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
(`tests_root_cause.rs:490-529`) constructs a Problem whose evidence carries a
**deliberately loud** Lane-A signal:

```
loud = RECURRING_SIGNATURE_THRESHOLD (2) + RECURRENCE_ESCALATION_THRESHOLD (3) + 5 = 10
Signal::RecurringSignature { signature: "goal:blocked:research", occurrences: 10 }
```

with **empty Lane-B recall** (`analyze(&problem, …, &[])`). Asserts:

- `problem.why.recurrence == 0` — the loud Lane-A count does **not** bleed in.
- `decide(&problem)` returns `Intervention::UnblockGoal{..}` — a `×10` Lane-A
  signal does **not** trip Lane-B's `≥3` escalation gate; the goal **self-heals**.

The converse guard `lane_b_escalates_without_any_lane_a_signal`
(`tests_root_cause.rs:535-573`) proves Lane B escalates on its **own**
`PriorOccurrence` recall (≥3, cause-matched) with Lane A entirely silent. Both
green at HEAD. **This is a net-new verification test, not a behavior change** —
`git show --stat f9cefec1` = only `+99` lines in `tests_root_cause.rs`. The
isolation was already true; `f9cefec1` pins it against regression.

**Answer to the Lane-A→Lane-B feed question: NO feed. Isolated by design, guarded
by test.** (Confirms prior H1; reconciles with
`secondary_reemission_and_convergence` §"Lane isolation (holds, cited)".)

---

## 3. Intended signal vs recording defect — the architectural adjudication

### 3a. Why the recurrence is *intended signal* (not a recording bug)

The full observe→signature→count loop is deterministic and honest:

1. OBSERVE reads the board; `blocked_goals` is a **pure projection** of
   `GoalProgress::Blocked` (`sensor.rs:204-221`). No fabrication.
2. The composite `observation_signature` = `sort→dedup→"overseer-obs:{join('|')}"`
   over each problem's `dedup_key` (`mod.rs:1068-1073`). A **set hash** — stable
   iff the membership is stable.
3. Lane A's `occurrences` is a faithful **count of recalled episodes** carrying
   the same `failure_signature`. Two windows observing the same static set →
   two episodes → `occurrences = 2`. That is a correct measurement, not a
   double-count or replay: recall reads **live facts only**
   (`include_superseded:false`, `library_adapter.rs:763,773,830`), so there is
   no storage amplification.

The system re-emits because the *world state it measures* is unchanged — the
blocked goals are genuinely still blocked (D0: the completion gate cannot
reconcile an issue-closed-without-linked-merged-PR goal out of `Blocked`,
`completion_gate.rs:423-438`). **A truthful sensor reporting a stuck world is
signal, by definition.** Silencing the count would be the defect.

### 3b. Where a *recording* concern genuinely exists (and its boundary)

There **is** a recording-layer sharp edge, but it lives in **Lane B**, not in the
`×2` the user observed:

- `record_occurrence` writes via **non-idempotent `store_fact`** (`mod.rs:1034`),
  an append-only ratchet — so Lane B's `recurrence = recall_occurrences(...).len()`
  climbs by real re-observation (correct), *but* the naïve "fix" floated in the
  committed `CONSOLIDATED_FINDINGS §6.2b` (switch to `store_fact_with_caller_key`)
  would collapse recall to **1 forever** and make the `≥3` escalation rung **dead
  code** (`RECONCILIATION_LEDGER §2`; `secondary_dedup_recurrence_VALIDATION §4`).
  The correct shape is a caller-key **upsert carrying an `occurrence_count` in the
  fact content**, with escalation reading that field. **This is a settled prior
  finding — I confirm it, do not re-derive it.**

So: the **counting is honest**; the only recording-hardening needed is to make
Lane-B's cross-tick counter durable-without-ratchet, which is orthogonal to the
`×2` Lane-A cluster the user asked about.

### 3c. The architectural cost of correct isolation: the 2↔3 dead-zone

Because the lanes are (correctly) isolated, a signature stably re-observed at
Lane-A `×2` **raises priority in `orient` but never reaches Lane-B's `≥3`
escalation** unless Lane B *independently* accumulates ≥3 cause-matched
`PriorOccurrence`s. If the WHY-ladder's evidence source is unwired
(`memories.completion_evidence == None`, Gate 1 in
`ooda_loop/cycle.rs:582`), Lane B's recall stays thin and the goal **idles in the
band `2 ≤ n < 3`** — flagged, whispered, never closed. This dead-zone is a
**design consequence of the isolation, not a bug in it.** The remedy is a rung
that acts on Lane-A `×2` (promote the *matching* blocked-goal problem into
remediation), **not** merging the lanes.

---

## 4. Structural concerns (architect lens)

- **Steering-vs-closing asymmetry (the core structural defect).** The Decide
  table is rich in *steering* edges (Whisper/Flag/Escalate) and sparse in
  *closing* edges. `WorkstreamCoverage` has **no** board-mutating/issue-filing arm
  on either M1 or M2 — notify-only (`mod.rs:884-946`, `observer.rs:113-120`);
  `UnblockGoal` is perpetual-self-heal only. Only `ProcessHealth`/recurrence
  reaches `LaunchRecipe`/`FileIssue`. Every High-priority `ProblemKind` should
  converge through ≥1 closing edge. This is an **interface gap in the Decide
  table**, not a detection or recording bug. (Confirms secondary F1/F2.)
- **Reconciliation invisible on failure.** `sweep_done_goals` is `Option`-gated
  with no fail-loud when `completion_evidence` is `None`. A stalled deployment
  cannot distinguish "genuinely blocked" from "reconciler disabled." Emit a
  distinct signal when the evidence source is absent or repeatedly
  `CouldNotVerify`.
- **Composite opacity blocks a gap ledger.** `dedup_key` for coverage is the
  constant `"workstream-gap"` (`mod.rs:1371`); the set-hash cannot tell 2 gaps
  from 20, so a per-gap recurrence ledger is impossible until the key carries
  per-gap identity. Small change, outsized diagnostic value.
- **`resource:engineer_spawn` is a false lead.** Benign telemetry at `live ≥ 8`
  (`signal.rs:351`); no spawn semaphore gates goal work. State-coupled to the
  admission cap but never a *cause* of a blocked goal. Do not build capacity
  controls for this stall. (Confirms secondary F3.)

---

## 5. Reconciliation with prior artifacts (no re-derivation)

| Prior claim | This wave |
|---|---|
| H1: `×2` is honest cross-window re-observation, H0 (storage/replay artifact) REJECTED | **CONFIRM** — added the sensor/recall provenance chain proving the count is a truthful measurement |
| Lane A (RecurringSignature@2) and Lane B (recurrence@3) share no counter | **CONFIRM + empirically re-ran** the two guard tests green at HEAD |
| D0 completion-gate conjunction is the anchor's root | **ADOPT** (upstream of my scope; the reason the measured world stays static) |
| CONSOLIDATED §6.2b `store_fact_with_caller_key` fix is a trap | **CONFIRM** — the isolation makes clear the counter must not be collapsed |
| Convergence-rung asymmetry / notify-only gaps | **CONFIRM** — reframed as a Decide-table interface gap |

**No contradictions with prior verdicts.** New architect contribution: an
explicit adjudication that the recurrence is **intended signal** and that the
2↔3 dead-zone is a *design consequence of correct lane isolation*, plus the
signal-vs-recording boundary (recording concern lives only in Lane-B durability,
not in the `×2`).

---

## 6. Recommendation (diagnosis only — underlying goals OUT OF SCOPE)

**No action on the counter or the lane isolation — both are correct and now
guarded.** The recurrence is a truthful under-throughput signal. Land, in order
of leverage:

1. **Give `WorkstreamCoverage` a closing edge** (route a per-gap `GapItem` into
   the existing stewardship file-or-match rung, keyed on `GapItem.signature`, at
   first proven recurrence `×2`) — closes the dead-zone from the Lane-A side
   **without** touching lane isolation.
2. **Reconcile issue-closed goals out of `Blocked`** (D0) and **fail loud** when
   `completion_evidence` is `None` — makes the measured world change so the honest
   sensor stops re-reporting the same set.
3. **Harden Lane-B recording** via a caller-key upsert with an in-content
   `occurrence_count` (per the settled VALIDATION finding) — do **not** use the
   naïve `store_fact_with_caller_key` swap.

**Do NOT:** merge the lanes, silence the `×2`, add spawn-capacity controls, or
touch the kgpacks-rs issues / simard-identity personas.

---

## 7. Verification performed

- Re-read `signal.rs:70,351,362,455-470`, `root_cause.rs:33,53,65-113`,
  `mod.rs:440-470,965-1044,1340-1371,1068-1073`, `sensor.rs:204-221`,
  `tests_root_cause.rs:490-573` at HEAD `f9cefec1`.
- Ran `cargo test --lib overseer::tests_root_cause` → **21 passed, 0 failed**,
  including `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` and
  `lane_b_escalates_without_any_lane_a_signal`.
- Cross-checked against `RECONCILIATION_LEDGER`,
  `secondary_reemission_and_convergence_HEAD_f9cefec1`,
  `tertiary_orchestration_synthesis_and_remediation_HEAD_f9cefec1` — consistent
  with every prior verdict.

**Bottom line:** Lane-A does not feed Lane-B (isolated by construction, guarded
by `f9cefec1`, green by test). The blocked-cluster `×2` is **intended signal** —
a faithful measurement of a genuinely static blocked set — **not** a recording
defect. The real defects are the missing closing action (steering-vs-closing
asymmetry / D0) and the 2↔3 dead-zone that is itself a *consequence* of the
correct isolation. Remediate the closing edge and the anchor reconciliation;
leave the counter and the lane boundary alone.
