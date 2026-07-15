# Specialist (knowledge-archaeologist) — Prior-Findings Reconciliation vs. current HEAD

**Role:** Specialist / knowledge-archaeologist — reconcile the accumulated investigation
corpus (69 artifacts, 15 prior HEADs) against **current HEAD**, and produce a
**validate-don't-re-derive** checklist. No new root-cause derivation.
**HEAD verified:** `3fac68a5` (`git rev-parse HEAD`) — package `simard` v0.32.1
**Prior reconciliation baseline:** `RECONCILIATION_LEDGER.md` @ `dea65df8`
**Method:** (1) diff `dea65df8..HEAD` to find real source drift; (2) independently
re-read each load-bearing line at HEAD; (3) re-execute all referenced verification tests.

---

## 0. Headline verdict

**Do not re-derive. Validate.** The prior investigation is **sound and current.**
Between the last reconciliation baseline (`dea65df8`) and HEAD (`3fac68a5`) there is
**exactly one source change**: commit `f9cefec1` *added* the test file
`src/overseer/tests_root_cause.rs` (+99 lines, net-new, no logic touched). Every other
commit in that range is documentation-only under `ai_working/investigation/`.

Consequence: **every load-bearing root-cause citation still resolves exactly at HEAD**,
and **all four referenced test suites pass (144 tests, 0 failures)**. The corpus does
**not** need re-derivation; it needs (a) citation-line re-pin, done below, and (b)
one already-known remedy-trap correction carried forward.

---

## 1. Drift audit — `dea65df8..HEAD` (17 commits)

| Aspect | Finding |
|---|---|
| Source logic changed? | **No.** Only `src/overseer/tests_root_cause.rs` added (`f9cefec1`, test-only). |
| `git diff --stat dea65df8..HEAD -- src/` | `1 file changed, 99 insertions(+)` — the new test. |
| All other commits | Docs under `ai_working/investigation/` (waves 7–14 + re-verification passes). |
| Net effect on citations | **Zero drift.** Line numbers below re-pin identically. |

> Archaeological note: the corpus has iterated across 15 HEADs
> (`85b9398a → … → 3fac68a5`) but the *code under investigation has been frozen*
> since before `dea65df8`. The waves are re-validation of a stable target, not
> tracking a moving one. This is why re-derivation has near-zero expected value.

---

## 2. Load-bearing citations — re-pinned at HEAD `3fac68a5` (independently read)

| Claim | Ledger loc | HEAD loc | Status |
|---|---|---|---|
| `observation_signature` = `sort_unstable()`→`dedup()`→`format!("overseer-obs:{}", keys.join("\|"))` | mod.rs:1068-1073 | **mod.rs:1068-1073** | ✅ exact |
| `record_occurrence` writes via non-idempotent `store_fact` (append-only ratchet, Lane B) | mod.rs:1034 | **mod.rs:1034** | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps`, no launch/file edge | mod.rs:1534-1543 | **mod.rs:1534-1543** | ✅ exact |
| Dual-path quarantine: observer M1 mode also maps `WorkstreamCoverage → Report` (not FileIssue) | observer.rs:120 | **observer.rs:120** | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | mod.rs:1613 | **mod.rs:1613** | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B bar) | root_cause.rs:33 | **root_cause.rs:33** | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` (Lane A) | signal.rs:362,463 | **signal.rs:362,463** | ✅ exact |
| `root_cause_signature = "{dedup_key}::{label}"` helper | root_cause.rs:53-55 | **root_cause.rs:53** | ✅ exact |
| `store_fact_with_caller_key` = `DedupMode::CallerKey`, "exactly one live fact survives per key" | library_adapter.rs:870-915 | **library_adapter.rs:870,885-890,906** | ✅ exact (comment 885-889) |
| Recall reads live facts only (`include_superseded: false`) | library_adapter.rs:763,773,830 | **library_adapter.rs:763,773,830** | ✅ exact |
| D1 self-observation write-back nests recall-derived `overseer-obs:` tokens | wiring.rs:301 | **wiring.rs:301** (`write_back_observation(&cycle.problems)`) | ✅ exact |
| WHY double-gate starves Lane-B accrual | cycle.rs:582-702 | **ooda_loop/cycle.rs:583-…** | ✅ holds — **path correction** below |

**One path correction (not a logic drift):** the WHY-gate lives at
`src/ooda_loop/cycle.rs` (line 583, `no_progress_investigation_enabled()` gate),
**not** `src/overseer/cycle.rs` (which does not exist). Any doc that wrote a bare
`cycle.rs:582-702` should read `ooda_loop/cycle.rs`. The `overseer/` module has no
`cycle.rs`. Cosmetic; the gate logic itself is unchanged and confirmed.

---

## 3. Test-backed validation — re-executed at HEAD `3fac68a5`

`cargo test -p simard --lib <suite>`:

| Suite | Result | Backs |
|---|---|---|
| `overseer::tests_root_cause` | **21 passed / 0 failed** | Two-lane separation, stable signature, escalate-not-blind-unblock, never-file-issue |
| `overseer::tests_memory_recall` | **32 passed / 0 failed** | Recall reads live-only; dedup/idempotency boundary |
| `overseer::tests_gap_scan` | **21 passed / 0 failed** | Workstream-gap detection / notify path |
| `ooda_loop/goal_curation::*no_progress*` | **70 passed / 0 failed** | WHY-gate breaker + reinvestigation ladder |
| **Total** | **144 passed / 0 failed** | — |

Load-bearing named tests observed green:
`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` (H1 two-lane verdict),
`root_cause_signature_is_stable_and_combines_key_and_cause`,
`recurring_reblock_escalates_root_cause_not_blind_unblock`,
`recurring_reblock_never_files_an_issue`,
`lane_b_escalates_without_any_lane_a_signal`,
`occurrence_recall_accumulates_recurrence_across_ticks`.

---

## 4. Reconciliation table — prior claim → verifying artifact → status at HEAD

| Prior claim | Verifying artifact | Status @ 3fac68a5 |
|---|---|---|
| `×2` is honest **re-observation**, not dedup/replay defect | `primary_signature_recurrence_VERDICT_HEAD_b9f99879.md`, `primary_..._2x_verdict_HEAD_f455c06d.md` | **STILL HOLDS** — signature is a faithful fingerprint of a static problem set (source frozen) |
| Two decoupled counter lanes (A: episodes→`×2` @ thr 2; B: root-cause→escalation @ thr 3) | `secondary_dedup_recurrence_VALIDATION_HEAD.md §2`; test `loud_lane_a…does_not_feed_lane_b` | **STILL HOLDS** — now test-locked by `f9cefec1` |
| `2-vs-3` dead-zone: `×2` sits above one-off noise, below escalation bar 3; no remediation rung | `secondary_deadzone_and_overaggregation_HEAD_440e024c.md`; consts `signal.rs:362`,`root_cause.rs:33` | **STILL HOLDS** — thresholds hardcoded, unchanged |
| D3 routing hole: `WorkstreamCoverage` notify-only in **both** modes (no FileIssue, no LaunchRecipe) | `tertiary_gap_routing_and_remediation_rung.md`; `mod.rs:1543`, `observer.rs:120` | **STILL HOLDS** |
| D1 self-ingestion: overseer write-back re-enters its own observation input | `primary_self_ingestion_loop_pipeline_trace_HEAD_1de21e71.md`; `wiring.rs:301` | **STILL HOLDS** |
| D2 append-only ratchet at `store_fact` (`mod.rs:1034`) | `RECONCILIATION_LEDGER.md §1`; source read | **STILL HOLDS** |
| **§6.2b remedy trap**: literal `store_fact_with_caller_key` one-liner collapses recall to 1 → escalation becomes dead code | `RECONCILIATION_LEDGER.md §2`; `secondary_dedup_recurrence_VALIDATION_HEAD.md §4` | **STILL HOLDS as a WARNING** — fix must be **count-in-content** upsert, not the one-liner |
| `engineer_spawn ↔ workstream-gap` coupling / write-back self-feed | `tertiary_architecture_SPAWN_GAP_COUPLING_AND_SELFFEED_HEAD_3fac68a5.md` | **CURRENT** — already re-grounded at HEAD |
| OODA loop-closure gaps (blocked-goal ladder gating + gap missing-launch edge) | `secondary_ooda_closure_and_deadzone_HEAD_3fac68a5.md` | **CURRENT** — already re-grounded at HEAD |

**No claim is stale. No claim needs re-derivation.** Two claims were already
re-grounded at HEAD by the freshest wave (rows 8–9).

---

## 5. Validate-don't-re-derive checklist (for downstream agents)

Do these **validations** (cheap, deterministic). Do **not** re-derive root cause.

- [x] **Confirm source frozen:** `git diff --stat dea65df8..HEAD -- src/` ⇒ only
      `tests_root_cause.rs` added. → **done: confirmed.**
- [x] **Re-pin signature assembly:** read `src/overseer/mod.rs:1068-1073`. → **done: exact.**
- [x] **Re-pin thresholds:** `signal.rs:362` (=2), `root_cause.rs:33` (=3), both hardcoded. → **done.**
- [x] **Re-pin routing hole:** `mod.rs:1543` + `observer.rs:120` (notify-only, both modes). → **done.**
- [x] **Re-pin D1/D2:** `wiring.rs:301`, `mod.rs:1034`, `library_adapter.rs:885-890`. → **done.**
- [x] **Re-run tests:** `cargo test -p simard --lib tests_root_cause tests_memory_recall tests_gap_scan no_progress` ⇒ 144/0. → **done.**
- [ ] **Before implementing D2:** carry the §6.2b trap correction — use a
      **count-in-content caller-key upsert** (increment `occurrence_count`,
      `first_seen`/`last_seen`), with escalation reading that field, **not**
      `recall.len()`. The literal `store_fact_with_caller_key` one-liner makes
      `mod.rs:1613` dead code.
- [ ] **Before implementing D3:** key the coverage ledger on `GapItem.signature`,
      **not** the bare `"workstream-gap"` dedup_key (`mod.rs:1371`), or all gaps
      fold into one issue (INV-GAP-KEY trap).
- [ ] **Doc hygiene:** fix any `overseer/cycle.rs` reference to `ooda_loop/cycle.rs`.

---

## 6. Recommendation

**Proceed to implementation, not further investigation.** The evidence base is
saturated: source is frozen, all citations re-pin exactly, all 144 referenced tests
pass, and the freshest two waves are already grounded at HEAD `3fac68a5`. The only
open items are the two forward-carried remedy traps (§5, unchecked boxes), which are
**implementation guardrails**, not investigative gaps. Dependency-correct fix order
remains: **D2 (gate+counter, atomically) → D3 (closing rung) → D1 (write-back filter)
→ convergence gauges.**
