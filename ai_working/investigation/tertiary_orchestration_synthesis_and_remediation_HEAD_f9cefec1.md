# Tertiary Investigation (deep dive) — End-to-end orchestration synthesis & remediation levers for the recurring `goal:blocked…|workstream-gap` signature

**Role:** Tertiary investigator (architect).
**Date:** 2026-07-15.
**HEAD:** `f9cefec1` (branch `investigation/recurring-blocked-goals-workstream-gaps`).
The two latest commits are investigation artifacts (a docs consolidation and a
Lane-A/Lane-B decoupling test); **no remediation has landed** — every defect
below is live at HEAD.
**Focus:** Synthesize goal scheduling → workstream assignment → gap detection →
engineer spawn → block into one end-to-end loop, locate the stall point, and
name concrete remediation levers.
**Relationship to prior artifacts:** BUILDS ON the established D1/D2/D3 geometry
in [`tertiary_architecture_VALIDATION_HEAD.md`](./tertiary_architecture_VALIDATION_HEAD.md),
[`tertiary_gap_routing_and_remediation_rung.md`](./tertiary_gap_routing_and_remediation_rung.md),
and the H0–H8 matrix in
[`verification_results_ALL_HYPOTHESES.md`](./verification_results_ALL_HYPOTHESES.md).
It does not restate them. It contributes **one new load-bearing finding (D0)** —
an upstream reconciliation seam the prior waves under-weighted — and a
whole-loop remediation ordering.

---

## 0. Re-grounding at HEAD (load-bearing claims re-verified in source)

| Claim | Source @ `f9cefec1` | Status |
|---|---|:--:|
| Composite signature = `overseer-obs:{sorted∪dedup(dedup_key) join "\|"}` | `overseer/mod.rs:1068-1072` | ✅ |
| Blocked-goal `dedup_key` = `goal:blocked:{goal_id}` | `overseer/mod.rs:1336` | ✅ |
| Coverage `dedup_key` = fixed constant `"workstream-gap"` | `overseer/mod.rs:1371` | ✅ |
| Spawn `dedup_key` = `"resource:engineer_spawn"`, ResourcePressure/Normal | `overseer/mod.rs:1268-1271` | ✅ |
| `RecurringSignature` summary is literally *"recurring signature seen {n}× in cognitive memory ({sig})"* | `overseer/mod.rs:1360-1362` | ✅ (this investigation title is auto-generated from that Problem) |
| `RecurringSignature` fires at `occurrences ≥ 2` from recalled episodes | `signal.rs:362,463` | ✅ |
| `EngineerSpawnRate` fires only at `live ≥ 8`; observe-and-flag only | `signal.rs:351,394` | ✅ benign |
| Blocked goals are a pure projection of `GoalProgress::Blocked` on the board | `sensor.rs:204-221` | ✅ |
| Completion gate = **conjunction** (`pr_merged ∧ issue_closed [∧ deployed]`) | `goal_curation/completion_gate.rs:423-438` | ✅ **new** |
| `sweep_done_goals` reconciliation is gated on `completion_evidence.is_some()` | `operator_commands_ooda/daemon/mod.rs:1320-1343` | ✅ **new** |

**Verdict:** the `×2` is a faithful cross-window re-observation of a
near-static problem set (H1 CONFIRMED), not a storage/replay/collision artifact
(H0 REJECTED). The dominant, always-present anchor is
`goal:blocked:fix-agent-kgpacks-rs-issue-17-…`; everything else in the composite
is either co-blocked goals, the opaque `workstream-gap` token, or benign
`resource:engineer_spawn` drift.

---

## 1. The end-to-end orchestration loop (one diagram)

```
                         daemon tick  (operator_commands_ooda/daemon/mod.rs)
                                  │
     ┌────────────────────────────┴─────────────────────────────┐
     │  run_ooda_cycle(...)   ── contains the acting Overseer tick │
     │                                                            │
     │   OBSERVE  overseer/mod.rs:393  observe_board()            │
     │      blocked_goals ← blocked_goals_from_board  (sensor.rs) │◄──┐  same board,
     │      workstream_gaps ← detect_workstream_gaps             │   │  unchanged
     │      recall ← cognitive memory (last window's episodes)   │   │  between ticks
     │                                                            │   │
     │   ORIENT  signal.rs signals_from → mod.rs orient          │   │
     │      GoalBlocked   → problem goal:blocked:<id>            │   │
     │      WorkstreamGap → problem workstream-gap  (High)       │   │
     │      RecurringSignature(≥2) → High ProcessHealth problem  │   │
     │                                                            │   │
     │   DECIDE  mod.rs decide                                    │   │
     │      GoalHygiene       → Unblock / Escalate(@ recurrence≥3)│   │
     │      WorkstreamCoverage→ FlagWorkstreamGaps (NOTIFY-ONLY)  │───┼── D3 no
     │      ProcessHealth(rec)→ LaunchRecipe / Report            │   │   closing edge
     │                                                            │   │
     │   ACT + WRITE-BACK  mod.rs:534 write_back_observation      │   │
     │      records ALL problems under one overseer-obs: key     │───┼── D1 self-nest
     │      through a single 900 s WhisperGate                    │   │
     └────────────────────────────┬─────────────────────────────┘   │
                                  │                                  │
     POST-CYCLE reconciliation (daemon/mod.rs:1320)                 │
        if completion_evidence.is_some():                          │
            sweep_done_goals → gate.evaluate(goal)                 │
            Complete  ⟺  pr_merged ∧ issue_closed [∧ deployed]     │──┘  D0: issue-17
            else → goal stays Blocked ───────────────────────────────►    STAYS Blocked
```

The loop closes on itself: OBSERVE reads the board, the board is not mutated
into a non-blocked state by the only reconciler (D0), so the *next* tick observes
the identical set → identical composite signature → the recall lane counts it a
second time → `RecurringSignature{occurrences:2}`. Nothing in DECIDE/ACT for the
coverage or blocked-goal problems produces a state change that would break the
identity (D3 for gaps; D0+D2 for blocked goals).

---

## 2. NEW — D0: the reconciliation seam cannot clear an issue-closed-without-linked-merged-PR goal

The prior waves located three defects *inside the Overseer* (D1 emission
hygiene, D2 escalation counter, D3 coverage closing edge). All three explain why
the signature **persists and is re-counted**; none explains why the anchor goal
`goal:blocked:issue-17` is **on the board as Blocked in the first place while its
GitHub issue is closed.** That is upstream of the Overseer, at the *only* seam
that can move a goal out of `Blocked` on completion evidence:

**`CompletionEvidenceGate::evaluate` is a conjunction, not a disjunction**
(`completion_gate.rs:423-438`):

```
missing = []
if !pr_merged   { missing += PrNotMerged }
if !issue_closed{ missing += IssueOpen   }
if self_affecting && !deployed { missing += NotDeployed }
Complete  ⟺  missing.is_empty()          // needs pr_merged AND issue_closed
```

Consequences for the kgpacks issue-17 anchor:

1. **Issue closed ≠ goal complete.** A closed issue alone yields
   `missing = [PrNotMerged]` → `Blocked`, *not* `Complete`. If issue #17 was
   closed out-of-band (manually, as duplicate/wontfix, or by a PR the evidence
   source cannot tie to this goal's `wip_refs`/issue ref via `any_pr_merged`,
   `completion_gate.rs:670`), `sweep_done_goals` (`daemon/mod.rs:1323`) leaves it
   on the board **forever**. This is the precise reason the anchor never clears.
2. **Fail-closed on a flaky source compounds it.** Any error from `any_pr_merged`
   / `issue_closed` returns `Blocked{CouldNotVerify}` (`completion_gate.rs:399-403,443`).
   A rate-limited / unauthenticated `gh` evidence source therefore *re-blocks*
   the goal every cycle — a live, near-static blocked set is exactly what the
   `×2` recall needs.
3. **Reconciliation is conditionally wired.** The whole `sweep_done_goals` pass
   is guarded by `if let Some(evidence) = &memories.completion_evidence`
   (`daemon/mod.rs:1322`). On any deployment where that source is `None`, the
   reconciler is a no-op and *every* blocked goal is permanent. There is no
   fail-loud when it is absent — the stall is silent.
4. **Ordering guarantees at least one emission per tick regardless.** The
   Overseer OBSERVE (`mod.rs:393`) runs *inside* `run_ooda_cycle`, while
   `sweep_done_goals` runs *after* it (`daemon/mod.rs:1300` then `:1323`). So
   even a goal that *would* reconcile is still observed-as-blocked and written
   back once this tick before the sweep removes it next tick — a one-tick lag
   that is harmless when reconciliation works and fatal when it does not.

**D0 is the root of the anchor; D1/D2/D3 are why the anchor's signature
recurs, is mis-counted, and never converges.** They are complementary, not
competing.

---

## 3. What the three signature families actually mean (throughput view)

Confirming and sharpening H7/H8 from the throughput/orchestration angle:

| Family | Emitted from | Orchestration meaning | Closing seam today |
|---|---|---|---|
| `goal:blocked:<id>` (many) | board projection (`sensor.rs:209`) | work that *entered* Blocked and was never reconciled out (**D0**) | Unblock (perpetual self-heal only) / Escalate@3 (**D2** dead-zone) |
| `workstream-gap` (opaque, repeated) | `detect_workstream_gaps` | uncovered p1/p2 work with no active engineer/PR | **none** — notify-only on both M1 and M2 (**D3**) |
| `resource:engineer_spawn` | `live ≥ 8` (`signal.rs:351`) | benign capacity pressure; **not** a semaphore, **not** starving goals | observe-and-flag only |

There is **no spawn semaphore** anywhere in `agent_supervisor` / `engineer_loop`
gating goal work (grep at HEAD finds only the `ENGINEER_SPAWN_THRESHOLD = 8`
observe threshold). The "resource:engineer_spawn → workstream-gap → block"
starvation feedback loop hypothesized in the strategy is **not present**:
`engineer_spawn` is a passenger in the composite string, not a driver. Treat it
as noise for remediation purposes (matches the "potential dead ends" guidance).

The blocked and gap families are **two views of one under-throughput
condition**: important work exists (goal blocked, or gap uncovered) and the
orchestrator has no converging action that changes the board state, so it
re-observes the same thing. The system is *steering-only* on both — it flags,
whispers, and (rarely) escalates, but does not *close*.

---

## 4. Remediation levers — ordered by leverage on the stall

Ranked by how directly each breaks the observe→identical-signature→re-count
loop. Each names the seam and the minimal change shape; none requires touching
the kgpacks-rs issues themselves (they are out of scope; issue #17 is already
closed — the fault is orchestration state).

**L0 — Reconcile issue-closed goals out of `Blocked` (fixes D0; highest leverage).**
Add a disjunctive "objective already satisfied" path so a goal whose *linked
issue is closed* is auto-completed even without a linked merged PR, OR
downgraded from `Blocked` to a distinct `AwaitingReconciliation` state that the
observe projection (`sensor.rs:209`) does **not** surface as `GoalBlocked`.
Options at `completion_gate.rs:423-438`:
  - treat `issue_closed && !self_affecting` as `Complete` (issue closure is the
    goal's own definition of done for "fix issue N" goals); or
  - keep the conjunction for *verification* but add a separate
    `sweep_stale_blocked` pass that tombstones a goal blocked > N cycles whose
    issue is closed. This is the single change that makes the anchor disappear.
  Also: **fail loud** when `memories.completion_evidence` is `None`
  (`daemon/mod.rs:1322`) instead of silently skipping reconciliation.

**L1 — Give `WorkstreamCoverage` a closing edge (fixes D3).**
Add a `WorkstreamCoverage → LaunchRecipe` (or `FileIssue`) arm in DECIDE
(`mod.rs:1534-1543`) so a persistent gap converges to a tracked/launched
workstream instead of notify-only forever. Gate it behind a **cross-window
recurrence ledger** (the gap-remediation "rung" from
`tertiary_gap_routing_and_remediation_rung.md`) so it fires on *repeat* gaps,
not first sighting. Prerequisite: replace the opaque constant `"workstream-gap"`
dedup_key (`mod.rs:1371`) with a per-gap identity so the ledger can key on
*which* gap recurs.

**L2 — Close the 2↔3 escalation dead-zone (fixes D2).**
Lane A (`RecurringSignature` @ 2) and Lane B (`recurrence` escalation @ 3) share
no counter (verified green by the new test in commit `f9cefec1`,
`tests_root_cause.rs`). A signature stably re-observed at `×2` never reaches the
Lane-B escalation at 3, so it neither escalates nor self-heals — it idles. Add a
recurrence-aware rung *between* 2 and 3 (e.g., at Lane-A `occurrences ≥ 2`,
promote the *matching* blocked-goal problem to trigger L0/L1 remediation rather
than only raising its priority in `orient`, `mod.rs:1217-1219`).

**L3 — Stop the write-back self-nesting (fixes D1; hygiene, lowest urgency).**
`write_back_observation` records **all** problems including the recall-derived
`RecurringSignature`, whose `overseer-obs:` key then nests inside the next
signature (`mod.rs:534-563`, `:1353-1363`). Exclude `ProblemKind::ProcessHealth`
problems whose evidence is `Signal::RecurringSignature` from the write-back set.
This cleans the `overseer-obs:…|overseer-obs:…` shape but does **not** by itself
stop the loop (D0/D3 do); it is cosmetic-plus-bounded.

**Ordering:** L0 first (it removes the anchor and most of the composite), then
L1 (removes the `workstream-gap` tail), then L2 (guarantees convergence when a
residual persists), then L3 (hygiene). L0 alone likely collapses the observed
`×2` to nothing for the kgpacks anchors; L1+L2 generalize the fix so the class
of stall cannot recur for other goals/gaps.

---

## 5. Structural concerns / interface notes

- **Steering vs. closing asymmetry.** The Overseer's Decide table has rich
  *steering* actions (Whisper, Flag, Escalate) but sparse *closing* actions
  (Unblock is perpetual-only; only ProcessHealth/recurrence reaches
  `LaunchRecipe`). Every High-priority `ProblemKind` should converge through at
  least one board-mutating or issue-filing edge; `WorkstreamCoverage` is the
  lone exception on both M1 and M2 (§1, D3). This is an *interface* gap in the
  Decide table, not a detection bug.
- **Reconciliation is invisible on failure.** The single most impactful
  correctness path (`sweep_done_goals`) is behind an `Option` with no
  observability when absent, and fails closed on source errors. A stalled
  deployment cannot tell whether it is "genuinely blocked" or "reconciler
  disabled/flaky." Emit a distinct signal when `completion_evidence` is `None`
  or repeatedly `CouldNotVerify`.
- **Composite opacity.** `observation_signature` (`mod.rs:1068`) hashes the
  *set* of dedup_keys; with a constant `workstream-gap` token it cannot tell 2
  gaps from 20, and a recurrence ledger keyed on it is impossible. Per-gap
  identity (L1 prerequisite) is a small change with outsized diagnostic value.
- **`engineer_spawn` is a false lead.** Do not build spawn-capacity controls to
  address this stall; the signal is a benign co-occurrence (§3).

---

## 6. Verification performed

- Traced every seam in §0 to source at HEAD `f9cefec1` (line-cited).
- Confirmed the completion gate is a conjunction and `sweep_done_goals` is
  `Option`-gated (D0 — the new finding) by reading
  `completion_gate.rs:394-439` and `daemon/mod.rs:1320-1343`.
- Confirmed no spawn semaphore exists (grep of `agent_supervisor`,
  `engineer_loop`, `signal.rs` for `Semaphore`/`capacity`/`max_engineers`
  returns only the `ENGINEER_SPAWN_THRESHOLD = 8` observe threshold), rejecting
  the starvation-loop hypothesis.
- Cross-checked against the prior H0–H8 matrix: this report is consistent with
  every verdict and adds D0 as the upstream cause the matrix classified only as
  "real re-observation of a near-static set" (H1) without naming *why* the set
  stays static.

**Bottom line:** the recurring signature is a genuine, low-count orchestration
stall, not an artifact. Its anchor persists because the completion gate cannot
reconcile an issue-closed-without-linked-merged-PR goal out of `Blocked` (D0),
and the loop never converges because coverage gaps have no closing edge (D3) and
the escalation lane has a 2↔3 dead-zone (D2). Land L0 → L1 → L2 (→ L3) to clear
it; do **not** chase the kgpacks issues or engineer-spawn capacity.
