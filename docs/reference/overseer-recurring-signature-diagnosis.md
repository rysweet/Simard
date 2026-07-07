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
  loop. Round-3 consolidates the parallel deep dives, reconciles all source
  line anchors to HEAD 0180b75c, and records the executed H1/H2 test results.
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

The Overseer auto-filed **9 near-duplicate** `rysweet/Simard` issues for this one
signature between 2026-07-06 11:01Z and 2026-07-07 08:21Z (all still OPEN):

- parity-only signature: #2669, #2672, #2678, #2691
- nesting `issue-17`: #2744, #2750, #2757
- nesting `issue-16`: #2768 (2026-07-07 02:02Z), #2841 (2026-07-07 08:21Z)

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
9-issue tail that a reconciliation step would have suppressed.

---

## 2. `quality:gym_skipped` and `workstream-gap` — independent ambient co-signals

Both are folded into the **same Observe tick** as the `goal:blocked` signals and
then swept up by the same self-recall nesting. Neither is a *cause* of the block;
each is an independent, separately-sourced indicator.

### 2a. `quality:gym_skipped` — an operator configuration, not a workstream failure

`Signal::GymSkipped` (`src/overseer/signal.rs:398`) fires whenever
`ObservedState.gym_skipped` is true, and maps to a **`QualityRegression`,
`Priority::Low`** problem with dedup key `quality:gym_skipped`
(`src/overseer/mod.rs:1292`).

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
path (`provider.rs:61` → `sensor.rs:125` → `signal.rs:398` → `mod.rs:1292`) with no
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

## 3. `resource:engineer_spawn` — an amplifier under AIMD contraction, not the root cause

`Signal::EngineerSpawnRate { live }` (`src/overseer/signal.rs:393`) fires when
`ObservedState.live_engineers >= ENGINEER_SPAWN_THRESHOLD = 8` (`:351`), sourced
from `StatusSnapshot.resources.live_engineers`
(`src/overseer/capabilities.rs:81`). It maps to a **`ResourcePressure`,
`Priority::Normal`** problem, dedup key `resource:engineer_spawn`
(`src/overseer/mod.rs:1280`) — i.e. "≥8 engineers are live right now."

Does that contention block the parity workstreams? **It can, but only as a
secondary amplifier — it is not the root cause.**

- New engineer starts are bounded per cycle to `coverage_cap`
  (`src/ooda_loop/cycle.rs:316–334`), which is the AIMD scaler's `current_max()`
  (or `max_concurrent_actions`, default `SIMARD_MAX_CONCURRENT_ACTIONS = 5`,
  `src/ooda_loop/types.rs:288`; auto-scaling ceiling `= 4 ×` base). Decide applies
  the same `limit` (`src/ooda_loop/decide.rs:41`).
- `ensure_goal_coverage` **prioritizes** giving every uncovered incomplete goal
  exactly one engine *ahead of* extra parallelism for already-covered goals
  (`cycle.rs:310–322`). So coverage starvation bites **only** when the cap is the
  binding constraint — i.e. when more goals need coverage than `coverage_cap`
  slots, which under `SIMARD_SCALING=auto` happens after AIMD has *contracted* the
  cap in response to failures/budget pressure.
- With `live_engineers ≥ 8` the cap must have expanded above base 5 at some point,
  so the signal marks a system running hot in its upper range. If AIMD then
  contracts below the live count, genuinely-open work (e.g. #17) can be starved of
  a slot and make no progress — feeding the no-progress breaker that produces the
  *next* safeguard park.

**Bottom line:** spawn pressure is a plausible **contributor to fresh no-progress
parks** under AIMD contraction, but the recurring `goal:blocked` segments in this
signature are dominated by **stale** parks for already-delivered work (Section 1),
which spawn contention cannot explain. Treat `resource:engineer_spawn` as a
health/amplifier signal, not the blockage's root cause.

**Confidence: Medium.** The *negative* claim — spawn pressure is **not** the root
cause — is High: it is proven by Section 1's stale-park evidence, which is
independent of engineer count. The *positive* mechanism — that AIMD `current_max`
contraction below `live_engineers` starves genuinely-open work of a coverage slot —
is Medium: the code path (`cycle.rs:310–334`, `decide.rs:41`, `types.rs:288`) is
verified, but whether an actual contraction-below-live-count episode occurred was
**not** confirmed from runtime telemetry, so this remains a plausible-but-unobserved
contributor. This is the weakest link in the diagnosis, which is why the
corresponding remediation (P5) is explicitly conditional.

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
3. **P3 — Clear the current backlog.** Run `simard goal unblock-all` (or
   equivalent) to clear the four stale parks now; bulk-close the 9 duplicate
   `rysweet/Simard` issues (#2669, #2672, #2678, #2691, #2744, #2750, #2757,
   #2768, #2841) as artifacts of this loop, referencing this diagnosis.
4. **P4 — Resolve the ambient co-signals.** Decide `#17` (ws2 int8-pq-embed)
   explicitly — either run its parity gate and ship-behind-flag or mark it
   obsolete/deferred so it stops reading as open work. If the gym is intentionally
   off in this deployment, suppress or down-rank `quality:gym_skipped` so an
   expected config stops adding perpetual noise; otherwise unset `SIMARD_SKIP_GYM`.
5. **P5 — Spawn headroom (only if AIMD contraction is observed).** If telemetry
   shows the AIMD `current_max` contracting below `live_engineers` while open work
   waits, raise the floor / tune the scaler so genuinely-open workstreams are not
   starved of a coverage slot. Not required to fix the recurring signature.

**Confidence in remediation efficacy:** High for **P1–P3** — they sever the verified
mechanism (P1) and clear the verified stale inputs (P2/P3), each tied to a confirmed
root cause. Medium for **P4** (down-ranking `gym_skipped` is High; the correct
disposition of #17 is a product decision, not a diagnostic certainty) and **P5**
(gated on the Medium-confidence, unobserved AIMD-contraction claim in Section 3).

---

## 6. Confidence assessment

Overall confidence in the diagnosis: **High.** Five of the six findings rest on
disprovable, source-grounded or live-state evidence; the single Medium item (§3's
positive spawn-contention mechanism) is explicitly isolated and its dependent
remediation (P5) is gated behind an observation that has not yet been made.

| # | Finding | Confidence | Primary evidence | Residual uncertainty |
|---|---|---|---|---|
| 1 | Blocked segments are **stale safeguard-parks** for delivered work (#16/#18/#21/#22); #17 is an intentional gate | **High** | Live closed/open issue states + timestamps; `sensor.rs:204,209`, `no_progress_breaker.rs:58,69`, `completion_gate.rs:31,380`; smoking-gun timing (#2768/#2841 filed after #16 closed) | "No component re-runs the done-gate post-closure" is inferred (corroborated by the 9-issue tail) |
| 2a | `quality:gym_skipped` = ambient `SIMARD_SKIP_GYM` operator flag, not a workstream failure | **High** | Deterministic flag path `provider.rs:61`→`sensor.rs:125`→`signal.rs:398`→`mod.rs:1292` | None material (flag-set state entailed by the signal firing) |
| 2b | `workstream-gap` is disjoint from `goal:blocked` (uncovered *other* backlog) | **High** | Hard code invariant `sensor.rs:300` excludes blocked goals; `sensor.rs:288`, `signal.rs:475`, `mod.rs:1381` | Exact identity of the uncovered item not pinned (immaterial) |
| 3 | `resource:engineer_spawn` is an amplifier under AIMD contraction, **not** the root cause | **Medium** | Negative claim proven by §1; mechanism path `cycle.rs:310–334`, `decide.rs:41`, `types.rs:288` verified | A contraction-below-live-count episode was **not** confirmed from runtime telemetry |
| 4 | Recurrence (2×) = unfiltered self-recall + unbounded re-wrap over standing inputs | **High** | Round-1 finding **reproduced** by executable H1/H2 tests (`tests_memory_recall.rs`, round 2) | None material |
| 5 | Remediation P1–P3 sever the mechanism / clear stale inputs; P4–P5 conditional | **High (P1–P3)**, **Medium (P4–P5)** | Each action mapped to a confirmed root cause; P5 gated on §3 | P4 #17 disposition is a product decision; P5 depends on §3's Medium claim |

**Method note:** confidence is graded against how disprovable each claim is —
"High" means grounded in source (`file:line`) or live GitHub/board state that a
reviewer can independently re-check and that would falsify the claim if wrong;
"Medium" means the causal *path* is verified but a required runtime condition was
not directly observed.

---

## 7. Round-3 consolidation & verification

The round-3 pass (this update) consolidated the parallel deep dives against the
live working tree at **HEAD `0180b75c`** and **executed** the round-2 hypothesis
tests. Result: the diagnosis holds unchanged; every mechanism cited in Sections 1–4
was re-confirmed at its current-tree line, and the structural root cause is now
**proven by passing tests**, not just asserted.

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

### 7b. Reconciled current-tree line numbers (HEAD `0180b75c`)

The round-2 doc quoted line numbers from an earlier commit; they have since drifted
~3 lines. The mechanism is unchanged — only the anchors moved. Canonical anchors:

| Mechanism | Round-2 doc cite | **Current tree (`0180b75c`)** |
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
  `resource:engineer_spawn` is an amplifier, not the cause (§3).
- **The two axes are independent and both must be cut.** Severing the loop (P1)
  stops the growth/nesting and the dedup-escaping issue floods; reconciling stale
  parks (P2) + clearing the backlog (P3) removes the standing inputs. Neither alone
  is sufficient — P1 without P2/P3 leaves a single non-growing recurrence; P2/P3
  without P1 leaves the amplifier ready to re-nest on the next standing input.
- **Overall confidence: High** (unchanged). Five of six findings are
  source/live-state grounded; the lone Medium item (§3 positive spawn mechanism)
  remains isolated behind conditional remediation **P5**. Round-3 adds executable
  proof for the structural core, raising §4 to the strongest evidentiary tier.

No new remediation is introduced; **P1–P5 (Section 5) stand as the consolidated
action set**, with P1's fix now backed by a green REFUTE-by-fix test.

---

## 8. Provenance

Investigation-only follow-up (investigation-workflow, rounds 1–3). No production
behavior was changed by this document. Round-1 established the structural cause
([`overseer-memory-recall-api`](./overseer-memory-recall-api.md)); round-2 added the
semantic diagnosis and the executable H1/H2 tests; **round-3 (this update)
consolidated the parallel deep dives, reconciled all line anchors to HEAD
`0180b75c`, and executed the H1/H2 CONFIRM/REFUTE tests (4 passed, 0 failed).**
Source references were verified against the working tree at commit-time; GitHub
states were read from `rysweet/agent-kgpacks-rs` and `rysweet/Simard` on 2026-07-07.
The P1/P2/P5 code changes are recommendations for follow-up development tasks.
