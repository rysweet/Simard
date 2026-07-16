# Verification — Practical Tests per Hypothesis (RUNTIME-EMPIRICAL wave)

**HEAD:** `3e6b6933` · **Date:** 2026-07-16 · **Method:** live-state + log forensics
on the running daemon's `~/.simard/` store, cross-checked against source citations.

Prior waves verified the hypotheses by **code trace + unit tests**. This wave adds
the missing rung: **practical tests against the live daemon's runtime state** — the
actual `state/goal_board.json`, `overseer/activity.json`, and the 6-day tick history.
The runtime evidence **confirms H1 + H2 outright** and **REFUTES a key premise of H3**.

Signature under test (verbatim from `report.observed_details`):
> `ProcessHealth — recurring signature seen 2× in cognitive memory (overseer-obs:delivery:pr:…|goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca|overseer-obs:delivery:pr:…|…|resource:engineer_spawn|workstream-gap)`

---

## H1 — "×2 is an honest, persistent signal, not a counting/dedup artifact" → **CONFIRMED**

**Practical test (examine live state + tick history):**

1. **Goal is genuinely, continuously Blocked.** `state/goal_board.json` (cycle_count
   1686) carries goal `fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca` in
   `Blocked` state, `last_progress_update_at 2026-07-05`. Not Active/Done → the
   refuting "stale cache / resolved goal" branch is **rejected**.
2. **Count pinned at exactly 2 across 128 ticks / 6 days.** Parsing every
   `report.observed_details` in `overseer/activity.json.recent`:
   `128 occurrences, distinct N values = [2]` spanning `2026-07-10T23:25Z →
   2026-07-16T08:37Z`. Never 0, never 3. A restart/coincidence artifact would not
   hold a constant value for 128 consecutive emissions → refuting evidence **rejected**.
3. **Producer is deterministic + adjacent-dedup only.** `observation_signature`
   (`src/overseer/mod.rs:1068-1073`) sorts + `dedup()`s dedup_keys and prefixes
   `overseer-obs:` — collapses only identical keys, so cross-window repeats are
   genuine re-observations, not double counts.

**Verdict: CONFIRMED — with mechanism refinement.** The "2×" is honest, but it is a
**self-ingestion recurrence**: the problem `key` in `problem_entries` contains
`overseer-obs:` **twice** (`s.count('overseer-obs:') == 11` in the composite; the
prior write-back signature is re-observed as a fresh problem). The count is honest;
its *source* is the Overseer re-observing its own write-back plus the stably-blocked
kgpacks goal.

## H2 — "escalate-at-3 rung structurally unreachable (Reported ∈ takes_effect, ∉ records_occurrence)" → **CONFIRMED**

**Practical test (code trace + runtime counters):**

- **Predicate asymmetry confirmed in source.** `outcome_records_occurrence`
  (`src/overseer/wiring.rs:612-627`) omits both `ActOutcome::Reported` **and**
  `ActOutcome::WorkstreamGapsFlagged`; `outcome_takes_effect`
  (`wiring.rs:635-640`) returns true for everything except `WhisperSuppressed` /
  `GoalHealthSuppressed` → `Reported` **takes effect but records no occurrence**.
- **Accumulator → gate wiring confirmed.** `recurrence` fed to the `≥3` gate is
  `problem.why.recurrence` (`mod.rs:1469`), raised only via `record_occurrence`,
  itself gated by `outcome_records_occurrence` (`wiring.rs:276-280`). Threshold
  `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) consumed at
  `mod.rs:1613`.
- **Runtime proof the rung never fires.** 128 recurrences over 6 days never advanced
  the ProcessHealth counter past **2**, and `overseer/activity.json.totals` shows
  `issues_filed: 0` (the deduped root-cause escalation issue the ≥3 rung would file
  was **never** created). If occurrences accrued honestly, 128 repeats would have
  tripped the ≥3 gate long ago.

**Verdict: CONFIRMED.** The escalate-at-3 root-cause rung is empirically dead for this
lane; the signature is instead surfaced via the no-op `Reported` path each tick.

## H3 — "dead ENTRY DOOR: goal parked with BRAIN_FAILURE prefix but resolver selects only NO_PROGRESS prefix" → **PREMISE REFUTED (code asymmetry real, but not the cause here)**

**Practical test (compare park prefix vs resolver predicate on the live goal):**

- **The code asymmetry is real:** `is_no_progress_marker` (`no_progress_breaker.rs:88`)
  keys **only** on `NO_PROGRESS_BLOCKED_PREFIX`; `reinvestigate_bare_blocked_goals`
  (`no_progress.rs:808,832`) selects via `is_bare_no_progress_block` (needs that
  prefix); `sensor.rs:213` surfaces **both** prefixes for `needs_review`; the #1911
  brain-failure recovery (`advance_goal/mod.rs:101-125`) keys on
  `is_brain_failure_marker`. So the two resolver doors partition on prefix — as H3
  states.
- **BUT the live kgpacks block carries NEITHER prefix.** Reading the actual
  `status.Blocked` string from `state/goal_board.json`:
  > `Cycle 6, skip_count 0, failure_count 0, worktree active ~108m ago — the engineer
  > is healthy, not wedged… #16 is still OPEN in rysweet/agent-kgpacks-rs with no PR
  > and no landed baseline, so #17's gate is unmeasurable… This is a genuine hard
  > upstream dependency.`

  `has OODA-SAFEGUARD lock: False`, `OODA brain failing for: False`,
  `made no shippable progress: False`. It is a **plain descriptive
  upstream-dependency block**, deliberately authored (the engineer's own
  `record_blocker`), not a safeguard park.

**Verdict: REFUTED for this goal.** The BRAIN_FAILURE-vs-NO_PROGRESS door asymmetry
exists in code but does **not** explain the kgpacks non-resolution — the goal has no
safeguard prefix at all, so it is (correctly) neither reinvestigated nor
brain-failure-recovered. Consequently `needs_review = false` (`sensor.rs:213`), which
routes `decide_blocked_goal` to `Report` (`mod.rs:1630`) — this is what feeds H2's
dead-rung path. The block is a **legitimate** hold on upstream issue #16, not a
false-park needing terminal resolution.

## H4 — "success-shaped inaction: spawn-skip branches return success=true / 0 engineers, no breaker signal" → **CONFIRMED (code)**

**Practical test (trace return shape + runtime counters):**

- All three spawn-skip branches return `make_outcome(action, true, …)` with 0
  engineers: posture observe-only refusal (`advance_goal/spawn.rs:151-160`, "dispatches
  0 engineer(s)"), admission Defer (`spawn.rs:508-513`), resource Defer/ReclaimFirst
  (`spawn.rs:550-555`). Inline comment `spawn.rs:536-537` states the skip reuses the
  benign outcome with `goal_failure_counts` **untouched** → no no-action signal for the
  Door-1 breaker.
- **Runtime corroboration:** `totals.goals_unblocked: 0` (no goal ever terminally
  self-healed across all history) alongside `workstream_gaps_detected: 980` /
  `workstream_gaps_suppressed: 0` — gaps are detected en masse but never converted to a
  terminal fix; and `no_progress.counts` for the kgpacks goal sits at **1**
  (< breaker threshold 3), consistent with skips not accruing a no-progress strike.

**Verdict: CONFIRMED.** Spawn skips are success-shaped and emit no breaker signal.

---

## Consolidated verdict

| Hyp | Statement | Method | Verdict |
|-----|-----------|--------|---------|
| H1 | ×2 honest persistent signal | live state + 128-tick history | **CONFIRMED** (self-ingestion recurrence) |
| H2 | escalate-at-3 rung unreachable | code trace + `issues_filed:0`, pinned-at-2 | **CONFIRMED** |
| H3 | dead entry door via prefix asymmetry | park-prefix vs resolver on live goal | **PREMISE REFUTED** (asymmetry real; goal has no safeguard prefix — legit upstream block) |
| H4 | success-shaped spawn-skip inaction | return-shape trace + runtime counters | **CONFIRMED** |

**Root-cause synthesis (empirical):** The "seen 2×" alarm is an **honest but
self-inflicted** ProcessHealth signal. A **legitimately** Blocked goal (kgpacks WS2,
gated on the still-open upstream issue #16) plus a stable PR/gap set produce a stable
composite `overseer-obs:` signature every tick; the Overseer re-ingests its own
write-back (doubled `overseer-obs:` prefix), so ProcessHealth reports a recurrence
**pinned at 2**. Because that recurrence flows through the `Reported` no-op outcome
(H2: excluded from `outcome_records_occurrence`), it never accrues to the ≥3
root-cause-escalation rung (`issues_filed: 0`), and because the goal carries **no**
safeguard prefix (H3 refuted), neither WHY-reinvestigation nor brain-failure recovery
ever selects it. The alarm is therefore a durable low-grade repeat rather than a
symptom of a wedged goal — the underlying goal is correctly waiting on upstream #16.
