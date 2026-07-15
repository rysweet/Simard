# Tertiary (Architect) Deep Dive — Landing-Order-Safe Remediation Shape (Advisory)

**Investigation:** "recurring signature seen 2× in cognitive memory
(`overseer-obs:…|goal:blocked:<slug>-<hash>|workstream-gap|resource:engineer_spawn`)"
**Role:** TERTIARY / architect — remediation shaping only, **no implementation**
**HEAD verified:** `f455c06d` (`git rev-parse HEAD`)
**Method:** every load-bearing claim re-read against current source with file:line
citations; prior `ai_working/investigation/` artifacts reconciled, not trusted blind.

---

## 1. Verdict (up front)

- **Confirmed REAL, still LIVE, zero material source drift** since the prior tertiary
  wave (`0289572e`). `git diff --stat 0289572e HEAD -- src/` shows **exactly one**
  change: `+99` lines in `src/overseer/tests_root_cause.rs` — **test-only**. No
  production seam moved.
- **Net-new architectural fact this wave** (from those 99 test lines): the two
  recurrence counters are **provably decoupled at `decide`**. Lane A
  (`Signal::RecurringSignature`, the thing that reads "seen 2×") **cannot** trip
  Lane B (root-cause escalation). This *sharpens* the remediation: the resolution
  rung and the idempotency fix must both target **Lane B**, and the loop-breaker
  must **not** wire Lane A into escalation. See §3.
- **All three structural defects remain unmerged** — re-confirmed by source read +
  absence grep (§2).
- **Dependency-safe landing order holds and is re-justified against the decoupling
  fact:** `[1] loop-breaker → [2] Lane-B count-in-content (atomic) → [3] closing
  rungs (gap arm + decide dead-zone)` (§4). Regression-safety notes cite the exact
  tests each rung must deliberately update vs. must keep green (§5).

---

## 2. Re-verification ledger @ `f455c06d` (production seams)

| Claim | Loc @ HEAD | Re-read | Status |
|---|---|---|---|
| `observation_signature` = `sort_unstable→dedup→"overseer-obs:"+join("\|")` | `overseer/mod.rs:1068-1073` | ✅ exact | live |
| write-back folds whole `problems` slice, **no provenance filter** | `overseer/mod.rs:534-563` (sig at :546) | ✅ exact; `grep 'starts_with("overseer-obs'` in `mod.rs` = **no match** | loop-breaker **absent** |
| `record_occurrence` uses append-only `store_fact` (Lane B write) | `overseer/mod.rs:1004-1043` (write at :1034) | ✅ exact | idempotency **absent** |
| `StoredOccurrence` carries **no** count/first_seen/last_seen | `overseer/mod.rs:1180-1185` | ✅ 4 fields (signature, cause_label, action, outcome) | live |
| Lane B recurrence = `recall_occurrences(dedup_key).len()` via `analyze` | `overseer/mod.rs:972-997` | ✅ exact | live |
| `decide_blocked_goal`: 2 ≤ rec < 3, non-perpetual, non-review → `Report` | `overseer/mod.rs:1603-1631` (fallthrough at :1630) | ✅ exact | **dead-zone live** |
| `WorkstreamCoverage` → `FlagWorkstreamGaps` (notify-only) | `overseer/mod.rs:1534-1543` | ✅ exact | live |
| gap act = peek→notify→commit, **within-window `gap_gate` only**, no launch edge | `overseer/mod.rs:884-940` (notify at :930, commit at :933) | ✅ exact | closing rung **absent** |
| `RECURRING_SIGNATURE_THRESHOLD = 2` (Lane A floor / emit) | `overseer/signal.rs:362,463` | ✅ exact | live |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B floor) | `overseer/root_cause.rs:33` | ✅ exact | live |

**Drift conclusion:** consolidated analysis is fully consistent with HEAD. The only
change is additive test coverage that *confirms* prior findings — see §3.

---

## 3. Net-new fact: the two lanes are decoupled at `decide` (and why it matters)

The `+99` lines in `tests_root_cause.rs` add two pinning tests:

- `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` — a
  `RecurringSignature{ occurrences = 2+3+5 }` co-signal with an **empty** Lane-B
  recall leaves `why.recurrence == 0` and yields `UnblockGoal` (self-heal), **not**
  escalation. Proof: `decide` reads recurrence **only** from `why.recurrence`
  (populated by `analyze` over the `&[PriorOccurrence]` slice, `mod.rs:972-997`);
  `orient` uses the `RecurringSignature` co-signal **solely to raise priority**.
- `lane_b_escalates_without_any_lane_a_signal` — 3 matching `PriorOccurrence` with
  Lane A entirely silent still escalates. Lane B stands alone.

**Architectural consequence for remediation (this is the tertiary contribution):**

1. The string that literally reads *"recurring signature seen 2×"* is **Lane A**.
   Lane A is, by design, **inert for closure** — it only nudges priority. So *no*
   amount of tuning Lane A's `×N` will ever close a blocked goal. Any remediation
   that "makes the 2× stop" by touching Lane A is **cosmetic**.
2. The lane that *can* close/escalate is **Lane B**, and its count is
   cadence-inflated (append-only `store_fact`, no upsert). So the closing rung
   (§4[3]) is only trustworthy **after** Lane B's counter is made honest (§4[2]).
3. **Do not "fix" this by bridging Lane A → Lane B.** The two decoupling tests are
   now regression pins; bridging would break them *and* re-introduce the very
   self-feeding the loop-breaker (§4[1]) exists to sever. The correct shape keeps
   the lanes separate and fixes each on its own seam.

---

## 4. Landing-order-safe remediation (advisory — do NOT implement here)

Three seams, one strict dependency chain. Canonical names in **bold**.

```
 [1] Write-back self-observation guard          (loop-breaker; no deps)
       seam: overseer/mod.rs:546 (before observation_signature)
       shape: drop recall-derived problems (dedup_key starts_with "overseer-obs:")
              AND RecurringSignature-only problems from the slice fed to
              observation_signature / observation_content.
       effect: stops NEW nested "overseer-obs:overseer-obs:…" signatures forming;
               freezes the signature SET so [2] has a fixed target.
       ▼
 [2] Lane-B count-in-content + WHY-gate          (ATOMIC latch — ship together)
       seam: record_occurrence mod.rs:1004-1043; StoredOccurrence mod.rs:1180-1185;
             read path recall_occurrences mod.rs:972-997
       shape: signature-keyed caller-key UPSERT whose content carries
              occurrence_count / first_seen / last_seen; escalation reads that
              field, NOT recall.len().
       effect: Lane-B recurrence means "distinct windows" not "write cadence".
       ▼
 [3] Closing rungs                               (consume the now-honest count)
       (3a) decide_blocked_goal dead-zone: mod.rs:1603-1631
            add a first-recurrence remediation/escalation rung so the
            2 ≤ rec < 3 non-perpetual non-review band no longer falls to Report.
       (3b) gap-quarantine arm: mod.rs:884-940 + 1534-1543
            add a launch/escalation edge for WorkstreamCoverage recurrence and a
            CROSS-window ledger keyed on GapItem.signature (not bare
            "workstream-gap"); today it is within-window gap_gate + notify only.
```

**Why the order is a true dependency chain (not preference):**

- **[1] must precede [2].** Each nesting level (`overseer-obs:overseer-obs:…`) is a
  *different* signature; an idempotent upsert cannot collapse a moving target. Make
  the store idempotent first and it still bloats — one row per distinct nesting.
- **[2] must precede [3].** The closing rung consumes the occurrence count.
  Escalating on a cadence-inflated count (pre-idempotency) fires on write cadence,
  not real recurrence — noisy and wrong. [2] is a **latch**: shipping the count
  field OR the WHY-gate alone changes nothing observable; they must land atomically.
- **Trap to avoid:** the naïve `store_fact_with_caller_key` upsert for [2] collapses
  `recall.len()` to 1 forever, making the `>= 3` escalation **dead code**. The
  corrected remedy is **count-in-content** (the count lives in the payload, not in
  row multiplicity). Re-confirmed via `RECONCILIATION_LEDGER §2`.
- **Decoupling caveat (new):** because Lane A never feeds `decide`, [3a]/[3b] must
  read Lane B's honest count. Do **not** source the closing rung from the
  `RecurringSignature` co-signal — that would couple the lanes and break the two
  new `tests_root_cause.rs` pins.

**Minimality:** each rung is a localized edit at a single seam. No new module, no
capability-trait change, no storage-engine change. Provenance at the store seam is
already sound (`source_label = "overseer"` fixed by the adapter; recalled text
`sanitize_recalled`-cleaned) — this is a **control-flow feedback** defect, not a
security/injection one, so no security surface is touched.

---

## 5. Regression-safety notes (per rung → exact tests)

**[1] Loop-breaker** (mod.rs:546)
- Must keep GREEN: `tests_memory_recall.rs` (32 tests) — write-back still records a
  *clean* observation for genuinely-new problem sets; the guard only strips
  recall-derived/`RecurringSignature`-only entries.
- Must keep GREEN: `tests_whisper.rs` (28 tests) — `write_back_gate` window/cap
  semantics unchanged; the guard filters *inputs* to the signature, not the gate.
- Add: a test proving a slice containing only `overseer-obs:*` / RecurringSignature
  problems produces **no** write-back (returns `Ok(None)`), so the (G)→(B) self-edge
  is severed.

**[2] Lane-B count-in-content** (mod.rs:1004-1043 / :972-997 / :1180-1185)
- Must UPDATE deliberately: any test asserting `StoredOccurrence`'s 4-field shape
  (the struct gains count/first_seen/last_seen).
- Must keep GREEN and is the **key guard**: the two new decoupling pins
  `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` and
  `lane_b_escalates_without_any_lane_a_signal` (`tests_root_cause.rs`). The upsert
  must preserve "Lane B escalates on its own honest count; Lane A stays inert."
- Must keep GREEN: `recurring_reblock_never_files_an_issue` (`tests_root_cause.rs`)
  — escalation still routes through the per-goal gate, never a new issue.
- Add: an idempotency test — N write-backs of the *same* signature within/without a
  window yield `occurrence_count == N` via **one** logical record, and `recall`-based
  recurrence tracks distinct windows, not row count.

**[3a] decide dead-zone rung** (mod.rs:1603-1631)
- Must keep GREEN: `tests_no_progress.rs` / `_investigation.rs` / `_reinvestigation.rs`
  (8/6/11 tests) — the perpetual + no-progress **self-heal** branch (`UnblockGoal`)
  must remain the winner for that band; the new rung only fills the
  `2 ≤ rec < 3, non-perpetual, non-review` gap that currently falls to `Report`.
- Must keep GREEN: `loud_lane_a_…` (the false-park must still self-heal, not escalate).
- Add: a test pinning the previously-`Report` band to its new remediation/escalation
  outcome, with Lane-B recurrence honest.

**[3b] gap closing rung** (mod.rs:884-940 / :1534-1543)
- Must UPDATE deliberately (these currently PIN notify-only):
  `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`
  (`tests_gap_scan.rs:579`) and `flagged_gap_never_constructs_an_issue_brief`
  (`:663`) — the new launch/escalation edge changes "never files" for *recurring*
  gaps. Scope the change to **cross-window recurrence** so first-sighting behavior
  (single consolidated notification) stays intact.
- Must keep GREEN: `delegates_blocked_goals_to_goal_health_and_never_reflags_them`
  (`:413`), `disabled_gap_scan_holds_the_whole_action` (`:688`),
  `gap_scan_fails_closed_without_a_distinct_identity` (`:719`) — opt-out and
  identity-safety invariants are untouched.
- Add: a cross-window ledger test keyed on `GapItem.signature` proving a gap
  recurring across ≥2 windows takes the launch/escalation edge exactly once, while
  the bare `"workstream-gap"` tail stops re-notifying.

---

## 6. Structural notes on the composite string members

- `resource:engineer_spawn` and `workstream-gap` are **ordinary leaf `dedup_key`s**
  (spawn-rate signal; `WorkstreamCoverage` kind at `mod.rs:1369`), aggregate
  *members* of the composite blob — **not** separate signatures, **not**
  meta-observations, and they introduce **no** new dedup mechanism. `resource:engineer_spawn`
  is benign membership drift into the same composite (it does not reset recurrence
  counting); `workstream-gap|workstream-gap` is the symptom of the missing [3b]
  closing rung, not a write-back-exclusion candidate.
- The composite is a **content-derived signature string** (Lane A episodic +
  Lane B occurrence keys folded by `observation_signature`), not a memory *key* and
  not a summary artifact — the `"overseer-obs:"` prefix is applied at
  `mod.rs:1072` at write-back time.

---

## 7. Reconciliation with prior waves

- All prior verdicts (self-observation feedback loop, two counter lanes, recurrence
  dead-zone, notify-only gap arm, count-in-content over bare caller-key) are
  **consistent with source at `f455c06d`**. None superseded.
- This wave **adds** one load-bearing fact — the `decide`-level Lane-A/Lane-B
  decoupling is now a regression pin — and folds it into the landing order and the
  regression-safety matrix above.
- Source drift since `0289572e`: **test-only** (`tests_root_cause.rs +99`). All three
  production defects remain live and unmerged.

**This is an investigation deliverable. No source changed.** The three rungs touch
the write boundary, the occurrence store, and the decide/gap ladders; they warrant
the normal development workflow in the stated order, not a drive-by edit.
