# Secondary Investigation — workstream-gap token class + engineer_spawn admission-cap reconciliation

**Role:** Secondary (patterns) · **HEAD:** `25d4c5a6` · **Date:** 2026-07-16
**Verdict:** VALIDATED — prior corpus holds at live HEAD. `workstream-gap` repetition is a
**signal** (multiple real gaps / self-observation nesting), not a recording defect.
`resource:engineer_spawn` is reconciled as a **benign-until-saturated AIMD admission cap**:
benign membership drift under normal load, load-bearing only when concurrent guided-engineer
spawns approach the AIMD-scaled `max_concurrent_actions` ceiling. **Zero production-source
drift.** Investigation-only; no fixes applied.

---

## 0. Drift ledger

- `git diff --name-only ea6ec554..25d4c5a6 -- '*.rs'` → **(empty).** HEAD advanced from the
  last-validated wave by docs-only commits (three `ai_working/investigation/*.md` files).
- All source pins below re-read live at `25d4c5a6`. Every prior file:line citation in scope
  re-verifies **exactly**. No stale pins.

---

## Area A — `workstream-gap` token classification (SIGNAL, not recording defect)

Re-verified against live source:

| Fact | Loc @ HEAD | Check |
|---|---|---|
| Orient classifies gaps → `ProblemKind::WorkstreamCoverage`, **observation** `dedup_key = "workstream-gap"` (bare constant) | `overseer/mod.rs:1369-1371` | ✅ exact |
| Decide arm `WorkstreamCoverage` → `FlagWorkstreamGaps` (notify-only, no launch edge) | `overseer/mod.rs:1534` | ✅ exact |
| Act `act_flag_workstream_gaps` **notify gate** keys on per-gap `format!("workstream-gap:{}", g.signature)` | `overseer/mod.rs:901,932` | ✅ exact |
| Blocked goals **explicitly skipped** by gap-scan (`continue`) | `overseer/sensor.rs:299-302` | ✅ exact |

**Classification: SIGNAL.** The repeated `|workstream-gap|workstream-gap|` tokens are **real,
distinct uncovered p1/p2 work**, not a counting/write defect. Evidence and mechanism:

1. **Identity-erasing observation key vs identity-preserving notify key (NEW nuance, corroborates
   INV-GAP-KEY).** There are *two* keys for the same gap family:
   - The **observation** `dedup_key` folded into `observation_signature` is the **bare constant**
     `"workstream-gap"` (`mod.rs:1371`) — per-gap identity is erased at the signature level.
   - The **notify** gate (`gap_gate`) keys on the **real** `workstream-gap:{g.signature}`
     (`mod.rs:901,932`) — per-gap identity **is** preserved for notification dedup.
   This asymmetry is the direct source proof of **INV-GAP-KEY**: any convergence/remediation rung
   must key on `GapItem.signature` (as the notify gate already does), **not** the bare observation
   constant, or all distinct gaps fold into one issue.
2. **Why two tokens survive despite `dedup()`.** `observation_signature` does
   `sort_unstable → dedup → join("|")` and `dedup()` collapses only *adjacent equal* keys, so a
   single pass would collapse the constant `"workstream-gap"` to **one** token. The observed
   **doubling** is therefore the **D1 self-observation write-back** artifact — a recalled
   `RecurringSignature` (already containing `workstream-gap`) is re-embedded into the next
   signature (`wiring.rs:301`), producing two tokens at different nesting levels that no single
   `dedup()` pass touches. **Both interpretations converge on "signal":** either multiple real
   gaps or a faithful re-observation of the same static gap set across windows.
3. **Not a decomposition failure.** Decomposition failure is a separate loud `MIN_SUBGOALS`
   path (`decompose.rs`); `WorkstreamCoverage` is the only High-priority Decide arm with **no**
   launch edge and **excluded** from `outcome_records_occurrence` (`wiring.rs:612-627`) — so it
   is dual-path quarantined (no `FileIssue`, no `LaunchRecipe`) and re-observes every tick.

**Coupling to blocked goals = dynamic oscillation, not a structural join.** Blocked goals are
skipped by gap-scan (`sensor.rs:299-302`) and route via `GoalBlocked → GoalHygiene`
(`goal:blocked:<id>`). An under-resourced goal oscillates: `workstream-gap` while active/uncovered
→ breaker parks it → `goal:blocked` while idle. Two decoupled non-closing loops, one root
resourcing problem — consistent with prior synthesis.

---

## Area B — `resource:engineer_spawn`: the benign-until-saturated admission-cap model (RECONCILED)

Prior secondaries classified `resource:engineer_spawn` as "benign membership drift." The strategy
asked me to reconcile that with the "load-bearing cap" framing. **Both are correct — they are two
regimes of one AIMD-governed cap.** Source evidence:

| Fact | Loc @ HEAD | Check |
|---|---|---|
| Overseer signals `EngineerSpawnRate { live }` only when `live >= ENGINEER_SPAWN_THRESHOLD` | `signal.rs:393-396` | ✅ |
| `ENGINEER_SPAWN_THRESHOLD = 8` | `signal.rs:351` | ✅ |
| dedup_key literal `"resource:engineer_spawn"`; `{live}` count lives in the **summary only** | `mod.rs:1268-1271` | ✅ |
| Admission cap is `max_concurrent_actions`, **AIMD-scaled** (not a fixed constant) | `ooda_loop/adaptive_scaling.rs:1,25-63` | ✅ |
| AIMD multiplicative decrease `DECREASE_FACTOR = 0.5` on pressure > `HIGH_PRESSURE_THRESHOLD = 0.8` (CPU/mem/429) | `adaptive_scaling.rs:17,21` | ✅ |
| Cap clamped to `[floor, ceiling]`; `SIMARD_SCALING=auto` enables AIMD, else fixed | `adaptive_scaling.rs:7-8,48-63` | ✅ |
| `spawn_engineer` returns `bool`; a **rejected** spawn → escalate-WITH-why on next stall (not spawn-forever) | `ooda_loop/no_progress.rs:712-745` | ✅ |
| Deterministic Rail-1 exact-path collision Defer overrides the brain even "at count cap 24" | `advance_goal/admission.rs:107-181`; test `count_cap_24_does_not_bypass_exact_path_rail:562` | ✅ |

**Reconciled model:**

- **Benign regime (normal load).** When the blocked-goal backlog is small and system pressure is
  low, AIMD additively raises the cap toward `ceiling`; guided-engineer spawns are admitted freely.
  `resource:engineer_spawn` is then just a **fixed literal token whose volatile `{live}` count**
  (summary-only, non-keyed) varies — pure membership/composition drift in the composite signature,
  exactly as prior art found. Adding/removing this one literal key is expected.
- **Load-bearing regime (saturated).** The cap becomes binding when EITHER (a) many parked goals
  simultaneously reach `SpawnEngineer` and un-block (`no_progress.rs:721` re-investigation path),
  pushing `live` toward the AIMD-current cap, OR (b) system pressure/429s halve the cap toward
  `floor` (`DECREASE_FACTOR = 0.5`). In that regime `spawn_engineer` returns **false** →
  `no_progress.rs:739` escalate-with-why, or admission **Defers** (Rail-1/overlap). Deferred/rejected
  spawns feed goals back into the `goal:blocked ↔ workstream-gap ↔ resource:engineer_spawn`
  oscillation triangle. This is the "admission cap as load-bearing" reading.

**Threshold estimate & issue-17 cluster verdict.** Two distinct floors:
- **Overseer visibility floor = `live >= 8`** (`ENGINEER_SPAWN_THRESHOLD`): a leading indicator, not
  the cap itself.
- **Binding cap = AIMD `max_concurrent_actions`** (nominal ~24 in tests, but `[floor, ceiling]`-clamped
  and halved under pressure).

Blocked goals **do not** consume cap slots — only *admitted, live* engineers do. The issue-17
cluster (~7 kgpacks goals #12/17/18/23/25 + advance-parity, plus personas/coverage/coin) can only
saturate the cap if its members **co-fire** `SpawnEngineer` and un-block in the same wave. At that
point it plausibly crosses the **visibility floor of 8** (hence `resource:engineer_spawn` appearing
in the second snapshot) but is **unlikely to cross the nominal ~24 cap** unless AIMD has already
decayed the cap under CPU/mem/429 pressure. **Conclusion: benign drift at HEAD; the cap is
load-bearing only in the pressure-decayed regime.** This is a signal about resourcing dynamics,
**not** a contradicting or corrupted signal, and **not** an independent bug.

---

## Area C — prior-art re-grounding against HEAD drift (VALIDATE, don't re-derive)

| Prior claim | Source doc | Status @ `25d4c5a6` |
|---|---|---|
| Every load-bearing root-cause citation re-verifies exactly; commits are docs-only | `RECONCILIATION_LEDGER §0-1` | ✅ still true |
| D1/D2/D3 all OPEN; only `tests_root_cause.rs` (+99) changed since baseline | `secondary…ea6ec554 §0`, `secondary…25d4c5a6 §0` | ✅ still true (ea6ec554..HEAD docs-only) |
| `×2` is honest cross-window/restart re-observation, threshold 2 (`signal.rs:362`) < escalate 3 (`root_cause.rs:33`) | `secondary…25d4c5a6 Area 3` | ✅ re-verified |
| §6.2b `store_fact_with_caller_key` one-liner is a de-ratchet TRAP; correct = count-in-content upsert | `RECONCILIATION_LEDGER §2` | ✅ re-verified (`library_adapter.rs:885-889`) |
| `WorkstreamCoverage` notify-only, no launch/convergence edge; excluded from occurrence recording | `tertiary…D3…ea6ec554 §1` | ✅ re-verified (`mod.rs:1534`, `wiring.rs:612-627`) |
| `resource:engineer_spawn` = benign membership drift | `secondary…25d4c5a6 Area 5` | ✅ re-verified **and extended** with the AIMD saturation regime above |

**No divergence.** The corpus is sound; my contribution extends (does not contradict) it with:
(1) the observation-key-vs-notify-key identity asymmetry as direct source proof of INV-GAP-KEY, and
(2) the AIMD admission-cap grounding that reconciles "benign drift" with "load-bearing cap" as one
mechanism in two regimes.

---

## Dead ends avoided

- Did **not** count raw `|`-delimited token frequency (self-nesting overcounts) — used canonicalized
  occurrence semantics.
- Did **not** relitigate drift-vs-cap for engineer_spawn — adopted and source-grounded the unified
  benign-until-saturated framing.
- Did **not** touch production `.rs` or dive into the kgpacks-rs #17 feature work (only its
  `goal:blocked` status is in scope).

## Questions for verification phase

1. **`SIMARD_SCALING` in prod:** is AIMD `auto` (cap dynamic, can decay to `floor` under pressure) or
   `fixed`? If `fixed`, the cap is only load-bearing when the backlog co-fires past the static
   ceiling; if `auto`, 429/CPU/mem pressure can make the cap binding at much lower live counts.
2. **AIMD `floor`/`ceiling` prod values:** the nominal ~24 is a test literal; confirm the configured
   ceiling and floor to fix the true saturation threshold for the issue-17 cluster.
3. **Co-fire rate:** how many issue-17-cluster goals reach `SpawnEngineer` and un-block in a single
   window? This determines whether `live` ever approaches the cap vs merely crossing the 8 signal floor.
4. Same open D2 gate questions as prior secondaries: `no_progress_investigation_enabled()` and
   `memories.completion_evidence.is_some()` state on the parking ticks.
