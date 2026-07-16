# Secondary Investigation — Two non-closing loops + self-ingestion recall path

**Role:** Secondary (patterns) · **HEAD:** `e5257a33` · **Date:** 2026-07-16
**Verdict:** VALIDATED — prior corpus conclusions hold; zero production-source drift.

## Source-drift check (citations still bind)

`git diff --name-only 6e3113bc..HEAD -- '*.rs'` and `5a85317b..HEAD` both return a
single file: `src/overseer/tests_root_cause.rs` (+99 lines, tests only). It ADDS
`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` and
`lane_b_escalates_without_any_lane_a_signal` — which *reinforce* the two-lane
decoupling verdict. No production code changed. Every load-bearing citation
below was re-grounded to live line numbers at HEAD.

## Loop #1 — notify-but-never-launch (`workstream-gap`)

- Token stamped literal at `mod.rs:1371` (`"workstream-gap".to_string()`),
  kind `WorkstreamCoverage`, `Priority::High` (`mod.rs:1368-1373`).
- Origin is a backlog-coverage gap (`sensor.rs:288 detect_workstream_gaps`,
  category `GoalUncovered` at `:311`) — NOT a decomposition failure. Confirmed.
- Decide arm `WorkstreamCoverage` (`mod.rs:1534-1543`) → `FlagWorkstreamGaps` →
  `act_flag_workstream_gaps` (`mod.rs:884`). That handler does exactly three
  things: peek `gap_gate`, send ONE consolidated operator notification
  (`:929-930`), commit the gate (`:931-934`). **No `LaunchRecipe`, no
  `UnblockGoal`, no issue filed.**
- Contrast proves the missing rung is structural, not accidental:
  `StepFailure` → `Intervention::LaunchRecipe{...}` (`mod.rs:1565`);
  blocked-goal recurrence → `EscalateBlockedGoal` (`mod.rs:1613`).
  `WorkstreamCoverage` is the **only High-priority Decide arm with no
  convergence edge** — it observes, dedups, notifies, and loops.
- Anti-pattern: **"Observe-and-flag without a closing action."** The
  `gap_gate` (WhisperGate) suppresses re-notification within its window but
  never *removes* the condition, so the gap survives every window/restart.

## Loop #2 — park-without-classify (`goal:blocked:<slug>-<hash>`)

- Token stamped at `mod.rs:1336` (`goal:blocked:{goal_id}`); the
  `— needs human review` marker is appended when `needs_review` (`:1339-1343`).
- The WHY-classification ladder is **double-gated** in `ooda_loop/cycle.rs:582`:
  - Gate A: `if let Some(source) = &memories.completion_evidence`
  - Gate B: `if no_progress_investigation_enabled()`
  Only when BOTH pass does `apply_no_progress_breaker_investigated` +
  `reinvestigate_bare_blocked_goals` run the reasoner and route down
  `resolution_for_why` (`no_progress_breaker.rs:384`): `AlreadyComplete→MarkDone`,
  `Obsolete→Drop`, `MissingPrecondition→Heal`, `UpstreamDependency→defer`,
  `UnclearCriteria|GenuinelyStuck→escalate`.
- If either gate fails, control falls to the base `apply_no_progress_breaker`
  (verify-once), which parks a BARE `[OODA-SAFEGUARD] … needs human review`
  block — the exact `goal:blocked` token that recurs. The reinvestigate guard
  that would rescue bare parks lives *inside* Gate A+B, so a pre-#16 daemon or
  a daemon with the kill-switch off can never self-clear its own bare parks.
- Even when classification succeeds, `decide_blocked_goal` (`mod.rs:1603`) only
  *notifies*: recurrence≥3 → `EscalateBlockedGoal` (`:1613`); `needs_review` →
  `EscalateBlockedGoal` (`:1623`); recurrence=2, non-perpetual, no marker →
  `Report` (`:1630`). Escalation surfaces to a human but does not RESOLVE the
  block, so the `GoalBlocked` signal (`signal.rs:441`) re-fires next window.
- Anti-pattern: **"Classify-then-route the stall, don't park it"** (failure
  mode) + the same open observe-and-flag shape as Loop #1.

## Recurrence dead zone (why 2× is stuck)

- Lane A visible count: `RecurringSignature` fires at
  `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, emitted `:463-467` from
  recall episode `failure_signature` counts).
- Lane B escalation floor: `RECURRENCE_ESCALATION_THRESHOLD = 3`
  (`root_cause.rs:33`, gate at `mod.rs:1613`).
- 2× is **above one-off noise, below escalation, and coverage/park loops carry
  no auto-remediation rung** → the "recurrence dead zone." The two lanes are
  decoupled (Lane A = observation episodes; Lane B = root_cause occurrences),
  now positively pinned by the two added tests. Operator-visible 2× says
  nothing about Lane B.

## Self-ingestion recall path (inline `|`-repetition ≠ occurrence count)

- Single concatenation site: `observation_signature` (`mod.rs:1068-1073`) —
  collect `dedup_key`s → `sort_unstable` → `dedup` (consecutive-only) → join
  `|` → prefix `overseer-obs:`. Built once per surviving tick.
- Recall-derived `RecurringSignature` is admitted as a `ProcessHealth` problem
  whose dedup_key = `sanitize_recalled(signature)` (`mod.rs:1353-1363`).
- `write_back_observation(&cycle.problems)` (`mod.rs:534`) writes the WHOLE
  problem set — including that meta-problem — with **no recall-derived filter**
  at the write boundary. Next window, recall reads it back and its dedup_key
  (already containing an inner `overseer-obs:…`) is joined again as a DISTINCT
  key (so `dedup()` cannot collapse it) → nested `overseer-obs:…|overseer-obs:…`
  runs. This inflates INLINE repetition only; it is a serialization /
  self-observation artifact. The authoritative occurrence count remains **2×**
  (Lane A `occurrences`, `signal.rs:462-467`), independent of inline length.
- Anti-pattern: **"Self-observation feedback"** — treat recalled signatures as
  untrusted at the WRITE boundary, not just the read boundary.

## Signal-vs-defect verdict (restated, not re-derived)

2× is an **honest cross-window / daemon-restart re-observation**, NOT a dedup
bug. Within-window dedup is proven green
(`tests_memory_recall.rs:797 write_back_is_deduplicated_within_window`), gated
by the `write_back_gate` peek/commit (`mod.rs:548-556`). The gate is present
and correctly keyed on the composite signature but **in-memory / per-process**
(`guardrails.rs:294 last_delivered: HashMap`), so it cannot converge across
restarts. Defect = missing convergence rung + per-process gate state, NOT a
counting error.

## Unifying pattern

**"Two signatures, one root problem."** An under-resourced goal oscillates:
`workstream-gap` while active, `goal:blocked` once idle. Both loops share the
identical shape — detect → notify/park → dedup within window → re-detect — with
no action that removes the condition. The sibling set (kgpacks issues
12/17/18/23/25, coverage-audit, coin-harness, simard-identity personas) is one
resourcing/convergence problem viewed twice, not many independent bugs. Fix
convergence once, not per-goal.

## Questions for verification phase

1. Confirm `no_progress_investigation_enabled()` default in the running daemon
   (Gate B) — if off in prod, Loop #2 is guaranteed permanent bare-parks.
2. Confirm `completion_evidence` is populated on the ticks where these goals
   park (Gate A) — a `None` source silently disables the whole ladder.
3. Confirm the operator-facing 2× string exactly matches `mod.rs:1361`
   (`recurring signature seen {occurrences}× in cognitive memory ({signature})`).
