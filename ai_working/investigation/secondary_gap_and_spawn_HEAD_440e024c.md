# Secondary Investigation — workstream-gap & resource:engineer_spawn

**Role:** SECONDARY investigator (patterns / detection sources / cross-goal recurrence)
**HEAD:** `440e024c`
**Provenance:** `git diff --name-only {b9f99879,dea65df8}..HEAD -- '*.rs'` is **EMPTY** —
the `.rs` tree is byte-identical to every prior investigation wave. All prior secondary
citations re-verify exactly. This wave **confirms + regrounds**, it does not restart.

---

## Verdict (one line)

`workstream-gap` and `resource:engineer_spawn` are **two independent detection sources
with no causal code edge between them**; they co-occur in the recurring signature because
both conditions were true in the same observation window (backlog uncovered **AND**
engineers maxed ≥8). `workstream-gap` is a real **observe-and-flag-without-closing**
defect (missing convergence rung); `engineer_spawn` is **benign passive telemetry**.

---

## F1 — `workstream-gap`: detection source (sensor.rs) — CONFIRMED

`detect_workstream_gaps` (`sensor.rs:288-372`) emits a `GapItem` for each **uncovered**
high-priority backlog item, in three categories:

| Category | Trigger | Signature grammar |
|---|---|---|
| `GoalUncovered` | active p1/p2 goal, **not** Blocked, no assignee & no pr/branch/session/engineer wip-ref, uncovered (`sensor.rs:298-320`) | `goal:{id}` |
| `IssueUncovered` | open issue w/ high-signal label, no PR/workstream, uncovered (`sensor.rs:323-349`) | `issue:{repo_slug}#{n}` |
| `AnomalyUnaddressed` | live anomaly, no fix in flight (`sensor.rs:352-368`) | `anomaly:{slug}` |

Key design facts:
- **Blocked goals are explicitly skipped** (`sensor.rs:300-302`) and DELEGATED to
  `goal_health` — so a goal oscillates: `workstream-gap` while active/uncovered →
  `goal:blocked` once idle. **Same entity, two lenses** (explains why the same personas /
  coverage-audit / coin-harness / kgpacks goals appear in *both* families).
- Fields bounded by `MAX_GAP_FIELD_LEN=120` (`sensor.rs:240`), total by `MAX_GAPS_PER_TICK`.
- Signatures built from **trusted metadata only** (slugified repo + numeric id), never
  untrusted titles (`sensor.rs:332-335`, V3 injection hardening).

The bare literal `"workstream-gap"` in the composite signature is the Problem-level
`dedup_key` (`mod.rs:1369-1371`), distinct from the per-gap `GapItem.signature`.

## F2 — `workstream-gap`: the loop observes but never closes — CONFIRMED (the DEFECT)

`WorkstreamCoverage` Decide arm (`mod.rs:1534-1543`) → `Intervention::FlagWorkstreamGaps`
→ `act_flag_workstream_gaps` (`mod.rs:884-946`): **operator notification ONLY**
(email + Signal), deduped per-gap via `gap_gate` = `WhisperGate::new(900, 200)`
(`mod.rs:201,304`) keyed `workstream-gap:{g.signature}` (`mod.rs:901,932`).

- **Launches no workstream, files no issue, spawns no engineer.**
- This is the **only High-family Decide arm with no `launch.rs` edge** — `launch.rs`
  has a full `RecipeRunner`/`smart_orchestrator_args` spawn path, but nothing routes gaps
  into it. `tests_gap_scan.rs` even injects a mock `launch()` (`:99`) yet asserts only on
  *notifications* — the launch path is intentionally never exercised for gaps.
- 15-min dedup window ⇒ a persistently uncovered item re-notifies each window forever.
  **Root cause = missing convergence rung, not a counting bug.**

## F3 — `resource:engineer_spawn`: detection source — CONFIRMED (BENIGN)

- Emitted as `Signal::EngineerSpawnRate { live }` when `state.live_engineers >= 8`
  (`signal.rs:27,351,393-396`) — pure passive telemetry, value in summary only.
- `signal_to_problem` maps it to `ProblemKind::ResourcePressure`, **`Priority::Normal`**,
  dedup_key `"resource:engineer_spawn"` (`mod.rs:1267-1272`).
- Decide arm: `ResourcePressure => Intervention::Escalate { reason }` (`mod.rs:1444-1446`).
- `capabilities.rs:562` only mints the recall keyword `"engineer_spawn"` (recall term),
  not an action.
- **Actual spawning lives elsewhere** (OODA loop: `no_progress.rs` `SpawnEngineer` arm,
  bounded to one guided retry). No unfulfilled-spawn defect at the overseer boundary.

## F4 — No causal edge; co-occurrence = under-resourced STATE — CONFIRMED

There is **no code path** from `workstream-gap` to `engineer_spawn` or vice versa. They
share the composite `overseer-obs:` signature only because both predicates held in one
window: engineers saturated (≥8 live) **and** backlog coverage incomplete — a classic
**under-resourced system state**, not an orchestration cycle. This unifies the whole
signature: `goal:blocked` (idle stuck goals) + `workstream-gap` (active uncovered goals) +
`resource:engineer_spawn` (no spare executors to cover them) are **three symptoms of one
resourcing/convergence deficit**.

---

## Cross-goal recurrence explanation

Every named goal (kgpacks-rs #12/#17/#18/#23/#25, coverage-70%, coin-harness,
simard-identity personas) is a backlog item with **no spare engineer to cover it**
(≥8 already live) and **no convergence rung** to launch/file work. So each cycle:
active+uncovered → `workstream-gap` (notify, suppress-within-window, never close);
once idle → `goal:blocked` (park). The set is stable ⇒ the composite signature is stable
⇒ the honest `×2` re-observation across windows.

## Reconciliation with prior traps (still valid)

- **INV-GAP-KEY (directly my area):** any future remediation rung MUST key on
  `GapItem.signature` (per-gap), NOT the bare `"workstream-gap"` dedup_key — else all gaps
  fold into one issue. The bare literal is correct for the *observation signature*, wrong
  as a *remediation/issue key*.
- **CallerKey trap (Lane B, adjacent):** confirmed orthogonal — tokens are already
  idempotent; the storage-idempotency gap is on the occurrence *counter*
  (`record_occurrence` → non-idempotent `store_fact`). Do not conflate.

## Questions for verification phase

- **Q1:** Confirm the `Priority::Normal` `resource:engineer_spawn` → `ResourcePressure =>
  Escalate` (`mod.rs:1444`) is priority/dedup-gated in Act so elevated-but-normal spawn
  rate cannot escalate spuriously.
- **Q2:** Confirm no path re-arms the OODA `mark_guided_retry` bound every cycle under
  sustained gaps (no unbounded-spawn regression) — outside my source area, flag to primary.
- **Q3:** Confirm `MAX_GAPS_PER_TICK` truncation cannot silently drop a persistently
  uncovered high-priority gap below the notification set.
