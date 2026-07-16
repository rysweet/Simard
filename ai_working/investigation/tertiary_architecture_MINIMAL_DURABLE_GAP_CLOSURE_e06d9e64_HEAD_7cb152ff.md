# Tertiary (Architect) — Minimal Durable Gap-Closure for `goal:steward-ci-github-actions-health-across-all-gov-e06d9e64`

HEAD: `7cb152ff` · Role: TERTIARY / architecture · Scope: the single CI-health
goal-gap. **Track/link only — do NOT fix underlying CI bugs, do NOT redesign the
gap-scan.** Deliverable = the minimal durable action that drops this signature
out of `detect_workstream_gaps` so `gap_gate` stops re-flagging it every cycle.

---

## 1. The exact coverage predicate (source-quoted)

The gap is emitted by `detect_workstream_gaps` (`src/overseer/sensor.rs:288`),
fed by `BoardGoalCurator::workstream_gaps` (`src/overseer/wiring.rs:750`).

For a **goal**, `sensor.rs:298-320` emits `goal:<id>` iff ALL of:

- `!matches!(g.status, GoalProgress::Blocked(_))` (Blocked → delegated to
  goal_health, `sensor.rs:300`), AND
- `g.priority <= GAP_GOAL_PRIORITY_BAR` (p1/p2), AND
- `!goal_has_active_workstream(g)` (`sensor.rs:303`), AND
- `goal:<id> ∉ coverage`.

`goal_has_active_workstream` (`sensor.rs:377-384`):
```rust
if goal.assigned_to.is_some() { return true; }
goal.wip_refs.iter().any(|w|
    matches!(w.kind.as_str(), "pr" | "branch" | "session" | "engineer"))
```

**Two decisive facts:**

1. `coverage` NEVER contains `goal:` signatures. It is built ONLY by
   `issue_coverage_from_open_prs` (`wiring.rs:771,861`) which emits `issue:...`
   strings from **open** PRs. So an open/merged PR referencing the goal id does
   **not** cover the goal. Goals are covered EXCLUSIVELY through the board
   entry's `assigned_to` / `wip_refs`.
2. An `issue`-kind wip_ref does **NOT** count (`sensor.rs:383` lists only
   `pr|branch|session|engineer`). A tracking *issue* linked as a wip_ref is
   invisible to the predicate.

`gap_gate = WhisperGate::new(900, 200)` (`mod.rs:304`) only mutes
re-*notification* for 900 s; it is a dedup window, **not** coverage. Nothing in
`act_flag_workstream_gaps` (`mod.rs:884-948`) files an issue or launches a
recipe — it peeks → notifies operator → commits the gate. So until the board
entry satisfies the predicate above, the goal re-notifies forever.

## 2. Live state (verified against `gh`, 2026-07-16)

- **Goal is live on `board.active`**, p2 / NotStarted / `assigned_to=None` /
  `wip_refs=[]` → a genuine gap by the predicate.
- **PR #4181** ("ci(verify): resilience to transient artifact-service outages
  (CI-health steward)"), branch
  `engineer/steward-ci-github-actions-health-across-all-gov-e06d9e64-1784199297-169bd5`,
  is **MERGED** (`mergedAt 2026-07-16T12:00:40Z`). Merged ⇒ NOT in the open-PR
  coverage set ⇒ gives ZERO live coverage anyway (and coverage is `issue:` only).
- **Tracking issue #4172** ("tracking(ci-stewardship): owning workstream for
  goal …e06d9e64") is OPEN and is the intended "durable anchor" — but it is an
  **issue**, and issues do not touch the goal board. It satisfies NOTHING in
  `goal_has_active_workstream`. This is the notify-only trap in issue form.
- Recurrence is visible: the Overseer has re-emitted this same gap as issues
  #4201/#4191/#4190/#4186 (and this task, #? ) across the day.

**Root cause of recurrence:** the concrete deliverable merged (#4181) and a
tracking issue exists (#4172), yet NO mutation was ever made to the goal-board
entry (`assigned_to` or a `pr/branch/session/engineer` wip_ref). The predicate
reads the board, and the board is untouched.

## 3. Minimal durable action (the recommendation)

Disposition = **mark-done-or-link** (matches CONSOLIDATED plan row 10). Pick by
whether the goal is one-shot or standing:

### 3a. If the goal is a one-shot deliverable (delivered by #4181) — PREFERRED
Mark the board entry `GoalProgress::Completed`. `detect_workstream_gaps` iterates
only `board.active`; a `Completed` goal is dropped from the active projection
(reverse-adapter `Completed → BoardPlacement::Skip`, `operations.rs:1486,1518`)
so it is **permanently** out of the scan. This is the smallest, most durable
action and needs no live branch.

### 3b. If the goal is a standing/perpetual CI-health duty — LINK, don't complete
A standing goal MUST NOT be marked `Completed` (`types.rs:290`,
`mark_standing`). To flip the predicate, set on the board entry ONE of:
- `assigned_to = Some(<steward/engineer identity>)`, OR
- add `WipRef { kind: "engineer" | "branch" | "pr", reference: <live branch/PR> }`.

The wip_ref **must** be kind `pr|branch|session|engineer` — linking tracking
issue #4172 as an `issue` wip_ref will NOT cover it (`sensor.rs:383`).

### Which mechanism writes the board
Board mutations go through `BoardGoalCurator` over `save_goal_board`
(`wiring.rs:703,744`) under the `BoardWriteLock`; the operator-facing
mutators live in `goal_curation::operations` (`assigned_to`/status via
`update`-style dispatch, completion via the completion-gate → `Completed`).
`Overseer::launch.rs` (RecipeWorkstreamLauncher) only spawns recipes and returns
a `WorkstreamHandle`; it does **not** write wip_refs — so a launched recipe
alone still won't cover the goal unless its handle is written back to the board.

## 4. Architectural finding worth surfacing (not for this task to fix)

`detect_workstream_gaps` exempts `Blocked` goals but does **NOT** exempt
`is_perpetual` / `[standing]` goals. CI-health is the canonical standing duty
(`types.rs:709` test literally uses "watch CI health"). If this goal is standing,
it will oscillate back to uncovered every time its workstream closes, because a
standing goal can never be `Completed` and merged PRs give no coverage. The
durable systemic fix is either (a) exempt perpetual goals in the predicate the
same way `Blocked` is exempted, or (b) land meta-issue **#4126** (auto-launch /
auto-file on detected gap) so any residual signature self-covers within one tick.
Both are code changes outside this track/link task; flagged for the primary/meta
workstream.

## 5. Verification recipe

1. Apply 3a or 3b to the board entry via `save_goal_board`.
2. Re-run the Observe pass (or `BoardGoalCurator::workstream_gaps(&[])`): the
   returned `Vec<GapItem>` must NOT contain `goal:steward-ci-github-actions-health-across-all-gov-e06d9e64`.
3. Confirm with the existing unit contract `tests_gap_scan.rs::ignores_goal_covered_by_pr_assignment_or_coverage_set`
   (assignee/wip_ref path) and `detects_uncovered_p1_goal_and_unaddressed_anomaly`.
4. Next Overseer tick: `workstream_gaps_detected` for this signature = 0.

## 6. One-line answer

The gap recurs because the goal-board entry was never mutated — a merged PR
(#4181) and a tracking issue (#4172) are both invisible to `goal_has_active_workstream`.
Minimal durable close: **mark the board goal `Completed` (if one-shot) or add an
`assigned_to`/`engineer`-kind wip_ref (if standing)** via `save_goal_board`.
