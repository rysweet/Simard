# TERTIARY (architect) — reconciled verdict + minimal landing-safe remediation @ HEAD `3e6b6933`

- **Role:** Tertiary investigator (architecture / structural focus).
- **Mandate:** Reconcile prior `ai_working/investigation/` waves (23+) and design a **minimal,
  landing-safe** remediation (blocked-goal unblock rung / dedup gate). Do **not** re-derive the
  stable corpus; do **not** touch `kgpacks-rs` issue-17 (observed target, not the subject).
- **HEAD:** `3e6b6933`. **Production `.rs` drift since baseline `6e3113bc`: NONE**
  (`git diff --name-only 6e3113bc..HEAD -- '*.rs'` = `src/overseer/tests_root_cause.rs` only —
  test-only, additive). Every production citation below re-ground **byte-for-byte in live source
  by this investigator**, not trusted from the docs.

---

## 0. Bottom line (split verdict — CONFIRMED, no divergence from corpus)

- **The `2×` count / the signature → NO FIX.** `overseer-obs:…|goal:blocked:fix-agent-kgpacks-rs-
  issue-17…|resource:engineer_spawn|workstream-gap` "seen 2×" is an **honest cross-window
  re-observation tally** of a static, unresolved problem set. `2×` is the escalation *trigger
  threshold* `RECURRING_SIGNATURE_THRESHOLD = 2` (signal.rs:362), **not** a dedup/idempotency/
  replay/collision defect. H0 rejected, H1 confirmed. Suppressing the counter would hide a true
  signal.
- **The condition it fingerprints → MINIMAL, ADDITIVE FIX.** The signature recurs forever because
  two OODA loops Observe/Decide but never **Close**, and the count parks in a `[2,3)` dead zone
  between two decoupled counter lanes. Landing order **D2 → D3 → D1** (+ optional durable-gate);
  each edit is additive and none rewrites the OODA architecture.

The corpus is at a **fixpoint.** My contribution is the independent HEAD re-grounding + the
consolidated, drift-reconciled landing decision — not another re-observation.

---

## 1. Independent re-grounding at HEAD `3e6b6933` (all ✅ read live by this investigator)

| Load-bearing claim | Cited loc | Verified live |
|---|---|---|
| `observation_signature` = `sort_unstable()→dedup()→"overseer-obs:"+join("\|")` | `mod.rs:1068‑1073` | ✅ exact |
| Operator string == the investigation question, verbatim | `mod.rs:1361` | ✅ `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` |
| `RECURRING_SIGNATURE_THRESHOLD = 2` (Lane A emit floor), fires at `>=` | `signal.rs:362,463` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B escalate floor) | `root_cause.rs:33` | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps` (no launch edge) | `mod.rs:1534‑1543` | ✅ exact |
| `decide_blocked_goal` ladder; sub-threshold falls through to `Report` no-op | `mod.rs:1603‑1631` (esc `:1613`, unblock `:1621`, `Report` `:1630`) | ✅ exact |
| **D2 site:** `ActOutcome::Reported` **ABSENT** from `outcome_records_occurrence` | `wiring.rs:612‑627` | ✅ exact (set = Launched/Merged/Deployed/IssueFiled/Escalated/Whispered/GoalUnblocked/GoalEscalated/ConflictResolved/GoalTransferred/Audited — **no `Reported`**) |

**Drift verdict:** prior tertiary synthesis is **confirmed at HEAD, not superseded.**

---

## 2. Reconciled structural cause (three decoupled defects, one condition)

The signature is a faithful fingerprint of a **Decide→Act loop that never closes** on two
High-priority arms, made invisible-to-remediation by a two-lane threshold split:

- **D2 — Loop-A dead zone (blocked goals).** `decide_blocked_goal` has **no arm for a plain
  sub-threshold block**: recurrence `1→2` is below the `>=3` escalation bar and, if not
  `perpetual` no-progress and not `needs_review`, falls to `Intervention::Report` (`mod.rs:1630`),
  a passive no-op. Because `Report → ActOutcome::Reported` is **absent** from
  `outcome_records_occurrence` (`wiring.rs:612‑627`), Lane-B `recurrence` **never accrues** — so
  the `>=3` rung is unreachable for these blocks. The `[2,3)` interval is an **absorbing dead
  zone**. (Ordering hazard T1b also holds: escalation gate at `:1613` shadows the self-heal
  `UnblockGoal` at `:1621`, and the append-only Lane-B ratchet makes a `>=3` latch permanent.)
- **D3 — Loop-B missing closing edge (workstream gaps).** `WorkstreamCoverage` is the **only**
  High Decide arm terminating in a notify-only Act (`FlagWorkstreamGaps`, `mod.rs:1534‑1543`;
  Act `act_flag_workstream_gaps`, `mod.rs:884‑948` → `notifier.notify` only). Siblings
  (`StepFailure`, `ProcessHealth`, `CrossCutting`) all reach `LaunchRecipe`/`FileIssue`. The
  built receiver `route_failure` (`routing.rs:39`) is never wired to this path. The gap is
  terminal and re-emits the bare `workstream-gap` token every window.
- **D1 — self-referential write-back (fingerprint hygiene).** Recall-derived `RecurringSignature`
  is admitted back with `sanitize_recalled(signature)` (`mod.rs:1353‑1363`), nesting
  `overseer-obs:…|overseer-obs:…` runs. Cosmetic amplifier of the signature, not the cause.

The `workstream-gap ↔ goal:blocked ↔ resource:engineer_spawn` triad is **one under-throughput
condition, three views**: an under-resourced goal oscillates between "uncovered gap" (active) and
"parked block" (post-breaker); engineers spawn yet throughput stalls. `resource:engineer_spawn`
is benign membership drift (its `{live}` count lands only in the summary, never the key).

---

## 3. Minimal, landing-safe remediation (order D2 → D3 → D1; each additive)

Land **closing edges first** (D2, D3) so the loops actually drain, then the **fingerprint filter**
(D1) last — landing D1 early would mask a still-open loop by trimming the signature.

### D2 — close the Loop-A dead zone (LAND FIRST; ~1 line, lowest risk)
- **Edit:** add `ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612‑627`).
- **Effect:** a sub-threshold bare-park records an occurrence per act, so Lane-B `recurrence`
  climbs toward the **existing** `>=3` gate (`mod.rs:1613`) instead of latching at 0. Reaches the
  operator via already-built machinery.
- **Landing-safe:** no test pins the exclusion; the first observation still Reports.
- **TRAP — do NOT take:** the committed §6.2b one-liner
  `store_fact_with_caller_key(root_cause_signature(...))` at `mod.rs:1034` collapses
  `recall.len()` to **1 forever** (`DedupMode::CallerKey` keeps one live fact/key) → makes `>=3`
  **dead code**. If the append-ratchet is de-ratcheted at all, use a **count-in-content upsert**
  (`occurrence_count`/`first_seen`/`last_seen`, escalation reading the field). For the minimal
  fix, D2's sink inclusion alone suffices.
- **Regression test:** a bare-park goal observed across ≥3 acts reaches `EscalateBlockedGoal`
  (not perpetual `UnblockGoal`); a single observation still does not.

### D3 — add the Loop-B launch/file rung (ADD alongside, never swap)
- **Edit:** give `WorkstreamCoverage` a recurrence-aware **additive** Decide edge:
  `1× Notify / ≥2× LaunchRecipe (or route_failure→FileIssue) / ≥3× Escalate`.
- **Landing-safe:** **never replace** `FlagWorkstreamGaps` — it is hard-asserted at
  `tests_gap_scan.rs:860‑870`. Add the new edge next to the existing notify.
- **TRAP — INV-GAP-KEY:** key the cross-window ledger on `GapItem.signature` (used at
  `mod.rs:901,932`), **not** the bare `"workstream-gap"` dedup_key (`mod.rs:1371`), or all gaps
  fold into one issue.
- **Regression test:** a gap recurring ≥2 windows produces a `LaunchRecipe`/route brief keyed on
  `GapItem.signature`; the `FlagWorkstreamGaps` notify still fires at 1×.

### D1 — collapse self-nesting without suppressing the signal (LAND LAST)
- **Edit:** self-provenance filter in `write_back_observation` (`mod.rs:534‑563`) dropping
  `overseer-obs:`-keyed, recall-derived problems before `observation_signature`, and/or make the
  wrap idempotent (never re-prefix an already-`overseer-obs:` key).
- **Effect:** the composite reflects the true distinct problem set without touching the honest
  cross-window `×N` counter.
- **Regression test:** a genuine (non-self) cross-window `RecurringSignature` at `occurrences>=2`
  is preserved; only self-nested keys are collapsed.

### Durable-gate — restart hardening (independent; may land with D2)
- **Edit:** persist `WhisperGate.last_delivered` (`guardrails.rs:294`) across daemon restarts.
- **Why:** the map is in-memory/per-process; a restart clears it and the still-true condition
  re-records — a probable source of *exactly* `2×`. Additive behind peek→store→commit; falls back
  to in-memory if the durable store is unavailable.

---

## 4. Reconciliation & dead-ends avoided

| Check | Result |
|---|---|
| Production `.rs` drift `6e3113bc..HEAD` | **PASS** — only `tests_root_cause.rs` (additive) |
| Load-bearing citations re-ground verbatim @ `3e6b6933` (independent read) | **PASS** (§1) |
| Question string == `mod.rs:1361` verbatim | **PASS** |
| D2 site (`Reported` absent) live | **PASS** (`wiring.rs:612‑627`) |
| D3 pin (`FlagWorkstreamGaps`) live | **PASS** (`mod.rs:1534‑1543`) |
| §6.2b CallerKey remedy flagged as trap; count-in-content is the correction | **PASS** |
| Divergence vs 23+ prior waves | **NONE** — confirmation only |

**Dead-ends avoided:** did not audit the ~31 individual delivery PRs (signature tokens, not the
cause); did not touch `kgpacks-rs` issue-17 int8/PQ-embed internals; did not hunt a within-window
dedup bug (proven green); did not propose an OODA-architecture rewrite; did not re-derive the
stable corpus.

**Investigation: COMPLETE (fixpoint re-validated @ `3e6b6933`). Remediation: NOT STARTED —
sole open items are to land D2 (one line, test-safe), then D3 (add-alongside), durable-gate, and
D1 (last).**
