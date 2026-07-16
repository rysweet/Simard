# Secondary Investigation — blocked-goal + workstream-gap + engineer_spawn triad: symptom vs. failed-remediation & non-closing-loop detection

**Role:** SECONDARY investigator (triad coupling / non-closing loop classification)
**HEAD:** `3e6b6933` (current)
**Mandate:** RE-GROUND + VALIDATE, do NOT re-derive. Report drift vs. FINAL_SYNTHESIS.

---

## 0. Drift verdict — ZERO source drift; one net-new test STRENGTHENS the verdict

`git diff --name-only 5a85317b..HEAD -- '*.rs'` and `6e3113bc..HEAD -- '*.rs'` both return **exactly one file: `src/overseer/tests_root_cause.rs`** (a test). Every production `.rs` citation from prior waves re-verifies byte-for-byte at HEAD:

| Token / seam | Prior citation | HEAD `3e6b6933` | Holds? |
|---|---|---|---|
| `overseer-obs:` join | `mod.rs:1068-1072` | `fn observation_signature`, `format!("overseer-obs:{}", keys.join("|"))` | ✅ |
| `goal:blocked:<goal_id>` | `mod.rs:1336` | `format!("goal:blocked:{goal_id}")` | ✅ |
| `workstream-gap` literal | `mod.rs:1371` | `"workstream-gap".to_string()` | ✅ |
| `resource:engineer_spawn` literal | `mod.rs:1267-1272` | `"resource:engineer_spawn".to_string()` (`:1270`) | ✅ |
| `act_flag_workstream_gaps` notify-only | `mod.rs:884-948` | peek→notify→commit `gap_gate`; **no launch/file/spawn** | ✅ |
| `WorkstreamCoverage` Decide arm | `mod.rs:1534-1543` | `Intervention::FlagWorkstreamGaps` (no `LaunchRecipe`) | ✅ |
| `decide_blocked_goal` | `mod.rs:1603-1631` | escalate≥3 / unblock / escalate-review / Report | ✅ |
| sensor blocked-skip | `sensor.rs:299-302` | `if matches!(g.status, GoalProgress::Blocked(_)) { …skip… }` | ✅ |
| completion_evidence gate | `ooda_loop/cycle.rs:582` | `if let Some(source) = &memories.completion_evidence` | ✅ |

**Drift note (non-blocking, corroborating):** the one changed file adds a net-new test
`loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence` (`tests_root_cause.rs:+476..`).
It asserts a **loud Lane-A `RecurringSignature` (occurrences ≫ both floors) with EMPTY Lane-B
recall leaves `why.recurrence == 0` and `decide` returns `UnblockGoal` (self-heal), NOT
escalation.** This is a codified proof of the two-lane decoupling (FINAL_SYNTHESIS §2.3): the
visible `×2` observation lane **cannot** trip the `≥3` root-cause escalation lane. Prior
FINAL_SYNTHESIS is **confirmed and hardened**, not superseded.

---

## 1. Triad classification: SYMPTOM, not failed remediation

The three tokens are **three views of one under-resourced STATE**, co-occurring in a single
window because all three predicates held simultaneously — there is **no orchestration edge
between them**:

| Token | ProblemKind | Condition when it fires | Independent of others? |
|---|---|---|---|
| `goal:blocked:<slug>` | `GoalHygiene` | goal parked idle by no-progress breaker | yes |
| `workstream-gap` | `WorkstreamCoverage` | p1/p2 **active, non-blocked** goal/issue uncovered (`sensor.rs:288-345`) | yes |
| `resource:engineer_spawn` | `ResourcePressure` | live engineer count saturated (≥ cap) | yes |

**Verdict: the ~31 `delivery:pr` burst + triad is a SYMPTOM of under-throughput, NOT a failed
remediation.** Evidence:

- `act_flag_workstream_gaps` (`mod.rs:884-948`) is **notify-only** — it emails/Signals the
  operator and commits `gap_gate`. It never launches a workstream, files an issue, or spawns an
  engineer. There is no code path from `workstream-gap` → `engineer_spawn`. The spawn is not a
  remediation *for* the gap; both are passive observations of the same saturated state.
- Actual engineer spawning lives in the OODA loop (`no_progress` `SpawnEngineer` rung, bounded to
  one guided retry), **not** at the overseer boundary. So `resource:engineer_spawn` in the
  signature is **benign passive telemetry** (fixed literal key; the volatile `{live}` count lands
  only in the summary, never the dedup_key) — it does not perturb dedup/idempotency and must **not**
  be treated as a new defect.
- The `delivery:pr` multiplicity is *evidence of activity* (the system is delivering PRs) that
  co-occurs with a goal still marked blocked — i.e., **"done misread as stuck"** (kgpacks-rs
  canonical incident: issues CLOSED / PRs MERGED, yet the goal re-parks). That is a
  classification/done-gate accuracy question, not a spawn/launch orchestration defect.

## 2. Non-closing-loop detection: TWO surfaces, same shape

Both lanes that feed the recurring signature **observe-and-flag without a convergence rung**:

1. **Gap lane (D3):** `WorkstreamCoverage` is the **only** High-family Decide arm that yields
   `FlagWorkstreamGaps` with **no `LaunchRecipe` edge** — contrast the sibling `StepFailure` arm
   (`mod.rs:1549-1580`) which produces a real corrective `LaunchRecipe`, and `ProcessHealth`
   (`:1429`) / `CrossCutting` (`:1436`). The 15-min `gap_gate` re-notifies a persistently
   uncovered item **every window forever**. Missing edge = launch/file, keyed on
   `GapItem.signature` (per-gap), NOT the bare `"workstream-gap"` dedup_key (**INV-GAP-KEY trap**,
   `mod.rs:1371`, else all gaps fold into one).
2. **Blocked-goal lane (D2):** `decide_blocked_goal` (`mod.rs:1603-1631`) only ever escalates
   once-per-window (gated), unblocks, or `Report`s — it **never marks the observation lane
   resolved.** For terminally-classified goals (`UnclearCriteria`/`GenuinelyStuck`/
   `UpstreamDependency`-defer) staying blocked is *correct*, but nothing converges the recurring
   observation signal, so the `goal:blocked` token re-emits every write-back window.

**The count is honest; the defect is the missing closing action** — do NOT "fix" the counter.

## 3. Refinement carried forward (still valid at HEAD)

- **Signature-invariant recurrence:** `goal:blocked:<slug>` omits any WHY token (dedup_key is
  `goal_id` only). A *correctly-classified* terminal block is indistinguishable from a bare park
  in the signal. **Do not infer "no WHY classification" from recurrence alone** — the WHY reasoner
  IS wired by default at HEAD (`cycle.rs:582-636`); bare parks are a *degraded-configuration*
  artifact (absent `completion_evidence` source or the env kill-switch), not a missing subsystem.

---

## Patterns / anti-patterns (this focus)
- **Observe-and-flag without a closing action** — confirmed on BOTH lanes (gap notify-only;
  blocked-goal decide never resolves the observation signal). Same shape, two surfaces.
- **Recurrence dead zone** — `×2` above one-off noise, below the `3` escalation bar; the net-new
  test proves Lane A cannot cross into Lane B, so it parks at 2 with no auto-remediation rung.
- **Two signatures, one root problem** — a goal oscillates `workstream-gap` (active/uncovered,
  Blocked explicitly skipped in `sensor.rs:299-302`) ↔ `goal:blocked` (idle). Confirmed.

## Integration points
- Orient → `signal_to_problem` (`mod.rs:1262-1371`) mints all three dedup_keys.
- Write-back → `wiring.rs:301` → `observation_signature` (`mod.rs:1068`) = only lane feeding the
  visible composite signature; independent of the Act/notify phase (gap-scan opt-out at `mod.rs:596`).
- Act → `act_flag_workstream_gaps` (`mod.rs:671→884`), notify-only.
- OODA breaker → `ooda_loop/cycle.rs:582-698`, double-gated on `completion_evidence` +
  `no_progress_investigation_enabled`.

## Questions for verification phase
1. Does the production daemon supply `memories.completion_evidence`? If yes → sustained bare parks
   should self-heal; audit **reasoner classification accuracy** (kgpacks recurring despite
   `AlreadyComplete` ⇒ done-gate misfiring), not breaker wiring. If no → that absence is the
   concrete "no WHY" root cause. The signature alone cannot disambiguate.
2. Any remediation rung for gaps must key on `GapItem.signature`, not `"workstream-gap"`
   (INV-GAP-KEY, `mod.rs:1371,1543`).
3. D2 fix must ship the escalation gate + occurrence counter atomically and read a count-in-content
   field, not `recall.len()` (CallerKey dead-code trap).
4. Confirm `gap_scan_enabled` in production: if disabled, gaps recur in the signature with **zero**
   operator visibility (silent-degradation surface).

## Reconciliation
**Confirms and hardens** FINAL_SYNTHESIS (§2.3 two-lane, §2.5 gap notify-only, §2.6 one
under-resourced state, §2.7 D1/D2/D3) and `secondary_blocked_park_and_gap_spawn_coupling_HEAD_9fd1ea0a.md`
verbatim at HEAD `3e6b6933`. **No drift** in any production `.rs`; the sole `.rs` change is a
net-new test that codifies the two-lane decoupling. Verdict unchanged: **real non-closing
observation loop (missing convergence rung), NOT a counting/dedup defect; triad is symptom, not
failed remediation.**
