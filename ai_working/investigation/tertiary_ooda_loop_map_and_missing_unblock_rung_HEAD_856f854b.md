# TERTIARY (architect) — OODA feedback-loop map, the missing unblock rung, and the self-ingestion loop into routing.rs

- **Role:** TERTIARY investigator (architecture / structural focus)
- **HEAD:** `856f854b`
- **Assigned focus:** Map the OODA feedback loop; locate the **missing unblock/resolution rung** and any **self-ingestion loop** between emitted signatures and `routing.rs`.
- **Method:** Re-grounded every load-bearing seam in live `src/` and **validated** (did not re-derive) against the existing ledger — `FINAL_SYNTHESIS.md`, `RECONCILIATION_LEDGER.md`, `blocked_transition_and_escalation_idempotency.md`, `primary_self_ingestion_loop_pipeline_trace_*`. Verdicts below are consistent with that ledger.

---

## 0. One-paragraph architectural verdict

The recurring `overseer-obs:…|goal:blocked:…|workstream-gap` signature is the **faithful fingerprint of an OODA loop that Observes and Decides but does not Close.** Three structural seams keep the same problem set alive across windows, so the honest re-observation counter reads "2×" forever:

- **L1 — Self-ingestion loop (Memory→Observe re-entry):** the Overseer writes its own observation episode with a recoverable `[sig:overseer-obs:…]` marker and recalls it **without a self-provenance filter**, so its own output re-enters Observe as a first-class problem and gets **re-wrapped** with another `overseer-obs:` prefix. The loop's only brake (the 900 s write-back gate) is defeated because each generation mutates the signature.
- **L2 — Missing unblock rung (Decide→Act dead zone for blocked goals):** the resolution ladder that would move a goal `Blocked → active` is **double-gated off** and the escalation counter that feeds it is starved, so blocked goals re-park every window instead of resolving.
- **L3 — Dangling routing edge (Act→routing.rs never wired):** `WorkstreamCoverage` is the only High-priority Decide arm whose Act path **only notifies** — it never files/launches, and `stewardship::route_failure` (built to accept the Overseer's `"overseer"` gap briefs) is **never called** on this path. The gap is terminal, so it re-emits every window.

None of these is a dedup/storage bug. All three are **missing edges in the loop graph** — the "resolution rungs."

---

## 1. The OODA loop as a graph (Observe → Orient → Decide → Act → Memory → recall)

```
                          ┌──────────────────────────────────────────────┐
                          │                 COGNITIVE MEMORY              │
                          │  (amplihack-memory-lib; episodes + facts)     │
                          └───────▲───────────────────────────┬──────────┘
        (writeback, gated)        │                           │ (recall — NO
   record_observation             │ store_episode             │  provenance filter)
   wiring.rs:301 / mod.rs:534     │ "…[sig:overseer-obs:…]"   │ recall_episodic
                                  │                           │ wiring.rs:1013-1031
                                  │                           ▼
   ┌────────────┐   ┌──────────┐  │   ┌───────────────────────────────────┐
   │  SENSOR    │──▶│ OBSERVE  │──┼──▶│ ORIENT: signal_to_problem          │
   │ sensor.rs  │   │ signals  │  │   │  goal:blocked:<id>   mod.rs:1336   │
   │ gap-scan,  │   │ signal.rs│  │   │  workstream-gap      mod.rs:1371   │
   │ blocked    │   └──────────┘  │   │  RecurringSignature  mod.rs:1353   │◀── L1 re-entry
   │ goals      │        ▲        │   │    (overseer-obs: prefix KEPT)     │    (self token
   └────────────┘        │        │   └──────────────┬────────────────────┘     recalled)
                         │        │                  │
             Signal::RecurringSignature              ▼
             signal.rs:455-470 (≥2 ⇒ "2×")   ┌───────────────┐
                                             │    DECIDE      │
                                             │  decide arms   │
                                             │  mod.rs:1440+  │
                                             └───┬───────┬────┘
                          decide_blocked_goal ◀──┘       └──▶ WorkstreamCoverage
                          mod.rs:1603-1631                    mod.rs:1534-1543
                                │                                   │
             ┌──────────────────┼───────────────┐                  ▼
             ▼                  ▼                ▼          Intervention::FlagWorkstreamGaps
   recurrence≥3          perpetual & no-        needs_review        │
   EscalateBlockedGoal   progress marker        Escalate...         ▼   ACT
   (notify)              UnblockGoal ★           (notify)     act_flag_workstream_gaps
                         mod.rs:1621                          mod.rs:884-948
                              ▲                                     │
                       ★ THE UNBLOCK RUNG                    notify operator ONLY
                       (starved — see §3)                    (NO file, NO launch,
                                                              NO route_failure) — L3
                                                                    │
                                              ┌─────────────────────┘
                                              ▼
                                   stewardship::route_failure
                                   routing.rs:39  — reachable ONLY via
                                   process_orchestrator_run (mod.rs:75),
                                   NEVER from the Overseer gap path.  ✗ dangling
```

Legend: **★** = the resolution rung that should fire but is starved; **✗** = an edge that exists in code but is never connected on this path; **L1** = the self-ingestion re-entry.

---

## 2. L1 — the self-ingestion loop (Memory → Observe), the only *growing* loop

This is the loop that produces the **nested** `overseer-obs:goal:blocked:…` fragments inside an outer `overseer-obs:` wrapper (the exact shape in the question blob).

| Hop | Site | Structural fact |
|---|---|---|
| write-back embeds a self-marker | `wiring.rs:1076-1091`, source_label `"overseer"` (`wiring.rs:952`) | episode content carries `[sig:overseer-obs:…]` — recoverable |
| recall has **no provenance filter** | `recall_episodic` `wiring.rs:1013-1031`; `parse_failure_signature` `wiring.rs:976-986` | the Overseer's own episode is recalled and its marker lifted as a `failure_signature`; `source_label=="overseer"` is **not** excluded |
| recur-detect fires at 2 | `signal.rs:455-470`, `RECURRING_SIGNATURE_THRESHOLD=2` (`signal.rs:362`) | two self-episodes sharing a signature ⇒ `Signal::RecurringSignature` — **this is the "2×"** |
| dedup_key **keeps** the prefix | `mod.rs:1353-1360`; `sanitize_recalled` `capabilities.rs:468-482` | sanitize strips control chars/length only — the `overseer-obs:` prefix survives |
| Orient folds it in as a Problem | `mod.rs:1210-1231` | the recall-derived `overseer-obs:…` key sits alongside fresh bare keys |
| **re-wrap** | `observation_signature` `mod.rs:1068-1073` | `format!("overseer-obs:{}", keys.join("\|"))` over keys that already contain `overseer-obs:…` ⇒ nesting |
| gate cannot brake it | `write_back_gate = WhisperGate::new(900,5)` `mod.rs:299`; `guardrails.rs:291-343` | gate suppresses only a **byte-identical** signature; every generation mutates shape ⇒ new key ⇒ delivered again |

**Architectural defect (D1):** a **feedback edge with no cut-vertex.** The loop Memory→Observe→Orient→Memory has no node that removes self-provenance. Two independent brakes are structurally absent: (a) a provenance filter at recall (`source_label=="overseer"` should be excluded, or self-episodes tagged non-recurrable), and (b) an **idempotent re-wrap** (`observation_signature` should not re-prefix a key that already starts with `overseer-obs:`). Either single edge closes the growth; today neither exists.

---

## 3. L2 — the missing unblock/resolution rung (Decide → Act for blocked goals)

The **rung exists** (`Intervention::UnblockGoal`, `mod.rs:1621`; capability `GoalCurator::unblock`, `capabilities.rs:419-427` — the exact `simard goal unblock` mutation). It is **structurally reachable but starved on two axes**, so `goal:blocked:*` re-parks every window.

### 3.1 Axis A — the WHY reasoner is double-gated and fails open to a bare park
`cycle.rs:582-702` — the ladder that classifies WHY a goal stalled and self-resolves it (auto-complete / heal precondition / defer upstream / spawn one guided engineer) runs **only if both**:
- **Gate A:** `memories.completion_evidence.is_some()` (`cycle.rs:582`) — else the whole block collapses to `Vec::new()` (`:700-702`): no classification, no ladder.
- **Gate B:** `no_progress_investigation_enabled()` (`cycle.rs:583`) — else the legacy verify-once park (`:684-698`).

When either gate is off, all stall classes collapse to the same **bare** `[OODA-SAFEGUARD] … needs human review` park. There is a re-investigation pass for already-bare-blocked goals (`reinvestigate_bare_blocked_goals`, `cycle.rs:627-636`), but it is **inside the same double-gate**, so it never runs when the gates are off. **No invariant binds a `Blocked` reason to a resolution class** — the rung has no guaranteed trigger.

### 3.2 Axis B — the escalation counter that *could* route to the rung is a starved ratchet
`decide_blocked_goal` (`mod.rs:1603-1631`) chooses `UnblockGoal` only in the `perpetual && is_no_progress_marker` arm; otherwise it escalates at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD (3)` (`root_cause.rs:33`). But `recurrence` is:
- **Starved** relative to the visible counter: the operator-visible `×2` (observation-episode lane) and the escalation `recurrence` (root-cause-fact lane, `record_occurrence`→`store_fact`, `mod.rs:1034`) are **two decoupled storage lanes**. The `×2` says nothing about whether the escalation lane reached 3 — a **cross-lane visibility gap / dead zone** (above one-off noise, below the escalation bar, no rung between).
- **A non-idempotent ratchet:** `record_occurrence` uses plain `store_fact` (append-only, `mod.rs:1034`), so once `recurrence≥3` the goal **latches to `EscalateBlockedGoal` forever** and can never fall back to `UnblockGoal` even after the condition clears. (Note: the reconciliation ledger §2 records that the naive `store_fact_with_caller_key` fix is a **trap** — it would collapse recall to 1 and make escalation dead code; the correct remedy is a count-in-content upsert. This is a *fix-design* caveat, not a change to the root-cause map.)

**Architectural defect (D2):** the Decide→Act **resolution edge for blocked goals is conditional on state that the loop itself starves.** The transition `Blocked → active` has no unconditional, evidence-bound trigger; it depends on `completion_evidence` being present and on a counter that lives on a different lane than the one the operator sees. Canonical incident: seven `kgpacks-rs` goals parked "no progress" when the work was already **done** (issues closed, PRs merged) — the safeguard read *done* as *stuck* and no rung reclassified them.

---

## 4. L3 — the dangling routing edge (Act → routing.rs never connected)

`WorkstreamCoverage` is the **only** High-priority Decide arm whose Act path terminates in a **notify-only** action.

- Decide: `ProblemKind::WorkstreamCoverage ⇒ Intervention::FlagWorkstreamGaps` (`mod.rs:1534-1543`) — no `LaunchRecipe` sibling (compare `StepFailure`→`LaunchRecipe` at `mod.rs:1565`).
- Act: `act_flag_workstream_gaps` (`mod.rs:884-948`) — **only** `notifier.notify(...)`. It files **no** issue, launches **no** workstream, and **never calls `stewardship::route_failure`**.
- `route_failure` (`routing.rs:39`) is reachable **only** through `process_orchestrator_run` (`stewardship/mod.rs:75`), driven by orchestrator run summaries — a path the Overseer gap flow never touches.

Yet `routing.rs:11-15` was **explicitly built** to accept the Overseer's coverage briefs: its `DEFAULT_TARGET_REPO` fallback docstring names *"the Overseer's `"overseer"` workstream-gap briefs"* so an unmatched source lands in `rysweet/Simard` instead of erroring. **The receiver exists; the caller edge was never wired.** So the gap is doubly quarantined — cut off from both `FileIssue` and `LaunchRecipe` — and re-emits `workstream-gap` every window with the **bare family key** `"workstream-gap"` (`mod.rs:1371`; the per-gap `signature` only reaches the summary/gate, so distinct gaps collapse to one recurring token in the composite).

**Architectural defect (D3):** a **missing edge from the Overseer's gap Act to the stewardship routing/issue-filing subsystem.** The remediation rung is a dangling `route_failure` waiting for a caller. **INV-GAP-KEY caveat:** any future wiring must key the ledger on `GapItem.signature`, **not** the bare `"workstream-gap"` dedup_key, or every gap folds into one issue.

---

## 5. Why the cluster co-occurs (one loop, two faces)

An under-resourced important goal **oscillates between two of these seams**: while active-but-uncovered it emits `workstream-gap` (L3, `sensor.rs:288-320`, blocked goals skipped at `:300-302`); once the no-progress breaker parks it, it leaves gap-scan and reappears as `goal:blocked` (L2). This is why the kgpacks-rs issues (12/17/18/23/25 + parity), the simard-identity personas, the coverage audit, and the coin harness appear in **both** recurring families and land in the same composite episode. They do **not** require one shared upstream dependency — they share **one shared structural cause: two non-closing rungs (L2, L3) feeding one non-braked observation loop (L1).** The issue-17 (ws2 int8/PQ embed) block is therefore best classified as a **real, persistent block that the loop cannot resolve**, amplified by an over-counting recurrence lane — i.e. *real block, artifact-inflated magnitude*, not a pure observation artifact.

---

## 6. Where a "resolution rung" should fire but does not (summary table)

| Loop | Missing/broken edge | Should fire | Live site | Why it doesn't |
|---|---|---|---|---|
| L1 self-ingestion | Recall provenance filter **and/or** idempotent re-wrap | cut the Memory→Observe self-edge | `wiring.rs:1013-1031`; `mod.rs:1068-1073` | no `source_label` exclusion; re-wrap re-prefixes an already-prefixed key |
| L2 blocked-goal | `Blocked → active` unblock rung | reclassify/self-heal false parks | `mod.rs:1621`; `cycle.rs:582-702` | double-gated on `completion_evidence` + kill-switch; escalation counter starved on a separate lane / latched ratchet |
| L3 workstream-gap | Act → `route_failure`/`FileIssue`/`LaunchRecipe` | file or launch the gap | `mod.rs:884-948`, `1534-1543`; `routing.rs:39` | Act is notify-only; `route_failure` receiver built but never called from this path |

---

## 7. Recommendations (understanding-oriented; investigation only, no code changed)

1. **Model the three defects as three missing graph edges, not one bug.** They are independent; fixing any one shrinks the composite but does not stop recurrence — L1 stops the *growth/nesting*, L2 stops *goal:blocked* re-parks, L3 stops *workstream-gap* re-emits.
2. **L1 fix shape:** add a self-provenance cut — exclude `source_label=="overseer"` at `recall_episodic`, **or** make `observation_signature` idempotent (do not re-prefix a key already starting with `overseer-obs:`). Either single edge closes the loop.
3. **L2 fix shape:** give the `Blocked → active` rung an **unconditional evidence-bound trigger** independent of `completion_evidence`, and unify the two counter lanes (or read the count-in-content, per the reconciliation ledger's corrected remedy) so the operator-visible `×2` and the escalation bar live on one axis. Avoid the `store_fact_with_caller_key` trap (ledger §2).
4. **L3 fix shape:** wire `act_flag_workstream_gaps` to the already-built `route_failure`/issue-filing (or `LaunchRecipe`) edge, keyed on `GapItem.signature` (INV-GAP-KEY), so a coverage gap becomes real work instead of a repeating notification.
5. **Out of scope (confirmed dead ends):** implementing the issue-17 int8/PQ embedding fix; counting exact signature repetitions from the truncated blob (treat "recurring" qualitatively); adding the dedup/unblock code itself; deep-diving issue-25 CVE content.

## 8. Verification performed
- Re-read and reproduced the key-assembly string at `mod.rs:1068-1073` (`observation_signature`) and the per-token emitters (`goal:blocked:` `mod.rs:1336`; `workstream-gap` `mod.rs:1371`).
- Traced the self-ingestion edge end-to-end in live source (`wiring.rs:1013-1031`, `signal.rs:455-470`, `mod.rs:1353-1360`, `capabilities.rs:468-482`).
- Confirmed the WHY double-gate and the starved unblock rung (`cycle.rs:582-702`, `mod.rs:1603-1631`, `1621`, `capabilities.rs:419-427`).
- Confirmed the dangling routing edge: `route_failure` callers are `process_orchestrator_run` only (`stewardship/mod.rs:75`), never the Overseer gap Act (`mod.rs:884-948`); `routing.rs:11-15` receiver anticipates `"overseer"` briefs.
- Cross-checked every verdict against `RECONCILIATION_LEDGER.md` (D1/D2/D3 geometry, dual-path coverage quarantine, INV-GAP-KEY, the `store_fact_with_caller_key` trap) — **consistent, extends without contradiction.**
