---
title: Overseer recurring-signature blockage diagnosis (kgpacks-rs parity)
description: >
  Investigation follow-up decoding the recurring Overseer composite signature
  `overseer-obs:goal:blocked:…|quality:gym_skipped|workstream-gap|resource:engineer_spawn`
  observed for the `advance-rysweet-agent-kgpacks-rs-to-full-parity` goal and its
  child workstreams. Identifies the concrete per-workstream blocker for each
  goal:blocked segment (stale safeguard-park vs. genuine open item), gives the
  code-verified semantic root cause of the `quality:gym_skipped` and
  `workstream-gap` co-signals, and assesses whether `resource:engineer_spawn`
  contention contributes to the blockage. Complements the round-1 finding that
  the signature self-amplifies via an unfiltered write-back → recall → re-signal
  loop. Round-3 consolidated the parallel deep dives and recorded the executed
  H1/H2 test results; round-4 re-anchored the AIMD `engineer_spawn` path to HEAD
  20fb7539 and refuted the amplifier/contributor reading at the code level
  (engineer_spawn is a passenger health signal, not a park driver — §3 upgraded to
  High confidence, P5 downgraded to optional). Round-5 re-executed the H1/H2 tests
  (36 passed, 0 failed) and re-verified every source anchor and live GitHub/board
  claim at HEAD db02dd7b; the primary-investigator extension mapped every signature
  token to its authoritative emission file:line, read the six live block reasons
  directly, and found the loop still active (tenth duplicate #2875) — the diagnosis
  holds unchanged (§7e, §7f). Round-6 re-executed the H1/H2 tests (36 passed, 0 failed)
  and re-resolved every source anchor — including the prompt-cited recall-path anchors
  `wiring.rs:1013-1031`, `capabilities.rs:607-616`, `signal.rs:455` — and the live
  GitHub/board state at HEAD `941f40cc`; the diagnosis holds unchanged (§7h). Round-7
  re-executed the per-hypothesis practical tests at HEAD `1190abb5` (H1: `tests_memory_recall`
  36 passed/0 failed + verbatim source anchors; H2/H3: live GitHub state + the
  terminal-park/no-reconciliation code path) — all three hypotheses re-confirmed with zero
  drift; the diagnosis holds unchanged (§7i). Round-9 re-executed the per-hypothesis practical
  tests at HEAD `0196c4f6` (H1: `tests_memory_recall` 36 passed/0 failed + the four hypothesis
  tests 4/4 in isolation + verbatim source anchors; H2: live #16/#18/#21/#22 CLOSED +
  terminal-park/no-reconciliation path; H3: #17 OPEN with a live-timestamp staleness proof) —
  all three re-confirmed, 10-issue Simard tail unchanged, zero drift (§7m).
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./overseer-goal-board-health-api.md
  - ./overseer-memory-recall-api.md
  - ./overseer-workstream-gap-scan.md
  - ./no-progress-breaker-api.md
  - ../concepts/overseer-goal-board-health.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../howto/recover-goal-board.md
  - ../howto/configure-overseer-memory-recall.md
---

# Overseer recurring-signature blockage diagnosis (kgpacks-rs parity)

## Scope

The Overseer repeatedly observed — and auto-filed GitHub issues for — the composite
signature:

```
overseer-obs:goal:blocked:advance-rysweet-agent-kgpacks-rs-to-full-parity-…|
goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-…|…|
quality:gym_skipped|workstream-gap|quality:gym_skipped|
resource:engineer_spawn|workstream-gap
```

Round 1 ([`overseer-memory-recall-api`](./overseer-memory-recall-api.md), issue
#2628 follow-up) established the **structural** cause: the signature self-amplifies
because `recall_episodic` recalls the Overseer's own `source_label = "overseer"`
write-backs with no provenance filter, and `observation_signature` folds the
resulting `RecurringSignature` problem back into the next write-back. The `2×`
count is the recalled-episode count; a grown/nested signature is a distinct
`WhisperGate` key, so dedup never suppresses it.

This document answers the **semantic** questions round 1 left open:

1. What is the concrete blocker behind each `goal:blocked:…` segment?
2. What do `quality:gym_skipped` and `workstream-gap` mean, and how do they relate
   to the blocked state?
3. Does `resource:engineer_spawn` (spawn contention) contribute to the blockage?

Every claim below is grounded in source (`file:line`) or in the live GitHub state
of `rysweet/agent-kgpacks-rs` and `rysweet/Simard` at the time of investigation
(2026-07-07 ~08:14 UTC). Each section states an explicit **confidence level**
(High/Medium); Section 6 consolidates these into a single ratings table.

---

## 1. Per-workstream concrete blockers — mostly **stale safeguard-parks**

### The finding

The five workstreams named in the signature are, in reality, **four delivered +
one intentionally-gated**. Only one is a live open item, and it is optional by
design:

| Segment (kgpacks-rs) | Issue | State @ investigation | Concrete blocker |
|---|---|---|---|
| ws1 `full-pack-cve` | #16 | **CLOSED** 2026-07-06 20:16Z | None — delivered. `goal:blocked` is **stale**. |
| ws2 `int8-pq-embed` | #17 | OPEN (unchanged since 2026-07-02) | **Intentional gate**: "spike… Ship behind a flag ONLY if parity holds." Optional, not a failure. |
| ws3 `versioned-rel` | #18 | **CLOSED** 2026-07-06 10:33Z | None — delivered. Stale. |
| ws6 `resumable-pip` | #21 | **CLOSED** 2026-07-06 13:29Z | None — delivered. Stale. |
| ws7 `sign-the-release` | #22 | **CLOSED** 2026-07-06 12:07Z | None — delivered. Stale. |
| parity umbrella | — | Substantially complete | 4/5 children delivered; the 5th (#17) and the only other open item (#32, "optional semantic-embeddings, non-default") are both explicitly optional. |

So **four of the five `goal:blocked` segments cite work that was already
finished** — in three cases 6–13 hours *before* the Overseer filed the issue for
it. The remaining one (#17) is a deliberately deferred, flag-gated spike, not a
blockage.

### Why the goal board still says "blocked" (root cause)

A `goal:blocked:{id}` signal is a pure projection of the goal board:
`sensor::blocked_goals_from_board` (`src/overseer/sensor.rs:204`) →
`blocked_goal_of` (`:209`) emits one `BlockedGoal` per goal whose status is
`GoalProgress::Blocked(reason)`. `needs_review` is set when the reason carries a
safeguard sentinel (`is_no_progress_marker || is_brain_failure_marker`).

These goals were parked by the **no-progress breaker**
(`src/goal_curation/no_progress_breaker.rs`). After
`NO_PROGRESS_BREAKER_THRESHOLD = 3` (`:58`) consecutive no-action cycles it runs
the done-gate **once** and, when it cannot certify completion, sets
`GoalProgress::Blocked` to the `NO_PROGRESS_BLOCKED_PREFIX` "…needs human review"
sentinel (`:69`). The done-gate requires hard evidence — a merged PR **and** a
closed issue **and** a deploy — and short-circuits to *not done* when
`!issue_closed` (`src/goal_curation/completion_gate.rs:31,380`).

The key defect: **a safeguard `Blocked` is terminal for the breaker** — the goal
"leaves the no-action loop and cannot accumulate another cycle" and waits for a
human (`simard goal unblock-all`). Nothing **re-runs the done-gate to auto-close
the goal once its issue is closed later.** So when ws1/ws3/ws6/ws7 shipped and
their issues closed hours after the park, the goal board kept the stale `Blocked`
rows, `blocked_goals_from_board` kept emitting `goal:blocked:{id}`, and — via the
round-1 self-recall loop — the composite signature kept recurring.

### Empirical confirmation

The Overseer auto-filed **10 near-duplicate** `rysweet/Simard` issues for this one
signature between 2026-07-06 11:01Z and 2026-07-07 11:31Z (all still OPEN; the tail
grew from 9 to 10 *during* the investigation — see §7f/§7g):

- parity-only signature: #2669, #2672, #2678, #2691
- nesting `issue-17`: #2744, #2750, #2757
- nesting `issue-16`: #2768 (2026-07-07 02:02Z), #2841 (2026-07-07 08:21Z), #2875 (2026-07-07 11:31Z)

**Smoking gun:** #2768 and #2841 were filed for `goal:blocked:…issue-16…`
**≈6 h and ≈12 h after kgpacks-rs #16 was closed** (2026-07-06 20:16Z). The blocker
they report did not exist at file time. This is exactly the stale-goal-board +
self-recall interaction: nine issues, a growing/nesting key, none deduped, none
resolved.

**Confidence: High.** Grounded in (a) the live closed/open state and timestamps of
kgpacks-rs #16/#18/#21/#22/#17, (b) the exact code path that projects `goal:blocked`
from parked goals (`sensor.rs:204,209`; `no_progress_breaker.rs:58,69`;
`completion_gate.rs:31,380`), and (c) the disprovable smoking-gun timing (issues
filed *after* the backing issue closed). The only inferential leap — that no
component re-runs the done-gate post-closure — is corroborated by the observed
9-issue tail (now 10, §7f/§7g) that a reconciliation step would have suppressed.

---

## 2. `quality:gym_skipped` and `workstream-gap` — independent ambient co-signals

Both are folded into the **same Observe tick** as the `goal:blocked` signals and
then swept up by the same self-recall nesting. Neither is a *cause* of the block;
each is an independent, separately-sourced indicator.

### 2a. `quality:gym_skipped` — an operator configuration, not a workstream failure

`Signal::GymSkipped` (`src/overseer/signal.rs:398`) fires whenever
`ObservedState.gym_skipped` is true, and maps to a **`QualityRegression`,
`Priority::Low`** problem with dedup key `quality:gym_skipped`
(`src/overseer/mod.rs:1295`).

`gym_skipped` is set in `sensor::observed_from_snapshot`
(`src/overseer/sensor.rs:125`) as `snap.gym.skip_gym || telemetry.gym_skipped` —
and both trace back to a single environment flag:
`let gym_skipped = env_flag("SIMARD_SKIP_GYM")` (`src/status/provider.rs:61`).
When that flag is set, `gym_runner_client::skip_gym()` short-circuits the gym to a
synthetic success (`src/gym_runner_client.rs:45,259,286`) — the self-eval never
runs.

**Semantic root cause:** the daemon is running with `SIMARD_SKIP_GYM` set, so
**every** Observe pass emits `quality:gym_skipped`. It is an *ambient, standing*
low-priority quality-regression signal (self-eval turned off deployment-wide),
completely independent of any kgpacks-rs workstream. Its doubling in the signature
(`quality:gym_skipped` ×2) is the round-1 recalled+fresh duplication, not two
distinct skips.

**Confidence: High.** The whole chain is a single deterministic, source-traced flag
path (`provider.rs:61` → `sensor.rs:125` → `signal.rs:398` → `mod.rs:1295`) with no
branching ambiguity; the only environmental assumption — that `SIMARD_SKIP_GYM` is
in fact set in the running daemon — is entailed by the signal firing at all.

### 2b. `workstream-gap` — uncovered backlog, and it **cannot** overlap a blocked goal

`Signal::WorkstreamGap` (`src/overseer/signal.rs:475`) carries every genuine
backlog-coverage gap the tick surfaced and maps to a **`WorkstreamCoverage`,
`Priority::High`** problem, dedup key `workstream-gap` (`src/overseer/mod.rs:1381`).

The gaps come from `sensor::detect_workstream_gaps`
(`src/overseer/sensor.rs:288`): a candidate is a gap iff it has **no active
workstream AND no open PR** (and, for anomalies, no fix in flight). It surveys
p1/p2 goals, high-signal open issues (labels `bug`/`P1`/`workflow:default`), and
live anomalies.

Crucially, **blocked goals are explicitly excluded** from the gap-scan and
delegated to goal-health instead (`:280`, and the guard at `:300`:
`if matches!(g.status, GoalProgress::Blocked(_)) { continue; }`). Therefore
`workstream-gap` and `goal:blocked:{id}` are **disjoint** — the same goal is never
counted as both. The `workstream-gap` segment is **other** uncovered high-priority
work (a p1/p2 goal or a high-signal issue with no live engineer and no open PR),
*not* the parity workstreams.

**Relation to the blocked state:** none causally. `workstream-gap` is a co-occurring
"we are under-covered somewhere" signal. It shares the blocked goals' underlying
condition — *a high-signal item with no live engineer making progress* — but views
a different slice of the board. It co-recurs in the signature only because it, too,
gets nested by the self-recall loop.

**Confidence: High.** The disjointness of `workstream-gap` and `goal:blocked` is a
hard code invariant, not an inference: the explicit guard
`if matches!(g.status, GoalProgress::Blocked(_)) { continue; }` (`sensor.rs:300`)
makes it impossible for a blocked goal to be counted as a gap. The exact identity of
the currently-uncovered item is not pinned down here (Medium on *that* detail), but
it is immaterial to the diagnosis.

---

## 3. `resource:engineer_spawn` — a passenger health signal, **not** a park driver (round-4 refutation of the AIMD-contraction contributor claim)

`Signal::EngineerSpawnRate { live }` (`src/overseer/signal.rs:393`) fires when
`ObservedState.live_engineers >= ENGINEER_SPAWN_THRESHOLD = 8` (`:351`), sourced
from `StatusSnapshot.resources.live_engineers`
(`src/overseer/capabilities.rs:81`). It maps to a **`ResourcePressure`,
`Priority::Normal`** problem, dedup key `resource:engineer_spawn`
(`src/overseer/mod.rs:1283`) — i.e. "≥8 engineers are live right now."

Round-2/3 rated this a *Medium-confidence amplifier* on the hypothesis that AIMD
`current_max` contraction below `live_engineers` **starves genuinely-open work of a
coverage slot, feeding the no-progress breaker that produces the next park.**
**Round-4 traced that hypothesised path through the code and refutes it:** neither
way AIMD contention can hurt a goal reaches the breaker that authors a
`goal:blocked` park. The signal is a co-emitted health indicator, not a cause.

### 3a. The AIMD mechanics (verified)

- The scaler is **AIMD**: multiplicative decrease (halve, `DECREASE_FACTOR = 0.5`)
  on a recent 429 **or** system pressure `> 0.8`; additive increase (`+1`) on
  pressure `< 0.3`; hold otherwise (`adaptive_scaling.rs:110–121`). Decrease is
  fast (8→4→2→1 in three cycles), recovery is slow (`+1`/cycle), so a 429 burst
  **can** drive `current_max` to the floor and hold it low. Bounds:
  floor `1`, ceiling `4 × max_concurrent_actions`, `initial = SIMARD_MAX_CONCURRENT_ACTIONS = 5`
  (`types.rs:288–301`).
- `decide_with_brain` sets the cycle `limit = scaler.adjust()` (`decide.rs:41–46`);
  `coverage_cap = scaler.current_max()` reuses that same value
  (`cycle.rs:316–320`) and bounds both allocation and Act dispatch (`cycle.rs:334`).
- **Decoupling A — the cap governs *new starts per cycle*, not the live count.**
  `live_engineers` is `count_live_engineer_claims` — a census of **already-running**
  worktree claims (`src/ooda_brain/context.rs:111`), independent of `current_max`.
  Contracting the cap does not stop the 8 live engineers; it only throttles *new*
  spawns. So the `≥8` that fires the signal and a contracted `current_max` are
  orthogonal axes — and, if anything, `≥8 live` means coverage is **abundant**,
  the *opposite* of a starved board.

### 3b. Why contraction cannot manufacture a `goal:blocked` park (two hard gaps)

A `goal:blocked` safeguard park is authored **only** by the no-progress breaker
(`no_progress.rs:249–261` → `GoalProgress::Blocked` sentinel). The breaker iterates
**`&outcomes`** and increments a goal's no-action counter **only** when
`outcome_made_no_progress(outcome)` is true, which requires
`outcome.success == true` **and** a semantic `detail.starts_with("no-action:")`
(`advance.rs:432–437`; `no_progress.rs:166–179`). Both AIMD failure modes miss it:

1. **Deferral ≠ outcome.** A coverage-starved goal is *truncated out* of `planned`
   (`coverage.rs:150–157`, `combined.truncate(cap)`) and is **never dispatched**, so
   it produces **no `ActionOutcome`** — the breaker never sees it and its counter is
   never bumped. Persistent starvation yields an *idle/uncovered* goal (which surfaces
   as `workstream-gap`, §2b), **not** a `goal:blocked` park.
2. **Failure ≠ no-progress.** A dispatched spawn that fails under 429/rate-limit sets
   `outcome.success = false`. That path routes to `state.goal_failure_counts`
   (`cycle.rs:360–370`) — an **urgency-demotion cooldown** consumed by Orient
   (`orient.rs:93–117`) — and is *explicitly excluded* from the breaker
   (`outcome_made_no_progress` requires `success == true`). A 429'd goal is *demoted*,
   never *parked*.

The only thing that parks a goal is a **dispatched engineer turn that succeeds but
ships nothing** ("no-action:" / rejected progress claim) — a *semantic* livelock that
is independent of the AIMD cap and of engineer count.

**Bottom line:** `resource:engineer_spawn` is a **pure passenger** — a standing
health signal folded into the same Observe tick and nested by the §4 self-recall
loop, exactly like `gym_skipped`. It rides the ambient `quality:gym_skipped |
workstream-gap` cluster of the composite key and is **never adjacent to a
`goal:blocked` segment** — the structural placement matches its role. It is neither
the root cause (§1 proves that) nor a demonstrable contributor to any park in this
signature.

**Confidence: High.** Both the *negative* root-cause claim (unchanged from §1) **and**
the *refutation of the contributor mechanism* are now source-grounded, not inferential:
the two decoupling gaps are hard code invariants (`advance.rs:433`,
`no_progress.rs:166`, `cycle.rs:360`, `coverage.rs:156`) that a reviewer can falsify by
inspection. The runtime known-unknown from round-3 (whether `current_max` ever
contracts below `live_engineers`) is now **immaterial**: even granting the contraction,
the code shows it *cannot* produce a `goal:blocked` park. This closes the round-3
"weakest link"; the dependent remediation (P5) is downgraded to optional health tuning
(see §5).

---

## 4. Why it recurred (2×) instead of resolving

Confirmed in round 1 and reinforced here:

- **Structural:** unfiltered self-recall (`recall_episodic` has no `source_label`
  filter) + re-wrap (`observation_signature` folds the recalled
  `RecurringSignature` back in) → the key grows/nests each generation; each grown
  key is distinct, so `WhisperGate` dedup can't suppress it. `2×` = recalled-episode
  count, not a retry counter.
- **Semantic (this doc):** the `goal:blocked` inputs are themselves **stale** —
  safeguard parks for work that later shipped but was never reconciled — so the
  Observe pass keeps regenerating the same segments every tick. `gym_skipped`
  (env-flag) and `workstream-gap` (standing coverage gap) are likewise ambient, so
  they persist tick-over-tick. A self-referential loop fed by standing inputs
  recurs indefinitely.

The empirical tail (9 open near-duplicate issues, growing keys) is the observable
consequence.

**Confidence: High.** The structural cause is confirmed by round-1 **and**
reproduced here through the real code path by the executable H1/H2
CONFIRM / REFUTE-by-fix tests added in `src/overseer/tests_memory_recall.rs`
(round 2): H1 shows `RecurringSignature` is re-emitted purely from the Overseer's
own write-backs, and H2 shows `observation_signature` stacks a prefix unboundedly.
The semantic cause follows directly from the High-confidence Sections 1–2.

---

## 5. Prioritized remediation

Ordered by leverage. P1/P2 sever the mechanism; P3/P4 clear the standing inputs.

1. **P1 — Cut the self-recall loop (root cause; round-1 Option A).** In
   `recall_episodic` (`src/overseer/wiring.rs`), drop episodes whose
   `source_label == OVERSEER_SOURCE_LABEL` before mapping to `RecalledEpisode`, so
   the Overseer stops recalling its own write-backs. Invert the H1–H4 tests to
   assert the loop is broken. This alone stops the growth/nesting and the
   dedup-escaping issue floods.
2. **P2 — Reconcile stale safeguard-parks against issue/PR closure.** Add a
   goal-board reconciliation step that re-runs the done-gate for
   safeguard-`Blocked` goals whose backing issue is now closed / PR merged, and
   auto-archives (or drops) them instead of holding a terminal park forever. This
   removes the *stale* `goal:blocked` segments for #16/#18/#21/#22 at the source
   (see `howto/recover-goal-board.md`, `howto/unblock-stuck-ooda-goals.md`).
3. **P3 — Clear the current backlog (only durable when paired with P2).** Run
   `simard goal unblock-all` (or equivalent) to clear the stale parks now; bulk-close
   the 10 duplicate `rysweet/Simard` issues (#2669, #2672, #2678, #2691, #2744, #2750,
   #2757, #2768, #2841, #2875) as artifacts of this loop, referencing this diagnosis.
   **Caveat (round-5, #2707):** a bare unblock does **not** stick — the umbrella is a
   standing goal not tagged perpetual, so it re-parks every breaker cycle. P3 must be
   paired with P2 (done-gate reconciliation + perpetual-tag/archival) to hold.
4. **P4 — Resolve the ambient co-signals.** Decide `#17` (ws2 int8-pq-embed)
   explicitly — either run its parity gate and ship-behind-flag or mark it
   obsolete/deferred so it stops reading as open work. If the gym is intentionally
   off in this deployment, suppress or down-rank `quality:gym_skipped` so an
   expected config stops adding perpetual noise; otherwise unset `SIMARD_SKIP_GYM`.
5. **P5 — Spawn/throughput tuning (optional health, *not* a fix for this signature).**
   Round-4 shows AIMD contention **cannot** author a `goal:blocked` park (deferral
   produces no outcome; 429 dispatch failures route to `goal_failure_counts`
   urgency-demotion, never the breaker — §3b), so no spawn change is required to stop
   the recurrence. Independently, if telemetry shows `current_max` contracting below
   `live_engineers` while open work waits, raising the floor / tuning the scaler is a
   reasonable *throughput* improvement — but treat `resource:engineer_spawn` like
   `gym_skipped`: a standing ambient signal. P1's provenance filter already stops it
   being nested/amplified, so it needs no dedicated remediation for this signature.

**Confidence in remediation efficacy:** High for **P1–P3** — they sever the verified
mechanism (P1) and clear the verified stale inputs (P2/P3), each tied to a confirmed
root cause. Medium for **P4** (down-ranking `gym_skipped` is High; the correct
disposition of #17 is a product decision, not a diagnostic certainty). **P5 is
optional** and no longer gated on an unobserved AIMD claim — round-4 (§3b) shows spawn
contention cannot produce a park in this signature at all, so P5 is a throughput
nicety, not a fix.

---

## 6. Confidence assessment

Overall confidence in the diagnosis: **High.** As of round-4 **all six** findings rest
on disprovable, source-grounded or live-state evidence; the former Medium item (§3's
spawn-contention mechanism) has been **refuted at the code level** and its dependent
remediation (P5) downgraded to an optional throughput nicety.

| # | Finding | Confidence | Primary evidence | Residual uncertainty |
|---|---|---|---|---|
| 1 | Blocked segments are **stale safeguard-parks** for delivered work (#16/#18/#21/#22); #17 is a **stale-premise dep-block** (same reconciliation defect), still optional to ship | **High** | Live closed/open issue states + timestamps; direct `simard goal list` board read (§7f); `sensor.rs:204,209`, `no_progress_breaker.rs:58,69`, `completion_gate.rs:31,380`; smoking-gun timing (#2768/#2841/#2875 filed after #16 closed) | "No component re-runs the done-gate post-closure" is inferred (corroborated by the 10-issue tail + #2707 re-park escalation) |
| 2a | `quality:gym_skipped` = ambient `SIMARD_SKIP_GYM` operator flag, not a workstream failure | **High** | Deterministic flag path `provider.rs:61`→`sensor.rs:125`→`signal.rs:398`→`mod.rs:1295` | None material (flag-set state entailed by the signal firing) |
| 2b | `workstream-gap` is disjoint from `goal:blocked` (uncovered *other* backlog) | **High** | Hard code invariant `sensor.rs:300` excludes blocked goals; `sensor.rs:288`, `signal.rs:475`, `mod.rs:1384` | Exact identity of the uncovered item not pinned (immaterial) |
| 3 | `resource:engineer_spawn` is a **passenger health signal**, not the root cause **and not a park contributor** (round-4 refutation) | **High** | Two hard decoupling gaps: deferral produces no outcome (`coverage.rs:156`, `no_progress.rs:166`); 429 failure → `goal_failure_counts` demotion, excluded from breaker (`advance.rs:433`, `cycle.rs:360`, `orient.rs:93–117`); `live_engineers` census decoupled from `current_max` (`context.rs:111`) | None material — the runtime AIMD-contraction question is now immaterial (contraction cannot park a goal regardless) |
| 4 | Recurrence (2×) = unfiltered self-recall + unbounded re-wrap over standing inputs | **High** | Round-1 finding **reproduced** by executable H1/H2 tests (`tests_memory_recall.rs`, round 2) | None material |
| 5 | Remediation: **P1** severs the amplifier, **P2** removes standing stale inputs and is required for the fix to *stick*, **P3** clears the 10-issue backlog only when paired with P2, P4 conditional, **P5 optional** | **High (P1–P3)**, **Medium (P4)**, **P5 optional** | Each action mapped to a confirmed root cause; #2707 shows bare unblock (P3-alone) re-parks; P5 no longer gated on §3 | P4 #17 disposition is a product decision |

**Method note:** confidence is graded against how disprovable each claim is —
"High" means grounded in source (`file:line`) or live GitHub/board state that a
reviewer can independently re-check and that would falsify the claim if wrong;
"Medium" means the causal *path* is verified but a required runtime condition was
not directly observed.

---

## 7. Consolidation & verification (rounds 3–10)

The consolidation pass (rounds 3–4, this update) reconciled the parallel deep dives
against the live working tree at **HEAD `20fb7539`** and **executed** the round-2
hypothesis tests. Result: the diagnosis holds unchanged; every mechanism cited in
Sections 1–4 was re-confirmed at its current-tree line, the structural root cause is
now **proven by passing tests**, and the round-3 residual Medium item (§3
spawn-contention) was **refuted at the code level** in round-4 (§7d). Round-3
originally reconciled the anchors at HEAD `0180b75c`; they carry forward to `20fb7539`
with no line drift.

### 7a. Executable proof — H1/H2 tests PASS

`cargo test --lib overseer::tests_memory_recall::h` → **4 passed, 0 failed**:

| Test | Claim | Result |
|---|---|---|
| `h1_confirm_self_recall_reemits_recurring_signature_from_own_writebacks` | Recalling the Overseer's own `source_label="overseer"` write-backs re-emits `RecurringSignature` from self-authored data | **ok** |
| `h1_refute_by_fix_provenance_filter_collapses_the_loop` | Dropping `source_label==OVERSEER_SOURCE_LABEL` episodes before mapping breaks the loop | **ok** |
| `h2_confirm_observation_signature_stacks_prefix_each_generation` | `observation_signature` re-prefixes `overseer-obs:` every generation → unbounded nesting | **ok** |
| `h2_refute_by_fix_idempotent_signature_is_a_fixed_point` | An idempotent (prefix-collapsing) signature is a fixed point | **ok** |

The CONFIRM tests reproduce the defect **through the real `MemoryRecallOps`
adapter** (`recall_episodes_ranked → recall_episodic`); the REFUTE-by-fix tests
demonstrate that remediation **P1** (provenance filter) and its H2 analogue
(idempotent signature) collapse the loop. This upgrades §4's structural cause from
"reproduced" to "verified green in CI-runnable form."

### 7b. Reconciled current-tree line numbers (HEAD `20fb7539`)

The round-2 doc quoted line numbers from an earlier commit; they have since drifted
~3 lines. The mechanism is unchanged — only the anchors moved. Canonical anchors:

| Mechanism | Round-2 doc cite | **Current tree (`20fb7539`)** |
|---|---|---|
| Composite key assembly `format!("overseer-obs:{}", …)` | `mod.rs:1081-1086` | `mod.rs:1081` (def), body `1082-1086`; re-prefix at **`1085`** |
| Called from Observe | — | `mod.rs:544` (`let signature = observation_signature(problems)`) |
| Write-back gate `WhisperGate::new(900, 5)` | `mod.rs:297` | **`mod.rs:297`** (`write_back_gate`) |
| dedup key `resource:engineer_spawn` | `mod.rs:1280` | **`mod.rs:1283`** |
| dedup key `quality:gym_skipped` | `mod.rs:1292` | **`mod.rs:1295`** |
| dedup key `goal:blocked:{id}` | `mod.rs:1349` | **`mod.rs:1349`** |
| dedup key `workstream-gap` | `mod.rs:1381` | **`mod.rs:1384`** |
| `RECURRING_SIGNATURE_THRESHOLD = 2` | `signal.rs:362` | **`signal.rs:362`** |
| Recall counting loop (`signals_from`) | `signal.rs:455-470` | **`signal.rs:366`** (fn), count+emit **`458-464`** |
| Provenance dropped — `RecalledEpisode` has no `source_label` | `wiring.rs:1013-1031` | **`wiring.rs:1024-1029`** (map omits `source_label`) |
| Fixed write-back provenance | `wiring.rs:1088` | **`wiring.rs:952`** (`OVERSEER_SOURCE_LABEL`), stored at **`1088`** |
| `[sig:…]` marker re-parse | `wiring.rs:976-986` | **`wiring.rs:976`** (`parse_failure_signature`) |
| H1/H2 tests | `tests_memory_recall.rs:1108/1194` | **`:1109` / `:1199`** (H1/H2 CONFIRM) |

### 7c. Consolidated conclusion

The parallel deep dives converge on a single, coherent account with **no
contradictions** between round-1 (structural) and round-2 (semantic):

- **One self-amplifying loop, two standing input classes.** The `2×` is the
  recalled-episode count from an unfiltered self-recall + unbounded re-wrap
  (structural, §4, now test-proven §7a). It never resolves because its inputs are
  *standing*: four **stale safeguard-parks** for already-shipped kgpacks-rs work
  (#16/#18/#21/#22) plus one intentional gate (#17) (§1), an **ambient
  `SIMARD_SKIP_GYM`** flag (§2a), and a **disjoint standing coverage gap** (§2b).
  `resource:engineer_spawn` is a passenger health signal, not the cause (§3;
  round-4 refutes even the amplifier/contributor reading).
- **The two axes are independent and both must be cut.** Severing the loop (P1)
  stops the growth/nesting and the dedup-escaping issue floods; reconciling stale
  parks (P2) + clearing the backlog (P3) removes the standing inputs. Neither alone
  is sufficient — P1 without P2/P3 leaves a single non-growing recurrence; P2/P3
  without P1 leaves the amplifier ready to re-nest on the next standing input.
- **Overall confidence: High** (unchanged verdict, strengthened evidence). Round-3
  raised §4 to test-proven; **round-4** upgraded §3 from Medium to High by refuting
  the AIMD-contributor mechanism at the code level, so **all six** findings are now
  source/live-state grounded with no remaining Medium diagnostic item.

No new remediation is introduced; **P1–P5 (Section 5) stand as the consolidated
action set** (P5 now optional), with P1's fix backed by a green REFUTE-by-fix test.

### 7d. Round-4 addendum — AIMD `engineer_spawn` refutation

The round-4 tertiary deep dive re-anchored the AIMD path to HEAD `20fb7539` and
**refuted** the round-3 "Medium-confidence amplifier/contributor" reading of
`resource:engineer_spawn`. Two hard code-level decouplings (fully detailed in the
rewritten §3b) prove AIMD contention **cannot** author a `goal:blocked` park:

| Hypothesised harm from AIMD contraction | Why it never reaches the park-producing breaker |
|---|---|
| Coverage-starve a genuinely-open goal (deferral) | Deferred goals are `truncate`d out of `planned` (`coverage.rs:150–157`) → **no `ActionOutcome`** → breaker iterates `&outcomes` (`no_progress.rs:166`) and never sees them → surfaces as `workstream-gap` (§2b), not `goal:blocked` |
| 429/rate-limit a dispatched spawn | `outcome.success = false` → routed to `goal_failure_counts` urgency-demotion (`cycle.rs:360–370` → `orient.rs:93–117`); `outcome_made_no_progress` **requires** `success == true` (`advance.rs:432–437`) → excluded from the breaker |

Additionally `live_engineers` (running-worktree census, `context.rs:111`) is
decoupled from `current_max` (per-cycle new-start cap), and the `≥8` firing
threshold co-occurs with **abundant**, not starved, coverage. Structurally, in the
observed composite key `resource:engineer_spawn` appears **only** inside the ambient
`quality:gym_skipped | workstream-gap` cluster and never adjacent to a `goal:blocked`
segment — its placement matches a passenger, not a driver. Net: §3 → **High**; the
round-3 runtime known-unknown (does `current_max` contract below `live_engineers`?)
is now **immaterial**; **P5 → optional**.

### 7e. Round-5 addendum — practical verification re-executed at HEAD `db02dd7b`

Round 5 (this update) **re-ran the executable hypothesis tests and re-checked every
source anchor and live-state claim** against the current working tree at HEAD
`db02dd7b` (2026-07-07 ~12:25 UTC). HEAD moved from the round-4 anchor `20fb7539`
by exactly one commit (`db02dd7b`, docs-only), so no source line drifted; every
mechanism resolves at the §7b/§3b anchors (±1–3 lines, within the noted tolerance).
The diagnosis holds unchanged.

**Executable proof — full module green.** `cargo test --lib
overseer::tests_memory_recall` → **36 passed, 0 failed**, including the four
hypothesis tests re-run in isolation (`…::h` → **4 passed, 0 failed**):

| Hypothesis | Test | Result |
|---|---|---|
| H1 CONFIRM | `h1_confirm_self_recall_reemits_recurring_signature_from_own_writebacks` | **ok** |
| H1 REFUTE-by-fix | `h1_refute_by_fix_provenance_filter_collapses_the_loop` | **ok** |
| H2 CONFIRM | `h2_confirm_observation_signature_stacks_prefix_each_generation` | **ok** |
| H2 REFUTE-by-fix | `h2_refute_by_fix_idempotent_signature_is_a_fixed_point` | **ok** |

The surrounding dedup tests (`write_back_is_deduplicated_within_window`,
`write_back_persists_again_for_a_distinct_signature`, `tick_writes_observation_back_once`)
also passed, independently corroborating the §4 write-back → recall → re-signal
mechanics.

**Source anchors re-resolved (spot-checked at HEAD `db02dd7b`).** All present and
semantically unchanged: §1 `sensor.rs:204` `blocked_goals_from_board`, `:209`
`blocked_goal_of`, `no_progress_breaker.rs:58` `NO_PROGRESS_BREAKER_THRESHOLD = 3`,
`:69` `NO_PROGRESS_BLOCKED_PREFIX`, `completion_gate.rs:31` `issue_closed`, `:380`
`if !issue_closed`; §2a `provider.rs:61` `env_flag("SIMARD_SKIP_GYM")`,
`sensor.rs:125` `skip_gym || …gym_skipped`, `signal.rs:399` `Signal::GymSkipped`,
`mod.rs:1295` `quality:gym_skipped`; §2b `sensor.rs:300`
`matches!(g.status, GoalProgress::Blocked(_)) { continue; }` (hard disjointness
guard), `sensor.rs:288` `detect_workstream_gaps`, `mod.rs:1384` `workstream-gap`;
§3b `advance.rs:432` `outcome_made_no_progress` requiring `outcome.success` (`:433`)
+ `detail.starts_with("no-action:")` (`:435`), `no_progress.rs:171/175` breaker gate
+ `success` check → `:260` `GoalProgress::Blocked`, `cycle.rs:316–322/359–362`
`coverage_cap = current_max` + `goal_failure_counts` demotion, `coverage.rs:156`
`combined.truncate(cap)`, `context.rs:111` `count_live_engineer_claims`,
`adaptive_scaling.rs:21` `DECREASE_FACTOR = 0.5`; §7b `mod.rs:297` `write_back_gate`,
`:544` `observation_signature(problems)`, `:1081` `fn observation_signature`, `:1283`
`resource:engineer_spawn`, `:1349` `goal:blocked:{id}`, `signal.rs:351`
`ENGINEER_SPAWN_THRESHOLD = 8`, `:362` `RECURRING_SIGNATURE_THRESHOLD = 2`, `:463`
`occurrences >= threshold`.

**Live state re-confirmed (2026-07-07 ~12:25 UTC).** Unchanged from §1:

- `rysweet/agent-kgpacks-rs`: **#16 CLOSED** 2026-07-06T20:16:25Z, **#18 CLOSED**
  10:33:04Z, **#21 CLOSED** 13:29:03Z, **#22 CLOSED** 12:07:33Z (all four
  delivered → stale parks); **#17 OPEN** ("int8/PQ … gated on parity" — intentional
  gate). Every timestamp matches §1 to the second.
- `rysweet/Simard`: all **9 duplicate issues still OPEN** (#2669, #2672, #2678,
  #2691, #2744, #2750, #2757, #2768, #2841), each titled "recurring signature seen
  2× in cognitive memory". **Smoking gun intact:** #2768 (created 2026-07-07T02:02:16Z,
  ≈6 h after #16 closed) and #2841 (08:21:50Z, ≈12 h after) both report a
  `goal:blocked:…issue-16…` blocker that no longer existed at file time.

Net: the four executable hypotheses (H1/H2 CONFIRM + REFUTE-by-fix) **pass**, and the
source-anchored semantic hypotheses (§1 stale-park root cause, §2a ambient
`gym_skipped`, §2b `workstream-gap` disjointness, §3 `engineer_spawn` passenger) are
each **re-verified** by resolvable code anchors and live GitHub/board state. No finding
changed; overall confidence remains **High**.

### 7f. Round-5 primary-investigator extension — token→source map + live-board read (HEAD `db02dd7b`)

This pass (parallel to §7e) adds the explicit **per-token emission map** the
investigation asked for and three live-state facts that post-date or refine §7e: a
**tenth** duplicate issue, a **direct goal-board read** of the actual block reasons,
and the **stewardship escalation** that explains why symptom-level unblocks don't stick.

**Signature token → authoritative emission (file:line), HEAD `db02dd7b`.** Every
composite-signature token maps to exactly one authoritative emission site. `observer.rs`
is listed in the investigation scope but **emits no token** — it is the Decide-phase
*consumer* (`decide_read_only`, `observer.rs:126`; reads `problem.dedup_key` at
`:161/:209`), not a source.

| Token (occurrences in key) | Fires when | Signal (`signal.rs`) | Dedup key (`mod.rs`) | Ultimate provenance | Conf. |
|---|---|---|---|---|---|
| `overseer-obs:` prefix | composite key assembled each Observe tick | — | `observation_signature` → `format!("overseer-obs:{}")` `:1085`; called `:544` | self write-back re-wrap (§4) | **High** |
| `goal:blocked:{id}` (×6: parity umbrella + #16/#17/#18/#21/#22) | goal status is `GoalProgress::Blocked` | `GoalBlocked` `signal.rs:441` (fed by `ObservedState.blocked_goals`, `capabilities.rs`; populated by `blocked_goals_from_board` `sensor.rs:204`→`blocked_goal_of` `:209`) | `goal:blocked:{goal_id}` `:1349` | no-progress breaker park (`no_progress_breaker.rs:69`) for 5 of 6; #17 = engineer `record_blocker` dep-block (see below) | **High** |
| `quality:gym_skipped` (×2) | `ObservedState.gym_skipped == true` | `GymSkipped` `signal.rs:398` | `quality:gym_skipped` `:1295` | single `env_flag("SIMARD_SKIP_GYM")` `provider.rs:61`, fanned to `assemble_gym` (`:78`) **and** `assemble_telemetry` (`:83`), rejoined by availability-tolerant OR-fold `sensor.rs:125-126` | **High** |
| `workstream-gap` (×2) | `!ObservedState.workstream_gaps.is_empty()` | `WorkstreamGap` `signal.rs:475` | `workstream-gap` `:1384` | `detect_workstream_gaps` `sensor.rs:288`; blocked goals **excluded** by hard guard `:300` (disjoint from `goal:blocked`) | **High** (mechanism); **Medium** (identity of the uncovered item) |
| `resource:engineer_spawn` (×1) | `live_engineers >= ENGINEER_SPAWN_THRESHOLD (8)` | `EngineerSpawnRate` `signal.rs:393-396` (threshold `:351`) | `resource:engineer_spawn` `:1283` | `StatusSnapshot.resources.live_engineers` → `ObservedState.live_engineers` `capabilities.rs:81` (running-worktree census `context.rs:111`) | **High** |
| token **doubling / block nesting** (the `×2` counts; `overseer-obs:goal:blocked:…` repeated ~6×) | ≥2 recalled episodes share a `failure_signature` | `RecurringSignature` `signal.rs:463-467` (count `:459-463`) | dedup key = the recalled composite string itself, `sanitize_recalled(signature)` `:1372` | unfiltered self-recall `recall_episodic` `wiring.rs:1024-1030` (no `source_label` drop) over own write-backs `record_observation` `:1088` (`source_label="overseer"`); the giant recalled key survives `keys.dedup()` `:1084` next to the short fresh tokens → same token appears twice / block nests | **High** (test-proven, §7a) |

The last row is the mechanistic linchpin behind the "seen **2×**" and the visible
doubling: the `2×` is the recall `occurrences` count (`signal.rs:459-463`), and because
`RecurringSignature`'s dedup key is the *entire prior composite string* (`mod.rs:1372`),
`observation_signature`'s sort+`dedup()` (`:1084`) cannot collapse it against the short
fresh `quality:gym_skipped` / `workstream-gap` / `resource:engineer_spawn` tokens — so
each recalled generation nests intact beside a fresh copy. Not two upstream events; one
recalled + one fresh.

**Live goal-board read (`simard goal list`, 2026-07-07 ~12:30 UTC) — actual block
reasons.** This directly reads the six blocked rows (previously inferred):

| Goal | Live board status | Classification |
|---|---|---|
| `advance-…-full-parity-f29bb15c` (umbrella) | `🔒 [OODA-SAFEGUARD] … needs human review` | **stale safeguard-park** |
| `…issue-16-ws1-full-pack-cve` | `🔒 [OODA-SAFEGUARD] … needs human review` | **stale park** (#16 CLOSED 20:16Z) |
| `…issue-17-ws2-int8-pq-embed` | `Cycle 6 … engineer healthy … "WS2 #17's done-criterion is gated on eval recall parity, which depends on WS1 #16's eval baseline. #16 is still OPEN … no landed baseline … genuine hard upstream dependency"` | **genuine dep-block on a STALE premise** — see below |
| `…issue-18-ws3-versioned-rel` | `🔒 [OODA-SAFEGUARD] … needs human review` | **stale park** (#18 CLOSED) |
| `…issue-21-ws6-resumable-pip` | `🔒 [OODA-SAFEGUARD] … needs human review` | **stale park** (#21 CLOSED) |
| `…issue-22-ws7-sign-the-rele` | `🔒 [OODA-SAFEGUARD] … needs human review` | **stale park** (#22 CLOSED) |

The `[OODA-SAFEGUARD] … needs human review` text matches `NO_PROGRESS_BLOCKED_PREFIX`
(`no_progress_breaker.rs:69`) + `…_SUFFIX` (`:74`) verbatim, confirming §1's park path
from the live board (not just inference).

**Refinement to §1 for #17.** #17 is *not* a safeguard-park — its live block is an
engineer-authored `record_blocker` naming a real upstream dependency (WS1 #16's eval
baseline). But its premise is **now stale**: the reason asserts "#16 is still OPEN … no
landed baseline," whereas **#16 CLOSED 2026-07-06T20:16:25Z**. So #17 is blocked-on-a-
stale-dependency-premise that nothing re-evaluated after #16 closed — the *same* missing
done-gate/block reconciliation defect as §1, now demonstrated for a **non-safeguard**
block too. (#17 remains legitimately deferrable as the flag-gated spike; the point is the
block *reason* is unreconciled, not that #17 must ship.)

**Loop still live — tail is now 10, not 9.** A **tenth** near-duplicate,
**#2875** (created 2026-07-07T11:31:36Z), was filed ≈3 h after #2841 and carries the
identical composite key from the investigation prompt. Full parity-signature tail (all
OPEN): #2669, #2672, #2678, #2691, #2744, #2750, #2757, #2768, #2841, **#2875**. The
recurrence is ongoing at investigation time, not historical.

**Stewardship escalation explains the persistence — #2707.** `rysweet/Simard` **#2707**
(`[stewardship] recurring_goal_reblock in simard::overseer`) records that the parity
umbrella goal `is repeatedly re-parked despite symptom-level unblocks`, with systemic
root cause `standing-goal-not-tagged-perpetual` and failed-step
`goal-unblock:advance-…-full-parity-f29bb15c`. This corroborates P2/P3: operator
`unblock` has already been tried and does **not** stick, because the umbrella is a
standing goal that can never satisfy a terminal done-gate (`completion_gate.rs` requires
`pr_merged` `:28` ∧ `issue_closed` `:31` ∧ `deployed` `:34`) and is not tagged perpetual,
so it re-parks every breaker cycle. The durable fix is tagging/archival + done-gate
reconciliation (P2), not another symptom unblock (P3 alone is insufficient here).

**Repo routing confirmed intact (not a `repo=None` mis-route).** The kgpacks-rs goals
park with SAFEGUARD/dependency reasons — **none** shows a repo-resolution error. By
contrast, an unrelated board goal (`fix-rustsec-…-amplihack-xpia-defender`) *does* park
with `NOT_A_REPO: '…/amplihack-xpia-defender' is not inside a valid git worktree`
(emitted at `error/display.rs:158`). That contrast proves the resolver is live and that
routing to `rysweet/agent-kgpacks-rs` resolves cleanly; the recurrence is a
stale-sentinel + missing done-gate re-run problem, **not** a routing failure.

Net: the token map resolves every signature segment to a single High-confidence emission
site; the live board read upgrades §1's #16/#18/#21/#22 parks and the #17 dep-block from
inference to direct observation; #2707 + the growing tail (→#2875) confirm the loop is
active and that symptom unblocks don't hold. No prior finding changed; §1's #17 reading
is *refined* (stale-premise dep-block, still optional to ship). Overall confidence
remains **High**.

### 7g. Round-5 consolidation — parallel dives unified (HEAD `941f40cc`)

This pass unifies the two round-5 parallel deep dives — **§7e** (practical
re-verification: full `tests_memory_recall` module + anchor/live-state re-check) and
**§7f** (primary-investigator extension: token→emission map + direct board read) — into
one conclusion and folds their net-new facts into the canonical findings (§1/§5/§6).
Both dives were anchored at HEAD `db02dd7b`; HEAD has since advanced **two docs-only
commits** to `941f40cc` (`ec5e11e1` = §7e, `941f40cc` = §7f), so **no source line
drifted** and every §7b/§3b/§7f anchor still resolves.

**Independent re-verification (this consolidation, 2026-07-07 ~12:49 UTC)** — re-run by
the consolidator, not inherited:

- `cargo test --lib overseer::tests_memory_recall::h` → **4 passed, 0 failed** (H1/H2
  CONFIRM + REFUTE-by-fix) — the structural root cause (§4/§7a) is still proven green.
- `rysweet/agent-kgpacks-rs`: **#16 CLOSED 2026-07-06T20:16:25Z** (to the second),
  **#17 OPEN** — §1 stale-park and §7f #17-dep-block premises intact.
- `rysweet/Simard` duplicate tail: **exactly 10** (#2669, #2672, #2678, #2691, #2744,
  #2750, #2757, #2768, #2841, #2875; newest @ 2026-07-07T11:31:36Z), **no 11th** at
  consolidation time. **#2707** stewardship escalation **OPEN**.

**Zero contradictions between the parallel dives.** §7e and §7f agree on every shared
claim (test results, code anchors, #16/#17 state, smoking-gun timing) and are additive,
not conflicting: §7e re-proves the mechanism through the tests; §7f observes it on the
live goal board. The consolidation makes exactly **one weighting change** (P3→must-pair-
with-P2, below); no finding is overturned.

**Net-new round-5 facts folded into the canonical findings:**

1. **§1 parks upgraded inference → observation.** The live `simard goal list` read (§7f)
   shows five rows carrying the verbatim `[OODA-SAFEGUARD] … needs human review`
   sentinel (`no_progress_breaker.rs:69`), directly confirming §1's park path rather than
   inferring it.
2. **#17 reading refined (not overturned).** #17 is an engineer `record_blocker`
   dep-block on WS1 #16's eval baseline whose premise ("#16 still OPEN") went **stale when
   #16 closed 20:16Z** — the *same* missing done-gate/block reconciliation defect as §1,
   now shown for a **non-safeguard** block too. #17 remains optional to ship (§6 row 1
   updated accordingly).
3. **Loop is live; tail 9 → 10.** #2875 (≈3 h after #2841) carries the identical composite
   key — recurrence is ongoing at investigation time, not historical (§1 empirical +
   §5 P3 updated).
4. **Why symptom-unblocks don't stick (#2707).** The umbrella goal is a **standing goal
   not tagged perpetual**, so it can never satisfy the terminal done-gate
   (`completion_gate.rs` `pr_merged`:28 ∧ `issue_closed`:31 ∧ `deployed`:34) and re-parks
   every breaker cycle. → **P3 (bulk unblock) alone is insufficient**; the durable fix is
   **P2** (done-gate reconciliation + perpetual-tag/archival). This is the sole
   remediation-weighting change round-5 makes (§5 P3 + §6 row 5 updated).
5. **Routing intact.** kgpacks-rs goals park with SAFEGUARD/dependency reasons, **not** a
   `repo=None`/`NOT_A_REPO` resolver error (contrast: an unrelated `amplihack-xpia-defender`
   goal *does* fail that way, `error/display.rs:158`) — the recurrence is a stale-sentinel
   + missing done-gate re-run, not a routing failure.

**Consolidated verdict (all 5 rounds).** Unchanged and strengthened — **High** confidence,
all six findings source- or live-state-grounded, no remaining Medium diagnostic item.
One self-amplifying loop (unfiltered self-recall + unbounded re-wrap, **test-proven**)
fed by standing inputs: four **stale safeguard-parks** (#16/#18/#21/#22), one
**stale-premise dep-block** (#17), an ambient **`gym_skipped`** flag, and a disjoint
**`workstream-gap`**; **`engineer_spawn` is a passenger**, not a driver (round-4
refutation). Action set: **P1** (provenance filter) severs the amplifier; **P2**
(done-gate/park reconciliation + perpetual tagging) removes the standing stale inputs and
is required for the fix to *stick*; **P3** clears the current 10-issue backlog but only
durably when paired with P2; **P4** conditional (#17 disposition + `gym_skipped`
down-rank); **P5** optional throughput nicety.

### 7h. Round-6 addendum — practical verification re-executed at HEAD `941f40cc`

Round 6 (this update) **re-ran the executable hypothesis tests and re-resolved every
source anchor and live-state claim** at HEAD `941f40cc` (2026-07-07 ~12:50 UTC). HEAD
advanced from round-5's `db02dd7b` by one docs-only commit (`941f40cc`, the §7f
extension), so **no source line drifted**; every mechanism resolves at its §7b/§7f
anchor.

**Executable proof — full module still green.** `cargo test --lib
overseer::tests_memory_recall` → **36 passed, 0 failed**. The four hypothesis tests
pass, and the write-back/dedup corroborators independently reproduce the §4 loop
mechanics:

| Hypothesis | Test | Result |
|---|---|---|
| H1 CONFIRM | `h1_confirm_self_recall_reemits_recurring_signature_from_own_writebacks` | **ok** |
| H1 REFUTE-by-fix | `h1_refute_by_fix_provenance_filter_collapses_the_loop` | **ok** |
| H2 CONFIRM | `h2_confirm_observation_signature_stacks_prefix_each_generation` | **ok** |
| H2 REFUTE-by-fix | `h2_refute_by_fix_idempotent_signature_is_a_fixed_point` | **ok** |
| §4 write-back provenance | `adapter_write_back_uses_fixed_overseer_source_label` | **ok** |
| §4 write-back once/dedup | `tick_writes_observation_back_once`, `write_back_is_deduplicated_within_window`, `write_back_persists_again_for_a_distinct_signature` | **ok** |
| §7 recall count/emit | `recurring_signature_emitted_when_two_episodes_share_signature`, `recurring_signature_not_emitted_for_single_occurrence` | **ok** |

**Prompt-cited H1 anchors re-resolved exactly.** The round-6 hypothesis input cited the
recall path at `wiring.rs:1013-1031`, `RecalledEpisode` at `capabilities.rs:607-616`, and
the recall-count block at `signal.rs:455`. All three resolve verbatim at HEAD `941f40cc`:

- `wiring.rs:1013` `recall_episodic`; the provenance-**dropping** map is `1024-1029`
  (`RecalledEpisode { failure_signature, id, summary, score }` — **no `source_label`
  field is carried**). The write-back it recalls is tagged `OVERSEER_SOURCE_LABEL =
  "overseer"` (`wiring.rs:952`, stored `:1088`), so the Overseer re-ingests its own
  observations — the H1 defect, confirmed by source **and** the two green H1 tests.
- `capabilities.rs:607-616` `struct RecalledEpisode` has **no `source_label`** — the
  structural reason recall cannot distinguish self-authored episodes.
- `signal.rs:455` opens the recall-count block; `:458-459` tally per `failure_signature`,
  `:462-467` emit `RecurringSignature` once `occurrences >= RECURRING_SIGNATURE_THRESHOLD`
  (`= 2`, `:362`) — the emission behind "seen **2×**".

**Semantic anchors (§1–§3) spot-checked — all present, unchanged.** §1
`sensor.rs:204` `blocked_goals_from_board`, `:209` `blocked_goal_of`;
`no_progress_breaker.rs:58` `NO_PROGRESS_BREAKER_THRESHOLD = 3`, `:69`
`NO_PROGRESS_BLOCKED_PREFIX`; `completion_gate.rs:29` `pr_merged` ∧ `:31` `issue_closed`
∧ `:36` `deployed`. §2a `provider.rs:61` `env_flag("SIMARD_SKIP_GYM")`,
`sensor.rs:125-126` OR-fold, `signal.rs:399` `GymSkipped`, `mod.rs:1295`
`quality:gym_skipped`. §2b `sensor.rs:288` `detect_workstream_gaps`, **hard disjointness
guard `sensor.rs:300-302`** (`if matches!(g.status, GoalProgress::Blocked(_)) { continue; }`),
`mod.rs:1384` `workstream-gap`. §3 `signal.rs:351` `ENGINEER_SPAWN_THRESHOLD = 8`,
`capabilities.rs:81` `live_engineers`, `mod.rs:1283` `resource:engineer_spawn`. §7
`mod.rs:544` `observation_signature(problems)`, `:1081`/`:1085` re-prefix
`overseer-obs:`, `:297` `write_back_gate = WhisperGate::new(900, 5)`, `:1372`
`sanitize_recalled(signature)`.

**Live state re-confirmed (2026-07-07 ~12:50 UTC).** Unchanged from §7e/§7f:

- `rysweet/agent-kgpacks-rs`: **#16 CLOSED** 2026-07-06T20:16:25Z, **#18 CLOSED**
  10:33:04Z, **#21 CLOSED** 13:29:03Z, **#22 CLOSED** 12:07:33Z (four delivered → stale
  parks); **#17 OPEN** ("int8/PQ … gated on eval" — intentional gate). Timestamps match
  §1 to the second.
- `rysweet/Simard`: the duplicate-signature tail is **10 open** and identical to round-5
  (#2669, #2672, #2678, #2691, #2744, #2750, #2757, #2768, #2841, #2875) — **no new
  duplicate** filed in the intervening ~20 min. The loop remains live but produced no
  eleventh issue this pass.

Net: the four executable hypotheses **pass** (36/36 module-green), every prompt-cited and
semantic anchor **resolves at HEAD `941f40cc` with zero drift**, and the live board state
is **unchanged**. No finding changed; overall confidence remains **High**.

### 7i. Round-7 addendum — practical verification re-executed at HEAD `1190abb5`

Round 7 (this update) **re-executed the per-hypothesis practical tests** at HEAD
`1190abb5` (2026-07-07 ~12:57 UTC). HEAD advanced from round-6's `941f40cc` by one
docs-only commit (`1190abb5`, the §7h addendum), so **no source line drifted**; every
anchor resolves at its §7b/§7f/§7h line.

**H1 — self-amplifying recall loop (method: trace_code + executable test). CONFIRMED.**

- `cargo test --lib overseer::tests_memory_recall` → **36 passed, 0 failed**. The four
  hypothesis tests (`h1_confirm_self_recall_reemits_recurring_signature_from_own_writebacks`,
  `h1_refute_by_fix_provenance_filter_collapses_the_loop`,
  `h2_confirm_observation_signature_stacks_prefix_each_generation`,
  `h2_refute_by_fix_idempotent_signature_is_a_fixed_point`) and the write-back/dedup/count
  corroborators are all green.
- **No provenance filter on recall.** `recall_episodic` (`wiring.rs:1013`) maps each
  ranked episode to `RecalledEpisode { failure_signature, id, summary, score }`
  (`:1024-1029`) — the map **drops `source_label`**; `struct RecalledEpisode`
  (`capabilities.rs:607-616`) has **no `source_label` field**, so recall cannot exclude
  self-authored episodes. The write-back it re-ingests is tagged
  `OVERSEER_SOURCE_LABEL = "overseer"` (`wiring.rs:952`) — confirming the self-recall.
- **Re-wrap grows/nests the key.** `observation_signature` (`mod.rs:1081-1085`)
  re-prefixes `format!("overseer-obs:{}", …)` every generation.
- **Escapes WhisperGate dedup.** The `RecurringSignature` dedup key is
  `sanitize_recalled(signature)` (`mod.rs:1372`) — the entire recalled composite string —
  so a grown/nested key hashes to a distinct entry and is never suppressed.
- **The `2×` = recalled-episode count.** `signal.rs:455-470` tallies per
  `failure_signature` (`:458-459`) and emits `Signal::RecurringSignature` once
  `occurrences >= RECURRING_SIGNATURE_THRESHOLD` (`= 2`, `:362`) — not a retry counter.

**H2 — stale safeguard-parks for #16/#18/#21/#22 (method: verify_config). CONFIRMED.**

- `gh issue view` at HEAD: **#16 CLOSED** 2026-07-06T20:16:25Z, **#18 CLOSED**
  10:33:04Z, **#21 CLOSED** 13:29:03Z, **#22 CLOSED** 12:07:33Z — four delivered, matching
  §1 to the second.
- **Park creation is terminal, with no close-reconciliation.**
  `NO_PROGRESS_BREAKER_THRESHOLD = 3` (`no_progress_breaker.rs:58`) → done-gate runs
  **once** (`verify_stuck_goal`, `:179`; "the no-progress breaker ran the done-gate once …
  Blocked pending human review", `:211-214`) → sets the
  `NO_PROGRESS_BLOCKED_PREFIX`/`_SUFFIX` sentinel (`:69/:74`). A source scan of the module
  finds **no branch that clears a park when its backing issue later closes** — the gate
  runs once and the disposition maps a gate `Blocked` only to `Unresolved`/`Obsolete`
  (`:194`), never re-firing on a later close. So the four closed issues keep stale
  `goal:blocked` rows that `blocked_goals_from_board` (`sensor.rs:204`) re-emits every tick.

**H3 — #17 stale-premise dep-block (method: verify_config + live board). CONFIRMED.**

- **#17 OPEN** (unchanged; intentional int8/PQ eval-gated spike). Its live block is an
  engineer `record_blocker` on WS1 #16's eval baseline whose premise ("#16 still OPEN") is
  **stale** — #16 CLOSED 20:16:25Z — the same missing done-gate/block reconciliation
  defect as §1, on a non-safeguard block. #17 remains optional to ship.

**Loop still live — tail unchanged at 10.** `rysweet/Simard` duplicate-signature tail is
**10 open** (#2669, #2672, #2678, #2691, #2744, #2750, #2757, #2768, #2841, #2875; newest
@ 2026-07-07T11:31:36Z) — **no new duplicate** in the intervening ~90 min. Stewardship
escalation **#2707 OPEN**.

Net: all three hypotheses **re-confirmed** — H1 by 36/36 green tests + verbatim source
anchors, H2/H3 by live GitHub state + the terminal-park/no-reconciliation code path. Every
anchor resolves at HEAD `1190abb5` with **zero drift**; live state is **unchanged**. No
finding changed; overall confidence remains **High**.

### 7j. Round-7 primary-investigator deep dive — literal token decode, deterministic ordering, cross-goal contamination & whisper rule-out (HEAD `c868d6bd`)

This pass owns the two assigned focus areas — **(A/C) signature-token decode + emission-site
mapping** and **(D) the self-recall/write-back amplification loop across
`signal.rs`/`mod.rs`/`wiring.rs`/`whisper_ops.rs`** — and adds four facts not in §7f/§7i:
the *literal* token multiplicities of the observed key, a **proof that the token order is
deterministic**, a **seventh `goal:blocked:` contamination token** in the newest generation,
and the first analysis of **`whisper_ops.rs`**, which is **ruled out** as an amplifier.
Anchored at the repo's actual HEAD `c868d6bd`; §7h/§7i prose cites `941f40cc`/`1190abb5`, but
every commit since `941f40cc` is **docs-only**, so all source lines are identical — verified
by re-resolving each anchor below.

**A. Literal token decode (10 distinct tokens).** Splitting the observed composite on `|`
yields exactly ten distinct tokens; each maps to one authoritative emission (file:line @
`c868d6bd`):

| Token | Decodes to | Signal (`signal.rs`) | dedup_key (`mod.rs`) | Ultimate source |
|---|---|---|---|---|
| `overseer-obs:` (leads each generation) | one **self-recall generation** boundary | — | `observation_signature` `:1085` | self write-back re-wrap |
| `goal:blocked:advance-…-full-parity-f29bb15c` | parity umbrella parked | `GoalBlocked` `:441` | `goal:blocked:{id}` `:1349` | no-progress park `no_progress_breaker.rs:69` |
| `…issue-16-ws1-full-pack-cve-…` | ws1 (#16 **CLOSED**) | `GoalBlocked` `:441` | `:1349` | **stale** park |
| `…issue-17-ws2-int8-pq-embed-…` | ws2 (#17 OPEN, gated spike) | `GoalBlocked` `:441` | `:1349` | stale-premise dep-block |
| `…issue-18-ws3-versioned-rel-…` | ws3 (#18 **CLOSED**) | `GoalBlocked` `:441` | `:1349` | **stale** park |
| `…issue-21-ws6-resumable-pip-…` | ws6 (#21 **CLOSED**) | `GoalBlocked` `:441` | `:1349` | **stale** park |
| `…issue-22-ws7-sign-the-rele-…` | ws7 (#22 **CLOSED**) | `GoalBlocked` `:441` | `:1349` | **stale** park |
| `goal:blocked:fix-rustsec-2026-0204-in-amplihack-xpia-defende-…` | **unrelated** xpia-defender goal, `NOT_A_REPO` park | `GoalBlocked` `:441` | `:1349` | resolver park `error/display.rs:158` — **contamination (new)** |
| `quality:gym_skipped` | gym self-eval skipped (operator flag) | `GymSkipped` `:398-399` | `quality:gym_skipped` `:1295` | `env_flag("SIMARD_SKIP_GYM")` `provider.rs:61` |
| `resource:engineer_spawn` | ≥8 live engineers | `EngineerSpawnRate` `:393-396` | `resource:engineer_spawn` `:1283` | `live_engineers` `capabilities.rs:81` |
| `workstream-gap` | uncovered backlog (disjoint from blocked) | `WorkstreamGap` `:475` | `workstream-gap` `:1384` | `detect_workstream_gaps` `sensor.rs:288` |

The `2×` in the prompt summary is the recall **occurrences** count (`signal.rs:462-467`,
threshold `= 2` `:362`) rendered verbatim by the signal→problem summary
`format!("recurring signature seen {occurrences}× in cognitive memory ({signature})")`
(`mod.rs:1373-1375`) — the prompt text *is* this line's output. It is **not** a retry
counter and is orthogonal to the visible nesting depth.

**B. The token order is deterministic — a provable consequence of `observation_signature`.**
`observation_signature` sorts the dedup_keys (`keys.sort_unstable()` `mod.rs:1083`) then
prefixes `overseer-obs:` (`:1085`). Under byte ordering the token *classes* sort strictly:
`goal:blocked:*` (`g`) < the recalled `overseer-obs:*` composite (`o`) < `quality:*` (`q`) <
`resource:*` (`r`) < `workstream-gap` (`w`) — verified empirically (`LC_ALL=C sort`). This
exactly predicts the observed left-to-right layout of **every** generation: all
`goal:blocked:` tokens first (umbrella `advance-…` sorts before `fix-…`, and `fix-agent-…`
before `fix-rustsec-…`), then the nested prior composite, then the ambient
`gym_skipped`/`resource`/`workstream-gap` tail. The layout is not incidental — it *is* the
sort.

**C. Nesting decode + generational growth (new).** Each `overseer-obs:` prefix marks one
recalled generation folded in whole (the `RecurringSignature` dedup_key is the *entire* prior
composite, `mod.rs:1372`). In the observed key the **inner (older) generations carry only
`goal:blocked:` tokens** (no ambient tail) while a **single ambient tail sits at the
outermost** — consistent with §1's issue timeline (parity-only #2669 → +issue-17 #2744 →
+issue-16 #2768). Decisively, the **outermost generation carries a seventh `goal:blocked:`
token — `fix-rustsec-2026-0204-in-amplihack-xpia-defende-…` — that the inner generations
lack.** That token is an **unrelated** goal parking with a `NOT_A_REPO` resolver error
(`error/display.rs:158`), not a kgpacks park. Its appearance in only the newest generation is
direct, disprovable evidence that (i) the signature **grows per generation** (a new board row
appeared between recalls) and (ii) the composite **cross-contaminates across unrelated goals**
— *any* goal `Blocked` on the board at recall time is swept into the same self-amplifying key.

**D. The closed amplification loop across the four focus files (one cycle).**
1. `mod.rs:544` `observation_signature(problems)` → `:1081-1085` build
   `overseer-obs:{sorted, dedup'd dedup_keys}`.
2. `mod.rs:552` `record_observation` → `wiring.rs:1076-1090`
   `store_episode(content⧺"[sig:…]", OVERSEER_SOURCE_LABEL="overseer", {signature})`.
   `WhisperGate::new(900,5)` (`mod.rs:297`, peeked `:546`) dedups only an **identical**
   signature within the 900 s window.
3. next tick: `mod.rs:496-513` `recall_pass` → `wiring.rs:1013-1031` `recall_episodic` →
   `RecalledEpisode { failure_signature = parse_failure_signature(content), … }` —
   **`source_label` is dropped** (`capabilities.rs:607-616` has no such field). ← the H1 gap.
4. `signal.rs:455-469` `signals_from` tallies episodes by `failure_signature`; ≥2 →
   `Signal::RecurringSignature { signature, occurrences }` (`:362`, `:464`).
5. `mod.rs:1366-1376` signal→problem: `dedup_key = sanitize_recalled(signature)` = the
   **whole** prior composite; summary = the prompt's verbatim line.
6. GOTO 1: that giant dedup_key folds back as **one** element, re-wrapped with a fresh
   `overseer-obs:` → strictly-longer signature → distinct `WhisperGate` key → never deduped.
   A closed, monotonically-growing loop. Test-proven: `h1_confirm…` / `h2_confirm…` green
   (36/36).

**E. `whisper_ops.rs` ruled OUT as an amplifier (new negative finding).** The whisper channel
— the fourth focus file — does **not** feed the loop, on three independent grounds:
- It writes **`MeetingHandoff`s** via `write_meeting_handoff` (`whisper_ops.rs:94-105`) and
  **never calls `store_episode`** — so `recall_episodic` (which reads *episodic* memory) can
  never surface a whisper. The amplifier is the episodic write-back path exclusively.
- Whisper handoffs carry **empty `decisions`/`action_items`** (`whisper_ops.rs:119-121`), so
  `curate` can never promote one into a goal/backlog → a whisper can **never create a
  `goal:blocked` token**.
- They are `WHISPER_THEME`-tagged (`:28,135`) and drained by `drain_overseer_whispers`
  (`curate.rs:384-385`), so Observe ignores the Overseer's own whispers.
Conclusion: `whisper_ops.rs` is a **non-amplifying sibling** steering channel; it neither
manufactures `goal:blocked` inputs nor re-enters recall. This removes "whispers feed the
signature" as a candidate mechanism.

**F. Two independent cut-points (both already test-encoded → P1).** (i) Carry `source_label`
onto `RecalledEpisode` and drop `== "overseer"` before the tally (`h1_refute_by_fix…` green);
(ii) make `observation_signature` idempotent — don't re-wrap an already-`overseer-obs:`-
prefixed recalled key (`h2_refute_by_fix…` green). Either severs the closed loop in D; either
alone halts the growth in C.

**Verification footer.** HEAD `c868d6bd`; `cargo test --lib overseer::tests_memory_recall` →
**36 passed, 0 failed**; every anchor above re-resolved verbatim; token sort-order confirmed
empirically (`g < o < q < r < w`). No prior finding changed; the decode and loop trace are
additive. Confidence **High**.

### 7k. Round-7 consolidation — parallel dives unified (HEAD `ca20e29b`)

This pass unifies the three deep dives accumulated since the last consolidation (§7g,
round-5) — **§7h** (round-6 practical re-verification: full `tests_memory_recall` module +
all §1–§3 semantic anchors at `941f40cc`), **§7i** (round-7 per-hypothesis practical
verification at `1190abb5`), and **§7j** (round-7 primary-investigator deep dive at
`c868d6bd`: literal token decode, deterministic-sort proof, cross-goal contamination,
whisper rule-out) — into one conclusion and folds their net-new facts into the canonical
findings (§1/§5/§6). HEAD has since advanced to `ca20e29b`; **every commit since `941f40cc`
is docs-only**, so no source line drifted and every §7b/§3b/§7f/§7j anchor still resolves.

**Independent re-verification (this consolidation, 2026-07-07 ~13:17 UTC)** — re-run by the
consolidator at HEAD `ca20e29b`, not inherited:

- `cargo test --lib overseer::tests_memory_recall` → **36 passed, 0 failed**; the four
  hypothesis tests re-run in isolation (`…::h`) → **4 passed, 0 failed** (H1/H2 CONFIRM +
  REFUTE-by-fix). The structural root cause (§4/§7a) remains proven green.
- `rysweet/agent-kgpacks-rs`: **#16 CLOSED 2026-07-06T20:16:25Z**, **#18 CLOSED
  10:33:04Z**, **#21 CLOSED 13:29:03Z**, **#22 CLOSED 12:07:33Z**, **#17 OPEN** — matching
  §1/§7f/§7h to the second. §1 stale-parks and the §7f/§7j #17 stale-premise dep-block
  hold intact.
- `rysweet/Simard` duplicate tail: **exactly 10 open** (#2669, #2672, #2678, #2691, #2744,
  #2750, #2757, #2768, #2841, #2875; newest @ 2026-07-07T11:31:36Z), **no 11th** at
  consolidation time. **#2707** stewardship escalation **OPEN**.

**Zero contradictions across the three dives.** §7h, §7i and §7j agree on every shared
claim — test results (36/36; 4/4 hypothesis), the H1 recall-path anchors
(`wiring.rs:1013-1031` no-`source_label` map; `capabilities.rs:607-616` no field;
`signal.rs:455-470` count/emit; threshold `= 2` `signal.rs:362`), the terminal-park /
no-close-reconciliation code path (`no_progress_breaker.rs:58/69/74`, gate-disposition
`:194`), and the live #16/#18/#21/#22-CLOSED + #17-OPEN board. They are strictly additive:
§7h/§7i re-prove the mechanism through the tests and re-resolve the anchors; §7j decodes
the literal key and traces the closed loop across the four focus files. This consolidation
overturns **no** finding and makes **no** remediation-weighting change (P1–P5 stand exactly
as consolidated in §7g).

**Net-new facts folded into the canonical findings:**

1. **Literal token decode + deterministic sort order (§7j-A/B) confirmed.** The composite is
   ten distinct tokens, each mapped to one authoritative emission; their left-to-right layout
   is a *provable consequence* of `observation_signature`'s `keys.sort_unstable()`
   (`mod.rs:1083`) under byte order (`g < o < q < r < w`). The `2×` is the recall
   `occurrences` count rendered verbatim by `mod.rs:1373-1375`, not a retry counter (§1
   empirical + §4 reinforced; no change to the verdict).
2. **Seventh cross-goal contamination token (§7j-C).** The newest generation carries a
   `goal:blocked:fix-rustsec-2026-0204-in-amplihack-xpia-defende-…` token — an **unrelated**
   goal parking with a `NOT_A_REPO` resolver error (`error/display.rs:158`), absent from the
   inner generations. This is disprovable evidence that the signature **grows per generation**
   and **cross-contaminates across unrelated goals**: any goal `Blocked` on the board at
   recall time is swept into the same self-amplifying key. Reinforces §1's stale-input thesis
   and §7f's "routing intact" contrast (kgpacks parks are stale sentinels, not resolver
   errors; the xpia-defender goal is the one true resolver failure).
3. **`whisper_ops.rs` ruled OUT as an amplifier (§7j-E), new negative finding.** The whisper
   channel writes `MeetingHandoff`s (`whisper_ops.rs:94-105`), never `store_episode`, carries
   empty `decisions`/`action_items` (`:119-121`), and is drained by `drain_overseer_whispers`
   (`curate.rs:384-385`) — so it can neither manufacture a `goal:blocked` token nor re-enter
   `recall_episodic`. The episodic write-back path is the **sole** amplifier; "whispers feed
   the signature" is eliminated as a candidate mechanism.
4. **Ambient lead-token drift across the tail (new, this consolidation).** The four **oldest**
   duplicates (#2669/#2672/#2678/#2691, all 2026-07-06) lead with
   `overseer-obs:anomaly:distill parse-fail rate 100%|goal:blocked:advance-…`, whereas the six
   **newer** ones (#2744→#2875) lead with `overseer-obs:goal:blocked:advance-…`. The
   `anomaly:distill parse-fail` co-signal cleared, so its token dropped out of later
   generations. This independently **corroborates §7j-B**: while that anomaly was a live
   problem it byte-sorted ahead of `goal:blocked:` (`a < g`) and therefore led the key —
   exactly the deterministic ordering §7j predicts — and it reconfirms that the composite's
   ambient inputs are **standing but time-varying**, driving the per-generation growth in
   §7j-C rather than a fixed payload.

**Consolidated verdict (rounds 1–7).** Unchanged and further strengthened — **High**
confidence, all six findings source- or live-state-grounded, no remaining Medium diagnostic
item. One self-amplifying loop — unfiltered self-recall (`recall_episodic` drops
`source_label`) + unbounded re-wrap (`observation_signature` re-prefixes `overseer-obs:`),
**test-proven** (H1/H2 green, 36/36 module) and now **fully decoded and loop-traced** (§7j) —
fed by standing, time-varying inputs: four **stale safeguard-parks** (#16/#18/#21/#22), one
**stale-premise dep-block** (#17), an ambient **`gym_skipped`** flag, and a disjoint
**`workstream-gap`**; **`engineer_spawn` is a passenger**, not a driver (round-4 refutation);
**`whisper_ops.rs` is a non-amplifying sibling** (round-7 rule-out). The loop is **live** (10
open duplicates, #2875 the newest; a seventh cross-goal token now nesting) and symptom-level
unblocks **do not stick** (#2707: umbrella is a standing goal not tagged perpetual). Action
set unchanged: **P1** (provenance filter) severs the amplifier; **P2** (done-gate/park
reconciliation + perpetual tagging) removes the standing stale inputs and is required for the
fix to *stick*; **P3** clears the 10-issue backlog only when paired with P2; **P4**
conditional (#17 disposition + `gym_skipped` down-rank); **P5** optional throughput nicety.
No new remediation is introduced.

### 7l. Round-8 primary-investigator deep dive — HEAD-anchored token→signal-variant→provenance table, #17 temporal-staleness proof, and the xpia-defender repo-exists refinement (HEAD `ca20e29b`)

This pass owns the assigned focus — the **signature-token → signal-variant → `file:line`
provenance map** across `src/overseer/`, and a **fresh per-`goal:blocked` classification
against live GitHub/board state**. It is anchored at the repo's current HEAD `ca20e29b`,
with **every source line independently re-resolved** (not inherited from §7f/§7j's older
`db02dd7b`/`c868d6bd` anchors). Only one non-Rust file changed since `941f40cc`
(`.github/hooks/amplihack-hooks.json`, 6 ± lines), so every Rust anchor is byte-identical —
verified by re-resolving each below. `cargo test --lib overseer::tests_memory_recall` →
**36 passed, 0 failed** at this HEAD.

**A. Authoritative token → signal-variant → provenance table (re-resolved at `ca20e29b`,
push-line precision).** Each token maps to exactly one `Signal` variant (all emitted inside
`signals_from`, `signal.rs:366`) and one `dedup_key` (assigned in `classify_signal`,
`mod.rs:1251`). Where §7f/§7j cited the match-arm / gate *start*, this table cites the exact
`out.push(…)` / `format!(…)` **emission line**:

| Token | Signal variant (`signal.rs`) | Emission line | dedup_key (`mod.rs`) | Root input (provenance) |
|---|---|---|---|---|
| `overseer-obs:` prefix | — (assembler, not a signal) | — | `observation_signature` fmt `:1085`, called `:544`→`record_observation` `:552` | self write-back re-wrap (§4) |
| `goal:blocked:{id}` (×7) | `GoalBlocked` | push `:441` | `format!("goal:blocked:{goal_id}")` `:1349` | `blocked_goals_from_board` `sensor.rs:204` → `blocked_goal_of` `:209` |
| `quality:gym_skipped` | `GymSkipped` | push `:399` (gate `:398`) | `"quality:gym_skipped"` `:1295` | `gym_skipped` OR-fold `sensor.rs:125-126` ← `SIMARD_SKIP_GYM` |
| `resource:engineer_spawn` | `EngineerSpawnRate` | push `:396` (gate `:393-394`) | `"resource:engineer_spawn"` `:1283` | `live_engineers` `capabilities.rs:81` |
| `workstream-gap` | `WorkstreamGap` | push `:476` | `"workstream-gap"` `:1384` | `detect_workstream_gaps` `sensor.rs:288`; blocked goals excluded `:300` |
| recalled composite (the `2×` driver) | `RecurringSignature` | push `:464` (tally `:456-460`, threshold `:463` = `2` `:362`) | `sanitize_recalled(signature)` `:1372` — the whole prior key | unfiltered self-recall `recall_episodic` `wiring.rs:1013`; `failure_signature` map `:1025` **drops provenance**; `RecalledEpisode` `capabilities.rs:607-615` has **no `source_label`** field; own write-back tagged `OVERSEER_SOURCE_LABEL` `wiring.rs:952`, stored `:1088` |

Refinement vs. §7f/§7j: the emission anchors are `signal.rs:396/399/441/464/476` (the
`out.push`), one-to-three lines below the arm/gate starts those tables cite
(`:393-396`,`:398`,`:441`,`:463-467`,`:475`). Substance is unchanged; this is the precise
byte-anchor at HEAD. `observer.rs` still emits **no** token (Decide-phase consumer only), as
§7f noted.

**B. Per-`goal:blocked` classification against live GitHub/board (re-read 2026-07-07
~13:1x UTC).** Six kgpacks tokens + one contamination token:

| `goal:blocked` token | Issue | Live state | Classification |
|---|---|---|---|
| `advance-…-full-parity-f29bb15c` | umbrella (no single issue) | standing goal, safeguard-parked | **stale safeguard-park** — un-satisfiable terminal done-gate, not tagged perpetual |
| `…issue-16-ws1-full-pack-cve` | #16 | **CLOSED** 07-06T20:16:25Z | **stale park** |
| `…issue-17-ws2-int8-pq-embed` | #17 | **OPEN**, `updatedAt` 07-02T23:22:49Z | **stale-premise dep-block** (see C) |
| `…issue-18-ws3-versioned-rel` | #18 | **CLOSED** 07-06T10:33:04Z | **stale park** |
| `…issue-21-ws6-resumable-pip` | #21 | **CLOSED** 07-06T13:29:03Z | **stale park** |
| `…issue-22-ws7-sign-the-rele` | #22 | **CLOSED** 07-06T12:07:33Z | **stale park** |
| `fix-rustsec-2026-0204-in-amplihack-xpia-defende` | (unrelated goal) | local-worktree park | **cross-goal contamination** (see D) |

**4 of the 6 kgpacks blockers reference already-CLOSED issues**; only #17 is open — the
board keeps five stale `goal:blocked` rows that `blocked_goals_from_board` (`sensor.rs:204`)
re-emits every tick.

**C. Temporal-staleness proof for #17 (new — sharper than §7i's prose reading).** #17's
engineer `record_blocker` asserts "#16 still OPEN … no landed baseline." But #17's own
`updatedAt` = **2026-07-02T23:22:49Z** while #16's `closedAt` = **2026-07-06T20:16:25Z** —
so the block reason was authored **≈3.8 days BEFORE #16 closed** and has had **no event
since** (`updatedAt_17 < closedAt_16`, and no post-close edit). The staleness is thus not
inferred from the sentence — it is **provable from the timestamps**. This is the same
missing done-gate/block-reconciliation defect as §1, demonstrated on a **non-safeguard**
block by timestamp. (#17 remains legitimately deferrable as the flag-gated spike; the point
is the block *premise* is unreconciled.)

**D. The xpia-defender contamination token is a LOCAL-worktree park, not a missing repo
(refines §7j-C/§7f).** Prior rounds framed `fix-rustsec-2026-0204-in-amplihack-xpia-defende`
as a `NOT_A_REPO` resolver error (`error/display.rs:158`) and contrasted it with intact
kgpacks routing. Refinement: **`rysweet/amplihack-xpia-defender` exists as a live public
GitHub repo** (`gh repo view` → `{"name":"amplihack-xpia-defender","isPrivate":false}`). So
the park is **not** a missing-GitHub-repo; the message reads `NOT_A_REPO:
'…/amplihack-xpia-defender' is not inside a valid git worktree` — a **local checkout** that
was never hydrated into a git worktree. The contamination *mechanism* (§7j-C: any
board-`Blocked` goal is swept into the self-amplifying key) is unchanged; the token's **own**
root cause is a local-worktree hydration gap, orthogonal to (and not evidence against) the
kgpacks recurrence.

**Verification footer.** HEAD `ca20e29b`; `cargo test --lib overseer::tests_memory_recall`
→ **36 passed, 0 failed**; every table anchor re-resolved verbatim at HEAD; live GitHub
re-read (#16/#18/#21/#22 CLOSED, #17 OPEN; 10-issue Simard tail unchanged — #2669/#2672/
#2678/#2691/#2744/#2750/#2757/#2768/#2841/#2875, #2707 open); `rysweet/amplihack-xpia-defender`
confirmed to exist. No prior finding overturned — §7l is strictly additive: HEAD-precise
push-line anchors, a timestamp proof for #17, and the local-vs-remote refinement of the
contamination token. Confidence **High**.

### 7m. Round-9 addendum — per-hypothesis practical tests re-executed at HEAD `0196c4f6`

Round 9 (this update) **re-executed the practical verification test for each hypothesis** at
HEAD `0196c4f6` (2026-07-07 ~13:24 UTC). HEAD advanced from round-8's `ca20e29b` by one
docs-only commit (`0196c4f6`, the §7k consolidation), so **no source line drifted**; every
anchor re-resolves verbatim at its §7b/§7f/§7j/§7l line.

**H1 — self-amplifying self-recall loop (method: executable test + trace_code). CONFIRMED.**

- `cargo test --lib overseer::tests_memory_recall` → **36 passed, 0 failed**. Re-running the
  four hypothesis tests in isolation (`…::tests_memory_recall::h`) → **4 passed, 0 failed**:
  `h1_confirm_self_recall_reemits_recurring_signature_from_own_writebacks`,
  `h1_refute_by_fix_provenance_filter_collapses_the_loop`,
  `h2_confirm_observation_signature_stacks_prefix_each_generation`,
  `h2_refute_by_fix_idempotent_signature_is_a_fixed_point` — CONFIRM + REFUTE-by-fix all green.
- **Recall carries no provenance filter (re-resolved verbatim).** `recall_episodic`
  (`wiring.rs:1013`) maps each ranked episode via `.map(|e| RecalledEpisode { failure_signature,
  id, summary, score })` (`:1024-1029`) — the map **drops `source_label`**; `struct
  RecalledEpisode` (`capabilities.rs:607-616`) has fields `id`/`summary`/`failure_signature`/
  `score` and **no `source_label`**, so recall cannot exclude self-authored episodes. The
  write-back it re-ingests is tagged `OVERSEER_SOURCE_LABEL = "overseer"` (`wiring.rs:952`).
- **Re-wrap + escape-dedup + count all re-resolve.** `observation_signature` re-prefixes
  `format!("overseer-obs:{}", keys.join("|"))` after `keys.sort_unstable()`/`keys.dedup()`
  (`mod.rs:1081-1085`); the `RecurringSignature` dedup_key = `sanitize_recalled(signature)` — the
  whole prior composite — and the summary is the prompt's verbatim line
  `"recurring signature seen {occurrences}× in cognitive memory ({signature})"`
  (`mod.rs:1372-1375`); the tally emits `Signal::RecurringSignature` once `occurrences >=
  RECURRING_SIGNATURE_THRESHOLD` (`= 2`, `signal.rs:362`; loop `:455-470`) — the `2×` is the
  recalled-episode count, not a retry counter.

**H2 — stale safeguard-parks for #16/#18/#21/#22 (method: verify_config + live GitHub). CONFIRMED.**

- `gh issue view` at HEAD: **#16 CLOSED** 2026-07-06T20:16:25Z, **#18 CLOSED** 10:33:04Z,
  **#21 CLOSED** 13:29:03Z, **#22 CLOSED** 12:07:33Z — four delivered, matching §1/§7l to the
  second.
- **Terminal-park / no-close-reconciliation code path re-resolves.**
  `NO_PROGRESS_BREAKER_THRESHOLD = 3` (`no_progress_breaker.rs:58`) → the done-gate runs once and
  sets the `NO_PROGRESS_BLOCKED_PREFIX`/`_SUFFIX` sentinel (`:69/:74`); no branch clears a park
  when its backing issue later closes, so `blocked_goals_from_board` (`sensor.rs:204`) →
  `blocked_goal_of` (`:209`) keeps re-emitting the four stale `goal:blocked` rows every tick.

**H3 — #17 stale-premise dep-block (method: verify_config + live board + timestamp proof). CONFIRMED.**

- **#17 OPEN**, `updatedAt` **2026-07-02T23:22:49Z**. Its block premise ("#16 still OPEN") is
  provably stale: `updatedAt_17` (07-02T23:22:49Z) **precedes** `closedAt_16`
  (07-06T20:16:25Z) by ≈3.8 days with no event since — the §7l timestamp proof re-confirmed
  from the live values. Same missing done-gate/block-reconciliation defect as §1; #17 remains
  legitimately deferrable as the flag-gated spike.

**Loop still live — tail unchanged at 10.** `rysweet/Simard` duplicate-signature tail is
**exactly 10 open** (#2669, #2672, #2678, #2691, #2744, #2750, #2757, #2768, #2841, #2875;
newest @ 2026-07-07T11:31:36Z) — **no 11th** at round-9 time. Stewardship escalation **#2707
OPEN**.

Net: all three hypotheses **re-confirmed** — H1 by 36/36 module + 4/4 hypothesis tests green
and verbatim source anchors, H2 by live CLOSED states + the terminal-park/no-reconciliation
path, H3 by the live-timestamp staleness proof. Every anchor resolves at HEAD `0196c4f6` with
**zero drift**; live state is **unchanged**. No finding changed; overall confidence remains
**High**.

### 7n. Round-10 consolidation — parallel dives unified (HEAD `85245e87`)

This pass unifies the two deep dives accumulated since the last consolidation (§7k,
round-7) — **§7l** (round-8 primary-investigator deep dive at `ca20e29b`: HEAD-anchored
token→signal-variant→provenance table with push-line precision, the #17 temporal-staleness
timestamp proof, and the xpia-defender local-worktree refinement) and **§7m** (round-9
per-hypothesis practical re-verification at `0196c4f6`: H1 via the full module + four
hypothesis tests, H2 via live CLOSED states + the terminal-park path, H3 via the
live-timestamp staleness proof) — into one conclusion and folds their net-new facts into the
canonical findings (§1/§5/§6). HEAD has since advanced to `85245e87`; **every commit since
`ca20e29b` is docs-only** (verified: `git diff --name-only ca20e29b..HEAD` returns only this
file), so **no source line drifted** and every §7b/§3b/§7f/§7j/§7l anchor still resolves
verbatim.

**Independent re-verification (this consolidation, 2026-07-07 ~13:39 UTC)** — re-run by the
consolidator at HEAD `85245e87`, not inherited:

- `cargo test --lib overseer::tests_memory_recall` → **36 passed, 0 failed**; the four
  hypothesis tests re-run in isolation (`…::tests_memory_recall::h`) → **4 passed, 0 failed**
  (H1/H2 CONFIRM + REFUTE-by-fix). The structural root cause (§4/§7a) remains proven green.
- `rysweet/agent-kgpacks-rs`: **#16 CLOSED 2026-07-06T20:16:25Z**, **#18 CLOSED 10:33:04Z**,
  **#21 CLOSED 13:29:03Z**, **#22 CLOSED 12:07:33Z**, **#17 OPEN** (`updatedAt`
  2026-07-02T23:22:49Z) — matching §1/§7l/§7m to the second. **4 of the 6 kgpacks blockers
  reference already-CLOSED issues.**
- `rysweet/Simard` duplicate tail: **exactly 10 open** (#2669, #2672, #2678, #2691, #2744,
  #2750, #2757, #2768, #2841, #2875; newest @ 2026-07-07T11:31:36Z), **no 11th** at
  consolidation time. **#2707** stewardship escalation (`[stewardship] recurring_goal_reblock
  in simard::overseer`) **OPEN**.
- `rysweet/amplihack-xpia-defender` confirmed to **exist as a live public GitHub repo**
  (`gh repo view` → `{"name":"amplihack-xpia-defender","isPrivate":false}`), re-confirming the
  §7l-D local-worktree refinement.

**Zero contradictions across the two dives.** §7l and §7m agree on every shared claim — test
results (36/36 module; 4/4 hypothesis), the H1 recall-path anchors (`wiring.rs:952/1013-1031`
no-`source_label` map; `capabilities.rs:607-616` no field; `signal.rs:362/455-470`
count/emit/threshold `= 2`; `mod.rs:1081-1085/1372-1375` re-wrap/summary), the
terminal-park / no-close-reconciliation path (`no_progress_breaker.rs:58/69/74`,
`sensor.rs:204/209`), and the live #16/#18/#21/#22-CLOSED + #17-OPEN board with the 10-issue
Simard tail. They are strictly additive: §7l re-resolves the full token→variant→provenance map
at HEAD with push-line precision and adds two new proofs (the #17 timestamp proof, the xpia
local-worktree refinement); §7m re-executes the practical per-hypothesis tests and re-confirms
all three at the same HEAD-line anchors. This consolidation overturns **no** finding and makes
**no** remediation-weighting change (P1–P5 stand exactly as consolidated in §7g/§7k).

**Net-new facts folded into the canonical findings:**

1. **HEAD-precise emission anchors (§7l-A) adopted as canonical.** The
   token→signal-variant→provenance table now cites the exact `out.push(…)` / `format!(…)`
   **emission** lines (`signal.rs:396/399/441/464/476`; `mod.rs:1085/1283/1295/1349/1372/1384`;
   `wiring.rs:952/1013/1025/1088`; `capabilities.rs:81/607-615`;
   `sensor.rs:125-126/204/288/300`) rather than the arm/gate *starts* cited by §7f/§7j —
   substance unchanged, byte-anchor sharpened. Every one re-resolves verbatim at HEAD
   `85245e87` (zero source drift since `ca20e29b`).
2. **#17 staleness upgraded prose → timestamp proof (§7l-C, §7m-H3).** The block premise
   ("#16 still OPEN") is provably stale from the live timestamps alone: `updatedAt_17`
   (2026-07-02T23:22:49Z) **precedes** `closedAt_16` (2026-07-06T20:16:25Z) by ≈3.8 days with
   **no event since** — the same missing done-gate / block-reconciliation defect as §1, now
   demonstrated on a **non-safeguard** block by timestamp rather than by reading the sentence.
   (#17 remains legitimately deferrable as the flag-gated spike; only its block *premise* is
   unreconciled.)
3. **xpia-defender contamination token reclassified missing-repo → local-worktree park
   (§7l-D).** `rysweet/amplihack-xpia-defender` is a live public GitHub repo (re-confirmed this
   pass), so the `NOT_A_REPO` park (`error/display.rs:158`) is a **local checkout never
   hydrated into a git worktree**, not a missing remote. The contamination *mechanism* (§7j-C:
   any board-`Blocked` goal is swept into the self-amplifying key) is unchanged; the token's
   **own** root cause is a local-worktree hydration gap, orthogonal to the kgpacks recurrence.
4. **Live state stable across rounds 8→10 (new corroboration).** Four consecutive independent
   reads (§7l, §7m, and this consolidation) return the **identical** board — #16/#18/#21/#22
   CLOSED, #17 OPEN, the 10-issue Simard tail (newest #2875 @ 2026-07-07T11:31:36Z), #2707
   open. The loop is **live** but the tail has **not grown since #2875**, and symptom-level
   unblocks still **do not stick** (§7g-4/§7k: the umbrella goal is a standing goal not tagged
   perpetual, so it re-parks every breaker cycle).

**Consolidated verdict (rounds 1–10).** Unchanged and further strengthened — **High**
confidence, all six findings source- or live-state-grounded, no remaining Medium diagnostic
item. One self-amplifying loop — unfiltered self-recall (`recall_episodic` drops
`source_label`) + unbounded re-wrap (`observation_signature` re-prefixes `overseer-obs:`),
**test-proven** (H1/H2 green, 36/36 module; 4/4 hypothesis), **fully decoded and loop-traced**
(§7j), and **HEAD-line-anchored** (§7l) — fed by standing, time-varying inputs: four **stale
safeguard-parks** (#16/#18/#21/#22), one **stale-premise dep-block** (#17, timestamp-proven),
an ambient **`gym_skipped`** flag, and a disjoint **`workstream-gap`**; **`engineer_spawn` is a
passenger**, not a driver (round-4 refutation); **`whisper_ops.rs` is a non-amplifying
sibling** (round-7 rule-out). The loop is **live** (10 open duplicates, #2875 the newest; a
seventh cross-goal xpia-defender token nesting as a local-worktree park) and symptom-level
unblocks **do not stick** (#2707 open). Action set unchanged: **P1** (provenance filter) severs
the amplifier; **P2** (done-gate/park reconciliation + perpetual tagging) removes the standing
stale inputs and is required for the fix to *stick*; **P3** clears the 10-issue backlog only
when paired with P2; **P4** conditional (#17 disposition + `gym_skipped` down-rank); **P5**
optional throughput nicety. No new remediation is introduced.

---

## 8. Provenance

Investigation-only follow-up (investigation-workflow, rounds 1–10). No production
behavior was changed by this document. Round-1 established the structural cause
([`overseer-memory-recall-api`](./overseer-memory-recall-api.md)); round-2 added the
semantic diagnosis and the executable H1/H2 tests; round-3 consolidated the parallel
deep dives, reconciled all line anchors to HEAD `0180b75c`, and executed the H1/H2
CONFIRM/REFUTE tests (4 passed, 0 failed); **round-4 re-anchored the
AIMD `engineer_spawn` path to HEAD `20fb7539` and refuted the §3 amplifier/contributor
mechanism at the code level, upgrading §3 to High and making P5 optional (§3b, §7d);
round-5 (this update) re-executed the full `tests_memory_recall` module (36 passed,
0 failed) and re-verified every source anchor and live GitHub/board claim at HEAD
`db02dd7b` — the diagnosis holds unchanged (§7e); the primary-investigator extension
(§7f) added the explicit token→emission map, a direct `simard goal list` read of the
six block reasons, the stewardship escalation #2707, and the tenth duplicate #2875;
round-5 consolidation (§7g) unified the two parallel dives, independently re-verified
H1/H2 (4 passed, 0 failed) + the 10-issue live tail at HEAD `941f40cc`, and folded the
net-new facts into §1/§5/§6; round-6 (§7h) re-executed the full `tests_memory_recall`
module (36 passed, 0 failed), re-resolved the prompt-cited H1 anchors
(`wiring.rs:1013-1031`, `capabilities.rs:607-616`, `signal.rs:455`) and all §1–§3 semantic
anchors at HEAD `941f40cc` with zero drift, and re-confirmed the unchanged live board
(kgpacks-rs #16/#18/#21/#22 CLOSED, #17 OPEN; 10-issue Simard tail); round-7 (§7i)
re-executed the per-hypothesis practical tests at HEAD `1190abb5` — H1 via
`tests_memory_recall` (36 passed, 0 failed) + verbatim source anchors, and H2/H3 via live
GitHub state (kgpacks-rs #16/#18/#21/#22 CLOSED, #17 OPEN) plus the
terminal-park/no-reconciliation code path — re-confirming all three hypotheses with zero
drift and an unchanged 10-issue Simard tail; a round-7 primary-investigator deep dive (§7j,
HEAD `c868d6bd`) added the literal 10-token decode + deterministic sort-ordering proof
(`g<o<q<r<w`), identified a seventh cross-goal contamination token
(`fix-rustsec-…-amplihack-xpia-defende`, `NOT_A_REPO` park), traced the closed amplification
loop across `signal.rs`/`mod.rs`/`wiring.rs`, and ruled `whisper_ops.rs` OUT as a
non-amplifying sibling channel (36/36 tests green); round-7 consolidation (§7k, HEAD
`ca20e29b`) unified the round-6/round-7 dives (§7h/§7i/§7j), independently re-verified the
full module (36 passed, 0 failed) + the four hypothesis tests in isolation (4 passed,
0 failed), re-confirmed kgpacks-rs #16/#18/#21/#22 CLOSED + #17 OPEN and the 10-issue Simard
tail (#2707 open), folded the §7j token decode / deterministic-sort / cross-goal
contamination / whisper rule-out into §1/§4, and added the ambient lead-token drift across
the tail (oldest four lead with `anomaly:distill parse-fail`, newer six with `goal:blocked:`)
as fresh corroboration of the deterministic-sort and generational-growth findings — no
finding overturned, no remediation-weighting change, confidence remains High; round-8
primary-investigator deep dive (§7l, HEAD `ca20e29b`) re-resolved the full
token→signal-variant→provenance map at the current HEAD with push-line precision
(`signal.rs:396/399/441/464/476` emission lines; `mod.rs:1085/1283/1295/1349/1372/1384`;
`wiring.rs:952/1013/1025/1088`; `capabilities.rs:81/607-615`; `sensor.rs:125-126/204/288/300`),
re-ran the module (36 passed, 0 failed) at HEAD, re-read the live board (kgpacks-rs
#16/#18/#21/#22 CLOSED, #17 OPEN — **4 of 6 blockers reference already-closed issues**),
added a **timestamp proof** that #17's block premise is stale (`updatedAt` 07-02T23:22:49Z <
#16 `closedAt` 07-06T20:16:25Z, no event since), and **refined** the xpia-defender
contamination token from "missing repo" to a **local-worktree park** (`rysweet/amplihack-xpia-defender`
verified to exist as a public GitHub repo; the `NOT_A_REPO` park is a local checkout not
hydrated into a git worktree) — additive only, no finding overturned, confidence remains High.**
Round-9 (§7m, HEAD `0196c4f6`) re-executed the per-hypothesis practical tests — H1 via
`tests_memory_recall` (36 passed, 0 failed) plus the four hypothesis tests re-run in isolation
(4 passed, 0 failed) and verbatim re-resolution of the H1 recall-path anchors
(`wiring.rs:952/1013-1031`, `capabilities.rs:607-616`, `signal.rs:362/455-470`,
`mod.rs:1081-1085/1372-1375`); H2 via live kgpacks-rs #16/#18/#21/#22 CLOSED + the
terminal-park/no-reconciliation path (`no_progress_breaker.rs:58/69/74`, `sensor.rs:204/209`);
and H3 via #17 OPEN with a live-timestamp staleness proof (`updatedAt` 07-02T23:22:49Z < #16
`closedAt` 07-06T20:16:25Z) — all three re-confirmed, 10-issue Simard tail unchanged
(#2707 open), zero drift, confidence remains High.
Round-10 consolidation (§7n, HEAD `85245e87`) unified the round-8/round-9 dives (§7l/§7m),
independently re-verified the full module (36 passed, 0 failed) + the four hypothesis tests in
isolation (4 passed, 0 failed), re-confirmed kgpacks-rs #16/#18/#21/#22 CLOSED + #17 OPEN and
the 10-issue Simard tail (#2707 open, no 11th) with `rysweet/amplihack-xpia-defender` re-verified
as a live public repo, confirmed **every commit since `ca20e29b` is docs-only** (zero source
drift), and folded §7l's net-new facts (HEAD-precise `out.push`/`format!` emission anchors, the
#17 `updatedAt < closedAt_16` timestamp-staleness proof, and the xpia-defender missing-repo →
local-worktree-park reclassification) into the canonical findings — no finding overturned, no
remediation-weighting change, confidence remains High.
Source references were verified against the working tree at commit-time; GitHub
states were read from `rysweet/agent-kgpacks-rs` and `rysweet/Simard` on 2026-07-07.
The P1/P2 code changes are recommendations for follow-up development tasks; P5 is an
optional throughput nicety, not required to fix this signature.
