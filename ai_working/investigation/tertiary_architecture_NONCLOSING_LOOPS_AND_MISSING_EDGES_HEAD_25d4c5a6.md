# Tertiary (Architect) — Non-closing OODA / workstream-gap control loops & missing remediation edges

- **HEAD:** `25d4c5a6fabe6c83fcaa5fa8a16b41044aa10721`
- **Drift check:** `git diff --stat HEAD -- src/overseer` → empty (clean). All citations below are live HEAD source.
- **Scope (area 4):** the two OODA control loops that fail to close, the specific missing code edges, and the gating that stalls them. Investigation-only — no fixes applied.

## 1. How the symptom is assembled (context, not re-derived)

The recurring "signature" is the Overseer's **own observation write-back**, not a single problem:

- `observation_signature(problems)` → `format!("overseer-obs:{}", sorted_deduped_dedup_keys.join("|"))` — `mod.rs:1068-1073`.
- Written by `write_back_observation` behind `write_back_gate` (dedup/rate window) → `caps.memory.record_observation` → `store_episode` — `mod.rs:534-563`, `wiring.rs:1076-1088`.

So every tick that still observes the SAME unresolved set of problems re-emits the SAME composite key. It is deduped **within** a window; when the window elapses the identical observation is recorded again, recurrence climbs, and eventually `Signal::RecurringSignature` fires (`signal.rs:64-70`). **The composite recurs because the underlying problems never reach a state-changing Act.** Two such problem families do this structurally.

## 2. Loop A — Blocked-goal resolution ladder: dead zone + double gate

**Emit:** `Signal::GoalBlocked` per blocked goal (`signal.rs:57-63`, built in `signal.rs:441-445` from `sensor.rs:213-218`) → dedup_key `goal:blocked:{goal_id}` (`mod.rs:1336`) → `ProblemKind::GoalHygiene`.

**Decide ladder** (`decide` → `decide_blocked_goal`, `mod.rs:1447-1483`, `1603-1631`):

| Condition | Intervention | Closes? |
|---|---|---|
| `recurrence >= RECURRENCE_ESCALATION_THRESHOLD` (=3, `root_cause.rs:33`) | `EscalateBlockedGoal` | yes (operator) |
| `perpetual && is_no_progress_marker(reason)` | `UnblockGoal` (self-heal) | yes |
| `needs_review` | `EscalateBlockedGoal` | yes |
| **else** | **`Report`** | **NO — no-op** |

`Intervention::Report` → `ActOutcome::Reported` (`mod.rs:658`), and is classified `Remediation::acknowledged()` (`remediation_for`, `mod.rs:1129`) — i.e. "nothing to fix; deliberate block." **The goal state is never touched.**

`fix-agent-kgpacks-rs-issue-17-…` (and the sibling `goal:blocked:*` slugs) are plain dependency/operator blocks: **not perpetual**, **no** no-progress/brain-failure `needs_review` marker (`sensor.rs:213`), and recurrence **1→2** sits **below the escalation threshold of 3**. They therefore fall into the `else → Report` branch. This is the **"dead zone"**: occurrences 1 and 2 produce a no-op `Report`, the goal stays `Blocked`, and the next Observe pass re-emits the identical `goal:blocked:{slug}` key — an **honest re-observation**, not a dedup/replay bug. Closure is only reachable at recurrence 3.

**Two-gate stall (why even the escalating arm can stall):** reaching the operator requires clearing BOTH:
1. the classification gate in `decide_blocked_goal` (recurrence≥3 OR needs_review), AND
2. the per-goal `blocked_goal_gate` dedup/rate gate inside `act_escalate_blocked_goal` (`mod.rs:823-852`), whose non-`Deliver` decisions collapse to `GoalHealthSuppressed` (a no-op, `goal_health_suppressed`, `mod.rs:861-879`).
If either gate suppresses, the outcome is a no-op and the goal remains blocked and re-observable.

**Missing remediation edge (Loop A):** `decide_blocked_goal` has **no arm that advances a plain, sub-threshold block** — no `LaunchRecipe` to work the blocking dependency and no durable `FileIssue`. The only sub-threshold action is `Report`, and `Report` carries **no WHY** (`why` is constructed only in `mod.rs:1469-1474` and consumed exclusively by the Escalate/Unblock arms). Hence: *blocked goals parked without WHY classification*.

## 3. Loop B — WorkstreamCoverage: notify-only terminal

**Emit:** `detect_workstream_gaps` (`sensor.rs:288-371`) → one consolidated `Signal::WorkstreamGap { gaps }` (`signal.rs:71-79`) → dedup_key literal `"workstream-gap"` (`mod.rs:1371`) → `ProblemKind::WorkstreamCoverage`.

**Decide:** `WorkstreamCoverage => Intervention::FlagWorkstreamGaps { gaps }` (`mod.rs:1534-1544`) — carries evidence forward verbatim, nothing more.

**Act:** `act_flag_workstream_gaps` (`mod.rs:884-948`) **notifies the operator on both channels and returns** `WorkstreamGapsFlagged`. Its own docstring states: *"Routine observations never create GitHub issues or stewardship backlog items"* (`mod.rs:881-883`). Guardrails class = `RiskClass::Routine` (`guardrails.rs:60`) so it always runs but never mutates state.

**Consequences — three structural defects:**

1. **No launch edge.** There is **no** `WorkstreamCoverage → LaunchRecipe` mapping anywhere (`grep WorkstreamGap|FlagWorkstreamGaps|WorkstreamCoverage … launch|recipe|file_issue` in `src/overseer/*.rs` → empty). The gap that says "this important work has NO active workstream" is **never converted into a workstream**. The uncovered work stays uncovered → the gap re-surfaces every tick → `"workstream-gap"` stays in every observation composite.

2. **No durable finding.** The acting path files nothing. Contrast `decide_read_only` (`observer.rs:98-121`), which routes recurring defects (`GoalHygiene`, `ProcessHealth`, …) to a deduped `FileIssue`, but maps `WorkstreamCoverage → Report` and comments it as "unreachable in M1." So the ONE path that surveys gaps (the acting Overseer) is exactly the path with no `FileIssue` edge — the durable-issue rung falls through the crack between M1 and M2.

3. **Contract drift / masking.** The `Intervention::FlagWorkstreamGaps` enum doc claims it will *"file one deduped issue per gap"* (`intervention.rs:71-78`), but the implementation only notifies — the enum contract is not honored. Worse, `remediation_for` classifies `FlagWorkstreamGaps` via the `_ => Remediation::root_cause()` catch-all (`mod.rs:1130`), so a **notify-only** action is labeled a **root-cause-addressing** remediation. This semantically *masks* the non-closing loop: telemetry reports the gap as "remediated at root cause" while no work was created.

## 4. Structural verdict

Both are textbook non-closing OODA loops: Observe→Orient→Decide→**Act-that-does-not-mutate-the-observed-state**. The Act rungs for these two problem kinds are *advisory* (`Report`, operator `notify`) rather than *state-changing* (`UnblockGoal`/`LaunchRecipe`/`FileIssue`), so the very next Observe sees the identical condition. The write-back path faithfully records that identical condition, which is the observed 2× recurrence.

- **Loop A** self-seals via `Remediation::acknowledged()` (looks intentional; isn't).
- **Loop B** self-seals via `Remediation::root_cause()` (looks fixed; isn't).

The `resource:engineer_spawn` token appearing in one snapshot but not the other is consistent with benign membership drift of the composite (a `ResourcePressure` `Signal::EngineerSpawnRate`, `signal.rs:26-27`), not a contradicting signal — it changes which dedup_keys are joined, not the closure behavior of Loops A/B. (Full drift classification is the secondary's area 5; noted here only for reconciliation.)

## 5. Missing edges — precise landing sites (for the dev workflow; NOT implemented here)

- **Loop B, launch rung:** add a `WorkstreamCoverage` decision edge that emits `LaunchRecipe` (spawn a workstream to cover a genuine gap), analogous to `ProblemKind::ProcessHealth`/`StepFailure` in `decide` (`mod.rs:1429-1435`, `1549-1580`). Missing today.
- **Loop B, durable rung:** and/or a deduped `FileIssue` in `act_flag_workstream_gaps` to honor the `FlagWorkstreamGaps` doc contract (`intervention.rs:75-78`). Missing today.
- **Loop A, sub-threshold rung:** add an arm in `decide_blocked_goal` (`mod.rs:1603-1631`) that takes a WHY-classified, state-advancing action for a plain block before recurrence 3 (e.g. launch/file), instead of the bare `Report` no-op.
- **Loop A, remediation labeling:** `remediation_for` (`mod.rs:1122-1132`) treats every sub-threshold blocked-goal `Report` as `acknowledged`; a genuinely-blocked-but-sub-threshold goal is not "deliberate," so this hides the open loop.

## 6. Reconciliation

Consistent with prior artifacts in `ai_working/investigation/` — `tertiary_architecture_NONCLOSING_LOOPS_DEADZONE_D0_HEAD_d00e4c3f.md`, `tertiary_ooda_loop_map_and_missing_unblock_rung_HEAD_856f854b.md`, `tertiary_gap_routing_and_remediation_rung.md`, and `secondary_two_loops_VALIDATED_HEAD_973c294b.md`. No source drift at HEAD `25d4c5a6` (`git diff src/overseer` empty), so those conclusions re-confirm against current HEAD. Thresholds re-verified live: `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`), `NO_PROGRESS_BREAKER_THRESHOLD = 3` (`goal_curation/no_progress_breaker.rs:59`).
