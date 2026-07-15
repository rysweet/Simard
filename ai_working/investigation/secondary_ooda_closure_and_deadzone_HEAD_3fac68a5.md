# Secondary Investigation — OODA Loop-Closure Gaps & the 2-vs-3 Dead-Zone

**Role:** Secondary investigator (patterns / loop-closure focus)
**HEAD grounding:** `3fac68a5288e965a1aceee029a3e10ae105db3c0`
**Prior grounding:** `f455c06d` (secondary_two_loops_and_drift_HEAD_f455c06d.md),
`440e024c` (secondary_deadzone_and_overaggregation_HEAD_440e024c.md)

**Verdict (one line):** Every load-bearing citation re-verifies EXACTLY at
current HEAD; `git diff f455c06d..HEAD -- src/` is **empty** (the two
intervening commits are documentation-only). Both OODA arms are confirmed
non-closing, the 2-vs-3 dead zone is confirmed and is deeper than "no rung,"
and the escalation threshold is **hardcoded, not configurable**.
**Extend the prior investigation — do not restart.**

---

## 0. Drift verification (validate-don't-re-derive)

- `git diff --stat f455c06d..HEAD -- src/` → **empty**. Production source is
  byte-identical to the prior secondary grounding. HEAD advanced only by two
  documentation-only investigation commits (`d6ba8b25`, `3fac68a5`).
- **Regression baseline re-run at HEAD 3fac68a5 (green):**
  - `no_progress` (all): **77 passed / 0 failed** — matches prior baseline exactly.
  - `tests_root_cause` + `tests_gap_scan` + `tests_memory_recall` + `tests_whisper`:
    **102 passed / 0 failed**.
- All file:line citations below were read directly from live `src/` at HEAD,
  not trusted from prior docs.

---

## 1. Re-verified citation table (read directly at HEAD 3fac68a5)

| Claim | Cited loc | Status |
|---|---|---|
| `RECURRING_SIGNATURE_THRESHOLD = 2` (Lane A) | `overseer/signal.rs:362` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` (Lane B) | `overseer/root_cause.rs:33` | ✅ exact |
| `INVESTIGATED_BREAKER_THRESHOLD = NO_PROGRESS_BREAKER_THRESHOLD` | `ooda_loop/no_progress.rs:1148` | ✅ exact |
| `decide_blocked_goal` rung ladder | `overseer/mod.rs:1603-1631` | ✅ exact |
| Escalation gated only at `recurrence >= 3` | `overseer/mod.rs:1613` | ✅ exact |
| Default arm = `Intervention::Report` (no-op) | `overseer/mod.rs:1630` | ✅ exact |
| WorkstreamCoverage → `FlagWorkstreamGaps` (no launch/issue) | `overseer/mod.rs:1534-1543` | ✅ exact |
| `act_flag_workstream_gaps` = peek/dedup + ONE notify + commit | `overseer/mod.rs:884-948` | ✅ exact |
| Header: "Routine observations never create GitHub issues or backlog items" | `overseer/mod.rs:881-883` | ✅ exact |
| WorkstreamCoverage Problem `dedup_key = "workstream-gap"` (bare) | `overseer/mod.rs:1371` | ✅ exact |
| `observation_signature` = sort→dedup→`overseer-obs:{join("\|")}` | `overseer/mod.rs:1068-1073` | ✅ exact |
| WHY outer gate `no_progress_investigation_enabled()` | `ooda_loop/cycle.rs:583` | ✅ exact |
| WHY inner gate `INVESTIGATED_BREAKER_THRESHOLD` | `ooda_loop/cycle.rs:607,635` | ✅ exact |

**No stale citations.** Both prior secondary artifacts stand verbatim at HEAD.

---

## 2. Loop A — blocked-goal resolution ladder: DEAD ZONE CONFIRMED

`decide_blocked_goal` (`mod.rs:1603-1631`) is the ONLY closing path for a
`goal:blocked:*` problem. Read directly at HEAD, its arms in order:

```
recurrence >= 3 (Lane B)         → EscalateBlockedGoal   (mod.rs:1613)  [notify-only]
perpetual && is_no_progress_marker → UnblockGoal          (mod.rs:1620-1621)
needs_review                     → EscalateBlockedGoal    (mod.rs:1623-1628)
else                             → Report (no-op)         (mod.rs:1630)
```

- **Dead zone = `recurrence ∈ {0,1,2}`** for any goal that is neither a
  `perpetual + no_progress_marker` false-park nor `needs_review`. It lands on
  rung 4 `Report` — a no-op — **every cycle, forever**. Every
  `goal:blocked:<slug>-<hash>` token in the queried signature (kgpacks
  #12/#17/#18/#23/#25, simard-identity personas, coverage-to-70, coin harness)
  sits here.
- **The dead zone gates OFF the resolution ladder.** Two nested WHY gates
  (`cycle.rs:583` outer kill-switch; `cycle.rs:607,635` inner
  `INVESTIGATED_BREAKER_THRESHOLD`) mean a goal observed 2× has not cleared the
  breaker floor, so it authors no terminal transition. Even the ≥3 rung is
  **non-closing**: `EscalateBlockedGoal` is a notification, not a block-removing
  action.
- **New emphasis (from 440e024c §2.1, re-confirmed):** the "raise-priority" rung
  the strategy assumed exists is **structurally unreachable for the composite**.
  The `orient` merge predicate `p.dedup_key == key` (`mod.rs:1211`) can never
  match an `overseer-obs:` composite key against a per-goal `goal:blocked:` key
  (`mod.rs:1359` vs the per-goal key), so the 2× exerts **zero** priority
  pressure on the actual goals. Dead zone is "priority never raised AND never
  escalated," not merely "raised but not escalated."

**Loop A is confirmed unclosed.**

---

## 3. Loop B — workstream-gap arm: MISSING LAUNCH EDGE CONFIRMED

`WorkstreamCoverage` (`mod.rs:1534-1543`) is the **only** "work-exists /
work-uncovered" ProblemKind routing to neither `LaunchRecipe` nor `FileIssue`.
Its Act handler `act_flag_workstream_gaps` (`mod.rs:884-948`), read at HEAD,
does exactly three things:

1. **Peek + dedup** every gap against `gap_gate` keyed
   `format!("workstream-gap:{}", g.signature)` (`mod.rs:900-908`) — per-gap
   signature at the *gate*.
2. Send **ONE** consolidated operator notification for the fresh gaps
   (`mod.rs:929-930`).
3. **Commit** each fresh gap to the gate (`mod.rs:931-934`).

**There is no edge into `launch.rs` / `caps.recipes.launch` and no `FileIssue`.**
The header comment makes the choice explicit (`mod.rs:881-883`). The Decide→Act
path terminates at a notification; the OODA loop never closes.

**Contrast that proves the hole (all read at `mod.rs:1402-1580`):**
DeliveryReady→VerifyAndMergePr, QualityRegression→FileIssue,
ProcessHealth→LaunchRecipe, CrossCutting→LaunchRecipe,
StepFailure→LaunchRecipe all converge. Only WorkstreamCoverage (and the global
ResourcePressure→Escalate) are notify-only. **The convergence machinery already
exists and is exercised by ≥4 sibling arms — the fix REUSES it, it does not
build it.**

**INV-GAP-KEY (re-confirmed live):** although the *gate* keys per-gap
(`g.signature`), the WorkstreamCoverage **Problem** carries the bare
`dedup_key = "workstream-gap"` (`mod.rs:1371`). Any closing-edge ledger MUST key
on `GapItem.signature`, NOT this bare dedup_key, or all distinct gaps fold into
one launched unit / one issue.

**Loop B is confirmed unclosed.**

---

## 4. The 2-vs-3 dead-zone semantics — two decoupled lanes, hardcoded

| Lane | Threshold | Location | Counts |
|---|---|---|---|
| A — episodic recall | `= 2` | `signal.rs:362` | recalled write-back **episodes** with byte-identical `failure_signature` |
| B — semantic root cause | `= 3` | `root_cause.rs:33` | recalled `PriorOccurrence`s with the same `cause_label` |

- The `2×` in the queried signature is **Lane A**. The escalation gate in
  `decide_blocked_goal` reads **Lane B** (`recurrence`, `mod.rs:1613`).
  **They never share a counter** — this is a cross-lane visibility gap, not a
  single mis-set threshold.
- **Dead zone = count sits above one-off noise (Lane A `2`) but below the
  escalation floor (Lane B `3`) with no remediation rung in between.** A
  first-*proven* recurrence produces neither a raised priority (unreachable, §2)
  nor an escalation nor a launch — only another `Report`.
- **Configurable vs hardcoded — ANSWERED:** both thresholds are
  `pub const … : u32` compile-time constants. `grep` of `config.rs` and
  `tuning.rs` for either name returns **nothing** (exit 1). **The escalation
  threshold is HARDCODED, not runtime-configurable.** Any tuning requires a code
  change + recompile; there is no operator knob to move the 3 down to 2.

---

## 5. One root problem (oscillation) — corroborated

An under-resourced standing goal oscillates: **active** → `WorkstreamGap` →
Loop B (notify-only); **idle/parked** → `GoalBlocked` → Loop A
(report/dead-zone). Neither arm removes the underlying condition, so the same
episode re-observes indefinitely, alternating tokens — the interleaved
`goal:blocked:<slug>-<hash>` runs and `workstream-gap|workstream-gap` runs, all
nested under `overseer-obs:`. The `workstream-gap|workstream-gap` **doubling**
is NOT two distinct gap keys (the Problem dedup_key is the single bare
`workstream-gap`, `mod.rs:1371`); it is the D1 self-feed nesting
(`sanitize_recalled` recall re-entry, `mod.rs:1359`) — a primary-owned
mechanism. Treat as ONE resourcing/convergence problem, not two counting bugs.
The `2×` is an HONEST re-observation count; the defect is the missing closing
action, not the counter.

---

## 6. Patterns / anti-patterns discovered

- **Anti-pattern — "Observe-and-flag without a closing action":** Loop B
  (`act_flag_workstream_gaps`) notifies but never launches/files. The only
  fix that makes the signal trend to zero is a convergence rung.
- **Anti-pattern — "Recurrence dead zone":** a proven recurrence (2×) that is
  above noise but below the escalation floor (3) with no intermediate rung.
- **Pattern — "Classify-then-route the stall, don't park it":** the `Report`
  default arm is *correct* for a deliberate operator/upstream block; the defect
  is the code cannot distinguish benign-park from genuinely-stuck-but-unmarked,
  so both collapse to `Report`. Any 2× rung must be gated on the WHY class, and
  the WHY reasoner (which today fails open to bare-park) must be un-gated as a
  coupled pair.
- **Anti-pattern — "Threshold buried as a compile-time constant":** the
  escalation floor is unreachable to operators; recurrence policy cannot be
  tuned without a rebuild.

---

## 7. Integration points

- **OODA Decide table** (`mod.rs:1402-1580`) — the shared routing surface where
  the missing WorkstreamCoverage launch edge would attach, reusing the existing
  `LaunchRecipe`/`FileIssue` machinery (`mod.rs:1429-1435`).
- **WHY reasoner / breaker** (`ooda_loop/cycle.rs:582-702`,
  `no_progress.rs:1148`) — the double gate feeding the Lane-B accrual the
  `decide_blocked_goal` escalation reads.
- **Signature write-back** (`mod.rs:1068-1073`, `wiring.rs:301`) — over-aggregates
  the whole cycle into one composite; the substrate for both the unreachable
  merge (§2) and the D1 self-feed (§5). Primary-owned, cross-referenced only.

---

## 8. Advisory remediation shape (investigation-only; nothing landed)

Reuses existing convergence machinery; do NOT redesign the OODA loop.

1. **Close Loop A dead zone (atomic).** Insert a rung between `Report` and the
   ≥3 escalation in `decide_blocked_goal` (`mod.rs:1613-1630`): at first *proven*
   recurrence (Lane-A `2×`) for a `goal:blocked` whose WHY class carries no
   benign explanation, route to a launched/filed unit of work. Gate + counter
   MUST ship together. **Do NOT** use the literal
   `store_fact_with_caller_key(root_cause_signature)` remedy — `DedupMode::CallerKey`
   collapses recall to 1 forever and makes `recurrence>=3` dead code
   (RECONCILIATION_LEDGER §2). Use a **count-in-content upsert** (`occurrence_count`
   + first/last_seen; escalation reads the field, not `recall.len()`).
2. **Close Loop B.** Add a `LaunchRecipe`/`FileIssue` edge to the
   WorkstreamCoverage arm (`mod.rs:1534-1543`), guarded so first-sight gaps stay
   on the notify path and only proven-recurring gaps launch/file. **Key the
   closing-edge ledger on `GapItem.signature`, NOT the bare `workstream-gap`
   dedup_key** (`mod.rs:1371`) — INV-GAP-KEY trap.
3. **(Optional) Make the escalation floor configurable** so the 2-vs-3 policy is
   an operator knob, not a recompile.

**Landing order:** dead-zone rung (atomic) → Loop-B closing edge (INV-GAP-KEY
guarded) → optional config knob. Each guarded by an existing green suite
(no_progress 77, tests_root_cause / gap_scan / memory_recall / whisper 102).

---

## 9. Questions for the verification phase

1. Assert empirically that the `orient` merge at `mod.rs:1211` NEVER matches an
   `overseer-obs:` composite key against a `goal:blocked:` per-goal key (static
   reading says never; feed both in a unit test to confirm the dead zone's
   "priority never raised" claim).
2. Confirm any new 2× / launch rung respects the anti-issue-storm guardrail —
   one launch/issue per **gap signature** (not per cycle) via a
   `gap_gate`-equivalent signature-keyed launch gate.
3. Confirm the proposed 2× rung would NOT swallow deliberate operator blocks —
   i.e. the `Report` default is only reached for genuinely-benign blocks once a
   WHY class is wired.
4. Decide whether the hardcoded escalation floor (`root_cause.rs:33`) should be
   promoted to `config.rs`/`tuning.rs` so recurrence policy is tunable without a
   rebuild.

**Verdict (secondary):** Both OODA arms are CONFIRMED non-closing at HEAD
`3fac68a5`; the 2-vs-3 dead zone is CONFIRMED and deeper than "no rung"
(raise-priority is structurally unreachable for the composite); the escalation
threshold is HARDCODED (no config/tuning override). Zero source drift; baseline
suites green (no_progress 77, overseer 102). Investigation-only — no change
landed.
