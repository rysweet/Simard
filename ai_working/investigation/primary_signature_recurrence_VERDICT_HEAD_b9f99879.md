# PRIMARY verdict — recurring `2×` signature: computation, recurrence & goal:blocked lifecycle

**Role:** PRIMARY investigator (this wave).
**HEAD:** `b9f99879` (`git diff 6b2bf5e1..HEAD -- src/overseer src/stewardship` is empty →
overseer/stewardship pipeline byte-identical to last code change; all citations current).
**Focus:** signature computation/recurrence (`signal.rs`, `dedup.rs`) + `goal:blocked`
lifecycle (`observer.rs`/`no_progress.rs`).
**Method:** independent line-by-line re-read of live source + ran the behavioral-oracle
test suites. **Reconciles with — does not restart —** the prior investigation.

---

## 0. Verdict (high confidence, empirically validated)

The recurring `overseer-obs:…|goal:blocked:…|workstream-gap|workstream-gap|resource:engineer_spawn`
signature is a **faithful, honest fingerprint of a static, unresolved problem set** —
a **real cross-window re-observation loop, NOT a dedup / counting / storage / hash bug.**
The `×2` count is correct. The bug is the **absence of a closing action**, plus a
**self-observation write-back** that nests the Overseer's own recall output back into memory.

Three independent defects, on three seams:

- **D1 — Self-observation feedback (open loop).** `recall_episodic`
  (`wiring.rs:1013-1031`) parses `[sig:…]` out of **every** episode, including the
  Overseer's own write-backs stored under `OVERSEER_SOURCE_LABEL = "overseer"`
  (`wiring.rs:952`, `record_observation` `wiring.rs:1076-1088`). **No source-label
  self-exclusion at the recall/count boundary.** The `overseer-obs:…` composite thus
  re-enters recall as a `failure_signature`, becomes a standalone `RecurringSignature`
  Problem, and gets folded into the next `observation_signature` — producing the nested
  `overseer-obs:…|overseer-obs:…` runs seen in the data.
- **D2 — Recurrence dead zone + append-only ratchet.** Emit threshold is `2`
  (`RECURRING_SIGNATURE_THRESHOLD`, `signal.rs:362`, emit at `>=2` `signal.rs:463`);
  escalation threshold is `3` (`RECURRENCE_ESCALATION_THRESHOLD`, `root_cause.rs:33`,
  gate at `mod.rs:1613`). A `2×` signal sits **above one-off noise, below escalation**,
  with no auto-remediation rung → recurs forever. The occurrence lane is written via
  **non-idempotent `store_fact`** (`mod.rs:1034`), an append-only ratchet with no
  signature-keyed upsert.
- **D3 — Notify-only routing hole.** The `WorkstreamCoverage` Decide arm
  (`mod.rs:1534-1543`) returns `Intervention::FlagWorkstreamGaps` only —
  `act_flag_workstream_gaps` (`mod.rs:671`, body ~`884`) emits a `workstream-gap`
  **notification** and launches no workstream / files no issue. Gaps are re-observed
  every tick.

---

## 1. Signature computation — how `×2` is produced (code-evidenced)

| Stage | Location | Behavior |
|---|---|---|
| Composite mint | `observation_signature` `mod.rs:1068-1073` | `keys.sort_unstable(); keys.dedup(); format!("overseer-obs:{}", keys.join("\|"))` — sort-then-dedup is correct; **no dedup bug**. |
| Write-back store | `write_back_observation` `mod.rs:534-563`; `record_observation` `wiring.rs:1076-1088` | gates on exact composite via `write_back_gate.peek` (900 s), stores `… [sig:{signature}]` under `"overseer"`, commits slot only after successful store. |
| Recall | `recall_episodic` `wiring.rs:1013-1031` | `failure_signature = parse_failure_signature(&e.content)` over **all** episodes — **no self-exclusion (D1)**. |
| Count → emit | `signals_from` `signal.rs:455-470` | counts `ep.failure_signature` into a `BTreeMap`, emits `RecurringSignature{signature, occurrences}` at `occurrences >= 2`. **This is the read-side `2×` gate.** |
| Classify → token | `mod.rs:1353-1363` | `RecurringSignature` arm sets `dedup_key = sanitize_recalled(signature)`; a composite `overseer-obs:…` matches no in-cycle key, so it becomes a **standalone** Problem → nested into next composite. |

**`stewardship/dedup.rs` is NOT on this path.** `failure_signature` (`dedup.rs:63-75`)
mints a 16-hex fingerprint for GitHub-issue dedup (`find_existing`); the recurring token
is human-readable. dedup.rs defines the *vocabulary* the naming imitates but does not mint
the recurring memory token. **Ruled out as origin.**

### Why the write-back gate does not stop it
`WhisperGate::new(900, …)` suppresses **same-window** duplicates only, by design. It is
**not** a loop breaker. Across windows the identical (or nesting-mutated, hence always
*distinct*) signature re-delivers, accumulating ≥2 episodes → recall's `>=2` fires. The
loop is **open at HEAD**.

---

## 2. goal:blocked lifecycle (observer.rs / no_progress.rs)

- **Entry:** blocked goals surface in `ObservedState` (`mod.rs:393-394`); `Signal::GoalBlocked`
  per goal (`signal.rs:440-448`) → classify `goal:blocked:{goal_id}` (`mod.rs:1336`).
- **WHY ladder:** `no_progress.rs` classifies via `NoProgressWhyReasoner` / `NoProgressClass`
  (`AlreadyComplete`/`MissingPrecondition`/`UpstreamDependency`/`UnclearCriteria`, ~`no_progress.rs:996-1034`)
  and routes via `resolution_for_why` → rungs `MarkDone`/`Drop`/`Defer`/`Escalate`
  (`no_progress.rs:587-748`). `reinvestigate_bare_blocked_goals` (`no_progress.rs:808`,
  issue #17) drives goals **away from bare** `[OODA-SAFEGUARD] … needs human review`.
- **Failure mode:** when the WHY reasoner is unwired or misclassifies, a goal degrades to a
  **bare park** — no closing rung fires, so `goal:blocked:*` persists cycle after cycle.
  Oracle `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` (passes) confirms
  the perpetual-idle path parks rather than converges.

**Named goals map to self-resolvable classes** (from prior waves, re-affirmed): kgpacks-rs
#12/#17/#18/#23/#25 → AlreadyComplete/MissingPrecondition; coverage-70% → UnclearCriteria;
coin-harness → MissingPrecondition/UpstreamDependency; simard-identity personas →
GoalUncovered/UnclearCriteria. All would self-clear **if** the WHY route fired.

---

## 3. workstream-gap & resource:engineer_spawn (co-occurrence explained)

- **workstream-gap** = a **backlog-coverage gap**, emitted by `detect_workstream_gaps`
  (`sensor.rs:288`) per-goal (`goal:{id}` `sensor.rs:306`), per-issue (`issue:{ref}`
  `sensor.rs:335`), per-anomaly (`anomaly:{slug}` `sensor.rs:357`), deduped by `is_covered`.
  **Distinct from** the `decompose.rs` `<2-subgoal` failure — do NOT conflate. One
  consolidated `Signal::WorkstreamGap` (`signal.rs:475-479`); notify-only Act (D3).
- **resource:engineer_spawn** = saturation signal: `Signal::EngineerSpawnRate{live}`
  emitted when `live >= ENGINEER_SPAWN_THRESHOLD` (`=8`, `signal.rs:351,393-396`) →
  token `"engineer_spawn"` (`capabilities.rs:562`). Indicates engineer/agent spawn
  capacity is at/near cap — the **resourcing bottleneck** that leaves goals under-resourced.
- **Unifying observation:** an under-resourced goal **oscillates** — `workstream-gap` while
  active, `goal:blocked` once idle — with `resource:engineer_spawn` firing when spawn
  capacity saturates. **Same resourcing/convergence problem in three views**, not three
  independent bugs.

---

## 4. Empirical validation (oracles run at HEAD)

| Suite | Result | Confirms |
|---|---|---|
| `overseer::tests_memory_recall` | **32 passed** | `recurring_signature_emitted_when_two_episodes_share_signature` (×2 emit); `…_not_emitted_for_single_occurrence` (threshold=2); `…_is_additive_not_replacing` (nesting); `write_back_is_deduplicated_within_window` (same-window gate); `write_back_persists_again_for_a_distinct_signature` (**cross-window/distinct-signature re-persist = the loop mechanism**). |
| `overseer::tests_gap_scan` | **21 passed** | per-identity gap signatures, coverage dedup, fail-closed without identity. |
| no-progress / goal-health | **77 passed** | `perpetual_no_progress_goal_is_unblocked_once_and_not_escalated` — parks, does not converge. |

---

## 5. Root-cause verdict & minimal remediation (non-binding)

**One upstream cause with three seams**, not coincidental independent blocks. The OODA/Overseer
loop **observes, notifies, and dedups but never removes the condition**, and re-observes its own
observation. Recurrence is honest; the missing piece is a **convergence rung**.

Dependency-correct, minimal fix set (from prior synthesis, re-affirmed — diagnosis only):
1. **D2 gate+counter atomically:** carry the occurrence counter **in fact content**
   (caller-key upsert with incremented `occurrence_count` + `first_seen`/`last_seen`);
   escalation reads that field, **not** `recall.len()`. **Trap:** a literal
   `store_fact_with_caller_key(root_cause_signature, …)` collapses recall to 1 forever →
   escalation at `mod.rs:1613` becomes dead code (per `RECONCILIATION_LEDGER.md §2`).
2. **D3 closing rung:** give `WorkstreamCoverage` a real Act edge (route via the
   built-but-dangling `stewardship/routing.rs`), keyed on `GapItem.signature` (INV-GAP-KEY)
   not the bare `"workstream-gap"` dedup_key, else all gaps fold into one issue.
3. **D1 write-back filter:** at the recall/count boundary, exclude `OVERSEER_SOURCE_LABEL`
   episodes (or drop `overseer-obs:*` failure_signatures) so self-authored write-backs are
   never counted as recurring failures. Breaks the nesting loop at its source.

---

## 6. Open questions / connections for synthesis
- D1 alone breaks the *nesting* but not the underlying blocked/gap lanes (need D3).
- Confirm the WHY reasoner is actually wired in the running daemon build (source supports it;
  runtime wiring not verifiable from read-only repo) — an unwired reasoner is the difference
  between self-clearing and permanent bare park for the named goals.
- `resource:engineer_spawn` at `>=8` suggests the spawn cap itself may be the binding
  constraint linking gaps→blocks; tuning that cap is orthogonal to D1–D3 but may be the
  fastest relief.
