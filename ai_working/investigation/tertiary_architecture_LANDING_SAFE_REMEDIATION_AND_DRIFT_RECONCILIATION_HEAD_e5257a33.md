# Tertiary (Architect) — Minimal landing-safe remediation + HEAD source-drift reconciliation

**Role:** Tertiary investigator (architect). **Mandate:** produce a minimal, landing-safe
remediation (dedup collapse *without* signal suppression + missing launch rung + durable gate)
and reconcile all findings against the existing investigation corpus with a HEAD source-drift
check. **Do not** touch `kgpacks-rs` issue-17 — it is the observed target, not the subject.

**HEAD:** `e5257a33` (twenty-first wave, §29). **Date:** 2026-07-16.
**Method:** VALIDATE-don't-re-derive. Every load-bearing citation below was re-read
byte-for-byte in live source at HEAD; the overseer test suite was re-run.

---

## 0. Bottom line (split verdict — unchanged, re-validated at HEAD)

- **Signature / the `×2` count → NO FIX.** `overseer-obs:goal:blocked:…-7f5afcca` seen `2×`
  is an **honest cross-window re-observation tally** of a static, unresolved problem set —
  prefix *nesting*, not duplication; not a dedup/storage/replay/collision defect. Suppressing
  the counter would hide a true signal.
- **Response the signal indicates → MINIMAL FIX (three additive edges + one durability edge).**
  The signal recurs forever because the OODA loop Observes and Decides but never Closes. The
  landing-safe remediation is **L0 → D2 → D3 → D1**, plus optional **durable-gate** hardening,
  each additive and none rewriting overseer architecture.

The corpus is at a **fixpoint across 21 waves**; my contribution is the decision + the
drift-reconciled fix spec, not another re-observation.

---

## 1. HEAD source-drift check (PASS — zero production drift)

```
git rev-parse HEAD                     → e5257a33
git diff --name-only 6e3113bc..HEAD -- '*.rs'  → src/overseer/tests_root_cause.rs   (ONLY)
```

The single `.rs` delta is **test-only and additive**: two net-new decoupling proofs
(`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`,
`lane_b_escalates_without_any_lane_a_signal`) that **corroborate** the two-lane finding
(Lane A episodic recall floor `2` vs Lane B root-cause escalation floor `3`, sharing no
counter). **No production source changed.** Every `src/overseer/*` line citation in the corpus
(`FINAL_SYNTHESIS.md`, `RECONCILIATION_LEDGER.md`, prior tertiary reports) holds verbatim.

### 1.1 Load-bearing citations re-grounded at `e5257a33` (all ✅ exact)

| Claim | Cited loc | Re-read @ HEAD |
|---|---|---|
| `observation_signature` = `sort_unstable()→dedup()→"overseer-obs:"+join("\|")` — sole prefix producer | `mod.rs:1068-1073` | ✅ exact |
| Base key `format!("goal:blocked:{goal_id}")` | `mod.rs:1336` | ✅ exact |
| `RecurringSignature` admission via `sanitize_recalled`; summary string verbatim | `mod.rs:1353-1363` (`:1361`) | ✅ `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` |
| `workstream-gap` literal family key; `gaps.len()` → summary only | `mod.rs:1371` | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps` (no launch edge) | `mod.rs:1534-1543` | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `mod.rs:1613` | ✅ exact |
| `record_occurrence` writes via non-idempotent `store_fact` (append ratchet) | `mod.rs:1034` | ✅ exact |
| `write_back_gate = WhisperGate::new(900,5)`; `gap_gate = WhisperGate::new(900,200)` | `mod.rs:299,304` | ✅ exact |
| `write_back_observation` single site; recall-gated; commit-after-store | `mod.rs:534` / `wiring.rs:301` | ✅ exact |
| `record_observation → store_episode(content,"overseer",{sig})` | `wiring.rs:1076-1091` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD=2`; emit at `occurrences>=2`; recall loop | `signal.rs:362,463,455-469` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD=3`; `root_cause_signature="{dedup_key}::{label}"` | `root_cause.rs:33,53-55` | ✅ exact |
| `recurrence = recall.filter(cause_label==primary).count()` (CallerKey-trap basis) | `root_cause.rs:79-82` | ✅ exact |
| `last_delivered: HashMap<String,i64>` — in-memory / per-process gate state | `guardrails.rs:294` | ✅ exact |
| `outcome_records_occurrence` list — **`ActOutcome::Reported` ABSENT** (D2 site) | `wiring.rs:612-627` | ✅ exact (Reported used at `:420,:545`, never in this list) |
| `FlagWorkstreamGaps` hard-pinned (D3 landing-safety) | `tests_gap_scan.rs:865-868` | ✅ exact |

### 1.2 Test re-execution at HEAD (green)

- `cargo test --lib overseer::` → **361 passed, 0 failed** (matches corpus).
- Named, `--exact`:
  - `overseer::tests_memory_recall::write_back_is_deduplicated_within_window` → **ok** (within-window dedup proven).
  - `overseer::tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` → **ok** (Lane A ⇏ Lane B).
  - `overseer::tests_root_cause::lane_b_escalates_without_any_lane_a_signal` → **ok** (Lane B independent).

---

## 2. Minimal, landing-safe remediation

Whole-loop order: **L0 → D2 → D3 → D1** (durable-gate is an independent hardening edge that may
land with D2). Each is additive.

### L0 — prerequisite (no store change)
Ensure bare parks carry their real WHY down the ladder (WHY-reasoner wiring). Per the
twentieth-wave drift correction the no-progress investigation is **default-on**, so L0 is
narrow: the `completion_evidence` **Gate A** still admits bare parks. **Do not** blind
`unblock-all` (operator-rejected antipattern, `mod.rs:1588`, `:1620-1621`).

### D2 — missing remediation rung between count 2 and escalation bar 3 (Lane B) — LAND FIRST
**Edit:** add `ActOutcome::Reported` to `outcome_records_occurrence` (`wiring.rs:612-627`).
**Why:** a bare park routes Rung-4 `Report → Reported`, which today records **no occurrence**,
so Lane-B `recurrence` never leaves `0` and the `>=3` escalation rung (`mod.rs:1613`) is dead
code — the absorbing `[2,3)` dead zone. Including `Reported` lets recurrence climb toward the
**existing** gate.
**Landing-safe:** no test pins the exclusion; the first observation still Reports; one-line,
lowest-risk.
**TRAP (do not take):** the committed §6.2b one-liner
`store_fact_with_caller_key(root_cause_signature(...))` at `mod.rs:1034` collapses recall to
**1 forever** (`DedupMode::CallerKey` keeps one live fact/key, `library_adapter.rs:885-889`;
`recurrence = recall.len()`, `root_cause.rs:79-82`) → makes `>=3` dead code. If the ratchet is
addressed, use a **count-in-content upsert** (`occurrence_count`/`first_seen`/`last_seen`,
escalation reading the field), never the literal CallerKey swap. For the *minimal* fix, D2's
sink inclusion alone suffices; the ratchet correction is optional/secondary.
**Regression test to add:** a bare-park goal observed across ≥3 acts reaches
`EscalateBlockedGoal` (not perpetual `UnblockGoal`); a single observation still does not.

### D3 — missing launch rung for WorkstreamCoverage (Act→routing edge)
**Edit:** give `WorkstreamCoverage` a recurrence-aware **additive** Decide arm:
`1× Notify / ≥2× LaunchRecipe (via already-built stewardship::route_failure) / ≥3× Escalate`.
**Why:** `WorkstreamCoverage` is the only High Decide arm with no `launch.rs`/`FileIssue` edge —
notify-only `FlagWorkstreamGaps` (`mod.rs:1534-1543`); `route_failure` receiver is built but the
Overseer-gap caller edge is never wired.
**TRAP (INV-GAP-KEY):** key the cross-window ledger on `GapItem.signature`, **not** the bare
`"workstream-gap"` dedup_key (`mod.rs:1371`), or all gaps fold into one issue.
**Landing-safe:** **never swap** `FlagWorkstreamGaps` — hard-asserted at `tests_gap_scan.rs:865`.
Add the new edge **alongside** the existing notify.
**Regression test to add:** a gap recurring ≥2 windows produces a `LaunchRecipe`/route brief
keyed on `GapItem.signature`; the `FlagWorkstreamGaps` notify still fires at 1×.

### D1 — dedup collapse without signal suppression (Memory→Observe nesting) — LAND LAST
**Edit:** a single-function self-provenance filter in `write_back_observation`
(`mod.rs:534-563`) that drops `overseer-obs:`-keyed, recall-derived problems before
`observation_signature`, **and/or** make the re-wrap idempotent (never re-prefix an
already-`overseer-obs:` key).
**Why:** collapses the `overseer-obs:|overseer-obs:` / `workstream-gap|workstream-gap` nesting
so the composite reflects the **true distinct problem set** — **without** touching the honest
cross-window `×N` counter (no signal suppression).
**Landing-order safety (store boundary):** D1 changes what future recall sees. Land it **after**
D2/D3 so their closing edges are already draining the loop; otherwise D1 masks a still-open loop
by trimming the fingerprint without closing the cause.
**Regression test to add:** genuine (non-self) cross-window `RecurringSignature` at
`occurrences>=2` is **preserved**; only self-nested `overseer-obs:`-derived keys are collapsed.

### Durable-gate — daemon-restart hardening (independent; may land with D2)
**Edit:** persist the `WhisperGate.last_delivered` map (`guardrails.rs:294`) across daemon
restarts (durable-back the `write_back_gate`, and Lane-B accrual state).
**Why:** the `last_delivered` map is in-memory/per-process; after a restart it starts empty and
the still-true condition **re-records** — the most probable source of *exactly* `2×`. Durability
eliminates the restart-induced re-record at source **without** suppressing legitimate
cross-window (>900 s) re-observations.
**Landing-safe:** additive persistence behind the existing peek→store→commit; if the durable
store is unavailable it falls back to in-memory (current behavior). Guard with a test that a
reconstructed gate suppresses an immediate re-record inside the window but admits it after the
window elapses.

---

## 3. Corpus reconciliation (PASS — verdict holds, no contradiction, no drift)

| Check | Result |
|---|---|
| Production `.rs` drift `6e3113bc..HEAD` | **PASS** — only `tests_root_cause.rs` (additive, corroborating) |
| Every load-bearing citation re-grounds verbatim @ `e5257a33` | **PASS** (§1.1) |
| Question string == `mod.rs:1361` verbatim | **PASS** |
| Within-window dedup green | **PASS** (`write_back_is_deduplicated_within_window`) |
| Lane A ⇏ Lane B; Lane B independent | **PASS** (two net-new tests) |
| §29/§28 verdict (NO-FIX signature / MINIMAL-FIX response, D2→D3→D1) | **PASS** — my spec matches; no re-derivation |
| §6.2b CallerKey remedy flagged as trap | **PASS** — preserved as trap; count-in-content is the correction |
| D1/D2/D3 unmerged at HEAD | **PASS** — all three still open; sole action item is **land D2** |

No source drift invalidates any citation. The prior verdict is **confirmed**, not re-derived.

---

## 4. Dead-ends explicitly avoided
- Did **not** touch `kgpacks-rs` issue-17 (observed target, not subject).
- Did **not** treat inflated inline pipe-repetition as literal count (authoritative count = `2×`).
- Did **not** hunt a within-window dedup bug (proven green); the gaps are cross-lane visibility,
  per-process gate state, and the missing closing/launch edges.
- Did **not** re-derive the stable corpus verdict — validated citations instead.

**Investigation: COMPLETE (fixpoint, re-validated @ `e5257a33`). Remediation: NOT STARTED —
sole open item is to land D2 (one line, test-safe), then D3, durable-gate, and D1.**
