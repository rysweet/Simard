# Secondary Investigation — D2 recurrence dead-zone + append-only ratchet, reconciled

**Role:** Secondary (patterns) · **HEAD:** `ea6ec554` · **Date:** 2026-07-16
**Verdict:** VALIDATED — D2 (and D1/D3) still OPEN; **zero production-source drift**; the
committed §6.2b remedy remains a TRAP. No production `.rs` remediation has landed.

---

## 0. Drift ledger (remediation status)

- `git diff --name-only e5257a33..HEAD -- '*.rs'` → **(empty).** `e5257a33..HEAD` is
  docs-only (4 files under `ai_working/investigation/`). HEAD advanced from the
  strategy-pinned `e5257a33` → `ea6ec554` with **no code change**.
- `git diff --name-only 5a85317b..HEAD -- '*.rs'` → **only** `src/overseer/tests_root_cause.rs`
  (+99, test-only). The lone commit `f9cefec1` adds two tests
  (`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`,
  `lane_b_escalates_without_any_lane_a_signal`) that **reinforce** the two-lane
  decoupling verdict — they are net-new verification, **not** a production guard.
- **Conclusion:** D1/D2/D3 are all OPEN at HEAD `ea6ec554`. Prior verdicts stand.

---

## D2 — dead-zone (re-verified against live source)

| Fact | Loc @ HEAD | Check |
|---|---|---|
| Lane A emit floor `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | ✅ |
| Lane A emits `RecurringSignature` at `occurrences >= 2` (counts recalled `failure_signature`s) | `signal.rs:462-467` | ✅ |
| Lane B escalation floor `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | ✅ |
| Escalation gate `if recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `mod.rs:1613` (`decide_blocked_goal`) | ✅ |
| `recurrence = recall_occurrences(dedup_key).len()` (live facts only) | `mod.rs:972-997` (`search_facts`, filter `o.signature == dedup_key`) | ✅ |

**Dead-zone confirmed.** The two counters are **decoupled storage lanes**: Lane A counts
*observation episodes* (visible `×2`); Lane B counts *root-cause occurrences* (drives
escalation). A `×2` operator string (`mod.rs:1361`: `"recurring signature seen
{occurrences}× in cognitive memory ({signature})"`) says **nothing** about Lane B. `2` is
above one-off noise, below the `3` escalation bar, and neither the coverage loop nor the
park loop carries an auto-remediation rung between them → the signal is stuck at `2×`.

**Accrual starvation (why `×2` rarely even climbs to `3`).** The WHY-classification ladder
that would advance Lane B is **double-gated** at `cycle.rs:582-583`:
Gate A `if let Some(source) = &memories.completion_evidence` **and**
Gate B `if no_progress_investigation_enabled()`. If either is false, control falls to the
bare `apply_no_progress_breaker` verify-once park — no occurrence is classified/recorded,
so Lane B never accrues and escalation at `mod.rs:1613` stays unreachable.

## D2 — append-only store_fact ratchet (re-verified)

- `record_occurrence` persists each occurrence via **non-idempotent** `store_fact`
  (`mod.rs:1034`, `concept = occurrence_concept(&entry.key)`). Every recurrence appends a
  new fact; `recall_occurrences` counts them → the count **ratchets monotonically** and
  **cannot converge** across daemon restarts (no cross-restart upsert, no idempotency key).
- The only present gate is the in-memory, per-process `WhisperGate`
  (`guardrails.rs` `last_delivered: HashMap`) and the within-window `write_back_gate`
  (`mod.rs:548-556`); within-window dedup is proven green
  (`tests_memory_recall.rs:797 write_back_is_deduplicated_within_window`) but neither
  survives a restart. **Effective idempotency gate on writeback: absent.**

## D2 — the §6.2b remedy TRAP (confirmed still present, still wrong)

`store_fact_with_caller_key` (`library_adapter.rs:870-914`) uses
`DedupMode::CallerKey` where **"exactly one live fact survives per key"** (comment
`:885-889`, archive-and-supersede). Because `root_cause_signature` is **stable** for a
repeating cause and `recurrence = recall.len()` reads **only live facts**, the literal
one-liner fix collapses recall to **1 forever** → `recurrence` can never reach `3` →
`mod.rs:1613` escalation becomes **dead code**. This trades the ratchet defect for a
*silent-never-escalate* defect. **Correct remedy = count-in-content upsert** (increment an
`occurrence_count` + `first_seen`/`last_seen` in the fact body; escalation reads that field,
not `recall.len()`). **Flag any verdict that re-proposes the bare one-liner.**

---

## Reconciliation ledger entry (baseline vs HEAD `ea6ec554`)

| Prior claim | Source doc | Status @ HEAD |
|---|---|---|
| No production drift; only `tests_root_cause.rs` changed | `RECONCILIATION_LEDGER §0-1`, `secondary…e5257a33 §Source-drift` | ✅ still true (extended: `e5257a33..HEAD` docs-only) |
| D2 = dead-zone **AND** append-only ratchet | `CONSOLIDATED §14`, `RECONCILIATION_LEDGER §4` | ✅ re-verified live |
| §6.2b `store_fact_with_caller_key` one-liner is a TRAP | `RECONCILIATION_LEDGER §2` | ✅ re-verified (`library_adapter.rs:885-889`) |
| Two decoupled lanes (A visible `×2`, B escalation) | `secondary…e5257a33 §Recurrence dead zone` | ✅ now pinned by `f9cefec1` tests |
| D1 self-observation writeback still open (no self-exclusion) | `secondary…e5257a33 §Self-ingestion` | ✅ no production filter added |
| D3 WorkstreamCoverage notify-only, no convergence edge | `DISCOVERIES #2`, `secondary…e5257a33 Loop #1` | ✅ no launch/unblock edge added |

**No divergence from baseline.** The only delta since the baseline commit is the two
reinforcing tests. Nothing contradicts the prior corpus; nothing remediates it.

---

## Signal-vs-defect classification (D2 scope)

- `2×` in the signature → **signal** (honest cross-window / restart re-observation of a
  static, unresolved problem set), not a dedup/count defect.
- The stuck-at-2 behaviour → **defect**: (a) dead-zone between emit=2 and escalate=3 with
  no remediation rung; (b) append-only, per-process, restart-non-convergent storage. Both
  must ship **atomically** (gate + count-in-content counter) or nothing changes.

## Questions for verification phase

1. **Gate B default in prod:** confirm `no_progress_investigation_enabled()` is ON in the
   running daemon — if OFF, Lane B never accrues and escalation is unreachable regardless
   of the counter fix.
2. **Gate A population:** confirm `memories.completion_evidence` is `Some` on the ticks
   where these goals park; a `None` source silently disables the whole ladder.
3. **Atomic landing:** the D2 fix must land the escalation-gate read AND the
   count-in-content upsert together; a caller-key upsert **without** count-in-content
   re-introduces the §6.2b dead-code trap.
