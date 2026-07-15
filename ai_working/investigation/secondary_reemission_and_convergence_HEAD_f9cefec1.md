# Secondary Investigation — Re-emission loop & convergence-rung asymmetry

**Role:** SECONDARY investigator (patterns / no_progress re-emission / gap+spawn causality)
**HEAD:** `f9cefec1`
**Focus:** Re-emission loop in `ooda_loop/no_progress.rs`; `workstream-gap` /
`resource:engineer_spawn` emission (`sensor.rs`, gap-scan) and their causal link to
`goal:blocked`; `stewardship/routing.rs`.
**Drift check:** `git diff --name-only da7ea0fd..HEAD -- '*.rs'` = **only
`src/overseer/tests_root_cause.rs`** (the f9cefec1 Lane-A/B isolation test). No
uncommitted `.rs` drift. Every prior line citation re-verifies. This wave
**confirms, regrounds, and adds two refinements**.
**Verification:** ran `cargo test --lib no_progress` (77 pass), `gap_scan`
(21 pass), `root_cause` (21 pass) — all green, behavior empirically confirmed.

---

## Verdict (one line)

The `×2` is an honest re-observation of a **stable set of uncovered/blocked goals**.
Root cause is a **convergence-rung asymmetry**: run-*failures* get a durable,
idempotent GitHub issue (a closing action), but the two signatures in my scope —
`workstream-gap` and `resource:engineer_spawn` — get **only ephemeral notify/report
with no closing action**, so they re-emit every dedup window forever. Fix the missing
closing action, **not** the counter (documented trap).

---

## F1 — The convergence-rung asymmetry (the core defect, NEW framing)

Simard has exactly **one** durable convergence mechanism: the stewardship loop
`process_orchestrator_run` (`stewardship/mod.rs:70-115`) → `route_failure`
(`routing.rs:39`) → search-or-file a **deduplicated** GitHub issue. It is reached
**only** for orchestrator **run failures** via `StewardshipIssueFiler`
(`observer.rs:53-68`) and, in the acting decide map, only for these `ProblemKind`s
(`observer.rs:99-107`):
`ProcessHealth | QualityRegression | GoalHygiene | StepFailure | CrossCutting → FileIssue`.

The two signatures in my scope are **routed away** from that rung:

| Signal | Problem kind | Decide arm | Closing action? |
|---|---|---|---|
| `workstream-gap` | `WorkstreamCoverage` (High) | `FlagWorkstreamGaps` → `act_flag_workstream_gaps` (`mod.rs:884-946`) | **notify only** (email+Signal), `gap_gate` 900s dedup — no file, no launch |
| `resource:engineer_spawn` | `ResourcePressure` (Normal) | `Escalate{reason}` (`mod.rs:1444-1446`) / M1 `Report` (`observer.rs:113-120`) | **notify/report only** — no file, no spawn |

So a run failure *closes* (issue filed once, `MatchedExisting` thereafter — idempotent
convergence); a gap or a saturation event **never closes** and re-emits each window.
**Empirically pinned** by `tests_gap_scan::flagged_gap_never_constructs_an_issue_brief`
and `flags_gaps_notifies_both_channels_without_filing_then_dedupes_on_repeat`.

## F2 — Doc/impl mismatch on the gap rung (NEW, minor but concrete)

`observer.rs:117-119` asserts gaps are *"acted on by the acting Overseer (notify +
**deduped file**)"*, and `routing.rs:11-15` provisions a default-repo fallback for
*"the Overseer's `overseer` **workstream-gap briefs**"*. **Neither is wired.** The acting
path (`act_flag_workstream_gaps`) only notifies; no code constructs an
`OrchestratorRunSummary`/`OrchestratorRunBrief` from a `GapItem`. The routing comment and
the observer comment describe a **file rung that does not exist** — evidence the convergence
rung was *intended* and dropped, not deliberately omitted. This is the exact
**observe-and-flag-without-closing** anti-pattern the meta-pattern predicted.

## F3 — `resource:engineer_spawn` is telemetry, but the `8`-coupling is real

Confirmed benign as an *emitter* (`Signal::EngineerSpawnRate{live}` when `live>=8`,
`signal.rs`; fixed literal dedup_key `resource:engineer_spawn`, `mod.rs:1267-1272`;
`Priority::Normal`). No **code** edge to `workstream-gap`. I **endorse the prior
starvation-coupling refinement** (`secondary_starvation_coupling_HEAD.md`): the emit
threshold (`ENGINEER_SPAWN_THRESHOLD=8`) equals the hard admission cap
(`max_concurrent_engineers=8`), so the same state that mints the observation is the state
that **rejects** new engineer spawns. That is a **state coupling, not a data-flow edge** —
`resource:engineer_spawn` is an *effect/early-warning* of saturation, never a *cause* of a
blocked goal. Do not spin a spawn-failure hypothesis; do treat "saturated AND uncovered" as
one actionable event. `tests_root_cause::resource_pressure_escalation_is_labelled_symptom_mitigation`
confirms it is classified as symptom mitigation, not a fix.

## F4 — Why blocked goals never clear: the re-emission mechanics

The no-progress breaker (`no_progress.rs`) **does** have closing rungs — the `Escalate`
arm (`no_progress.rs:748-769`) files an issue + sets `Blocked`, and `SpawnEngineer`
(`:712-747`) attempts one guided retry then escalates *with why* next stall. The
re-emission of `goal:blocked` therefore comes from the **terminal state itself**: once a
goal is `Blocked`, `sensor.rs:300-302` **skips it in gap detection** (delegating to
`goal_health`), and the overseer re-observes it as `goal:blocked` every window. The breaker
never *un-blocks* a genuinely-stuck goal; nothing drains the blocked set. Hence the stable
membership → stable composite signature → honest `×2`.

**The oscillation** (same goal in *both* families): active+uncovered → `workstream-gap`
(`sensor.rs:298-320`, blocked skipped); once idle/stuck → `goal:blocked`. One entity, two
lenses — explains why kgpacks #12/17/18/23/25, the personas, coverage-70%, and coin-harness
appear in both families.

## F5 — Refinement of the "double-gated WHY ladder" claim (CORRECTION)

Prior artifacts state the WHY resolution ladder is *"double-gated off"*
(`cycle.rs:582-702`). **Precision correction, verified at HEAD:**
- Gate 1: `if let Some(source) = &memories.completion_evidence` (`cycle.rs:582`) — requires
  the completion-evidence memory to be wired. If `None` → breaker does **not** run at all.
- Gate 2: `no_progress_investigation_enabled()` (`cycle.rs:583`) is **default-TRUE**
  (`no_progress.rs:203-207`, `unwrap_or(true)`; only `SIMARD_NO_PROGRESS_INVESTIGATE=off`
  disables it).

So it is **not** "off by two default-off flags." Gate 2 is on by default; the *operative*
gate is Gate 1 (evidence source wiring, itself tied to `SIMARD_PROGRESS_EVIDENCE`). When
both hold, the ladder runs and resolves via `resolution_for_why` (`no_progress_breaker.rs:384-414`:
`MarkDone | Drop | Heal | Defer | SpawnEngineer | Escalate`). The investigation/reinvestigation
tests (77 pass) prove the resolving path works **when wired**. This reframes priority #3:
the risk is a **mis-provisioned evidence source in production**, not a hard-off ladder.

---

## Pattern classification

- **Meta-pattern (holds):** the recurrence count is honest — audit the closing action, not
  the counter. Confirmed: the counter is `RecurringSignature.occurrences` (Lane-A), the
  defect is a missing closing action (Lane-C, resourcing/convergence).
- **Anti-patterns present:** *Observe-and-flag-without-closing* (F1/F2, gaps),
  *Recurrence dead zone* (`×2 < RECURRENCE_ESCALATION_THRESHOLD=3`), *Resource starvation
  coupled to a missing convergence rung* (F3), *Doc/impl drift on an intended rung* (F2).
- **Lane isolation (holds, cited):** `tests_root_cause::loud_lane_a_recurring_signature_does_not_feed_lane_b_recurrence`
  (f9cefec1) proves Lane-A `RecurringSignature` does **not** feed Lane-B root-cause recurrence.
  The 2↔3 dead zone is green-by-test.

## Integration points

- `overseer/mod.rs:671,884-946` (gap decide→act) · `observer.rs:53-120` (stewardship filer +
  M1 decide map) · `stewardship/mod.rs:70-115` + `routing.rs` (the sole convergence rung) ·
  `sensor.rs:288-372` (gap detection, blocked-skip) · `ooda_loop/cycle.rs:582-702` (breaker
  gating) · `no_progress.rs:712-769` (SpawnEngineer/Escalate rungs) ·
  `no_progress_breaker.rs:384-414` (WHY→resolution ladder).

## Recommendation (diagnosis only — underlying goals OUT OF SCOPE)

**ACT on one defect, DO NOT touch the counter.** Add the missing convergence rung for
`WorkstreamCoverage`: route a fresh `GapItem` into the existing stewardship
file-or-match path (or `launch.rs`), **keyed on `GapItem.signature`** (per-gap), never the
bare `"workstream-gap"` dedup_key (INV-GAP-KEY — else all gaps fold into one issue). Place it
at first proven recurrence (`×2`) to close the dead zone. Secondary: reconcile the
`observer.rs:117-119` / `routing.rs:11-15` comments with reality (either wire the file rung
they promise or delete the claim). No action on `resource:engineer_spawn` beyond optionally
escalating it **only when co-occurring** with an unmet `workstream-gap`.

## Questions for verification phase

- **Q1:** In the production daemon config, is `memories.completion_evidence` actually wired
  (`SIMARD_PROGRESS_EVIDENCE` ≠ `off`)? If not, Gate 1 (F5) silently disables the entire WHY
  ladder → bare blocks. This is the single highest-leverage config check.
- **Q2:** Confirm no path constructs an `OrchestratorRunSummary` from a `GapItem` on HEAD
  (F2) — i.e., the promised gap-file rung is genuinely absent, not wired elsewhere.
- **Q3:** Confirm a cap-rejected `SpawnEngineer` guided retry (`no_progress.rs:713-717`,
  `mark_guided_retry`+`reset_count` fire regardless of accept/reject) is retryable next
  window and does not push the goal to permanent bare-block under sustained saturation.
