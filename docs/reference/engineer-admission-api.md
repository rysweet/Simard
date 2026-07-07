---
title: Engineer-admission API reference
description: >
  Reference for the dependency/overlap-aware engineer-admission gate — the
  CandidateGoal / LiveEngineerSignal / EngineerAdmissionCtx / EngineerAdmissionDecision
  types, the OodaBrain::decide_engineer_admission method, the gather→reason→apply
  seam and its two rails (the deterministic exact-path rail and the fail-open
  brain-error rail), the overlap module, the LiveEngineerWorktree.worktree_path
  addition, the reasoning recipe, the sanitization boundary, the observability
  record and metric, and the SIMARD_ENGINEER_ADMISSION kill-switch.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/dependency-overlap-aware-scheduling.md
  - ./ooda-engineer-admission-recipe.md
  - ./concurrent-engineer-dispatch.md
  - ./outcome-verification-api.md
  - ./recipe-context-var-sanitization.md
  - ../howto/diagnose-a-deferred-engineer-spawn.md
  - ../operations/engineer-admission-kill-switch.md
  - ../../src/ooda_actions/advance_goal/spawn.rs
  - ../../src/ooda_actions/advance_goal/admission.rs
  - ../../src/ooda_actions/advance_goal/overlap.rs
  - ../../src/ooda_brain/mod.rs
  - ../../src/engineer_worktree/discovery.rs
---

# Engineer-admission API reference

> **Status: implemented.** This reference describes the shipped
> typed surface in present tense. The types, trait method, seam, rails, overlap
> module, and kill-switch below live in
> [`src/ooda_actions/advance_goal/admission.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/admission.rs)
> and
> [`src/ooda_actions/advance_goal/overlap.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/overlap.rs)
> (wired into `dispatch_spawn_engineer` in
> [`src/ooda_actions/advance_goal/spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs));
> `EngineerAdmissionCtx` / `EngineerAdmissionDecision` and the
> `OodaBrain::decide_engineer_admission` method live in
> [`src/ooda_brain/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/mod.rs);
> the `worktree_path` field on `LiveEngineerWorktree` in
> [`src/engineer_worktree/discovery.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_worktree/discovery.rs);
> and the reasoning asset at
> [`prompt_assets/simard/recipes/ooda-engineer-admission.yaml`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/recipes/ooda-engineer-admission.yaml).

This reference specifies the API for the engineer-admission gate. For the
rationale, see
[dependency/overlap-aware engineer scheduling](../concepts/dependency-overlap-aware-scheduling.md).
The gate is a third instance of the brain-seam pattern already used by
[`decide_engineer_lifecycle`](ooda-engineer-lifecycle-recipe.md) and
[`decide_goal_outcome_verification`](outcome-verification-api.md):
`Ctx → OodaBrain method → RecipeBrain(recipe.yaml) → typed Decision → apply`.

## Contents

- [`CandidateGoal`](#candidategoal)
- [`LiveEngineerSignal`](#liveengineersignal)
- [`EngineerAdmissionCtx`](#engineeradmissionctx)
- [`EngineerAdmissionDecision`](#engineeradmissiondecision)
- [`OodaBrain::decide_engineer_admission`](#oodabraindecide_engineer_admission)
- [The seam and the two rails](#the-seam-and-the-two-rails)
- [Overlap detection (`overlap.rs`)](#overlap-detection-overlaprs)
- [`LiveEngineerWorktree` addition](#liveengineerworktree-addition)
- [The reasoning recipe](#the-reasoning-recipe)
- [Sanitization boundary](#sanitization-boundary)
- [Observability](#observability)
- [Kill-switch](#kill-switch)
- [Test matrix](#test-matrix)

## `CandidateGoal`

The goal Simard is about to spawn an engineer for, plus its **predicted file
footprint**. Assembled best-effort by the gather step.

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CandidateGoal {
    /// Goal id.
    pub id: String,
    /// Goal title / task text — the work order the engineer would receive.
    pub title: String,
    /// Predicted target paths (repo-relative POSIX), derived best-effort from
    /// the goal's `wip_refs` then prior-PR file lists. EMPTY when unknown —
    /// an empty scope means "no overlap knowable" ⇒ admit (fail-open), and the
    /// exact-path rail is inert.
    pub predicted_scope: Vec<String>,
}
```

## `LiveEngineerSignal`

One in-flight engineer, with the facts the brain weighs to judge overlap.

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LiveEngineerSignal {
    /// Goal id the live engineer is pursuing (recovered from its worktree dir).
    pub goal_id: String,
    /// PID recorded in the worktree claim sentinel.
    pub pid: i32,
    /// The engineer's worktree path (repo-relative or absolute; used only to
    /// compute `changed_files`).
    pub worktree_path: String,
    /// Files this engineer is touching: `git diff --name-only <merge-base>` ∪
    /// working-tree diff, repo-relative POSIX. Empty on any git error
    /// (absent-tolerant ⇒ no overlap ⇒ fail-open).
    pub changed_files: Vec<String>,
    /// Intersection of `changed_files` with the candidate's `predicted_scope`.
    /// Non-empty ⇒ an overlap signal.
    pub overlap_with_candidate: Vec<String>,
    /// `true` when the candidate goal's `wip_refs` reference this engineer's
    /// goal_id / PR (an explicit dependency, not just an incidental overlap).
    pub depended_on: bool,
}
```

## `EngineerAdmissionCtx`

The structured context handed to the brain. Assembled by
`gather_engineer_admission_ctx` — a **pure, best-effort** function. Every `gh` /
`git` call is made **off the state lock**, is absent-tolerant, and degrades to a
default (empty) value; the gather step never panics and never blocks.

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EngineerAdmissionCtx {
    /// The goal about to be spawned, with its predicted file footprint.
    pub candidate: CandidateGoal,
    /// Every OTHER live engineer (the candidate's own goal is excluded — the
    /// same-goal case is already handled upstream by the lifecycle branch).
    pub live_engineers: Vec<LiveEngineerSignal>,
    /// Resolved target repo root (used for merge-base resolution + rendering).
    pub repo_root: String,
}
```

## `EngineerAdmissionDecision`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum EngineerAdmissionDecision {
    /// No blocking overlap — spawn now (the existing path, unchanged).
    Admit { rationale: String },
    /// A live engineer is touching files this goal needs — do NOT spawn this
    /// cycle. Retried naturally next OODA round. `blocked_by` names the goal(s)
    /// in the way; `retry_after_secs` is an optional advisory hint.
    Defer {
        blocked_by: Vec<String>,
        rationale: String,
        retry_after_secs: Option<u64>,
    },
    /// Spawn now, but instruct the engineer to rebase onto `after_goal_id`'s
    /// work before editing `overlap_files`. Advisory hint threaded into the
    /// engineer `task` string — no new machinery.
    SerializeAfter {
        after_goal_id: String,
        overlap_files: Vec<String>,
        rationale: String,
    },
}
```

The serde tag/rename convention (`choice`, snake_case) matches
`EngineerLifecycleDecision` and `GoalOutcomeDecision`. As with those, the recipe
does **not** emit the `choice`-tagged form directly — it emits a flat
`{"decision": "<snake_case_variant>", "rationale": "…", …}` envelope which a
dedicated mapper (`admission_decision_from_variant`, analogous to
`lifecycle_decision_from_variant` in `recipe_brain.rs`) translates into the
`choice`-tagged Rust enum.

> **`Defer` and `SerializeAfter` carry load-bearing extra fields.** The shared
> `DecisionEnvelope` shim reads only `decision` + `rationale`; every extra
> struct-variant field defaults to empty (the same constraint documented for
> `GoalOutcomeDecision::replan_hint`). Because `blocked_by`, `after_goal_id`, and
> `overlap_files` are load-bearing, the admission parser **must** read them
> explicitly (extend the envelope with `#[serde(default)]` fields or add an
> `AdmissionEnvelope`). If it follows the base shim unchanged, `blocked_by` is
> always empty and the `serialize_after` rebase hint never carries a target.

## `OodaBrain::decide_engineer_admission`

Added as a **defaulted** trait method so every existing `OodaBrain` impl and test
double compiles unchanged; the default is the fail-**open** `Admit` (scheduling is
an optimization — an un-migrated brain must never block a spawn).

```rust
pub trait OodaBrain: Send + Sync {
    // … existing methods …

    /// Decide whether to admit a NEW engineer for `ctx.candidate` right now,
    /// given the live engineer set and file-overlap signals (issue #2690).
    /// Scheduling optimization only — MUST fail OPEN. Default is `Admit` so
    /// unmigrated impls never stall the fleet; the production `RecipeBrain`
    /// overrides this to run the recipe.
    fn decide_engineer_admission(
        &self,
        _ctx: &EngineerAdmissionCtx,
    ) -> SimardResult<EngineerAdmissionDecision> {
        Ok(EngineerAdmissionDecision::Admit {
            rationale: "admission-scheduling not implemented by this brain".into(),
        })
    }
}
```

| Impl | Behavior |
| --- | --- |
| `RecipeBrain` | Loads `ooda-engineer-admission.yaml` (adapter tag `recipe-engineer-admission-brain`), renders the ctx, runs the recipe, parses the decision envelope, records a judgment. On any error → `engineer_admission_fallback(ctx)` returning `Admit`. |
| `DeterministicLifecycleBrain` (floor) | Inherits the defaulted method (`Admit`) — never blocks a spawn. |
| `RustyClawdBrain<S>` | Not migrated — inherits the defaulted method (`Admit`). |
| Test doubles (`StubAdmissionBrain`) | Return the injected decision (or `Err`) for hermetic tests. |

The three existing `OodaBrain` impls plus every test double compile unchanged
because the method is defaulted — verify this at implementation step 2.

## The seam and the two rails

The gate is `gather → reason → apply` invoked from `dispatch_spawn_engineer` in
[`spawn.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs),
positioned **after** the same-goal guards + depth guard + repo-resolve and
**before** worktree allocation. The gather step, the two rails, the fail-open
fallback, the kill-switch, and the observability recorder are realized in a
dedicated sibling module
[`admission.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/admission.rs)
(`run_admission_gate` → `gather_engineer_admission_ctx` → `decide_admission`),
keeping `spawn.rs`'s change a single call site. Only the exact-path rail can
override the brain:

```rust
// Reached only for a genuinely NEW engineer on a DIFFERENT goal.
let ctx = gather_engineer_admission_ctx(state_root, &goal, &repo_root); // off-lock, best-effort

// Rail 1 — exact-path (hard, deterministic). A CERTAIN collision: the candidate's
// non-empty scope is fully held by ONE live engineer. Defer regardless of brain.
let scope: BTreeSet<&str> = ctx.candidate.predicted_scope.iter().map(String::as_str).collect();
if !scope.is_empty()
    && ctx.live_engineers.iter().any(|e| {
        let held: BTreeSet<&str> = e.changed_files.iter().map(String::as_str).collect();
        scope.is_subset(&held)
    })
{
    push_brain_judgment(/* EngineerAdmission, fallback = true, "exact-path collision" */);
    tracing::warn!(target: "simard::ooda_brain", goal = %goal.id, "engineer-admission: exact-path rail — deferring certain collision");
    return make_outcome(action, true, "spawn deferred: exact-path collision with a live engineer"); // no worktree, no failure
}

// Rail 2 — reason (fail-OPEN on error). A brain Err admits, but LOUDLY.
let decision = match brain.decide_engineer_admission(&ctx) {
    Ok(d) => d,
    Err(e) => {
        tracing::warn!(target: "simard::ooda_brain", goal = %goal.id, error = %e,
            "decide_engineer_admission FAILED — failing OPEN to Admit (scheduling is an optimization, not a stall gate)");
        push_brain_judgment(/* EngineerAdmission, fallback = true, error */);
        engineer_admission_fallback(&ctx) // → Admit
    }
};

match decision {
    EngineerAdmissionDecision::Admit { .. } => { /* fall through to worktree alloc + spawn */ }
    EngineerAdmissionDecision::Defer { blocked_by, .. } => {
        push_brain_judgment(/* EngineerAdmission */);
        return make_outcome(action, true, format!("spawn deferred: overlaps live engineer(s) {blocked_by:?}")); // no worktree, no failure
    }
    EngineerAdmissionDecision::SerializeAfter { after_goal_id, overlap_files, .. } => {
        task = append_rebase_hint(task, &after_goal_id, &overlap_files); // task-string channel
        push_brain_judgment(/* EngineerAdmission */);
        /* fall through to worktree alloc + spawn */
    }
}
```

| Rail | Guard | On fire |
| --- | --- | --- |
| **1 — Exact-path (hard)** | Candidate scope non-empty **and** `⊆` a single live engineer's `changed_files` | Deterministic **`Defer`** (skip cycle, `success=true`, no worktree, no failure counted) + judgment (`fallback=true`) + `tracing`. Overrides the brain. Inert when scope is empty. |
| **2 — Fail-open (soft)** | `decide_engineer_admission` returns `Err` | **`Admit`** via `engineer_admission_fallback` + loud `tracing::warn` + judgment (`fallback=true`). Never stalls. |

`Admit` proceeds to the **existing** worktree allocation + `spawn_subordinate`
path, unchanged. `Defer` reuses the existing benign spawn-skip outcome shape
(`make_outcome(action, true, …)`) — no new `ActionOutcome` variant, and
`goal_failure_counts` is **never** incremented. `SerializeAfter` reuses the
engineer `task` string — no new channel.

> **Polarity vs. outcome-verify.** The
> [outcome verifier](outcome-verification-api.md#the-seam-and-the-three-rails) is
> fail-**closed** (a brain `Err` keeps a goal open). This gate is fail-**open** (a
> brain `Err` admits). Same seam shape, opposite polarity, because wrongly
> stalling a spawn is cheaper to recover from than wrongly archiving a goal. The
> one hard guarantee that survives a broken brain is Rail 1.

## Overlap detection (`overlap.rs`)

The new module
[`src/ooda_actions/advance_goal/overlap.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/overlap.rs)
supplies the *facts* only.

```rust
/// Files a live engineer is touching in its worktree: the committed diff since
/// the merge-base plus the uncommitted working-tree diff, unioned and
/// normalized to repo-relative POSIX paths.
///
/// Absent-tolerant: any git error (no repo, detached HEAD, missing base) yields
/// an EMPTY set — an empty set means "no overlap knowable" ⇒ admit (fail-open).
/// Never panics, never blocks, never shells out under the state lock.
pub fn changed_files(worktree: &Path, base_branch: &str) -> Vec<String> { /* … */ }

/// Intersection of a candidate's predicted scope with an engineer's changed
/// files (repo-relative POSIX exact match). Non-empty ⇒ overlap.
pub fn overlap(candidate_scope: &[String], engineer_changed: &[String]) -> Vec<String> { /* … */ }
```

Base-branch resolution reuses the existing repo-default helper (the ancestry
helper style already in
[`src/overseer/deploy.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy.rs)).
`changed_files` runs `git -C <worktree> diff --name-only <merge-base>...HEAD` plus
`git -C <worktree> diff --name-only`.

## `LiveEngineerWorktree` addition

Overlap detection reuses the existing live-engineer enumeration rather than
inventing a new one. `LiveEngineerWorktree` in
[`discovery.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_worktree/discovery.rs)
gains **one additive field**:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEngineerWorktree {
    pub goal_id: String,
    pub pid: i32,
    /// NEW: absolute worktree path (populated from the dir `live_claimed_engineers`
    /// already scans) so callers can compute the engineer's changed-file set.
    pub worktree_path: PathBuf,
}
```

`live_claimed_engineers(state_root)` keeps its signature and claim-liveness
contract; it merely populates the new field from the `path` it already walks.
Because the change is a **field addition**, the dashboard "Active Engineers"
gauge and the existing `discovery` tests compile unchanged.

## The reasoning recipe

`prompt_assets/simard/recipes/ooda-engineer-admission.yaml` follows the
[`ooda-engineer-lifecycle.yaml`](ooda-engineer-lifecycle-recipe.md) shape: role →
overlap-context vars → snake_case `choice` options → JSON envelope → few-shot
examples. It is installed to the hot-reload path
`~/.simard/prompt_assets/simard/recipes/…` for production; tests use a
`home_override` tempdir. The full schema is documented in the
[recipe reference](ooda-engineer-admission-recipe.md).

Context vars (passed via `-c`, each rendered through
[`sanitize_context_var`](#sanitization-boundary)):

| Var | Meaning |
| --- | --- |
| `candidate_goal_id`, `candidate_goal_title` | The goal about to spawn. |
| `candidate_predicted_scope` | Rendered list of predicted target paths. |
| `live_engineers` | Rendered per-engineer block: `goal_id`, `changed_files`, `overlap_with_candidate`, `depended_on`. |
| `repo_root` | Resolved target repo. |

Output envelope (a fenced ```json block is fine; the shim strips banners):

```json
{"decision": "defer", "blocked_by": ["fix-goals-status-render"], "rationale": "live engineer already rewriting src/operator_commands_ooda/goals_status.rs, the file this goal must edit"}
```

where `decision` is exactly one of `admit`, `defer`, `serialize_after`. The
recipe's few-shot set **includes the goals_status.rs and Adapter-rename cases** so
the reasoning is anchored on the exact collisions this gate exists to catch. A
genuine "these are independent, parallelize" answer is a real decision — emit
`admit` explicitly; an unparseable output is a `brain_parse_error` that the
daemon fails **open** on (Rail 2), audited.

> **The recipe never carries the hard invariant.** The recipe is hot-reloadable
> and user-writable. The load-bearing certain-collision control (the `is_subset`
> exact-path rail) lives in Rust, so editing the prompt can change *scheduling
> quality* but can never make the daemon start a second engineer on top of one
> that already holds the exact target paths.

## Sanitization boundary

Every ctx field is rendered to a string and routed through the existing
[`sanitize_context_var`](recipe-context-var-sanitization.md) before it becomes a
recipe `-c` arg — goal ids, titles, and every path list. Because the recipe
delimits `candidate_goal_title` and file lists as **untrusted** input, and
because Rail 1 (not the prompt) is the hard decider, injection in a goal title or
path cannot forge or suppress a collision block.

- **Caps:** `candidate_goal_title ≤ 2000` chars; ids/paths ≤ 500 each.
- **Strip:** control characters and ANSI escape sequences.
- **Bound:** `live_engineers.len() ≤ 32` and `changed_files.len() ≤ 200` per
  engineer (prompt-cost DoS guard); overflow is truncated with a marker.

## Observability

| Surface | What it records |
| --- | --- |
| `BrainJudgmentRecord` (phase `BrainPhase::EngineerAdmission`) | Built by `BrainJudgmentRecord::from_engineer_admission(...)`, pushed via `push_brain_judgment`; carries the decision label, the overlapping goal ids, and the scrubbed rationale. Serialises as `"engineer_admission"`. Deterministic (Rail 1) and fail-open (Rail 2) blocks set `fallback = true`. |
| `engineer_admission_decision` metric | Appended to `metrics.jsonl` via [`self_metrics::record_metric`](telemetry-metrics.md); context carries the decision, the `blocked_by` / `after_goal_id`, and the overlap reasoning. |

Both persist **bounded, sanitized** summaries only. A test asserts a judgment
record is pushed on the decision path (see the [test matrix](#test-matrix)).

## Kill-switch

```text
SIMARD_ENGINEER_ADMISSION=off
```

Secure default is scheduling **ON**. Only the explicit documented value `off`
(case-insensitive) disables the gate — the seam skips gather/reason/rails and
admits every candidate (today's collision-blind spawn). Any unknown value **keeps
scheduling enabled**. Because the gate is already fail-open, the kill-switch is an
incident lever, not a safety necessity. See the
[engineer-admission kill-switch operations page](../operations/engineer-admission-kill-switch.md).

## Test matrix

All hermetic — stub brain + injected live-engineer signals; `gh`/`git`
absent-tolerant so tests never shell out.

| # | Scenario | Expected |
| --- | --- | --- |
| T1 | Seam: stub `Defer` | No worktree allocated; benign skip outcome (`success=true`); `goal_failure_counts` unchanged. |
| T2 | Seam: stub `Admit` | Spawn proceeds to worktree allocation + `spawn_subordinate`. |
| T3 | Seam: stub `SerializeAfter` | Spawn proceeds; the engineer `task` carries the rebase-after hint. |
| T4 | Hard rail (same goal) | Existing `find_live_engineer_for_goal` branch blocks regardless — reached before the gate (retained, unchanged). |
| T5 | Hard rail (exact-path) | Different-goal live engineer whose `changed_files` cover the candidate's scope ⇒ deterministic `Defer` **regardless of the stub's choice** (even stub `Admit`). |
| T6 | Fail-open (brain `Err`) | Visible `Admit` (loud `tracing::warn` + fallback judgment) — never a silent block; no `Err` propagated as a stall. |
| T7 | Empty scope | Unknown candidate scope ⇒ exact-path rail inert ⇒ brain decides (stub honored). |
| T8 | Observability | `push_brain_judgment` records `EngineerAdmission`; `engineer_admission_decision` metric carries the overlap reasoning. |
| T9 | Overlap unit (`overlap.rs`) | Two fake worktrees with known diffs → correct `changed_files`; erroring/empty git → empty set (fail-open). |
| T10 | Kill-switch | `SIMARD_ENGINEER_ADMISSION=off` ⇒ gate skipped, every candidate admitted; no judgment/metric emitted. |
| T-sec1 | Injection/newline/ANSI in `candidate_goal_title` or a path | Neutralized by `sanitize_context_var`. |
| T-sec2 | Prompt says `admit` but candidate scope ⊆ a live engineer | Rail 1 blocks the spawn (prompt cannot override the hard rail). |

## See also

- [Dependency/overlap-aware engineer scheduling (concept)](../concepts/dependency-overlap-aware-scheduling.md)
- [OODA engineer-admission recipe & prompt schema](ooda-engineer-admission-recipe.md)
- [How to diagnose a deferred engineer spawn](../howto/diagnose-a-deferred-engineer-spawn.md)
- [Engineer-admission kill-switch](../operations/engineer-admission-kill-switch.md)
- [Concurrent engineer dispatch](concurrent-engineer-dispatch.md) — the per-round dispatcher whose spawn path this gate guards.
- [Outcome-verification API](outcome-verification-api.md) — the sibling brain seam at the completion moment (fail-closed, opposite polarity).
- [Recipe context variable sanitization](recipe-context-var-sanitization.md) — the `sanitize_context_var` boundary.
