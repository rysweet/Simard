# Secondary Investigation — Areas 3 & 5: recurrence dead-zone (2×) + engineer_spawn drift

**Role:** Secondary (patterns) · **HEAD:** `25d4c5a6` · **Date:** 2026-07-16
**Verdict:** VALIDATED — prior corpus holds at live HEAD. The visible `×2` is an
**honest** cross-window re-observation, not a dedup/replay/collision/restart bug.
`resource:engineer_spawn` is **benign membership drift**, not a contradicting signal.
**Zero production-source drift.** Investigation-only; no fixes applied.

---

## 0. Drift ledger

- `git diff --name-only ea6ec554..HEAD -- '*.rs'` → **(empty).** The only commit since the
  last validated wave (`25d4c5a6`) is docs-only (twenty-third-wave test re-execution).
- `git diff --name-only 5a85317b..HEAD -- 'src/overseer/*.rs'` → only
  `src/overseer/tests_root_cause.rs` (+99, test-only; the two reinforcing lane tests from
  `f9cefec1`). **No production remediation has landed.** D1/D2/D3 remain OPEN.

---

## Area 3 — Recurrence semantics: why exactly `2×` (dead-zone verdict)

Re-verified against live source at HEAD `25d4c5a6`:

| Fact | Loc @ HEAD | Check |
|---|---|---|
| Lane A emit floor `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | ✅ |
| Lane A emits `RecurringSignature` at `occurrences >= 2`, counting recalled `failure_signature`s | `signal.rs:455-470` | ✅ |
| Operator string `"recurring signature seen {occurrences}× in cognitive memory ({signature})"` | `mod.rs:1360-1362` | ✅ |
| Lane B escalation floor `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | ✅ |
| Escalation gate `if recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `mod.rs:1613` (`decide_blocked_goal`) | ✅ |

**Dead-zone CONFIRMED.** `2` (emit floor) < `3` (escalation floor). The `2×` string sits
above one-off noise and below the escalation bar, and **no remediation rung** exists
between them in either the coverage loop or the park loop → the signal is stuck at `2×`
forever. This is the classic *recurrence dead-zone anti-pattern*.

**Two decoupled counter lanes (do NOT conflate):**
- **Lane A** — *observation episodes*: `store_episode`, +1 per ~900s window, threshold 2.
  This is the **visible `×2`** in the signature.
- **Lane B** — *root-cause occurrences*: `store_fact` (append-only), threshold 3, drives
  `mod.rs:1613` escalation. The `×2` operator string says **nothing** about Lane B.

**Empirical backing (all GREEN at HEAD):**
- `tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` — a
  loud Lane-A `×2` does NOT feed Lane-B recurrence (proves decoupling).
- `tests_root_cause::lane_b_escalates_without_any_lane_a_signal` — Lane B escalates
  independently of any Lane-A signal.
- `tests_root_cause::recurring_reblock_escalates_root_cause_not_blind_unblock`.
- `tests_memory_recall::recurring_signature_emitted_when_two_episodes_share_signature` /
  `recurring_signature_not_emitted_for_single_occurrence` — confirm the `>=2` emit floor.
- `tests_memory_recall::write_back_is_deduplicated_within_window` — within-window dedup works.

## WhisperGate lifetime / replay / restart hypotheses — REJECTED as the cause

- `WhisperGate.last_delivered` is an **in-memory, per-process** `HashMap<String,i64>`
  (`guardrails.rs:294`), initialised empty in `WhisperGate::new` (`:305`). It only dedups
  *within* the window (`:313` compares `now - last < window`; `:329` inserts).
- **No persistence, no cross-restart upsert.** A daemon restart clears `last_delivered`, so
  the next window **faithfully re-records** the still-unresolved problem set. The `×2` is a
  correct count of honest re-observation — **not** broken dedup, replay, or collision.
- **Storage-layer idempotency is absent by design** — the only gates are the in-memory
  WhisperGate and the within-window write-back gate; neither survives a restart.

**Root-cause attribution:** the defect is the **missing convergence rung**, not the counter.
The problem set persists because two observe-and-flag loops never close (see architect/area-4):
blocked goals bare-park with no WHY class (double-gated ladder, `cycle.rs:582-702`) and
workstream-gaps are notify-only (`WorkstreamCoverage` Decide arm has no launch edge).

## §6.2b remedy TRAP (still present, still wrong)

`store_fact_with_caller_key` (`library_adapter.rs`, `DedupMode::CallerKey`, "exactly one live
fact per key") would collapse `recurrence = recall.len()` to **1 forever** → escalation at
`mod.rs:1613` becomes dead code. **Correct remedy = count-in-content upsert** (persist an
`occurrence_count`/`first_seen`/`last_seen`; escalation reads the field, not `recall.len()`).
The gate fix and the counter fix must land **atomically**. Flag any verdict re-proposing the
bare one-liner.

---

## Area 5 — `resource:engineer_spawn` drift classification

Re-verified against live source:

| Fact | Loc @ HEAD | Check |
|---|---|---|
| dedup_key literal `"resource:engineer_spawn"` | `mod.rs:1270` (`EngineerSpawnRate` arm) | ✅ |
| `{live}` count goes to the **summary only** (`"elevated engineer spawn ({live} live)"`) | `mod.rs:1271` | ✅ |
| Capability dedup_key literal `"engineer_spawn"` | `capabilities.rs:562` | ✅ |
| Threshold-gated firing behaviour | `tests_m1::engineer_spawn_fires_at_and_above_threshold_only` (GREEN) | ✅ |

**Verdict: benign membership/composition drift — NOT a contradiction.** The token is a
**fixed literal key**; only its volatile `{live}` count varies, and that count lives in the
non-keyed summary. Its appearance in the second snapshot is a change of *which* problems were
present that cycle (composition delta), not a conflicting or corrupted signal. Because the
composite `observation_signature` sorts+dedups+`|`-joins dedup_keys (`mod.rs:1068-1073`),
adding/removing this literal is expected membership drift. It is **code-stable**: no source
drift touched this arm.

**Membership delta between the two passes:** the second observed snapshot adds the single
literal `resource:engineer_spawn` key; the `goal:blocked:*` / `workstream-gap` families are
otherwise stable. This is one under-throughput problem surfacing in an extra view, consistent
with the oscillation triangle (`workstream-gap ↔ goal:blocked ↔ resource:engineer_spawn`),
not an independent bug.

---

## Reconciliation with prior synthesis

Findings **reconcile fully** with `FINAL_SYNTHESIS.md`, `CONSOLIDATED_FINDINGS.md §14`,
`RECONCILIATION_LEDGER §2/§4`, and the prior secondary
(`secondary_D2_deadzone_ratchet_and_reconciliation_VALIDATE_HEAD_ea6ec554.md`). No divergence.
The only delta since baseline is the two reinforcing lane tests. HEAD drift check: **empty**.

## Questions for verification phase

1. **Gate B default in prod:** is `no_progress_investigation_enabled()` ON in the running
   daemon? If OFF, Lane B never accrues and escalation at `mod.rs:1613` is unreachable
   regardless of any counter fix.
2. **Gate A population:** is `memories.completion_evidence` `Some` on the ticks where these
   goals park? A `None` source silently disables the whole WHY ladder.
3. **Atomic landing:** the D2 fix must land the escalation-gate read AND a count-in-content
   upsert together; a caller-key upsert without count-in-content re-introduces the §6.2b
   dead-code trap.
