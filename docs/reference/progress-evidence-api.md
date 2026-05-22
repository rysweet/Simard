# Reference: Progress-evidence API

Crate: `simard` · Module: `simard::goal_curation::progress_evidence`

This module implements the gatekeeper described in
[Progress-evidence gating](../concepts/progress-evidence-gating.md). It
exposes one trait (`ProgressEvidenceChecker`), two concrete implementations
(`LlmReviewerProgressChecker` in
`simard::goal_curation::progress_reviewer`, `NoopProgressEvidenceChecker`),
and a single façade function (`update_goal_progress_with_evidence`) in the
sibling `simard::goal_curation::operations` module.

> **History:** Prior to PR #2007, the production implementation was
> `DefaultProgressEvidenceChecker`, which shelled out to `git log` and
> `gh pr list`. That struct and its helper traits (`GitRunner`, `GhRunner`)
> were removed in PR #2011. The gate now delegates to an LLM reviewer —
> no subprocess calls, no local tooling requirements.

All public symbols below are re-exported from `simard::goal_curation`.

---

## `EvidenceDecision`

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum EvidenceDecision {
    /// Evidence found — the caller may apply the progress update.
    Accept { reason: String },
    /// No evidence — the caller must keep the prior percent and emit
    /// a hallucination audit episode.
    Reject { reason: String },
}
```

The `reason` string in both variants is human-readable, ASCII-safe, and
suitable for inclusion in cognitive-memory episodes verbatim.

---

## `ProgressEvidenceChecker`

```rust
pub trait ProgressEvidenceChecker: Send + Sync {
    fn check(
        &self,
        goal: &ActiveGoal,
        old_percent: u32,
        new_percent: u32,
        since: DateTime<Utc>,
    ) -> EvidenceDecision;
}
```

The trait is `Send + Sync` so a single `Arc<dyn ProgressEvidenceChecker>`
can be installed on `OodaBridges` and shared across all OODA actions.

### Contract

- `check` MUST NOT mutate the goal board, cognitive memory, or any other
  daemon state. It is a read-only decision function.
- `check` MAY perform blocking I/O (LLM calls). It is called at most
  a few times per OODA cycle, only on progress-increase attempts.
- `check` MUST return `Accept` when evidence supports the claim and
  `Reject` otherwise. The production `LlmReviewerProgressChecker`
  accepts on LLM infrastructure failure (transport error, parse error,
  empty response) — the gate's purpose is to catch hallucinated jumps,
  not to block goals on LLM availability. See
  [`SIMARD_PROGRESS_EVIDENCE`](../operations/progress-evidence-kill-switch.md)
  for the operator escape hatch.
- The `since` argument is provided by the caller; the trait does not
  source it (the LLM reviewer ignores it — decisions are prompt-based).

### Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `goal` | `&ActiveGoal` | The goal whose progress is being claimed. Used for `goal.id` (engineer-branch slug) and `goal.wip_refs` (issue/PR cross-reference). |
| `old_percent` | `u32` | The current percent (0–100). |
| `new_percent` | `u32` | The proposed new percent (0–100). Only called when `new_percent > old_percent`. |
| `since` | `DateTime<Utc>` | The cutoff timestamp; only artifacts at or after this instant count as evidence. |

### Decision rules (LlmReviewerProgressChecker)

The production implementation (`src/goal_curation/progress_reviewer.rs`)
sends goal context to an LLM and parses a `{verdict, rationale}` JSON
response. No git introspection, no `gh` shellouts — the LLM reads the
problem, plan, and progress against the plan to determine whether the
claimed percent is honest.

| Condition | Result | `reason` template |
|---|---|---|
| `new_percent <= old_percent` | `Accept` (auto, no LLM call) | `"progress-assessment-reviewer: downward / no-change (<old> -> <new>) auto-accepted"` |
| LLM returns `{"verdict":"accept","rationale":"..."}` | `Accept` | `"progress-assessment-reviewer: accept — <rationale>"` |
| LLM returns `{"verdict":"reject","rationale":"..."}` | `Reject` | `"progress-assessment-reviewer: reject — <rationale>"` |
| LLM transport/parse error or empty response | `Accept` (fail-open) | `"progress-assessment-reviewer: LLM submit failed (<error>); accepting to avoid blocking goal"` |
| Unknown verdict string | `Accept` (fail-open) | `"progress-assessment-reviewer: unknown verdict \"<v>\"; accepting to avoid blocking goal"` |

The prompt template lives at
`prompt_assets/simard/progress_assessment_reviewer.md`. It substitutes
`{goal_id}`, `{problem}`, `{plan}`, `{prior_pct}`, `{claimed_pct}`, and
`{wip_summary}` before submission.

---

## `LlmReviewerProgressChecker`

```rust
// In src/goal_curation/progress_reviewer.rs

pub struct LlmReviewerProgressChecker<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> LlmReviewerProgressChecker<S> {
    pub fn new(submitter: S) -> Self;
}
```

The production checker. Generic over `LlmSubmitter` so tests can swap in a
canned-response stub without global state. Constructed at daemon startup
with the daemon's active `LlmSubmitter` implementation.

The checker:

1. Auto-accepts downward/no-change moves without an LLM call.
2. Renders the prompt template with goal context.
3. Submits to the LLM via `submitter.submit(...)`.
4. Parses the response JSON (tries: raw, fenced code blocks, brace-balanced
   spans, outermost-brace fallback — same strategy as `merge_judge`).
5. Maps `"accept"` / `"reject"` verdicts to `EvidenceDecision`.
6. Fails open on any infrastructure error (LLM down, parse failure, unknown
   verdict).

---

## `NoopProgressEvidenceChecker`

```rust
pub struct NoopProgressEvidenceChecker;

impl ProgressEvidenceChecker for NoopProgressEvidenceChecker {
    fn check(&self, _: &ActiveGoal, _: u32, _: u32, _: DateTime<Utc>)
        -> EvidenceDecision
    { /* always Accept */ }
}
```

Always returns `Accept { reason: "noop checker (no evidence enforced)" }`.
Used in two places:

1. **Tests.** Default test-helper `OodaBridges::for_tests()` installs this
   so existing tests do not need to provide an `LlmSubmitter` implementation.
2. **Operator escape hatch.** Selected at daemon boot when
   `SIMARD_PROGRESS_EVIDENCE=off`. See
   [the kill-switch operations doc](../operations/progress-evidence-kill-switch.md).

---

## `update_goal_progress_with_evidence` (façade)

Located in `src/goal_curation/operations.rs`.

```rust
pub fn update_goal_progress_with_evidence(
    board:   &mut GoalBoard,
    goal_id: &str,
    proposed: GoalProgress,
    checker: &dyn ProgressEvidenceChecker,
    memory:  &dyn crate::cognitive_memory::CognitiveMemoryOps,
    now:     DateTime<Utc>,
) -> SimardResult<EvidenceDecision>;
```

### Behavior

1. Look up the goal on `board`. Map current and proposed status to
   `(old_percent, new_percent)`:

    | `GoalProgress` variant | Percent |
    |---|---|
    | `NotStarted` | `0` |
    | `InProgress { percent }` | `percent` |
    | `Blocked(_)` | the goal's *current* percent (no change) |
    | `Completed` | `100` |

2. Determine `since` via the [three-step fallback chain](../concepts/progress-evidence-gating.md#sourcing-since--the-last-update-timestamp).

3. **Bypass set.** If any of the following hold, call the underlying
   `update_goal_progress` directly and return
   `Accept { reason: "bypass: non-increase" }` (or `"bypass: <variant>"`)
   **without** emitting a memory episode:

   - `proposed` is `Blocked(_)`
   - `proposed` is `NotStarted`
   - `new_percent <= old_percent`

4. **Otherwise** call `checker.check(...)`:

    - On `Accept`:
      - Call `update_goal_progress(board, goal_id, proposed)`.
      - Set `goal.last_progress_update_at = Some(now)`.
      - Emit one episode:
        ```
        goal progress accepted: <old>%-><new>% on <goal-id>
          -- evidence: <checker reason>
        ```
        importance `0.4`.
      - Return `Ok(Accept { reason })`.
    - On `Reject`:
      - Do **not** mutate the board.
      - Emit one episode:
        ```
        brain hallucination detected: rejected progress <old>%-><new>% on <goal-id>
          -- reviewer rationale: <checker reason>
        ```
        importance `0.7`.
      - Return `Ok(Reject { reason })`. **This is not an error.** The
        caller treats it as informational and proceeds without a percent
        bump.

`SimardResult::Err` is returned only for genuine failures: the goal id is
not on the board, the underlying `update_goal_progress` writer fails, or
the memory store fails to record an audit episode.

### Calling convention

The façade is invoked from four production sites. A fifth historical
caller — `subordinate.rs:262`, which writes `Blocked(reason)` — stays a
direct caller of `update_goal_progress` because `Blocked` is in the
bypass set (it does not increase the percent).

| Caller | Bypass path expected | Notes |
|---|---|---|
| `ooda_actions::goal_session::advance::assess_only_outcome` | Sometimes | Bumps come from brain text — exactly the case the gate targets. |
| `ooda_actions::goal_session::advance` pre-spawn site | Sometimes | Same as above. |
| `ooda_actions::advance_goal::subordinate` heartbeat (50%) | Sometimes | Engineer alive ≠ evidence. |
| `ooda_actions::advance_goal::subordinate` Completed | Always Accept (rule 1) | Routed for audit, never rejected in practice. |

### Error mapping for the OODA layer

Both `Accept` and `Reject` are returned as `Ok(...)`. Callers in
`ooda_actions` distinguish them like this:

```rust
match update_goal_progress_with_evidence(
    board, goal_id, new_progress,
    &*bridges.progress_evidence, &*bridges.memory, Utc::now(),
)? {
    EvidenceDecision::Accept { .. } => { /* happy path */ }
    EvidenceDecision::Reject { reason } => {
        return make_outcome(
            action,
            true,
            format!("no-action: progress claim rejected (no evidence): {reason}"),
        );
    }
}
```

`Reject` is **not** treated as a cycle failure: the OODA loop continues,
the rejection is observable via cognitive memory, and the percent stays
where it was.

---

## `OodaBridges` extension

`src/ooda_loop/types.rs` adds two fields:

```rust
pub struct OodaBridges {
    // ... existing fields ...
    pub repo_root: std::path::PathBuf,
    pub progress_evidence: std::sync::Arc<
        dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker
    >,
}
```

| Field | Default at daemon boot | Default in tests |
|---|---|---|
| `progress_evidence` | `Arc::new(LlmReviewerProgressChecker::new(submitter))`, or `Arc::new(NoopProgressEvidenceChecker)` when `SIMARD_PROGRESS_EVIDENCE=off` | `Arc::new(NoopProgressEvidenceChecker)` |

A new `OodaBridges::for_tests()` constructor wires the test defaults so
that existing OODA-loop tests need only a single-line change to adopt the
new fields.

---

## `ActiveGoal` schema extension

`src/goal_curation/types.rs`:

```rust
pub struct ActiveGoal {
    // ... existing fields ...

    /// Wall-clock timestamp of the last accepted progress update.
    /// `None` for goals created before #1967; the gate falls back
    /// to a memory scan, then to daemon process-start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_update_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

The `#[serde(default, skip_serializing_if = "Option::is_none")]` attribute
combination preserves both forward and backward compatibility:

- Older JSON files load with `last_progress_update_at = None`.
- Goals that have never reached the gate (e.g. pure `Blocked` history)
  continue to serialize without the field, keeping snapshots minimal.

No data migration is required.

---

## Stability

| Item | Stability |
|---|---|
| `EvidenceDecision`, `ProgressEvidenceChecker` | Public stable — semver-tracked. |
| `update_goal_progress_with_evidence` | Public stable. |
| `LlmReviewerProgressChecker` | Public stable — generic over `LlmSubmitter`. |
| `NoopProgressEvidenceChecker` | Public stable; safe to use in any test. |
| Episode prefix strings (`"goal progress accepted:"`, `"brain hallucination detected:"`) | **Behaviorally stable.** The dashboard and consolidation jobs match these prefixes verbatim; changing them is a breaking change. |
| Prompt template (`progress_assessment_reviewer.md`) | Implementation detail; may evolve. |

---

## See also

- [Progress-evidence gating (concept)](../concepts/progress-evidence-gating.md)
- [Diagnose rejected progress claims (how-to)](../howto/diagnose-rejected-progress-claims.md)
- [`SIMARD_PROGRESS_EVIDENCE` kill switch (operations)](../operations/progress-evidence-kill-switch.md)
- [Goal board API](goal-board-api.md)
- [Goal board corruption guard API](goal-board-corruption-guard-api.md)
- [Cognitive memory bridge helpers](cognitive-memory-bridge-helpers.md)
