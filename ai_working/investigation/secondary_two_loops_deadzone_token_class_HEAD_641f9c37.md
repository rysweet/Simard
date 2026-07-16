# Secondary Investigation — Two Non-Closing Loops, Dead-Zone, Token Classification

**HEAD:** `641f9c37` (source-identical to `b47b6413`; only docs changed since).
**Role:** Secondary investigator.
**Focus:** (1) blocked-goal ladder gating, (2) workstream-gap missing launch edge,
(3) recurrence dead-zone between 2 and escalation threshold 3, (4) per-token
signal-vs-drift classification.

---

## 0. Drift reconciliation (validate-don't-rederive)

The strategy's warning "DRIFT IS PRESENT in mod.rs/observer.rs/signal.rs/wiring.rs/
guardrails.rs" is **now STALE / overstated**. Verified:

```
git diff --name-only 6e3113bc..HEAD -- src/overseer/   →  src/overseer/tests_root_cause.rs  (ONLY)
git diff --name-only b47b6413..HEAD -- src/            →  (empty — docs only)
```

**Only a test file (`tests_root_cause.rs`) drifted since baseline `6e3113bc`.**
No load-bearing emission/decision source (mod.rs, observer.rs, signal.rs, wiring.rs,
guardrails.rs) changed. All prior source citations at `b47b6413` re-ground unchanged
at `641f9c37`. Load-bearing lines re-verified below.

---

## 1. Loop A — workstream-gap: observe-and-notify, NO closing edge  ✅ CONFIRMED

`act_flag_workstream_gaps` (`mod.rs:884–948`) is the terminal act for a coverage gap.
Its entire body: peek `gap_gate` → build `OperatorNotification::workstream_gap` →
`notifier.notify(...)` → `gap_gate.commit(...)` → return `ActOutcome::WorkstreamGapsFlagged`.

- **No `RecipeLauncher::launch` call. No `IssueFiler::file` call.** Pure notify.
- Chain: `Signal::WorkstreamGap` → Problem `WorkstreamCoverage` / dedup_key
  `"workstream-gap"` (`mod.rs:1368–1373`) → `decide` → `FlagWorkstreamGaps` →
  `act_flag_workstream_gaps` (notify-only).
- Dedup: `gap_gate = WhisperGate::new(900, 200)` (`mod.rs:304`). A gap recurring
  within 900 s is `SuppressDuplicate`; after 900 s it re-notifies. Because nothing
  ever *launches a workstream to fill the gap*, the same gap re-fires every window,
  **forever**. This is the "missing launch edge."
- **Test suite pins notify-only as intended:** `tests_gap_scan.rs` asserts a
  notification is produced (`n.kind == "workstream-gap"`, subject
  `"[Overseer] workstream-gap:"`), and in the refusal case asserts *no* issue filed
  (`filed…is_empty()`). **No test asserts a workstream is launched** — confirming
  the closing edge is absent by design, not by accident.

**Anti-pattern:** *Observe-and-flag without a closing action.* Remediation lands
here: add a convergence rung (launch a bounded workstream / file a tracking issue /
escalate on recurrence) so the signal can trend to zero.

---

## 2. Loop B — blocked-goal ladder: dead-ends at `Report`  ✅ CONFIRMED

`decide_blocked_goal` (`mod.rs:1603–1631`), reached via `ProblemKind::GoalHygiene`
(`mod.rs:1447–1483`). Ladder, in order:

1. `recurrence >= RECURRENCE_ESCALATION_THRESHOLD (=3)` → `EscalateBlockedGoal`
2. `perpetual && is_no_progress_marker(reason)` → `UnblockGoal` (self-heal)
3. `needs_review` → `EscalateBlockedGoal`
4. **else → `Intervention::Report`** → `ActOutcome::Reported` (`mod.rs:658`) →
   `Remediation::acknowledged()` (`mod.rs:1129`). **No closing action.**

A plain block that is (a) not recurrence≥3, (b) not perpetual-no-progress, and
(c) not needs_review falls to rung 4 and is merely acknowledged. The goal stays
blocked and re-emits `goal:blocked:<goal_id>` (`mod.rs:1336`) every cycle.

**No WHY-routing rung exists.** `resolution_for_why` / `reinvestigate_bare_blocked_goals`
(named in prior DISCOVERIES as a *proposed* fix) **do not exist in source** — grep
returns nothing. So a dead-zone blocked goal has literally no self-resolving path.

**Anti-pattern:** *Classify-then-route missing — the stall is parked, not routed.*

---

## 3. The recurrence DEAD ZONE  ✅ CONFIRMED (occurrence = 2 is honest)

Two independent thresholds create a gap:

| Mechanism | Value | File |
|---|---|---|
| Within-window dedup (noise floor) | 900 s | `guardrails.rs:312–317`, gates at `mod.rs:286–304` |
| Root-cause escalation threshold | 3 | `root_cause.rs:33` |

- **Below 900 s / same window:** `WhisperGate::peek` returns `SuppressDuplicate`
  (`guardrails.rs:313–316`) — not recorded.
- **recurrence ≥ 3:** `decide_blocked_goal` escalates (`mod.rs:1613`).
- **recurrence = 2:** above the dedup floor (it genuinely re-recorded across two
  windows / restarts) but below escalation → the only reachable rung is `Report`
  (blocked) or re-notify (gap). **No auto-remediation rung exists at 2.**

Therefore **"2× seen" is a faithful count, not a counting/hash/replay bug.** The
defect is the *absence of a convergence rung* in [1] and [2], not the counter.

**Storage-idempotency note:** `WhisperGate.last_delivered` is an in-memory
`HashMap<String,i64>` (`guardrails.rs:294`) — **per-process, cleared on daemon
restart, never persisted.** So the dedup window is not durable; a restart re-opens
the window and can re-emit a still-open signature. This *reinforces* honest
recurrence: the count reflects real re-observation, and any signature-keyed
idempotency (if desired) must live at the storage layer, not the in-memory gate.

---

## 4. Self-ingestion — the `overseer-obs:` nesting  ✅ CONFIRMED (drift, not signal)

Full loop verified end-to-end:

1. Recall runs in `run_cycle` (`mod.rs:423–440`); recall-derived
   `Signal::RecurringSignature{ signature, occurrences }` joins the signal set.
2. `orient`/`signal_to_problem` (`mod.rs:1353–1363`) maps it to a Problem with
   **`dedup_key = sanitize_recalled(signature)`** — i.e. the prior
   `overseer-obs:…` string *becomes a problem key*.
3. `wiring.rs:301` calls `overseer.write_back_observation(&cycle.problems)` with the
   **entire** problem set — **no filtering of recall-derived problems.**
4. `observation_signature` (`mod.rs:1068–1072`) sorts+dedups the dedup_keys and
   prefixes `overseer-obs:`, joining with `|`. The prior `overseer-obs:…` key is now
   embedded as a token → **nested `overseer-obs:goal:blocked:…` runs**, exactly as
   seen in the reported signature.

**Second self-feed (new corroboration):** because the `RecurringSignature` problem is
`ProblemKind::ProcessHealth` (`mod.rs:1357`), `decide()` routes it to
**`Intervention::LaunchRecipe`** (`mod.rs:1429–1435`) with
`task_description = problem.summary` (the recurring-signature text). So a recalled
self-observation can *spawn a recipe to investigate itself* — plausibly the origin of
this very investigation. (Subject to `gate()` + per-cycle launch cap, so it is
throttled, not unbounded.)

**Fix direction:** never write back recall-derived meta-problems (exclude
`ProblemKind::ProcessHealth` `RecurringSignature` from the write-back set), and/or do
not route a recalled self-signature to `LaunchRecipe`.

---

## 5. Per-token classification (signal vs drift)

| Token | Provenance (file:line) | Class | Notes |
|---|---|---|---|
| `goal:blocked:<goal_id>` | `mod.rs:1336` (Signal::GoalBlocked) | **SIGNAL** | Genuine board hygiene; honest per-goal. Persists only because Loop B has no dead-zone rung. |
| `workstream-gap` | `mod.rs:1371` (Signal::WorkstreamGap) | **SIGNAL** | Genuine backlog-coverage gap (`sensor.rs::detect_workstream_gaps`). Persists because Loop A never launches. NOT a decomposition failure. |
| `resource:engineer_spawn` | `mod.rs:1270` (Signal::EngineerSpawnRate) | **SIGNAL** | Real resource-pressure telemetry (`ResourcePressure`→`Escalate`, `mod.rs:1444`). Corroborates under-resourcing that *causes* the gap/blocked oscillation. |
| `overseer-obs:` (nested prefix runs) | `mod.rs:1072` + recall write-back (`wiring.rs:301`, `mod.rs:1359`) | **DRIFT** | Self-observation feedback. Each nested `overseer-obs:` layer is the Overseer re-ingesting its own prior signature — membership artifact, not new external signal. |

**Verdict:** the leaf tokens (`goal:blocked:*`, `workstream-gap`,
`resource:engineer_spawn`) are genuine signal; the **nesting / repeated
`overseer-obs:` layering is self-ingestion drift.** `resource:engineer_spawn` does
not change the verdict — it *corroborates* the single root cause below.

---

## 6. Two signatures, one root problem

`goal:blocked:*` and `workstream-gap` co-occur for the same entities because an
under-resourced goal **oscillates**: `workstream-gap` while active-but-uncovered,
`goal:blocked` once it idles out. `resource:engineer_spawn` is the resourcing
pressure underneath both. Treat as **one convergence/resourcing problem**, not
three bugs — and definitely not a counter bug.

---

## 7. Integration points / concerns for verification phase

- `gate()` (`mod.rs:569–`) can *hold* gap/blocked interventions when the respective
  opt-out flag is off (`gap_scan_enabled`, `goal_health_enabled`). Verify the live
  daemon has these **enabled**; if held, the loops never even reach their (already
  missing) closing edge.
- Any remediation adding a launch edge to Loop A must respect
  `max_launches_per_cycle` and the recursion guard (`act_flag_workstream_gaps:885`
  already fails closed on unconfigured identity) to avoid a spawn storm.
- Regression coverage to update when a rung is added: `tests_gap_scan.rs` (currently
  asserts notify-only), `tests_goal_health.rs`, `tests_root_cause.rs`.

## 8. Questions for verification

1. Is `memory_recall_enabled` true in production? (default is `false`, `mod.rs:298`.)
   If false, the `overseer-obs:` nesting cannot occur — so the observed nested
   signature implies recall **is** enabled in the live daemon. Confirm.
2. Does the write-back set actually contain the recall-derived problem in prod, or is
   it stripped upstream? (Source shows it is NOT stripped — `wiring.rs:301` passes
   `&cycle.problems` whole.)
3. Confirm the `2` in the reported signature originates from `occurrences` in
   `Signal::RecurringSignature` (`mod.rs:1355/1361`) and not from a separate counter.
