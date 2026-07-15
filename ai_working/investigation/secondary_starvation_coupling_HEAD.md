# Secondary Investigation — Executor-side starvation / feedback-loop pattern

**Role:** SECONDARY investigator (patterns)
**Focus:** `workstream-gap` + `resource:engineer_spawn` loop across `agent_supervisor`,
`agent_goal_assignment`, `engineer_loop/handoff/worktree`, `ooda_scheduler`, and the
capacity/admission layer. Detect starvation / feedback-loop.
**Method:** Re-grounded prior secondary work (`secondary_gap_and_spawn_HEAD_440e024c.md`)
against current HEAD and extended it into the executor/capacity layer that the prior
overseer-centric passes did not fully trace.

---

## Verdict (one line)

The two signatures are joined by a **shared coupling constant**, not by a data-flow edge:
the telemetry threshold that emits `resource:engineer_spawn` (`ENGINEER_SPAWN_THRESHOLD = 8`)
is **the same number** as the hard admission cap that blocks new engineers
(`max_concurrent_engineers = 8`). So `resource:engineer_spawn` is not "benign telemetry" —
it is an **early-warning that the concurrency cap is being hit**, and hitting that cap is
causally upstream of `workstream-gap` persistence (rejected/never-attempted spawns →
uncovered goals → gaps → notify-only → still uncovered). This is a genuine **resource-
starvation structure**, refining the prior "no causal edge / benign" framing.

---

## E1 — The coupled `8`s (NEW, the key finding)

| Constant | Value | Site | Role |
|---|---|---|---|
| `ENGINEER_SPAWN_THRESHOLD` | `8` | `overseer/signal.rs:351,394` | emits `Signal::EngineerSpawnRate{live}` → `resource:engineer_spawn` when `live_engineers >= 8` |
| `max_concurrent_engineers` | `8` (default; valid `1..=64`) | `typed_ooda/types.rs:680,795` | **hard admission cap** |

At **exactly 8 live engineer claims**, two things fire in the same window:
1. `signals_from` pushes `EngineerSpawnRate{live:8}` → `signal_to_problem`
   (`overseer/mod.rs:1267`) → `ProblemKind::ResourcePressure`, `Priority::Normal`,
   dedup_key `"resource:engineer_spawn"`.
2. The capability ledger `admit()` (`typed_ooda/ledger.rs:1792-1798`) rejects **any**
   `Action::SpawnEngineer` with `AdmissionRejected("engineer concurrency limit reached")`
   because `snapshot.concurrent_engineers >= self.policy.max_concurrent_engineers`.

So the very condition that mints the `resource:engineer_spawn` observation is the condition
that **prevents new engineers from covering the backlog** — the mechanistic bridge between
`resource:engineer_spawn` and `workstream-gap` that prior passes marked "no causal edge."
Refinement, not contradiction: there is no *code* edge, but there is a deterministic
*state* coupling on the constant `8`.

## E2 — Live-count is PID-liveness → cap is a saturation, not a deadlock

`count_live_engineer_claims` (`ooda_brain/context.rs:111-135`) counts engineer worktrees
whose `.simard-engineer-claim` sentinel PID `is_pid_alive_public`. So the cap **self-clears
when engineer subprocesses exit** — it is a *saturation ceiling*, not a permanent lock.

**But** the engineer subprocess has **no wall-clock timeout by design**
(`engineer_loop/agent_spawn.rs:29-33`, "intentionally no wall-clock timeout … liveness must
come from agent-emitted progress signals"). Consequence: a small number of long-running or
wedged engineers can hold slots indefinitely, **extending the starvation window** during
which every uncovered goal re-emits `workstream-gap` and, once idle, `goal:blocked`. The
absence of a liveness kill means the ceiling is only as transient as the slowest live agent.

## E3 — Two starvation paths, both stalled under saturation (NEW)

Under the 8-live saturation there are two distinct ways a named goal fails to get covered,
and **both are stalled**:

- **Active-uncovered goals (→ `workstream-gap`):** `detect_workstream_gaps`
  (`sensor.rs:288-372`) flags p1/p2 goals with no assignee/PR/branch/session/engineer wip-ref.
  These are routed to `act_flag_workstream_gaps` (`mod.rs:884-946`) which **only notifies**
  (email + Signal, deduped by `gap_gate = WhisperGate::new(900,200)`). There is **no edge
  from a gap into any spawn/launch path** — even if a slot were free, a gap never attempts a
  spawn. This is the *missing convergence rung* (confirmed from prior work).
- **Idle/blocked goals (→ `goal:blocked`):** the no-progress breaker's `SpawnEngineer` arm
  (`ooda_loop/no_progress.rs:712-731`) *does* attempt one guided spawn — but that spawn is
  routed through the effect executor (`typed_goal_session.rs:305,314`) and the capability
  ledger `admit()`, so under saturation it is **rejected by the same cap** and the goal stays
  bare-blocked.

Net: active goals never try to spawn; idle goals try but are cap-rejected. The set of
uncovered goals is therefore stable across windows ⇒ stable composite signature ⇒ the honest
`×2` re-observation.

## E4 — `self_relaunch_semaphore` is NOT the spawn limiter (refutes a strategy hypothesis)

The strategy hypothesized `self_relaunch_semaphore` might rate-limit engineer_spawn and
starve goals. **It does not.** `self_relaunch_semaphore` (`mod.rs`, `semaphore.rs`,
`handoff.rs`) is a **file-based leader semaphore for daemon self-relaunch/leadership handoff**
(build canary → verify → spawn child → transfer leadership), unrelated to per-goal engineer
concurrency. The engineer concurrency limiter is the capability-ledger `admit()` cap (E1).
Do not conflate them.

## E5 — Admission has two layers; only the ledger hard-gates on concurrency

- **Capability ledger `admit()`** (`ledger.rs:1792-1804`): deterministic hard reject on
  `concurrent_engineers >= max_concurrent_engineers` AND on `active_claims` conflict.
- **`run_resource_admission_gate`** (`resource_admission.rs:430-462`): gathers
  `in_flight_engineers` (`:371`) but feeds it to the *brain* (`decide_resource_admission`) as
  context; its only **deterministic hard rail is the disk-ceiling** (`:179-196`). So
  concurrency starvation is enforced by the ledger, while the resource-admission gate can
  *additionally* Defer under disk/memory/load pressure (fail-CLOSED on brain error, `:160`).
  Two independent gates can each suppress a spawn; a saturated system likely trips the ledger
  cap first.

---

## Pattern classification

- **Root-cause class:** *resource starvation coupled with a missing convergence rung* — not a
  scheduling bug, not a decomposition failure, not a counting/dedup artifact.
- **Feedback loop:** `8 live → resource:engineer_spawn emitted` **and** `spawn admission
  rejected` → uncovered goals persist → `workstream-gap` re-emitted each 900s window (notify-
  only, never closes) → idle goals degrade to `goal:blocked` → composite signature is stable →
  re-observed `×2`. The loop has **no draining edge** because (a) gaps are never routed to a
  spawn/launch, and (b) the only spawn path is cap-gated at the same threshold that raises the
  pressure signal.
- **Anti-patterns present:** *Observe-and-flag-without-closing* (gaps), *Recurrence dead zone*
  (2× is below `RECURRENCE_ESCALATION_THRESHOLD=3`), *no liveness kill on unbounded engineer
  subprocess* (extends the saturation window).

## Remediation levers (executor-side)

1. **Route gaps into the spawn/launch path** (add the convergence rung), keyed on
   `GapItem.signature` (per-gap), not the bare `"workstream-gap"` dedup_key (INV-GAP-KEY).
2. **Decouple the telemetry threshold from the hard cap**, or make `resource:engineer_spawn`
   escalate (currently `Priority::Normal`) when it *co-occurs* with unmet `workstream-gap`s —
   i.e., surface "saturated AND backlog uncovered" as one actionable pressure event.
3. **Prioritized admission / preemption:** under saturation, prefer admitting a p1/p2 gap-goal
   spawn over lower-value churn instead of first-come rejection.
4. **Bound engineer slot hold time** (progress-signal liveness, not wall-clock SIGKILL) so a
   wedged agent cannot hold a slot and extend starvation indefinitely (respect the
   PR#1988/#1989 no-SIGKILL constraint).

## Questions for verification phase

- **Q1:** Confirm `count_live_engineer_claims` and the ledger snapshot's `concurrent_engineers`
  are computed from the **same** claim source, so the emit-threshold (signal path) and the
  reject-threshold (ledger path) genuinely fire on the same count of `8` (E1 hinges on this).
- **Q2:** Confirm no code path raises `max_concurrent_engineers` above `ENGINEER_SPAWN_THRESHOLD`
  in production config (e.g. `SIMARD_OODA_*` / typed-ooda policy doc), which would decouple the
  two `8`s and weaken the coupling claim.
- **Q3:** Confirm the no-progress `SpawnEngineer` guided retry, when cap-rejected, does **not**
  consume `mark_guided_retry`/reset in a way that pushes the goal to permanent bare-block (i.e.
  a rejected spawn should be retryable next window, not counted as an exhausted retry).
- **Q4:** Confirm gaps never reach `launch.rs` `RecipeRunner` (prior claim) — verify no newer
  wiring routes `FlagWorkstreamGaps` into a spawn on the current HEAD.
