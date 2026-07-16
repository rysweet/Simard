# Secondary (re-validation) — Recurrence mechanism / dedup / idempotency / expiry

**Role:** Secondary investigator. **Mandate:** VALIDATE + EXTEND prior findings, do
not re-derive. **HEAD:** `a0c5ed4c` (prior ledger validated at `dea65df8`).
**Method:** independently re-read every load-bearing line in `src/` at current HEAD.

---

## 0. Bottom line

The prior secondary verdict **holds unchanged at HEAD `a0c5ed4c`** with **zero
citation drift**. The recurring `overseer-obs:…|goal:blocked:…|workstream-gap`
signature is a **faithful, honest re-emission of a static unresolved problem set —
a real re-observation loop, NOT a dedup/replay/idempotency artifact.** There is a
genuine *missing storage-layer idempotency/expiry*, but it corrupts the **escalation
counter (Lane B)**, not the visible `×2` count (Lane A). The two are decoupled.

**Drift check:** all commits between `dea65df8` and `a0c5ed4c` are **docs-only**
(`docs(investigation): …`). Every root-cause seam is still live and unmodified.

---

## 1. Re-verified citations at HEAD `a0c5ed4c` (read, not trusted)

| Claim | Location (prior) | At HEAD a0c5ed4c | Status |
|---|---|---|---|
| `observation_signature` = `sort_unstable()`→`dedup()`→`overseer-obs:{join("\|")}` | mod.rs:1068-1073 | mod.rs:1068-1073 (fmt @1072) | ✅ exact |
| `record_occurrence` writes via non-idempotent `store_fact` | mod.rs:1034 | mod.rs:1034 | ✅ exact, still `store_fact` |
| `store_fact` = append, no dedup mode passed | lib_adapter:657-683 | lib_adapter:657-683 | ✅ exact |
| `store_fact_with_caller_key` = CallerKey, "exactly one live fact per key" | lib_adapter:870-915 | lib_adapter:870-915 (comment 885-889) | ✅ exact |
| Recall reads live-only (`include_superseded:false`) | lib_adapter:763,773,830 | same | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit `>= 2` | signal.rs:362,463 | signal.rs:362,463 | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | root_cause.rs:33 | root_cause.rs:33 | ✅ exact |
| Escalation gate `recurrence >= 3` in `decide_blocked_goal` | mod.rs:1613 | mod.rs:1603-1619 (gate @1613) | ✅ exact |
| `OCCURRENCE_RECALL_LIMIT = 256` | mod.rs:1137 | mod.rs:1137, used :977 | ✅ exact |
| Write-back gate `WhisperGate::new(900,5)` (in-process) | mod.rs:299 | mod.rs:299; peek/commit :548-556 | ✅ exact |
| Self-feed `write_back_observation(&cycle.problems)` | wiring.rs:301 | wiring.rs:301 | ✅ exact |

**No stale citations. No behavioral drift.**

---

## 2. The memory store and its dedup semantics (my focus — grounded answer)

**Physical store:** cognitive memory lives behind `src/cognitive_memory/library_adapter.rs`
(a `LibraryAdapter` over the knowledge library). Both recurrence lanes persist here.

The store exposes **exactly three write disciplines**; the recurrence lanes use the
weakest ones:

1. **`store_fact` (657-683) — APPEND, no dedup.** No `dedup_key`, no `DedupMode`.
   Every call creates a new live fact. **This is what `record_occurrence` uses
   (mod.rs:1034).** → append-only ratchet.
2. **`store_fact_with_caller_key` (870-915) — CallerKey supersede.** Identical
   content reused; changed content archives prior + `superseded_by`/`SUPERSEDES`
   edge. **Exactly one live fact survives per key** (comment 885-889). *Not used on
   either recurrence lane.*
3. **In-process `WhisperGate` (guardrails)** — the *only* dedup actually applied to
   recurrence, and it is **not a storage-layer** mechanism: it is a per-process
   `HashMap` with a **time-boxed expiry** (`900 s` window + per-hour cap). No
   cross-restart, no persistence.

**Consequence:** the store has idempotency *available* (CallerKey) but the recurrence
paths deliberately bypass it. Dedup that *is* present (WhisperGate) **expires every
900 s and dies on restart**. So "identical keys persist across cycles" is expected,
not a bug in the store.

---

## 3. Recurrence mechanism verdict: per-cycle/per-window RE-EMISSION (confirmed)

Two decoupled lanes; **neither deduplicates at the storage layer**:

- **Lane A — observation episodes (the visible `×2`).** `write_back_observation`
  (mod.rs:534) → `record_observation` → `store_episode` (unconditional append).
  Suppressed **only within** the 900 s in-process `write_back_gate` window
  (mod.rs:548-556). Across windows/restarts it re-persists the same signature →
  the `×2` is a **true count of re-observation episodes of an unchanged problem
  set**, not a duplicated node. Signalled at `occurrences >= 2` (signal.rs:463).
- **Lane B — root-cause occurrences (escalation).** `record_occurrence` (mod.rs:1004)
  → `store_fact` **append** (mod.rs:1034). Counted by `recall_occurrences`
  (≤256, mod.rs:977) → escalation at `>= 3` (mod.rs:1613). No gate at all → a
  **monotonic ratchet on act-count**, not on distinct re-occurrences.

**So:** the recurrence is **per-cycle re-emission**, AND there is a real
**missing/expiring idempotency** — but the missing idempotency lives on Lane B
(append ratchet) + a **cross-restart expiry gap** on Lane A (in-process gate only).
The counting mechanism is honest; **audit the closing action, not the counter.**

---

## 4. Validated trap (do NOT re-derive — confirmed still live)

The committed §6.2b remedy — *replace `store_fact` with
`store_fact_with_caller_key(root_cause_signature(...))`* — is **REFUTED by source**
and **still applicable at HEAD** (fix not yet applied; mod.rs:1034 still `store_fact`).
CallerKey keeps exactly one live fact per key (lib_adapter:885-889) and
`recall_occurrences` reads live-only → recall collapses to **1 forever** → escalation
rung (mod.rs:1613) becomes **dead code**. **Correct remedy = count-in-content upsert**
(`occurrence_count` + `first_seen`/`last_seen`), escalation reading the field, not
`recall.len()`. Two decoupled lanes → a fix must name which lane it targets.

---

## 5. Extensions / notes for the verification phase

1. **A resolution rung DOES exist but is narrowly gated.** `decide_blocked_goal`
   has an `UnblockGoal` arm (mod.rs:1620-1621) — but only when
   `perpetual && is_no_progress_marker(reason)`. For issue-17 (real
   AlreadyComplete/MissingPrecondition block) this predicate is false, so the goal
   is never auto-unblocked → it re-parks and re-emits forever. This is the
   architect's "missing unblock transition" — it is *present-but-unreachable* for
   this cluster, not absent.
2. **Expiry semantics matter for reproduction:** because Lane A's only dedup is the
   in-process `WhisperGate(900,5)`, a flapping/restarting daemon inflates the
   visible `×2` independent of Lane B. Verification should confirm whether the two
   episodes came from two windows in one run vs two restarts.
3. **issue-17 block is REAL, not an observation artifact** (consistent with prior).
   Implementing that fix is OUT OF SCOPE.
4. No contradictions with `RECONCILIATION_LEDGER.md`,
   `secondary_dedup_recurrence_VALIDATION_HEAD.md`, or `CONSOLIDATED_FINDINGS.md`.
   This artifact **re-anchors** all of them at HEAD `a0c5ed4c`.

## 6. Open questions for verification

- Is Lane B's ACT path actually reached for these goals (dead-zone vs ratchet)?
  Owned by primary; determines whether Lane B is starved (never escalates) or
  ratcheting (over-escalates).
- Should recall-derived `overseer-obs:`-keyed `ProcessHealth` problems be excluded
  from `write_back_observation` (wiring.rs:301) to stop self-nesting without
  touching genuine recurrence signalling?
