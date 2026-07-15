# Secondary Investigation — Two Non-Closing Loops, Self-Reference, engineer_spawn

**Role:** Secondary investigator (patterns focus)
**HEAD grounding:** `f455c06de53a871165dbc671a18d62e7b47b974f`
**Prior grounding:** `ad5e1060` (secondary_two_loops_and_drift_HEAD_ad5e1060.md);
cited remedy HEAD `dea65df8` (RECONCILIATION_LEDGER.md)
**Focus:** the double-gated resolution ladder + notify-only WorkstreamCoverage
arm, the self-reference verdict, and `resource:engineer_spawn` classification.

**Verdict (one line):** Every load-bearing citation re-verifies EXACTLY at
current HEAD; production `.rs` has NOT drifted; both observe-and-flag loops are
confirmed non-closing; the signature IS self-referential; `engineer_spawn` is
benign membership drift. **Extend the prior investigation — do not restart.**

---

## 0. Source-drift verification (priority-4 deliverable, part 1)

- `git diff --stat ad5e1060 HEAD -- src/overseer/ src/ooda_loop/` → **empty**.
  Production Overseer/OODA source is byte-identical to the prior secondary
  grounding.
- `git diff --stat dea65df8 HEAD -- src/overseer/ src/ooda_loop/` → **only**
  `src/overseer/tests_root_cause.rs` (+99 lines, **test-only**). No production
  logic changed; the two lane-labelling tests it adds (Lane A =
  `RECURRING_SIGNATURE_THRESHOLD=2`, Lane B = `RECURRENCE_ESCALATION_THRESHOLD=3`)
  merely encode the prior finding.
- The intervening commits since `dea65df8` are documentation-only investigation
  consolidations. **All previously-identified defects remain live.**

**Regression baseline (run at HEAD f455c06d):** no_progress **77 passed**,
tests_whisper **28 passed**, tests_memory_recall **32 passed**, tests_root_cause
**21 passed**; 0 failed. The advisory landing order below is regression-anchored
to these suites.

---

## 1. Re-verified citation table (independently read at HEAD f455c06d)

| Claim | Cited loc | Status |
|---|---|---|
| `decide()` single Orient→Decide table | `overseer/mod.rs:1400-1580` | ✅ exact |
| WorkstreamCoverage → `FlagWorkstreamGaps` (notify-only, no launch/issue edge) | `overseer/mod.rs:1534-1543` | ✅ exact |
| `act_flag_workstream_gaps` = peek/dedup + ONE notification + commit; never launches/files | `overseer/mod.rs:884-948` | ✅ exact |
| Header: "Routine observations never create GitHub issues or stewardship backlog items" | `overseer/mod.rs:881-883` | ✅ exact |
| ResourcePressure → `Escalate` (notify-only) | `overseer/mod.rs:1444-1446` | ✅ exact |
| `decide_blocked_goal` rung ladder | `overseer/mod.rs:1603-1631` | ✅ exact |
| Escalation only at `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` | `overseer/mod.rs:1613` | ✅ exact |
| `RECURRENCE_ESCALATION_THRESHOLD = 3` | `overseer/root_cause.rs:33` | ✅ exact |
| `RECURRING_SIGNATURE_THRESHOLD = 2`; emit at `occurrences >= 2` | `overseer/signal.rs:362,463` | ✅ exact |
| `observation_signature` = sort→dedup→`overseer-obs:{join("\|")}` | `overseer/mod.rs:1068-1073` | ✅ exact |
| `write_back_observation(&cycle.problems)` at write boundary | `overseer/wiring.rs:301` | ✅ exact |
| `EngineerSpawnRate` → `resource:engineer_spawn`, ResourcePressure, Priority::Normal | `overseer/mod.rs:1267-1272` | ✅ exact |
| sig map `engineer_spawn` | `overseer/capabilities.rs:562` | ✅ exact |
| WHY double-gate (investigated breaker under kill-switch) | `ooda_loop/cycle.rs:582-702` | ✅ exact |
| `INVESTIGATED_BREAKER_THRESHOLD` inner gate | `ooda_loop/no_progress.rs:1148`; `cycle.rs:607,635` | ✅ exact |

**No stale citations.** Prior secondary artifact stands verbatim at HEAD.

---

## 2. Loop A — the double-gated resolution ladder (ooda_loop/cycle.rs)

Two nested gates starve the escalation-counter lane (Lane B), so a blocked goal
parks between "silently report" and "escalate at 3×":

- **Outer gate** — `no_progress_investigation_enabled()` (`cycle.rs:583`) wraps
  the entire investigated breaker + `reinvestigate_bare_blocked_goals` pass. When
  off (kill-switch), the loop falls back to the base verify-once ladder
  (`cycle.rs:684-698`) that never analyzes WHY.
- **Inner gate** — `INVESTIGATED_BREAKER_THRESHOLD` (`cycle.rs:607,635`;
  = `NO_PROGRESS_BREAKER_THRESHOLD`, `no_progress.rs:1148`) requires N verified
  no-progress cycles before the breaker fires. A goal observed 2× has not yet
  cleared this floor, so it authors no terminal transition.
- **Decide ladder** (`decide_blocked_goal`, `mod.rs:1603-1631`): rung 1 escalates
  only at `recurrence >= 3`; rung 2 (`UnblockGoal`) is the **only closing rung**
  and fires only for a `perpetual && is_no_progress_marker` false-park; rung 3
  escalates on `needs_review`; **rung 4 = `Report`** (untouched).

**Dead zone (confirmed):** a blocked goal at recurrence 1–2 that is neither a
perpetual false-park nor `needs_review` lands on rung 4 `Report` — no
remediation, no escalation. Every `goal:blocked:<slug>-<hash>` token in the
signature (kgpacks #12/#17/#18/#23/#25, simard-identity personas, coverage-to-70,
coin harness) sits in this zone. Even the ≥3 rung is **non-closing**:
`EscalateBlockedGoal` is a notification (`mod.rs:814-834`), not a block-removing
action. **Loop A is confirmed unclosed.**

---

## 3. Loop B — the notify-only WorkstreamCoverage arm (no launch.rs edge)

`WorkstreamCoverage` (`mod.rs:1534-1543`) is the **only** "work exists / work
uncovered" ProblemKind that routes to neither `LaunchRecipe` nor `FileIssue`. Its
Act handler `act_flag_workstream_gaps` (`mod.rs:884-948`) does exactly three
things: peek+dedup each gap against `gap_gate` (`mod.rs:900-908`), send ONE
consolidated operator notification for the fresh gaps (`mod.rs:929-930`), and
commit each fresh gap to the gate (`mod.rs:931-934`). **There is no edge into
`launch.rs` / `caps.recipes.launch` and no `FileIssue`.** The header comment
makes the choice explicit (`mod.rs:881-883`).

**Contrast that proves the hole:** DeliveryReady→VerifyAndMergePr,
QualityRegression→FileIssue, ProcessHealth→LaunchRecipe, CrossCutting→
LaunchRecipe, StepFailure→LaunchRecipe (`mod.rs:1402-1580`) all converge. Only
WorkstreamCoverage (and the global ResourcePressure→Escalate) notify-only. The
convergence machinery already exists and is exercised by 4 sibling arms — the
fix REUSES it.

**Lane-B durability sub-concern:** `gap_gate = WhisperGate::new(900, 200)` is
in-memory per-process (`mod.rs:201,304`); a daemon restart clears it, so a gap
re-notifies immediately regardless of the 900 s window. This is honest within a
process lifetime but not durable across restarts. It affects notification
volume, not persistence — even a perfect durable gate leaves the gap terminal
because nothing closes it. **Loop B is confirmed unclosed.**

---

## 4. Self-reference verdict — CONFIRMED at the WRITE boundary

The signature IS the Overseer observing its own write-backs. The exact geometry:

1. `write_back_observation(&cycle.problems)` (`wiring.rs:301` → `mod.rs:534-546`)
   computes `observation_signature(problems)` over **all** problems with **NO
   filter** excluding recall-derived meta-problems.
2. A recalled `Signal::RecurringSignature { signature, .. }` maps to a
   `ProcessHealth` problem whose `dedup_key = sanitize_recalled(signature)`
   (`mod.rs:1353-1359`). The mapping comment states the dedup_key IS the recalled
   signature (`mod.rs:1346-1352`).
3. In `orient`, that signal either MERGES into a same-key problem (`mod.rs:1211`,
   raising its priority) or becomes a standalone problem whose `dedup_key` is the
   recalled `overseer-obs:...` string (`mod.rs:1222`).
4. `observation_signature` then joins those dedup_keys → so a prior
   `overseer-obs:...` write-back is **nested into** the next `overseer-obs:...`
   write-back. This is precisely the observed nested structure
   `overseer-obs:goal:blocked:…|overseer-obs:goal:blocked:…`.

**Fix boundary (advisory):** exclude recall-derived meta-problems (those
originating from `Signal::RecurringSignature`, i.e. dedup_key prefixed
`overseer-obs:`) from `problems` **before** computing `observation_signature`
— either inside `write_back_observation` (`mod.rs:546`) or at the call site
(`wiring.rs:301`). This is defect **D1**. It changes only what gets re-persisted,
not recall/orient priority-raising, so it is regression-safe against
`tests_memory_recall`.

---

## 5. `resource:engineer_spawn` — new problem vs membership drift

**Verdict: benign membership drift into the same dedup_key set. NOT a new problem
and NOT a contradicting signal.**

- Minted from `Signal::EngineerSpawnRate { live }` → token
  `"resource:engineer_spawn"`, `ProblemKind::ResourcePressure`, `Priority::Normal`
  (`mod.rs:1267-1272`); sig-mapped `engineer_spawn` (`capabilities.rs:562`).
- Routes to `Intervention::Escalate` (`mod.rs:1444-1446`) — a **global**
  spawn-pressure escalation, notify-only. Fires **at-and-above** an 8-live
  threshold only (`tests_m1.rs:133-149`, verified green).
- It enters/leaves the active set as the live-engineer count crosses threshold,
  so it appears as an extra `dedup_key` folded into the same
  `observation_signature` join. It does **not** reset recurrence counting: the
  Lane-A visible count keys on the composite signature string, and Lane-B keys on
  `root_cause_signature = "{dedup_key}::{label}"` (`root_cause.rs:53-55`) per
  problem — a new independent token adds its own key, it does not perturb the
  existing goal:blocked / workstream-gap keys' counters.
- **No causal edge** couples `EngineerSpawnRate` to `WorkstreamGap`: the grep of
  `engineer_spawn`/`EngineerSpawnRate` shows only the independent
  signal→token→escalate chain (signal.rs, root_cause.rs:326, observer.rs:206,
  capabilities.rs:562, mod.rs:1267). The overlap ("uncovered work AND too many
  engineers live") is a legitimate resource-allocation tension at **different
  seams** (per-goal coverage vs global admission cap), not a defect. **Do NOT
  build a theory coupling them.**

Consistent with `secondary_token_provenance_membership_delta_HEAD_388e6c29.md`.

---

## 6. The two loops are ONE root problem (oscillation)

An under-resourced standing goal oscillates: **active** → `WorkstreamGap` → Loop B
(notify-only); **idle/parked** → `GoalBlocked` → Loop A (report/dead-zone).
Neither arm removes the underlying condition, so the same episode re-observes
indefinitely, alternating tokens — the interleaved `goal:blocked:<slug>-<hash>`
runs and `workstream-gap|workstream-gap` runs, all nested under `overseer-obs:`.
Treat as ONE resourcing/convergence problem, not two counting bugs. The "2×" is
an HONEST re-observation count (primary's domain); the defect is the missing
closing action, not the counter.

---

## 7. Advisory remediation shape (no code changes — landing-order-safe)

Reuses existing convergence machinery; do NOT redesign the OODA loop.

1. **D2 (atomic) — close the dead zone in Loop A.** Add a rung between `Report`
   and `EscalateBlockedGoal(≥3)` in `decide_blocked_goal` (`mod.rs:1613-1630`)
   that, at first *proven* recurrence (2×, no benign explanation), routes to a
   launched/filed unit of work. The gate + counter must ship together or nothing
   changes. **Do NOT** use the literal `store_fact_with_caller_key(root_cause_
   signature)` remedy — `DedupMode::CallerKey` collapses recall to 1 forever and
   makes escalation dead code (RECONCILIATION_LEDGER §2). Use a **count-in-content
   upsert** (`occurrence_count` + first/last_seen; escalation reads the field, not
   `recall.len()`).
2. **D3 — close Loop B.** Add a `LaunchRecipe`/`FileIssue` edge to the
   `WorkstreamCoverage` arm (`mod.rs:1534-1543`) reusing `mod.rs:1429-1435`
   machinery, guarded so **first-sight** gaps stay on the notify path and only
   **proven-recurring** gaps launch/file. **Key the closing-edge ledger on
   `GapItem.signature`, NOT the bare `workstream-gap` dedup_key** (`mod.rs:1371`),
   or all gaps fold into one issue (INV-GAP-KEY trap).
3. **D1 — cut the self-feed.** Filter recall-derived `overseer-obs:` meta-problems
   before `observation_signature` (§4). Regression-safe against
   `tests_memory_recall`.
4. Optionally persist `gap_gate` across restarts (durability sub-concern §3) — a
   signature-keyed idempotent upsert at `mod.rs:201,304,900-934`, not a counter
   change.

**Landing order:** D2 (gate+counter, atomic) → D3 (closing rung + INV-GAP-KEY
guard) → D1 (write-back filter) → durability/convergence gauges. Each step is
guarded by an existing green suite (no_progress, tests_root_cause,
tests_whisper, tests_memory_recall).

---

## 8. Questions for the verification phase

1. Confirm the new D2/D3 rungs respect the anti-issue-storm guardrail (one
   launch/issue per gap **signature**, not per cycle) via a `gap_gate`-equivalent
   signature-keyed launch gate.
2. Confirm the D1 write-back filter does not suppress LEGITIMATE recall-driven
   priority-raising in `orient` (`mod.rs:1217-1219`) — it must filter only at the
   WRITE (re-persist) boundary, not at the READ/merge boundary.
3. Confirm `engineer_spawn` remains uncoupled after any resourcing rung lands (no
   accidental edge from a spawn-based remediation back into WorkstreamGap).
