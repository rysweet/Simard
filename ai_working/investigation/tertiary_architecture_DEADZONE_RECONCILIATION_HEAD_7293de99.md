# Tertiary (architect) — Blocked→escalation dead-zone structure & HEAD reconciliation

**Role:** Tertiary investigator (amplihack:architect).
**Focus:** (1) the blocked→recurrence→escalation state machine and its structural
"dead-zone"; (2) reconciliation of **live HEAD behavior** against the existing
`ai_working/investigation/` corpus (`RECONCILIATION_LEDGER.md`,
`FINAL_SYNTHESIS.md`, `blocked_transition_and_escalation_idempotency.md`,
`CONSOLIDATED_FINDINGS.md`, `HYPOTHESES.md`).
**HEAD:** `7293de99` · **Branch:** `investigation/recurring-blocked-goals-workstream-gaps`
**Method:** independently re-read each load-bearing source line at HEAD (did not trust
doc citations); ran the four governing test modules to observe behavior empirically.

---

## 0. Verdict (one line)

The prior investigation is **CONFIRMED at HEAD with effectively zero drift** — the
only source change since the corpus's synthesis HEAD (`5a85317b`) is a single
*corroborating* test file. The blocked→escalation "dead-zone" is real and structural:
it is a **cross-lane visibility/accrual gap**, not a counting bug. **Extend, do not
restart.** One prior *remedy* (not analysis) remains a trap and is already superseded.

---

## 1. Reconciliation — is the corpus still true at HEAD 7293de99?

### 1.1 Drift measurement (objective)

`git diff --stat 5a85317b..HEAD -- src/overseer/*.rs src/ooda_loop/*.rs src/goal_curation/*.rs`
returns **exactly one changed file**:

```
src/overseer/tests_root_cause.rs | 99 +++++++++  (1 file changed, 99 insertions)
```

Every intervening commit (`3fac68a5`, `d6ba8b25`, `f455c06d`, `ad5e1060`,
`05c08919`, `1de21e71`, `bbddd23a`, `cb8cd1dc`, `f1db90f4`, `da7ea0fd`, …) is
`docs(investigation)` **except** `f9cefec1` which adds *tests only*. **Zero
production-source drift.** The corpus's `FINAL_SYNTHESIS.md` claim ("git diff …
is EMPTY … all `src/overseer/*` line citations hold") is still accurate at HEAD.

### 1.2 Independent re-verification of load-bearing citations @ HEAD

| Claim (corpus) | Cited loc | Re-checked @ 7293de99 | Status |
|---|---|---|---|
| `observation_signature` = `sort_unstable()`→`dedup()`→`format!("overseer-obs:{}", keys.join("\|"))` | `mod.rs:1068-1073` | read | ✅ exact |
| `record_occurrence` writes via non-idempotent plain `store_fact` (append-only ratchet) | `mod.rs:1004-1043` (`store_fact` @ `1034`) | read | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `mod.rs:1613` | read | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `root_cause.rs:33` | read | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; fire at `occurrences >= 2` | `signal.rs:362,463` | read | ✅ exact |
| `decide_blocked_goal` routing body | `mod.rs:1603-1631` | read | ✅ exact |
| `WorkstreamCoverage` Decide arm = notify-only `FlagWorkstreamGaps`, no `LaunchRecipe` edge | `mod.rs:1534-1543` | read | ✅ exact |
| WHY-reasoner **double-gate**: Gate A `completion_evidence.is_some()`, Gate B `no_progress_investigation_enabled()` | corpus said `cycle.rs:582-702` | `ooda_loop/cycle.rs:582-583` (+ `no_progress.rs:203`) | ✅ exact (file split; behavior identical) |

**Note on the only apparent "drift":** the corpus sometimes writes the WHY-reasoner
as living in `cycle.rs:582-702`. At HEAD the *gate* is still at `cycle.rs:582-583`,
but the classification helpers (`no_progress_investigation_enabled`,
`resolution_for_why`) now live in the sibling `ooda_loop/no_progress.rs`. This is a
**module-split refactor, not a behavior change** — the double-gate and the ladder are
intact. No citation is invalidated.

### 1.3 What `f9cefec1` (the one source delta) actually did — it *strengthens* the corpus

It added two tests to `tests_root_cause.rs` that **pin the corpus's central
structural claim** (two decoupled counter lanes, `FINAL_SYNTHESIS §2.3`):

- `loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` — a LOUD Lane-A
  `RecurringSignature{occurrences = 2+3+5}` with an EMPTY Lane-B recall keeps
  `why.recurrence == 0`, and `decide()` returns `UnblockGoal` (self-heal), **not**
  escalation. Proves Lane A cannot trip Lane B's `>=3` gate.
- `lane_b_escalates_without_any_lane_a_signal` — Lane B escalates purely on its own
  `PriorOccurrence` recall (`>=3`) with Lane A silent.

**Reconciliation conclusion:** the corpus is not merely still valid — the codebase has
since *codified* its key insight as a regression test. Confidence in the two-lane
model is now higher than when the synthesis was written.

### 1.4 Working-tree vs. committed divergence

`git status` shows the four newest investigation `.md` files staged (`A`), including
this wave's primary/secondary/tertiary drops. **No uncommitted source edits.** The
`RECONCILIATION_LEDGER.md` "commit the working-tree §6.2b correction" action item
refers to doc content, not code; it does not affect HEAD source behavior.

---

## 2. The blocked→escalation dead-zone — structural anatomy

### 2.1 The two-lane state machine (the load-bearing structure)

```
             LANE A  (operator-visible "seen N×")        LANE B  (escalation decision)
  emit:  record_observation → store_episode           record_occurrence → store_fact
         (UNCONDITIONAL, key=composite signature)      (UNCONDITIONAL, key=occurrence_concept)
  gate:  write_back_gate WhisperGate(900s)             (no window gate on the WRITE)
  rate:  +1 per 900 s window the set is still true     +1 per EFFECTIVE act touching the cause
  read:  RecurringSignature.occurrences  (signal.rs)   RootCause.recurrence = recall.len()
  bar:   >= 2  (RECURRING_SIGNATURE_THRESHOLD)         >= 3  (RECURRENCE_ESCALATION_THRESHOLD)
  role:  RAISES PRIORITY only (orient)                 DRIVES decide_blocked_goal escalation
                 │                                                    │
                 └───────────── SHARE NO COUNTER (proven by test) ────┘
```

The "2×" in the question is a **Lane A** number. Escalation is a **Lane B** decision.
They are storage-disjoint. This is the crux: **the visible count carries no information
about whether escalation will ever fire.**

### 2.2 Where the goals actually get stuck (the dead-zone, three coupled seams)

`decide_blocked_goal` (`mod.rs:1603-1631`) is *not itself* broken — read literally it
has full coverage below the escalation bar:

```
recurrence >= 3                      → EscalateBlockedGoal      (Lane B rung)
perpetual && is_no_progress_marker   → UnblockGoal (self-heal)  (false-park rung)
needs_review                         → EscalateBlockedGoal      (marker rung)
else                                 → Report                   (deliberate block)
```

The latch is produced **upstream and around** this function, on three seams:

- **Seam 1 — WHY double-gate starves Lane B accrual (`cycle.rs:582-583`).**
  The no-progress breaker parks goals with a **bare marker and no WHY class** whenever
  `completion_evidence == None` (Gate A collapses the block to `Vec::new()`) or
  investigation is disabled (Gate B → legacy park). With no classification, the
  auto-resolvable ladder (`AlreadyComplete→MarkDone`, `Obsolete→Drop`,
  `MissingPrecondition→Heal`, `UpstreamDependency→Defer`) never runs, so the *same*
  cause re-parks every window. Lane B increments only on **effective acts**; a goal
  parked as bare "needs human review" that self-heals then re-blocks generates
  occurrence records slowly/inconsistently, so `recall.len()` hovers **below 3** — the
  dead-zone. The operator sees Lane A "×2" (real re-observation) while Lane B never
  crosses its bar.

- **Seam 2 — the Lane B counter is an append-only ratchet (`mod.rs:1034`).**
  `record_occurrence` uses plain `store_fact` (unconditional CREATE, no dedup). Once a
  cause *does* accumulate 3 records, `recurrence >= 3` **latches on forever** — the
  goal escalates every future window even after its cause is gone, because nothing
  supersedes or prunes occurrence nodes. So the dead-zone has **two failure modes on
  the same axis**: *never escalate* (sub-3, the common case here) and *over-escalate &
  latch* (post-3). Confirmed by `occurrence_recall_accumulates_recurrence_across_ticks`
  (green) — recurrence only grows.

- **Seam 3 — the `workstream-gap` closing edge is missing (`mod.rs:1534-1543`).**
  `WorkstreamCoverage` is the **only** High-priority Decide arm that resolves to
  notify-only `FlagWorkstreamGaps` — no `LaunchRecipe` edge and no `FileIssue` path.
  Blocked goals are additionally skipped by gap-scan
  (`delegates_blocked_goals_to_goal_health_and_never_reflags_them`, green), so an
  under-resourced goal **oscillates** between `workstream-gap` (active, uncovered) and
  `goal:blocked` (parked) — feeding both recurring families in the signature with no
  terminal state on either side.

### 2.3 Why this is a "dead-zone" and not a threshold-tuning problem

Lowering `RECURRENCE_ESCALATION_THRESHOLD` from 3→2 would **not** fix it, because the
count that reaches "2" is on **Lane A** (episodes) while the escalation gate reads
**Lane B** (occurrences). The remediation rung must sit on the **episode lane** (first
provable at ×2), or the WHY-gate must be opened so Lane B can accrue. This matches
`FINAL_SYNTHESIS §Output 4` ("any remediation rung must sit on the episode lane, not
the occurrence lane") and is the correct architectural read.

---

## 3. The one surviving contradiction (reconciled, unchanged from ledger)

The committed `CONSOLIDATED_FINDINGS §6.2b` "obvious" de-ratchet fix —
`store_fact_with_caller_key(root_cause_signature(...))` — is a **trap**:
`DedupMode::CallerKey` keeps exactly one live fact per key, so `recall.len()` sticks at
**1 forever** and `recurrence >= 3` becomes **dead code**. The corpus already supersedes
this with the **count-in-content upsert** (counter carried in fact `content`,
escalation reads that field). **Confirmed still correct at HEAD** — nothing in the
source changed to alter this. Analysis holds; only that one remedy was wrong and is
already replaced in the working-tree docs.

---

## 4. In-flight remediation context (scope boundary respected)

Seven sibling worktrees exist for this exact issue —
`issue-4087/4088/4090/4092/4108/4112 (recurring-signature-seen-2-…)` and
`issue-4078 (self-diagnose-and-fix-a-failed-ooda-step)`. **None are merged into this
branch** (§1.1 shows zero source drift here). This confirms remediation is being
attempted in parallel but the defects D1/D2/D3 remain **live at HEAD**. Per the
investigation charter I stop at the proposed **landing order** and do not merge:

1. **D2 gate+counter, atomically** — open the WHY double-gate (or wire a
   sub-threshold remediation rung on the episode lane) **and** convert Lane B to
   count-in-content in the *same* change. Fixing either alone changes nothing
   observable (coupled pair).
2. **D3 closing edge** — give `WorkstreamCoverage` a real terminal action
   (`FileIssue` via observer `Report`, or a `LaunchRecipe` edge) + a cross-window gap
   ledger keyed on `GapItem.signature` (not the bare `"workstream-gap"` dedup_key,
   else all gaps fold into one issue — INV-GAP-KEY).
3. **D1 write-back hygiene** — exclude recall-derived `overseer-obs:*` tokens at the
   `write_back` boundary (`wiring.rs:301`) to break the self-observation nesting.
4. **Convergence gauges** — assert the recurring population trends toward zero.

---

## 5. Empirical verification (behavior observed, not just cited)

All four governing modules **green at HEAD 7293de99** (`cargo test --lib`):

| Module | Result | What it proves for this focus |
|---|---|---|
| `overseer::tests_root_cause` | **21/21 ok** | Two-lane decoupling (both direction tests); ratchet accrual; escalate-vs-self-heal routing |
| `overseer::tests_goal_health` | **11/11 ok** | Blocked-transition projection; escalate deduped per goal/window |
| `overseer::tests_whisper` | **28/28 ok** | Within-window write-back dedup (Lane A "honest count") |
| `overseer::tests_gap_scan` | **21/21 ok** | Blocked goals skipped by gap-scan; `WorkstreamCoverage`→notify-only; gap notify dedupes on repeat; never files an issue |

Total **81/81**. The suite does not merely pass — specific tests (`decide_routes_
workstream_coverage_to_flag_gaps`, `flagged_gap_never_constructs_an_issue_brief`,
`delegates_blocked_goals_to_goal_health_and_never_reflags_them`,
`loud_lane_a…does_not_feed_lane_b_recurrence`) are the **executable form** of the
corpus's structural claims. The dead-zone is a property the tests actively lock in as
current, intended-but-incomplete behavior.

---

## 6. Recommendations for understanding (architect view)

- **Read the "2×" as a health signal, not a defect.** Lane A is provably honest
  (window-gated, dedup test green). A correct count that never trends to zero indicts a
  **missing convergence rung**, not the counter.
- **Treat the dead-zone as a cross-lane wiring gap.** The fix surface is the WHY-gate +
  an episode-lane remediation rung, plus the D3 closing edge — *not* the escalation
  threshold value.
- **Land D2 as an atomic pair.** The gate and the count-in-content counter are coupled;
  a partial landing reproduces either "never escalate" or "latch-forever."
- **Prefer the count-in-content upsert; reject the one-line caller-key de-ratchet.** The
  latter turns escalation into dead code (verified against `DedupMode::CallerKey`).
- **The corpus is authoritative at HEAD.** Future waves should extend §21 of
  `CONSOLIDATED_FINDINGS`, not re-derive provenance — every citation re-grounds exactly.

**Net-new vs. re-validation:** this drop is ~90% re-validation (all citations, all
tests) and ~10% net-new — the objective drift measurement (§1.1), the observation that
`f9cefec1` *codifies* the two-lane model as a regression test (§1.3), and the
enumeration of the seven live fix worktrees establishing that D1/D2/D3 remain unmerged
at HEAD (§4).
