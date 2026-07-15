# Secondary Investigation — Emission Pipeline Trace, Self-Ingestion Loop & the Two Never-Closing Loops

**Role:** Secondary investigator (patterns / pipeline & loop-closure focus)
**HEAD grounding:** `7293de99d989a0a3735426e4faf61946e918bb14`
**Prior grounding:** `3fac68a5` (secondary_ooda_closure_and_deadzone), `f455c06d`,
`440e024c`. **Extends prior work — does not restart it.**

**Verdict (one line):** The `2×` is an HONEST re-observation count from a
faithful, sorted/deduped, in-window-deduped write-back; the defect (if any) is
(a) a **self-ingestion feedback loop** that re-nests the Overseer's own
`overseer-obs:` signature into memory it later recalls, and (b) **two OODA arms
that observe-and-flag but never close** — the blocked-goal resolution ladder
(dead zone at recurrence ∈ {0,1,2}) and the workstream-gap arm (notify-only, no
launch/issue edge). Not a counting/dedup bug.

---

## 0. Drift verification (validate, don't re-derive)

- `git diff 3fac68a5..HEAD -- src/` → **EMPTY**. Production source is
  byte-identical to the prior secondary grounding; HEAD advanced only by
  documentation-only commits. Every file:line below was read live at HEAD.
- The two intervening commits (`docs(investigation)…`) touch only
  `ai_working/investigation/*.md`.

---

## 1. End-to-end emission pipeline — traced live at HEAD

The queried string `overseer-obs:goal:blocked:…|workstream-gap|resource:engineer_spawn`
is produced by ONE composite path. Named functions with line refs:

| # | Stage | Function / site | Line | What it does |
|---|-------|-----------------|------|--------------|
| 1 | **run_cycle / Observe** | `Overseer::run_cycle` | `mod.rs:384` | Snapshots status, board (`blocked_goals`, `in_flight`), gap-scan (`workstream_gaps`), drained step failures. |
| 2 | **USE / recall** | `recall_pass` | `mod.rs:498` | Bounded, fail-closed episodic/semantic recall keyed off **pre-recall** problems (`mod.rs:425` `orient(&pre_signals,…)`). Fills `observed.recall`. |
| 3 | **Signals** | `signals_from` → `signal.rs` | `mod.rs:441` | Converts `observed` to `Signal`s — **now including any recall-derived `RecurringSignature`**. |
| 3a| **RecurringSignature emit** | recall-count arm | `signal.rs:462-468` | When ≥ `RECURRING_SIGNATURE_THRESHOLD` (=2, `signal.rs:362`) recalled episodes share a `failure_signature`, pushes `Signal::RecurringSignature{ signature, occurrences }`. |
| 3b| **The queried summary string** | `signal_to_problem` display arm | `signal.rs:644-647` | `format!("recurring failure signature '{signature}' seen {occurrences} time(s)")` — **this is literally the investigation-question title**. |
| 4 | **Orient** | `orient` | `mod.rs:1200` | Pure (`why=None`); emits `Problem`s with `dedup_key`s. Token literals: `goal:blocked:{goal_id}` (`mod.rs:1336`), bare `workstream-gap` (`mod.rs:1371`), `resource:engineer_spawn` (`mod.rs:1270`). |
| 5 | **Root-cause enrich** | `root_cause::analyze` loop | `mod.rs:455-459` | Attaches WHY per problem (best-effort recall). |
| 6 | **Decide + gate** | `decide`/`gate` loop | `mod.rs:466-480` | Plans interventions per problem. |
| 7 | **Write-back (composite assembly)** | `write_back_observation` | `mod.rs:534` | Called from `wiring.rs:301` with `&cycle.problems`. |
| 7a| **Composite signature** | `observation_signature` | `mod.rs:1068-1073` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` — sorted, deduped, `\|`-joined `dedup_key`s. **This is the outer `overseer-obs:` frame.** |
| 7b| **Idempotency gate** | `write_back_gate.peek/commit` | `mod.rs:548,556` | `WhisperGate::new(900, 5)` (`mod.rs:299`); 900 s dedup window, 5/hr cap. |
| 8 | **Store** | `record_observation` → `store_episode` | `mod.rs:554` → `cognitive_memory/mod.rs:231` | Persists ONE `ObservationEpisode{ content, signature }` whose `signature` == the `overseer-obs:` composite. |

**Verified emission of the exact string:** `signal.rs:647` is the emitter of
"`recurring failure signature '…' seen N time(s)`". The `overseer-obs:` frame is
assembled at `mod.rs:1068-1073`. Both are load-bearing and re-verify verbatim.

---

## 2. Self-ingestion feedback loop (D1) — CONFIRMED at the write boundary

The write-back is fed by `&cycle.problems`, and `cycle.problems` already
contains recall-derived meta-problems (the `RecurringSignature` promoted in
stage 3a). Concretely:

- Stage 2 recall (`mod.rs:498`) reads stored episodes whose `failure_signature`
  is a **prior `overseer-obs:` composite** (written at stage 8 on an earlier
  tick).
- Stage 3a re-raises it as `RecurringSignature` (`signal.rs:462-468`); stage 4
  turns it into a problem with a recall-derived `dedup_key`.
- Stage 7a folds THAT key back into the next `overseer-obs:` composite
  (`mod.rs:1068-1073`) and stage 8 re-stores it.

⇒ Each cycle can **nest** the prior `overseer-obs:` fragment inside the next,
which is exactly why the queried signature shows repeated
`overseer-obs:goal:blocked:…` runs. **Anti-pattern: "Self-observation feedback"
(PATTERNS.md:51).** The clean fix is at the WRITE boundary: exclude
recall-derived meta-problems from `observation_signature` input, not at the
counter.

**Durability note (restart-driven double-record):** the write-back gate's
`last_delivered` is an in-memory `HashMap` (`guardrails.rs:294`), reset every
process start. So the *second* store of an identical `overseer-obs:` composite is
an honest re-observation across either a `>900 s` window OR a daemon restart —
**not** a dedup miss. **Anti-pattern: "Missing storage-layer idempotency"
(PATTERNS.md:65)** — a signature-keyed durable upsert (as PR #2298 did for
procedures) would collapse restart-driven double records.

---

## 3. Loop A — blocked-goal resolution ladder: DEAD ZONE CONFIRMED

`decide_blocked_goal` (`mod.rs:1603-1631`) is the ONLY closing path for a
`goal:blocked:*` problem. Arms in order (read live):

```
recurrence >= 3 (RECURRENCE_ESCALATION_THRESHOLD)  → EscalateBlockedGoal (mod.rs:1613)  [notify-only]
perpetual && is_no_progress_marker(reason)         → UnblockGoal          (mod.rs:1620-1621)
needs_review                                       → EscalateBlockedGoal  (mod.rs:1623-1628)
else                                               → Report (no-op)       (mod.rs:1630)
```

- **Dead zone = `recurrence ∈ {0,1,2}`** for any goal that is neither a
  `perpetual + no_progress_marker` false-park nor `needs_review`. It lands on
  the `Report` no-op **every cycle, forever**. Every `goal:blocked:<slug>-<hash>`
  token in the signature (kgpacks #12/#17/#18/#23/#25, simard-identity personas,
  coverage-to-70, coin harness) sits here.
- The ladder is further gated OFF upstream by the **WHY double-gate**:
  outer kill-switch `no_progress_investigation_enabled()` (`cycle.rs:583`) and
  inner `INVESTIGATED_BREAKER_THRESHOLD` (`cycle.rs:607,635`). A goal seen 2×
  has not cleared the breaker floor, so it authors no terminal transition and
  degrades to a bare "needs human review" park — **Pattern: "Classify-then-route
  the stall, don't park it" (PATTERNS.md:29)** is violated.
- Even the `≥3` rung is **non-closing**: `EscalateBlockedGoal` is a
  notification, not a block-removing action.

**Loop A confirmed unclosed.**

---

## 4. Loop B — workstream-gap arm: MISSING LAUNCH EDGE CONFIRMED

`WorkstreamCoverage` (`mod.rs:1534-1543`) routes to `Intervention::FlagWorkstreamGaps`
— the ONLY "work-uncovered" ProblemKind routing to neither `LaunchRecipe` nor
`FileIssue`. Its Act handler `act_flag_workstream_gaps` (`mod.rs:884-948`) does
exactly three things:

1. **Peek + dedup** each gap against `gap_gate` keyed
   `format!("workstream-gap:{}", g.signature)` (`mod.rs:901,932`).
2. Send **ONE** consolidated operator notification for the fresh gaps
   (`mod.rs:929-930`).
3. **Commit** each fresh gap to the gate (`mod.rs:931-934`).

**No edge into `launch.rs` and no `FileIssue`.** The header comment makes the
choice explicit: "Routine observations never create GitHub issues or
stewardship backlog items" (`mod.rs:881-883`). **Anti-pattern:
"Observe-and-flag without a closing action" (PATTERNS.md:6).**

**Contrast that proves the hole:** DeliveryReady→VerifyAndMergePr,
QualityRegression→FileIssue, ProcessHealth→LaunchRecipe,
CrossCutting→LaunchRecipe, StepFailure→LaunchRecipe all converge. Only
WorkstreamCoverage (and global ResourcePressure→Escalate) are notify-only. **The
convergence machinery already exists and is exercised by ≥4 sibling arms — a fix
REUSES it.**

**INV-GAP-KEY trap:** the *gate* keys per-gap (`g.signature`) but the
WorkstreamCoverage **Problem** carries the bare `dedup_key = "workstream-gap"`
(`mod.rs:1371`). Any closing-edge ledger MUST key on `GapItem.signature`, not the
bare dedup_key, or all distinct gaps fold into one launched/filed unit. This
bare-key literal also explains why `workstream-gap|workstream-gap` doubling is
NOT two distinct gap keys — it is the §2 self-feed nesting.

**Loop B confirmed unclosed.**

---

## 5. The 2-vs-3 dead zone — two decoupled lanes, hardcoded

| Lane | Threshold | Location | Counts |
|---|---|---|---|
| A — episodic recall | `= 2` | `signal.rs:362` | recalled write-back **episodes** sharing a byte-identical `failure_signature` |
| B — semantic root cause | `= 3` | `root_cause.rs:33` | recalled `PriorOccurrence`s sharing a `cause_label` |

- The `2×` is **Lane A**. The escalation gate in `decide_blocked_goal` reads
  **Lane B** `recurrence` (`mod.rs:1613`). **They never share a counter** —
  cross-lane visibility gap, not a single mis-set threshold.
- **Dead zone = above one-off noise (Lane A `2`) but below the escalation floor
  (Lane B `3`) with no remediation rung in between.** A first-*proven* recurrence
  yields another `Report`. **Anti-pattern: "Recurrence dead zone"
  (PATTERNS.md:19).**
- Both thresholds are `pub const … : u32` compile-time constants; neither name
  appears in `config.rs`/`tuning.rs`. **The escalation floor is HARDCODED** —
  no operator knob. **Anti-pattern: "Threshold buried as a compile-time
  constant".**

---

## 6. One root problem (oscillation) — corroborated

An under-resourced standing goal oscillates: **active** → `WorkstreamGap` → Loop
B (notify-only); **idle/parked** → `GoalBlocked` → Loop A (report/dead-zone).
Neither arm removes the underlying condition, so the same episode re-observes
indefinitely, alternating tokens — the interleaved `goal:blocked:<slug>-<hash>`
runs and `workstream-gap|workstream-gap` runs, all nested under `overseer-obs:`.
`resource:engineer_spawn` (`mod.rs:1270`) is benign membership drift that
corroborates the under-resourcing, not a new defect. **Meta-pattern: "The
recurrence count is honest; audit the closing action, not the counter"
(PATTERNS.md:78).**

---

## 7. Integration points

- **OODA Decide table** (`mod.rs:1402-1580`) — shared routing surface where the
  missing WorkstreamCoverage launch edge attaches, reusing existing
  `LaunchRecipe`/`FileIssue` machinery.
- **WHY reasoner / breaker** (`cycle.rs:582-702`, `no_progress.rs:1148`) — the
  double gate feeding the Lane-B accrual the escalation reads; must be un-gated
  as a coupled pair with any 2× rung.
- **Signature write-back** (`mod.rs:534`, `1068-1073`; `wiring.rs:301`) —
  over-aggregates the whole cycle into one composite; the substrate for both the
  self-feed (§2) and the unreachable priority merge.

---

## 8. Advisory remediation shape (investigation-only; nothing landed)

Reuses existing machinery; no OODA redesign.

1. **Cut the self-feed at the write boundary.** Exclude recall-derived
   meta-problems (`RecurringSignature`-sourced) from `observation_signature`
   input (`mod.rs:1068-1073` / the `&cycle.problems` passed at `wiring.rs:301`).
   Stops the `overseer-obs:` nesting.
2. **Durable idempotency.** Persist `write_back_gate.last_delivered` (or move to
   a signature-keyed durable upsert à la PR #2298). Eliminates restart-driven
   double records. **Do NOT** use `DedupMode::CallerKey` — it collapses recall to
   1 forever and makes `recurrence>=3` dead code (RECONCILIATION_LEDGER §2). Use
   a count-in-content upsert (`occurrence_count` + first/last_seen).
3. **Close Loop A dead zone (atomic).** Insert a rung between `Report` and the
   `≥3` escalation in `decide_blocked_goal` (`mod.rs:1613-1630`): at first proven
   recurrence for a `goal:blocked` whose WHY class carries no benign explanation,
   route to a launched/filed unit. Gate + counter ship together.
4. **Close Loop B.** Add a `LaunchRecipe`/`FileIssue` edge to WorkstreamCoverage
   (`mod.rs:1534-1543`), guarded so first-sight gaps stay on notify and only
   proven-recurring gaps launch/file. **Key the ledger on `GapItem.signature`,
   NOT the bare `workstream-gap`** (INV-GAP-KEY).
5. **(Optional)** Promote the escalation floor to `config.rs`/`tuning.rs`.

**Landing order:** self-feed cut → durable idempotency → dead-zone rung (atomic)
→ Loop-B closing edge (INV-GAP-KEY guarded) → optional config knob. Each guarded
by existing green suites (no_progress 77; tests_root_cause / gap_scan /
memory_recall / whisper 102).

---

## 9. Questions for the verification phase

1. Confirm empirically that `observation_signature` input includes
   `RecurringSignature`-derived problem keys on tick N+1 (feed a stored
   `overseer-obs:` episode into recall and assert the composite nests).
2. Assert `write_back_is_deduplicated_within_window` still passes and that the
   *only* way a second identical `overseer-obs:` store occurs is `>900 s` OR a
   fresh `WhisperGate` (restart) — proving honesty of the `2×`.
3. Confirm any 2× / launch rung respects the anti-issue-storm guardrail — one
   launch/issue per **gap signature** (not per cycle).
4. Confirm the proposed 2× rung does NOT swallow deliberate operator blocks
   (`Report` default only for genuinely-benign, WHY-classified blocks).
5. Decide whether the hardcoded escalation floor (`root_cause.rs:33`) should be
   promoted to config/tuning.

**Verdict (secondary):** Pipeline traced end-to-end and every emitter confirmed
verbatim at HEAD `7293de99` (zero src drift). The `2×` is an honest count. Two
real defects: (D1) self-observation feedback nesting at the write boundary, and
(D2) two non-closing OODA arms with a hardcoded 2-vs-3 recurrence dead zone. All
citations re-verified live; investigation-only, nothing landed.
