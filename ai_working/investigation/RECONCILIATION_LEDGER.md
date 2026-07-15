# Reconciliation Ledger — committed artifacts (6e3113bc / dea65df8) vs. live source

**Role:** Specialist (knowledge-archaeologist) — reconcile prior artifacts and the
two investigation commits against current code to **extend/validate, not restart.**
**HEAD:** `dea65df8`  **Date:** 2026-07-15
**Method:** independently re-read each load-bearing line in `src/` (did not trust the
docs' own citations).

---

## 0. Verdict

The prior investigation is **sound and should be extended, not redone.** Every
load-bearing root-cause citation in `investigation_report.md`,
`CONSOLIDATED_FINDINGS.md`, and the two commit messages re-verifies **exactly**
against live source. There is **one** real contradiction to resolve: a **fix
recommendation** in the committed CONSOLIDATED (§6.2b) is a **trap** that the
uncommitted VALIDATION_HEAD docs already correct. Root-cause *analysis* holds;
one root-cause *remedy* was wrong and is superseded.

---

## 1. Independently re-verified citations (live source @ dea65df8)

| Claim (from committed docs) | Cited loc | My check | Status |
|---|---|---|---|
| `observation_signature` = `sort_unstable()`→`dedup()`→`overseer-obs:{join("\|")}` | `overseer/mod.rs:1068-1073` | read | ✅ exact |
| `record_occurrence` writes via non-idempotent `store_fact` (append-only ratchet) | `overseer/mod.rs:1034` | read | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps`, no launch edge | `overseer/mod.rs:1534-1543` | read | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `overseer/mod.rs:1613` | read | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `overseer/root_cause.rs:33` | read | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` | `overseer/signal.rs:362,463` | read | ✅ exact |
| `root_cause_signature = "{dedup_key}::{label}"` helper exists | `overseer/root_cause.rs:53-55` | read | ✅ exact |
| `store_fact_with_caller_key` = `DedupMode::CallerKey`, "exactly one live fact survives per key" | `cognitive_memory/library_adapter.rs:870-915` | read | ✅ exact (comment 885-889) |
| Recall reads live facts only (`include_superseded: false`) | `library_adapter.rs:763,773,830` | read | ✅ confirmed |

**No stale citations.** The two commits `6e3113bc`/`dea65df8` are documentation-only
(`git show --stat`), so every defect they describe is still live at HEAD.

---

## 2. The one contradiction — flag and resolve

**Committed** `CONSOLIDATED_FINDINGS.md@dea65df8`, §3a/§6.2b, recommends:
> replace `store_fact` at `mod.rs:1034` with
> `store_fact_with_caller_key(root_cause_signature(problem, primary), …)`

**This is refuted by source.** `DedupMode::CallerKey` keeps **exactly one live
fact per key** (archive-and-supersede, `library_adapter.rs:885-889`), and
`recurrence = recall_occurrences(...).len()` reads only live facts. Because
`root_cause_signature` is **stable** for a repeating cause, the literal fix
collapses recall to **1 forever** → `recurrence` can never reach `3` →
`decide_blocked_goal`'s escalation rung (`mod.rs:1613`) becomes **dead code**.
The committed fix trades the ratchet defect for a *silent-never-escalate* defect.

**Resolution (already drafted in the working tree, not yet committed):** the
counter must be carried **in the fact content** — a caller-key upsert whose
`content` holds an incremented `occurrence_count` + `first_seen`/`last_seen`,
with escalation reading that field instead of `recall.len()`. Captured in
`secondary_dedup_recurrence_VALIDATION_HEAD.md §4` and
`tertiary_architecture_VALIDATION_HEAD.md §2.1`. **Action:** commit the working-tree
correction to §6.2b so the committed record no longer carries the trap.

---

## 3. What later validation ADDED (extends, does not contradict)

| Delta | Where | Nature |
|---|---|---|
| **Two decoupled counter lanes** (A = observation episodes → visible `×2`; B = root-cause occurrences → escalation) | `secondary…VALIDATION_HEAD §2` | Sharpens the single "dead-zone axis" framing; both are true, on different storage lanes. |
| **Three-defect geometry D1/D2/D3** on three seams needing three fixes | `tertiary_architecture_VALIDATION_HEAD §1` | Structures the fix set; D2 (gate+counter) must ship **atomically** or nothing changes. |
| **Dual-path coverage quarantine** — `WorkstreamCoverage` is the only High kind cut off from **both** `FileIssue` (observer `Report`, `observer.rs:120`) **and** `LaunchRecipe` | `tertiary_gap_routing… §1` | New; both operating modes leave the gap terminal. |
| **INV-GAP-KEY** — ledger must key on `GapItem.signature`, not the bare `"workstream-gap"` dedup_key (`mod.rs:1371`), else all gaps fold into one issue | `tertiary_gap_routing… §2` | New trap avoided for the remediation rung. |
| **`route_failure` built-and-dangling**, already anticipating coverage briefs | `tertiary_gap_routing… §3`; `stewardship/routing.rs:11-15` | No new routing subsystem needed. |

---

## 4. Reconciled root-cause statement (unchanged in substance, one remedy corrected)

The recurring `overseer-obs:…|goal:blocked:…|workstream-gap` signature is a
**faithful fingerprint of a static, unresolved problem set** — a **real
re-observation loop, not a dedup/storage/replay artifact** — produced by three
independent defects: **D1** self-observation write-back nesting recall-derived
`overseer-obs:` tokens (`wiring.rs:301`); **D2** a blocked-goal escalation counter
that is *both* a dead-zone (WHY double-gate at `cycle.rs:582-702` starves accrual,
so `×2` never reaches the `3` bar) *and* an append-only ratchet (`mod.rs:1034`);
and **D3** a Decide-table routing hole leaving `WorkstreamCoverage` notify-only with
no cross-window memory. **The committed analysis of all three holds.** The **only**
correction to the committed record: the §6.2b de-ratchet remedy must be the
**count-in-content** upsert, **not** the literal `store_fact_with_caller_key`
one-liner (which makes escalation dead code).

**Recommendation:** do not restart. Commit the working-tree §6.2b correction and the
five validation artifacts, then proceed to implementation in the dependency-correct
order (D2 gate+counter atomically → D3 closing rung → D1 write-back filter →
convergence gauges).
