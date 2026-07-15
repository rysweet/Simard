# Secondary Investigation — Two Non-Closing OODA Loops + engineer_spawn/workstream-gap Drift

**Wave role:** SECONDARY investigator
**HEAD:** `b9f99879`  **Baseline:** `6e3113bc`
**Source drift:** `git diff --stat 6e3113bc..HEAD -- '*.rs'` → **EMPTY** (re-confirmed this wave).
All prior `src/overseer/*`, `src/ooda_loop/*`, `src/goal_curation/*` line citations remain valid.

## Verdict (short)
The `2×` recurrence is an **honest re-observation**, not a dedup/collision bug. Its persistence is
caused by **two structurally identical non-closing loops** that share one anti-pattern —
*observe-and-flag without a closing (convergence) action* — sitting in a **cross-lane recurrence
dead zone**. `resource:engineer_spawn` and repeated `workstream-gap` tokens are **benign
membership/count drift**, both symptoms of those same two loops, not new defects.

---

## 1. Loop (a): Blocked-goal WHY gating

**The remediation ladder exists and is well-formed** (`resolution_for_why`,
`src/goal_curation/no_progress_breaker.rs:384-417`). It classifies WHY a goal stalled and routes:

| Class (`no_progress_why.rs:53-72`) | Resolution | Reaches human? |
|---|---|---|
| AlreadyComplete | MarkDone | no |
| Obsolete | Drop | no |
| MissingPrecondition | Heal + retry (no block) | no |
| UpstreamDependency | Defer/Paused, **auto-clears** | no |
| UnclearCriteria / GenuinelyStuck | SpawnEngineer once → **Escalate (Blocked + WHY)** | yes, last resort |

**Where the loop fails to close:**
- The ladder is **double-gated** in `src/ooda_loop/cycle.rs:582-583`:
  - Gate A: `memories.completion_evidence.is_some()`
  - Gate B: `no_progress::no_progress_investigation_enabled()` (`no_progress.rs:203`, default on)
- In the **default production daemon** Gate A *is* satisfied — `completion_evidence` is wired to
  `GhCliEvidenceSource` (`daemon/mod.rs:455-471`, default on). So the classifier runs. **The gate is
  a latent closure hole only when a kill-switch (`SIMARD_COMPLETION_EVIDENCE=off`) or a non-daemon
  path leaves `completion_evidence = None`** (e.g. `client_factory.rs:109`, `daemon/mod.rs:1982`),
  in which case the entire block — including `reinvestigate_bare_blocked_goals` — is skipped
  (`cycle.rs:700-702` returns empty), and goals stay parked with a **bare block and no WHY**.
- Even when the classifier *does* run, `UnclearCriteria/GenuinelyStuck` legitimately terminate at
  **Escalate → Blocked, awaiting a human** (`no_progress_breaker.rs:402-410`). The Overseer then
  **re-observes** those goals every cycle as `goal:blocked:{goal_id}` (`mod.rs:1324-1345`). Unlike
  `UpstreamDependency`'s `Defer` (which **auto-clears**), the escalation path has **no convergence
  rung** — nothing trends the `goal:blocked` signal toward zero until a human acts. That is the
  static problem set feeding the `2×`.

**Anti-pattern:** terminal escalation without an auto-clearing / convergence edge.

## 2. Loop (b): Workstream-gap notified-but-never-launched

- `ProblemKind::WorkstreamCoverage` is the **only High-priority Decide arm with no
  `LaunchRecipe` edge** (`mod.rs:1534-1543`). It maps *only* to
  `Intervention::FlagWorkstreamGaps`.
- Act routes that to `act_flag_workstream_gaps` (`mod.rs:671 → 884-948`), which **only notifies the
  operator** (email + Signal) and commits a dedup gate. It **never launches a recipe, never files a
  backlog item, never spawns an engineer** (docstring `mod.rs:881-883` confirms this is by design).
- Confirmed structurally: `src/overseer/launch.rs` contains **zero** `WorkstreamGap` /
  `WorkstreamCoverage` / `FlagWorkstreamGaps` references — there is no launch path for gaps.
- Result: the uncovered work is flagged, suppressed within the dedup window, then flagged again —
  the `workstream-gap` count **never trends to zero**.

**Anti-pattern:** observe-and-flag without a closing action (matches PATTERNS.md).

## 3. Two signatures, one root problem
`sensor.rs:298-302` explicitly **excludes Blocked goals** from gap detection
("Blocked goals flow through goal_health; never re-flag them here"). So a single under-resourced
goal **oscillates**: while active-but-uncovered it emits `workstream-gap`; once it trips the breaker
to Blocked it emits `goal:blocked:*`. Loops (a) and (b) are therefore **two faces of one
resourcing/convergence gap**, not two independent bugs.

## 4. Recurrence dead zone (cross-lane visibility gap)
- Lane A (observation episodes): `RECURRING_SIGNATURE_THRESHOLD = 2` (`signal.rs:362`, fired at
  `:463`) — this is what makes the `2×` **visible**.
- Lane B (root-cause occurrences): `RECURRENCE_ESCALATION_THRESHOLD = 3` (`root_cause.rs:33`) — this
  is what would **escalate**.
- The two lanes count **different things with independent counters**. A signature can be observed
  `2×` on Lane A **forever** without ever incrementing Lane B toward 3. The defect is this
  **cross-lane blindness**, NOT the threshold value. **Raising 2→3 is the naïve counter trap** and
  must be avoided; the honest count needs a convergence rung, not a higher bar.

## 5. Drift classification — `resource:engineer_spawn` + repeated `workstream-gap`
**Both benign; both symptoms of §1–§2, not new defects.**

- `resource:engineer_spawn`: emitted by `EngineerSpawnRate` (`sensor/signal.rs:393-396`,
  `state.live_engineers >= ENGINEER_SPAWN_THRESHOLD`). Its dedup_key is the **fixed literal**
  `"resource:engineer_spawn"` (`mod.rs:1267-1272`); the volatile `{live}` count lives **only in the
  summary**, never in the key. So it adds **one stable member** to the pipe-joined signature and can
  never inflate a count-based hash. **Causal correlation:** the ladder's own guided-engineer
  dispatch (`cycle.rs:648-681`, `dispatch_spawn_engineer`) raises `live_engineers` — elevated
  engineer_spawn is the ladder repeatedly spawning engineers for stuck goals (Loop a), i.e. a
  read-back of the very stall it is trying to fix.
- Repeated `workstream-gap`: bare family key `"workstream-gap"` (`mod.rs:1371`) is
  evidence-independent — it **erases per-gap identity**, so distinct gap sets collapse to one token
  and no per-gap resolution can be tracked. Within a single write-back, `observation_signature`'s
  `sort_unstable()+dedup()` (`mod.rs:1069-1071`) collapses adjacent equals, so intra-signature
  duplicates cannot occur; the repeats in the question string come from **recall aggregating
  multiple prior episodes' `failure_signature`s** (Lane A `signal.rs:455-469`) into one
  `RecurringSignature` payload (`mod.rs:1353-1363`) — i.e. self-observation feedback, not a hash
  bug.

## Integration points / concerns for verification phase
1. Confirm no other Decide arm silently degrades to notify-only (audit all `ProblemKind` arms
   `mod.rs:1420-1570` for a missing `LaunchRecipe`/`launch.rs` edge).
2. Confirm `reinvestigate_bare_blocked_goals` cannot run when `completion_evidence = None`
   (it is nested under Gate A at `cycle.rs:628`) — this is the only rung that would rescue
   pre-#16 bare blocks, and it is gated out with the rest.
3. Confirm the Escalate→Blocked path has **no auto-clear** analogous to `Defer` — a convergence
   rung here (re-verify the escalated WHY each cycle; auto-clear when the filed issue closes /
   blocker resolves) is the minimal closure, distinct from bumping any threshold.

## Open questions
- Q1: Is there any config in the live deployment where `completion_evidence` is `None` (would turn
  the latent §1 gate into an active defect)? Needs runtime/config evidence, not source.
- Q2: Should `workstream-gap`'s dedup_key carry gap-set identity so the signal can converge
  per-gap, or is a launch/backlog rung (§2) sufficient on its own?
