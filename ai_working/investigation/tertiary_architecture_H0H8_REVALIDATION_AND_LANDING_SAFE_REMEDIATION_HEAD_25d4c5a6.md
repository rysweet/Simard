# Tertiary (Architect) — H0–H8 revalidation @ HEAD `25d4c5a6` + landing-safe remediation

**Role:** Tertiary investigator (architect). **Mandate:** revalidate the H0–H8 verification
suite at HEAD `25d4c5a6`, reconcile against the `ai_working/investigation/` prior waves
(23+), and produce a **landing-safe remediation recommendation**. Do **not** fix `kgpacks-rs`
issue-17 — it is the observed target, not the subject.

**HEAD:** `25d4c5a6fabe6c83fcaa5fa8a16b41044aa10721`
**Method:** VALIDATE-don't-re-derive. Every load-bearing citation was re-read byte-for-byte
in live source; the named empirical tests were re-executed. **Verdict: CONFIRMED — the
corpus holds at HEAD with zero production drift. No net-new divergence.**

---

## 0. Bottom line (split verdict — re-validated, unchanged)

- **The signature / the `×2` count → NO FIX.** `overseer-obs:…|goal:blocked:…issue-17…|
  workstream-gap` seen `2×` is an **honest cross-window re-observation tally** of a static,
  unresolved problem set. The `2×` is a dedup *counter over re-emitted identical keys*
  (H0 rejected: not a double-read, `dedup()` collapse, hash collision, or cross-store
  duplication). Suppressing the counter would hide a true signal.
- **The condition the signal indicates → MINIMAL, ADDITIVE FIX.** The signature recurs
  forever because two OODA loops Observe/Decide but never **Close**, and the count parks in
  a `[2,3)` dead zone. Landing order **D2 → D3 → D1** (+ optional durable-gate), each additive,
  none rewriting overseer architecture.

The corpus is at a **fixpoint**; my contribution is the HEAD revalidation + the drift-reconciled,
landing-safe decision — not another re-observation.

---

## 1. HEAD drift check (PASS — zero production drift)

```
git rev-parse HEAD                         → 25d4c5a6
git diff --name-only ea6ec554..HEAD        → (docs only; ai_working/investigation/*)
git diff --name-only 5a85317b..HEAD -- '*.rs' → src/overseer/tests_root_cause.rs  (ONLY)
```

The single `.rs` delta is **test-only and additive** (the two reinforcing lane-decoupling
proofs). **No production source changed** since the last synthesis. Every `src/overseer/*`
citation in the corpus holds verbatim.

---

## 2. H0–H8 revalidation at HEAD (each verdict re-confirmed)

### 2.1 Load-bearing citations re-grounded live (all ✅ exact @ `25d4c5a6`)

| Claim | Loc | Re-read @ HEAD |
|---|---|---|
| `observation_signature` = `sort_unstable()→dedup()→"overseer-obs:"+join("\|")` | `mod.rs:1068‑1073` | ✅ exact |
| Base member key `format!("goal:blocked:{goal_id}")` | `mod.rs:1336` | ✅ exact |
| Operator string == the investigation question, verbatim | `mod.rs:1361` | ✅ `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` |
| `workstream-gap` literal family key | `mod.rs:1371` | ✅ exact |
| `write_back_gate = WhisperGate::new(900, 5)` | `mod.rs:299` | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps` (no launch edge) | `mod.rs:1534‑1544` | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `mod.rs:1613` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2` (Lane A emit floor) | `signal.rs:362` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B escalate floor) | `root_cause.rs:33` | ✅ exact |
| **D2 site:** `ActOutcome::Reported` **ABSENT** from `outcome_records_occurrence` | `wiring.rs:612‑627` | ✅ exact (list = Launched/Merged/Deployed/IssueFiled/Escalated/Whispered/GoalUnblocked/GoalEscalated/ConflictResolved/GoalTransferred/Audited — no `Reported`) |
| **D3 landing pin:** `decide` hard-asserted to yield `FlagWorkstreamGaps` | `tests_gap_scan.rs:860‑870` | ✅ exact |

### 2.2 Empirical re-execution (all GREEN @ HEAD)

- Full overseer lib suite: **361 passed, 0 failed** (`cargo test -p simard --lib overseer::`).
- Named, hypothesis-backing tests (all `ok`):
  - `tests_memory_recall::write_back_is_deduplicated_within_window` → **H0 REJECTED** (within-window gate works; no double-read).
  - `tests_memory_recall::recurring_signature_emitted_when_two_episodes_share_signature` + `…_not_emitted_for_single_occurrence` → **H1/H5** emit-floor `>=2` confirmed.
  - `tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` → **H5** Lane A ⇏ Lane B (decoupled).
  - `tests_root_cause::lane_b_escalates_without_any_lane_a_signal` → **H5** Lane B independent.
  - `tests_root_cause::recurring_reblock_escalates_root_cause_not_blind_unblock` → **H2/H6** escalation is root-cause-aware, not blind unblock.

### 2.3 Hypothesis status (reconciled — no change from corpus)

| ID | Hypothesis | Verdict @ HEAD |
|----|-----------|----------------|
| H0 | Dedup/storage/replay/collision artifact | **REJECTED** (within-window dedup green) |
| H1 | Real re-observation of near-static set → the `×2` | **CONFIRMED** |
| H2 | WHY reasoner double-gated → bare parks (`goal:blocked:*`) | **CONFIRMED** |
| H3 | `WorkstreamCoverage` has no closing/launch edge | **CONFIRMED** (`mod.rs:1534‑1544` notify-only) |
| H4 | Self-observation write-back nests `overseer-obs:` tokens (D1) | **CONFIRMED** |
| H5 | `[2,3)` dead zone across two decoupled lanes | **CONFIRMED** (2 emit vs 3 escalate; `Reported` absent from occurrence sink) |
| H6 | Non-idempotent counters (compounding, non-causal) | **CONFIRMED** (amplifier, not cause) |
| H7 | `blocked ↔ gap` = one problem, two views | **CONFIRMED** |
| H8 | Three token families = one under-throughput condition | **CONFIRMED** (med-high) |

**Divergence vs prior 23 waves: NONE.** Confirmation only. The only delta since baseline is
additive test-only corroboration.

---

## 3. The two non-closing loops (structural cause of the recurrence)

Both are textbook non-closing OODA loops — Observe→Orient→Decide→**Act-that-does-not-mutate-
the-observed-state** — so the next Observe faithfully re-records the identical condition:

- **Loop A (blocked goals).** `decide_blocked_goal` (`mod.rs:1603‑1631`) has **no arm for a plain,
  sub-threshold block**: recurrence 1→2 is below the `>=3` escalation bar and the block carries
  no perpetual/needs-review marker, so it falls to `else → Report` (a no-op labeled
  `Remediation::acknowledged()`). Worse, `Report → ActOutcome::Reported` is **absent** from
  `outcome_records_occurrence` (`wiring.rs:612‑627`), so Lane-B `recurrence` never accrues and
  the `>=3` rung is effectively unreachable for these blocks → the **absorbing `[2,3)` dead zone**.
- **Loop B (workstream gaps).** `WorkstreamCoverage` is the only High Decide arm with no
  `launch.rs`/`FileIssue` edge — notify-only `FlagWorkstreamGaps` (`mod.rs:1534‑1544`), labeled
  `Remediation::root_cause()`, so telemetry reports "remediated" while no work is created. The
  gap re-surfaces every tick → `workstream-gap` stays in every composite.

The `workstream-gap → goal:blocked` coupling (H7): an under-resourced goal oscillates between
"uncovered gap" and "parked block," so the same goals appear in both recurring families and
neither loop drains — a self-feeding steady state.

---

## 4. Landing-safe remediation (D2 → D3 → D1; each additive)

Ordering rationale: land the **closing edges first** (D2, D3) so the loops actually drain, then
land the **fingerprint filter** (D1) last — D1 trims the signature, so landing it early would
mask a still-open loop.

### D2 — close the Loop-A dead zone (LAND FIRST; one line, lowest risk)
- **Edit:** add `ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612‑627`).
- **Effect:** a sub-threshold bare-park then records an occurrence per act, so Lane-B
  `recurrence` climbs toward the **existing** `>=3` escalation gate (`mod.rs:1613`) instead of
  latching at 0. Reaches the operator via already-built machinery.
- **Landing-safe:** no test pins the exclusion; the first observation still Reports.
- **TRAP (do NOT take):** the committed §6.2b one-liner
  `store_fact_with_caller_key(root_cause_signature(...))` at `mod.rs:1034` collapses
  `recall.len()` to **1 forever** (`DedupMode::CallerKey`, one live fact/key) → makes `>=3`
  dead code. If the append-ratchet is addressed at all, use a **count-in-content upsert**
  (`occurrence_count`/`first_seen`/`last_seen`, escalation reading the field), never the literal
  CallerKey swap. For the minimal fix, D2's sink inclusion alone suffices.
- **Regression test:** a bare-park goal observed across ≥3 acts reaches `EscalateBlockedGoal`
  (not perpetual `UnblockGoal`); a single observation still does not.

### D3 — add the Loop-B launch/durable rung (ADD alongside, never swap)
- **Edit:** give `WorkstreamCoverage` a recurrence-aware **additive** Decide edge:
  `1× Notify / ≥2× LaunchRecipe (via built stewardship::route_failure) / ≥3× Escalate`.
- **Landing-safe:** **never replace** `FlagWorkstreamGaps` — it is hard-asserted at
  `tests_gap_scan.rs:860‑870`. Add the new edge next to the existing notify.
- **TRAP (INV-GAP-KEY):** key the cross-window ledger on `GapItem.signature`, **not** the bare
  `"workstream-gap"` dedup_key (`mod.rs:1371`), or all gaps fold into one issue.
- **Regression test:** a gap recurring ≥2 windows produces a `LaunchRecipe`/route brief keyed
  on `GapItem.signature`; the `FlagWorkstreamGaps` notify still fires at 1×.

### D1 — collapse self-nesting without suppressing the signal (LAND LAST)
- **Edit:** a self-provenance filter in `write_back_observation` (`mod.rs:534‑563`) dropping
  `overseer-obs:`-keyed, recall-derived problems before `observation_signature`, and/or make the
  wrap idempotent (never re-prefix an already-`overseer-obs:` key).
- **Effect:** the composite reflects the true distinct problem set (`overseer-obs:|overseer-obs:`
  nesting stops) **without** touching the honest cross-window `×N` counter.
- **Landing-order safety:** land **after** D2/D3 so the closing edges are already draining the
  loop; otherwise D1 masks a still-open loop by trimming the fingerprint.
- **Regression test:** a genuine (non-self) cross-window `RecurringSignature` at `occurrences>=2`
  is **preserved**; only self-nested `overseer-obs:`-derived keys are collapsed.

### Durable-gate — restart hardening (independent; may land with D2)
- **Edit:** persist `WhisperGate.last_delivered` (`guardrails.rs:294`) across daemon restarts.
- **Why:** the map is in-memory/per-process; a restart clears it and the still-true condition
  re-records — a probable source of *exactly* `2×`. Additive behind peek→store→commit; falls
  back to in-memory if the durable store is unavailable.

---

## 5. Reconciliation & dead-ends avoided

| Check | Result |
|---|---|
| Production `.rs` drift `5a85317b..HEAD` | **PASS** — only `tests_root_cause.rs` (additive) |
| Load-bearing citations re-ground verbatim @ `25d4c5a6` | **PASS** (§2.1) |
| Question string == `mod.rs:1361` verbatim | **PASS** |
| Full suite + 6 named hypothesis tests green | **PASS** (§2.2) |
| D2 site (`Reported` absent) / D3 pin (`FlagWorkstreamGaps`) | **PASS** — both live |
| §6.2b CallerKey remedy flagged as trap | **PASS** — count-in-content is the correction |
| Divergence vs 23 prior waves | **NONE** — confirmation only |

**Dead-ends avoided:** did not touch `kgpacks-rs` issue-17; did not treat the inflated inline
pipe-repetition as a literal count (authoritative count = `2×`, a dedup counter); did not hunt a
within-window dedup bug (proven green); did not re-derive the stable corpus.

**Investigation: COMPLETE (fixpoint re-validated @ `25d4c5a6`). Remediation: NOT STARTED —
sole open item is to land D2 (one line, test-safe), then D3 (add-alongside), durable-gate, and
D1 (last).**
