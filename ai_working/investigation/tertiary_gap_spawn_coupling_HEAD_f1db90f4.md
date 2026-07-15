# Tertiary (Architect) Findings — workstream-gap ↔ engineer_spawn coupling & issue-17 ws2 blockage

**HEAD verified:** `f1db90f4` · **Test state:** `cargo test --lib overseer::` → **361 passed, 0 failed**
**Scope:** architectural coupling verdict + minimal remediation *only if trivial*.

## Coupling verdict: INDEPENDENT aggregation artifacts (no code coupling)

`workstream-gap` and `resource:engineer_spawn` share **no** producer input, ProblemKind,
dedup_key, root cause, or intervention. They are two disjoint lanes that merely co-sampled
`true` in the same Observe tick. Trace (all at HEAD):

| Aspect | `resource:engineer_spawn` | `workstream-gap` |
|---|---|---|
| Producer (`signal.rs:signals_from`) | `state.live_engineers >= 8` (`ENGINEER_SPAWN_THRESHOLD`, L393-397) | `!state.workstream_gaps.is_empty()` (L475-479) |
| Input field | `live_engineers: Option<u32>` | `workstream_gaps: Vec<GapItem>` |
| Signal | `EngineerSpawnRate { live }` | `WorkstreamGap { gaps }` |
| ProblemKind / Priority (`mod.rs:classify_signal`) | `ResourcePressure` / Normal (L1267) | `WorkstreamCoverage` / High (L1368) |
| dedup_key | `"resource:engineer_spawn"` | `"workstream-gap"` |
| Root cause (`root_cause.rs`) | `engineer-spawn-storm` (L326) | `important-work-with-no-active-workstream` (L438) |
| Intervention (`mod.rs:decide`) | `Escalate` (notify-only, L1444) | `FlagWorkstreamGaps` (notify-only, L1534) |
| Dedup gate | escalation whisper gate | `gap_gate` `workstream-gap:{signature}` (L901) |

There is **no** path where the live-engineer count feeds gap detection or vice-versa. The
long `workstream-gap|...|resource:engineer_spawn|workstream-gap|...` blob in the question is a
**flattened cognitive-memory recall concatenation of distinct persisted dedup_keys**, NOT a
causal chain. Interleaving reflects concurrent independent observations across ticks.

## issue-17 ws2 is a THIRD, deliberately isolated lane

`goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-...-7f5afcca` is a `GoalBlocked` →
`GoalHygiene` signal (one per blocked goal, `signal.rs:440-448`). It is **never** a
workstream-gap: `detect_workstream_gaps` explicitly `continue`s on
`GoalProgress::Blocked(_)` (`sensor.rs:300-302`), pinned by the passing regression
`delegates_blocked_goals_to_goal_health_and_never_reflags_them` (`tests_gap_scan.rs:413`).
The `-7f5afcca` suffix is the stable content hash of the dedup_key → one persisted record
recalled, not genuine re-duplication.

## Root-cause narrative for the blocked cluster

issue-17 ws2 and its siblings recur in memory because the goals **stay blocked on the board
across ticks**; each Observe pass re-emits one `GoalBlocked` per blocked goal, and recall then
reports the recurring signature. The bottleneck keeping them blocked lives **upstream of the
Overseer** (actual engineering progress / human `needs_review` resolution). The Overseer's
gap and spawn lanes are **diagnostic notify/escalate-only** — they neither spawn engineers nor
unblock goals, so they can be neither the cause nor the cure of the block.

The only real coupling is **semantic, at the operator/system level**: the `engineer-spawn-storm`
cause text itself reads *"a fan-out storm OR stuck workstreams"*. When ≥8 engineers are live
AND high-value goals sit blocked/uncovered, the plausible operational story is engineer
saturation → no fresh workstream available to unblock issue-17 ws2 → gaps accumulate. This is a
correlation surfaced as **two separate operator escalations by design**, not a mechanized loop.

## Minimal remediation: NONE warranted (would not be trivial-and-safe)

The system behaves as designed: three isolated lanes, notify/escalate-only, blocked goals
excluded from the gap scan, deterministic content-hashed dedup keys, all 361 tests green.
The "2×" recurrence is expected recall behavior, not a defect. There is **no trivial,
regression-safe code fix** — coupling the lanes or suppressing the recall would invent a fix
for non-broken behavior and risk regressing the pinned isolation guarantees. Recommendation:
**do not couple the lanes.** Any operator-facing improvement (an advisory correlation note in
the spawn/gap escalation text) is a product change, out of scope for a trivial fix.
