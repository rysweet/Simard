# CONSOLIDATED FINDINGS — recurring workstream-gap `goal:steward-ci-github-actions-health-across-all-gov-e06d9e64`

Consolidation of all parallel deep dives (primary predicate trace + tertiary
architecture disposition table + minimal-durable-closure architect pass) into a
single reconciled verdict. Scope: **track/link only** — do NOT fix underlying CI
bugs, do NOT redesign the gap-scan.

Date: 2026-07-16 · Verified against live `~/.simard/state/goal_board.json` and `gh`.

---

## 1. One-line verdict (all dives agree)

The gap recurs because the **goal-board entry was never durably mutated**. A
merged deliverable PR (#4181) and an open tracking issue (#4172) are both
**invisible** to the coverage predicate. The only cover it ever gets is a
transient `assigned_to` that the OODA loop clears every cycle — so it
oscillates uncovered → flagged → (briefly assigned) → uncovered forever. The
gap signature is **faithful**; the defect is the **missing durable closing
action**, not a false positive.

## 2. The exact coverage predicate (single condition to flip)

`detect_workstream_gaps` (`src/overseer/sensor.rs:288`) emits `goal:<id>` for a
goal iff ALL of:

- `!matches!(status, GoalProgress::Blocked(_))` (Blocked → goal_health, `:300`)
- `priority <= GAP_GOAL_PRIORITY_BAR` (p1/p2)
- `!goal_has_active_workstream(g)` (`:303`)
- `goal:<id> ∉ coverage`

`goal_has_active_workstream` (`sensor.rs:377-384`) — a goal is COVERED iff:
```rust
if goal.assigned_to.is_some() { return true; }          // A) assignee (TRANSIENT)
goal.wip_refs.iter().any(|w|
    matches!(w.kind.as_str(), "pr"|"branch"|"session"|"engineer")) // B) DURABLE ref
```

Two decisive, source-confirmed facts:
1. **`coverage` never contains `goal:` signatures.** It is built only by
   `issue_coverage_from_open_prs` (`wiring.rs:861,876`) which emits `issue:...`
   strings from **open** PRs. A merged/open PR referencing the goal id does not
   cover the goal. Goals are covered EXCLUSIVELY via the board entry.
2. **An `issue`-kind wip_ref does NOT count** (`sensor.rs:383` lists only
   `pr|branch|session|engineer`). Tracking issue #4172 linked as an issue is
   invisible to the predicate.

`gap_gate = WhisperGate::new(900, 200)` (`mod.rs:304`) mutes re-*notification*
for 900 s only — it is a dedup window, **not** coverage.

## 3. Why it recurs — the oscillation mechanism (proven)

- `advance_goal` sets `assigned_to = Some(engineer-…)` when a workstream spins up.
- `ooda_loop/cycle.rs` does `assigned_to.take()` each cycle → clears the assignee.
- `wip_refs` is **not** cleared by that sweep — but it was never populated.

Net: coverage predicate A (assignee) flickers true then false; predicate B
(durable wip_ref) is permanently false. So the signature re-emits every tick.

## 4. The Overseer act-arm is notify-only (systemic root)

`act_flag_workstream_gaps` (`mod.rs:884-948`) does exactly: peek gap_gate →
notify operator (email + Signal) → commit gate. It **never files an issue and
never launches a recipe**. The `describe_action` "…filed deduped issue(s)…"
string (`wiring.rs:567`) is aspirational wording, not a code path. There is no
`FileIssue` / `LaunchRecipe` rung for gaps. **Meta-issue #4126 (auto-launch /
auto-file on detected gap) is OPEN and is the systemic fix** — landing it makes
any residual signature self-cover within one tick and closes recurrence for the
whole gap class, not just this one.

## 5. Live state (verified 2026-07-16)

| Fact | Value |
|---|---|
| Board section | `board.active` |
| status / priority | `NotStarted` / p2 → in-scope for gap-scan |
| `assigned_to` | `Some(engineer-…e06d9e64-1784222745055)` — transient, cleared next cycle |
| `wip_refs` | `[]` — **no durable cover** |
| `standing` / `is_perpetual` | `None` / `None` — **not flagged standing on the board** |
| Deliverable PR #4181 | **MERGED** 2026-07-16T12:00:40Z — merged ⇒ 0 live coverage (and coverage is `issue:` only anyway) |
| Tracking issue #4172 | OPEN — an *issue*, satisfies NOTHING in `goal_has_active_workstream` |
| Recurrence evidence | Re-emitted as issues #4201/#4191/#4190/#4186 across the day |

## 6. Reconciled disposition — the minimal durable close

Choose ONE, by whether the goal is one-shot or a standing duty:

- **If one-shot (delivered by merged #4181) — PREFERRED, most durable:**
  `simard goal complete steward-ci-github-actions-health-across-all-gov-e06d9e64`.
  This tombstones the goal and removes it from `board.active` via
  `save_goal_board_with_removals`. `detect_workstream_gaps` iterates only
  `board.active`, so a tombstoned/removed goal is **permanently** out of scope.
  Note: the CLI **refuses** this for standing goals (auto-reopens) — the board
  entry is NOT flagged standing, so the command will succeed.

- **If standing/perpetual CI-health duty — LINK, do not complete:**
  On the board entry, either set `assigned_to = Some(<steward identity>)`
  **durably** OR (better, since assignees are swept) add
  `WipRef { kind: "engineer"|"branch"|"pr", reference: <live branch/PR #4181> }`.
  The wip_ref MUST be kind `pr|branch|session|engineer`; an `issue`-kind ref
  (e.g. #4172) will NOT cover it. Mutations must go through `BoardGoalCurator`
  / `save_goal_board` under `BoardWriteLock` — never hand-edit the JSON under a
  live OODA loop.

**Recommendation:** CI-health is conceptually a standing duty, but this board
entry is not marked standing and its concrete deliverable (#4181) merged. The
disposition table (row 2) resolves this as **dedupe (mark-done)** because
stewardship was delivered. Execute `simard goal complete …e06d9e64` to close
durably; if the operator intends CI-health to remain a perpetual watch, instead
re-add it with `goal add --standing` and attach an `engineer`-kind wip_ref, and
land the standing-goal exemption in the predicate (see §7).

## 7. Systemic findings surfaced for the meta-workstream (out of this task's scope)

1. **`detect_workstream_gaps` exempts `Blocked` goals but NOT standing/perpetual
   goals.** A standing goal can never be `Completed` and gets no coverage from
   merged PRs, so it oscillates forever. Durable systemic fix: exempt
   `is_perpetual`/`[standing]` goals in the predicate the same way `Blocked` is,
   OR land #4126.
2. **Land #4126 (auto-launch/auto-file on detected gap)** — the single
   highest-leverage fix; the manual dispositions here are a bridge, not the cure.
3. **Wire goal-board wip_ref linking** so in-flight engineer branches / merged
   deliverables auto-attach a durable wip_ref to their goal (same defect hits
   `goal:build-a-local-coin-benchmark…09e65e35`, which has live PRs
   #4171/#4161/#4149 yet still surfaces).

## 8. Verification recipe (definition of done for this gap)

1. Apply §6 to the board entry via the sanctioned mutator (`goal complete` or
   `save_goal_board` wip_ref link).
2. Re-run Observe / `BoardGoalCurator::workstream_gaps(&[])`: the returned
   `Vec<GapItem>` must NOT contain
   `goal:steward-ci-github-actions-health-across-all-gov-e06d9e64`.
3. Existing contracts hold: `tests_gap_scan.rs::ignores_goal_covered_by_pr_assignment_or_coverage_set`
   (assignee/wip_ref path) and `detects_uncovered_p1_goal_and_unaddressed_anomaly`.
4. Next Overseer tick: `workstream_gaps_detected` for this signature = 0.

## 9. Provenance (dives consolidated)

- PRIMARY: predicate trace + oscillation proof (`sensor.rs` / `cycle.rs` /
  `wiring.rs`), live board + session inspection.
- TERTIARY architecture: `tertiary_architecture_MINIMAL_DURABLE_GAP_CLOSURE_e06d9e64_HEAD_7cb152ff.md`.
- TERTIARY architecture: `tertiary_architecture_GAP_COVERAGE_DISPOSITION_TABLE_HEAD_7cb152ff.md` (row 2).

**All dives converge with zero contradiction.** Signature faithful; close is a
one-line durable board mutation; systemic cure is #4126.
