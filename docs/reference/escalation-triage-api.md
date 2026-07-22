---
title: Escalation-triage API reference
description: >
  Reference for the blocked-goal escalation-triage feature — the
  escalation_triage.md reasoning contract and its output schema, the
  goal_board_store::mutate correction closure (rewrite done-criteria, set
  assigned_to, Blocked->NotStarted), the done-gate the completion gate certifies
  from existing PR/issue evidence (a numeric coverage threshold would need new
  plumbing — see the design note), the three input-validation helpers (module,
  threshold, owner), and the signal_conversation::channel::notify translation
  table + forbidden-token denylist. Covers the decision/escalate invariant, the
  fail-closed rules, and the thin act_escalate_blocked_goal trigger that stays
  read-only.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/blocked-goal-escalation-triage.md
  - ../reference/completion-evidence-gate-api.md
  - ../reference/overseer-operator-notifications.md
  - ../reference/goal-board-api.md
  - ../../prompt_assets/simard/overseer/escalation_triage.md
  - ../../src/goal_board_store/mod.rs
  - ../../src/goal_curation/completion_gate.rs
  - ../../src/goal_curation/types.rs
  - ../../src/signal_conversation/channel.rs
  - ../../src/signal_conversation/operator_safe.rs
  - ../../src/overseer/triage.rs
---

# Escalation-triage API reference

> **Status: implemented (issue #4419).** The reasoning contract lives in
> [`prompt_assets/simard/overseer/escalation_triage.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md).
> The correction is applied through
> [`goal_board_store::mutate`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs),
> certified by the existing completion gate in
> [`goal_curation::completion_gate`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
> from a tracked PR/issue (a numeric coverage threshold would need new plumbing —
> see the design note),
> and narrated through
> [`signal_conversation::channel::notify`](https://github.com/rysweet/Simard/blob/main/src/signal_conversation/channel.rs).
> The thin trigger
> [`overseer::act_escalate_blocked_goal`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> and `RECURRENCE_ESCALATION_THRESHOLD` are **read-only** — they launch triage
> but never own the decision. Conceptual overview:
> [Triage & course-correct a blocked goal](../concepts/blocked-goal-escalation-triage.md).

## Inputs (from the thin trigger)

`act_escalate_blocked_goal` builds a `RecipeBrief` whose task description carries
the structured context and points the agent at the recipe. The recipe receives:

```json
{
  "goal_id":      "audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a",
  "problem_seed": "plain-English problem seed (refine it)",
  "next_step_seed": "plain-English next-step seed (refine it)",
  "internal_why": "opaque typed blocker; no measurable done-gate; demoted after 5 no-progress cycles; unassigned",
  "reason_marker": "health-review:blocked-goal"
}
```

`internal_why` and `reason_marker` are **evidence to translate**, never text to
forward verbatim.

## Output schema

The brain returns exactly six fields:

| Field | Type | Contract |
| --- | --- | --- |
| `problem` | string | Plain English. No jargon, no marker tokens. |
| `next_step` | string | Plain English; smallest clear unblocking action. |
| `root_cause` | string | 1–2 sentences, grounded in the evidence. |
| `decision` | enum | `rewrite-done-gate` \| `complete-delivered-goal` \| `ask-operator-one-question` |
| `action_taken` | string | What was actually done, or the single operator question. |
| `escalate` | string \| null | Reason a human is required, or `null`. |

**Invariant (validated before emit):** `escalate` is non-null **iff**
`decision == "ask-operator-one-question"`. For `rewrite-done-gate` and
`complete-delivered-goal`, `escalate` is `null`. Enforced by
[`overseer::triage::validate_triage_escalation(CourseCorrection, Option<&str>)`](https://github.com/rysweet/Simard/blob/main/src/overseer/triage.rs),
where `CourseCorrection::requires_human_escalation()` is `true` only for
`AskOperatorOneQuestion`; a violation returns `TriageInvariantError`.

## Correction: `goal_board_store::rewrite_blocked_goal_done_gate`

The `rewrite-done-gate` correction is applied by a single public helper that wraps
one `goal_board_store::mutate` closure, so the entire read-modify-write is atomic
under `flock` (no TOCTOU, no lost update against a concurrent daemon/CLI writer):

```rust
pub fn rewrite_blocked_goal_done_gate(
    state_root: &Path,
    goal_id: &str,
    target: &FirstSliceTarget,
) -> SimardResult<CorrectionOutcome>;
```

`FirstSliceTarget::new(module_path, threshold_percent, owner, tracking_ref)`
validates every field up front (see *Input validation* below) and returns
`Err(CorrectionRejected)` on the first malformed input, so a bad target can never
reach the board. Inside the one `mutate` window the helper:

1. rewrites the unmeasurable done-criteria (`ActiveGoal::description` — there is no
   separate `success_criteria` field; the verify layer clones `description`) into a
   concrete, bounded, per-module slice;
2. attaches the observable `tracking_ref` (idempotently) so the completion gate has
   a signal it can read;
3. assigns `assigned_to = Some(owner)` so the goal has a responsible engineer; and
4. transitions `Blocked(..) -> NotStarted` so the goal re-enters the active list.

The return value is a `CorrectionOutcome`:

```rust
pub enum CorrectionOutcome {
    Corrected(ActiveGoal),        // the goal exactly as persisted
    Rejected(CorrectionRejected), // board left untouched
}

pub enum CorrectionRejected {
    GoalNotFound { goal_id },
    NotBlocked { goal_id, status },      // no block to course-correct
    ThresholdOutOfRange { got },
    UnsafeModulePath { path },
    InvalidOwner { owner },
}
```

An unknown goal id (`GoalNotFound`) or a goal that is not `Blocked` (`NotBlocked`)
is **rejected without mutating the board** — never fabricated, never a silent
fallback. Do **not** reuse `ActiveGoal::roll_to_new_cycle` here: it also resets to
`NotStarted` but **clears** `assigned_to` and `wip_refs`, which would wipe the owner
and the tracking ref this correction just set.

**Constraints:**

- Only **pre-existing** `ActiveGoal` fields are written (`description`,
  `assigned_to`, `status`, `wip_refs`) — no serde-shape change, so old
  `goal_board.json` snapshots stay byte-compatible.
- The transition is exactly `Blocked(..) -> NotStarted`; no other status edge.
- State read outside the lock is never persisted; the whole edit lives in the
  closure.

`ActiveGoal`, `GoalProgress`, `WipRef`, and `GoalBoard` are defined in
[`src/goal_curation/types.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs).

## Done-gate: how the rewritten slice is certified

The rewritten done-criteria are certified by the **existing** completion gate —
no new `MissingEvidence` variant and no serde change — using the evidence it
already understands:

- **Merged PR / closed issue.** A PR in the goal's `wip_refs` (or referencing its
  issue) observed `MERGED`, or the linked issue observed `CLOSED`. Modeled by
  `CompletionEvidence { pr_merged, issue_closed, .. }` and made verifiable by
  [`has_derivable_signal`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs),
  which returns `true` only when the goal carries a `pr` / `issue` wip-ref or is
  self-affecting. The intent-revealing counterpart
  [`done_gate_is_machine_checkable`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
  (same rule) is the predicate the triage correction uses to prove a rewritten
  done-gate is actually evaluable before it persists the rewrite.

So `rewrite-done-gate` MUST attach such a signal: the measurable slice is landed
through a tracked PR (or issue) that `has_derivable_signal` recognizes. Without a
`pr` / `issue` wip-ref (or self-affecting status), a `Blocked` verdict means
"nothing to verify", not "done".

```text
has_derivable_signal(goal) == true   // goal has a pr/issue wip-ref, or is self-affecting
CompletionVerdict::Complete(..)      // slice certified (e.g. pr_merged)
CompletionVerdict::Blocked { .. }    // slice not yet met: keep goal active
```

> **Design note — a numeric coverage threshold is NOT certifiable by the existing
> gate.** The spec asks to certify done from a *per-file line-% in
> `coverage-summary.json`* while *reusing* `CompletionEvidence` +
> `has_derivable_signal` with *no new variant*. These conflict:
> `has_derivable_signal` inspects only `wip_refs` kinds and self-affecting status
> (`completion_gate.rs:157`), and `CompletionEvidence` is boolean-only
> (`pr_merged`, `issue_closed`, `self_affecting`, `deployed`) — there is no
> numeric field a line-% could occupy. A coverage threshold therefore needs **new
> evidence plumbing** (a new evidence field + a new `has_derivable_signal`
> branch), which is not additive and does change the gate's serde shape. Choose
> explicitly: **(a)** certify via a tracked PR/issue with the existing boolean
> evidence (recommended, additive), or **(b)** add a first-class numeric-coverage
> evidence path and accept the variant/serde change.

A coverage command, if run, uses an **argv array** (never `format!` into a shell
string) and treats the criteria text as inert data.

## Input validation (three helpers, fail closed)

Before any state is persisted, the three correction inputs are validated by
`FirstSliceTarget::new`, which calls `validate_threshold`, `validate_module_path`,
and `validate_owner` in that fixed order. Any failure returns the corresponding
`CorrectionRejected` and diverts the triage brain to the
`ask-operator-one-question` path rather than persisting a bad gate:

| Input | Rule |
| --- | --- |
| **module path** | Validated by **form**, not filesystem existence (pure, cwd-independent): must be non-empty and repo-relative, character set limited to `[A-Za-z0-9_./-]`, and reject `..` traversal, absolute (`/…`) paths, and shell/glob metacharacters and control chars. |
| **threshold** | Numeric `u32`, `0 <= N <= 100`. Rejects unsatisfiable (`> 100`) gates. |
| **owner** | Bounded plain identifier (≤ 128 bytes). Rejects empty and any control char / newline (log- and Signal-injection safe). |

`assigned_to` is **attribution only** — it is never an authorization or
capability grant downstream.

## Operator Signal: translation table + denylist

Every operator-facing payload is checked by
[`signal_conversation::operator_safe::ensure_operator_safe(message: &str) ->
Result<(), OperatorMessageRejected>`](https://github.com/rysweet/Simard/blob/main/src/signal_conversation/operator_safe.rs)
**before** it is sent through `signal_conversation::channel`. The triage brain
emits three async plain-English messages (problem restatement, next step, action
taken); each is pre-checked and **fails closed** — an ambiguous or marker-bearing
message is rejected (never silently scrubbed). The check rejects any newline /
control character (Signal payloads are single-line) and any token in
`OPERATOR_FORBIDDEN_MARKERS`, matched case-insensitively:

```text
Denylisted (never reach the operator):
  OODA-SAFEGUARD   UNCLEAR-CRITERIA   GENUINELY-STUCK
  evidence=[       why=               🔒 (lock token)
  health-review:blocked-goal
```

Translation table:

| Internal marker | Operator-facing text |
| --- | --- |
| opaque typed blocker / no measurable done-gate | "Simard can't automatically tell when this goal is finished, so it keeps re-checking without shipping." |
| demoted after 5 no-progress cycles | "It stalled repeatedly and dropped off the active list." |
| no engineer assigned | "No one is currently responsible for moving it." |

Signal payloads are single-line; `notify` is presentation-only and never a state
authority.

## The thin trigger (read-only)

`overseer::act_escalate_blocked_goal(goal_id, reason, why, problem, next_step,
link)` remains a structured launcher: it builds the `RecipeBrief`, derives a
stable per-goal `recipe_dedup_key`, suppresses a second in-flight triage for the
same goal, and launches `escalation_triage.md` through the `RecipeLauncher` seam.
It **does not** decide the outcome. `RECURRENCE_ESCALATION_THRESHOLD` (in
`src/overseer/root_cause.rs`, re-exported from `src/overseer/mod.rs`) still gates
*when* triage fires but not *what* triage does. Neither is modified by this
feature.

## Configuration

| Setting | Purpose | Default |
| --- | --- | --- |
| `RECURRENCE_ESCALATION_THRESHOLD` | No-progress cycles before triage launches (read-only) | unchanged |
| `NODE_OPTIONS=--max-old-space-size=32768` | Saved memory preference for the agent runtime (change via `~/.amplihack/config`) | 32768 MB |

No new kill-switch is introduced; the correction path reuses the existing
completion-gate and goal-board seams.

## Error handling & security

- **Fail loud:** every fallible step propagates `SimardResult` with `?`;
  structured `tracing` + OTel only. No stray `print!`, no silent fallback, no
  `Bridge` naming.
- **Command/shell injection:** module path is allowlisted; any coverage command
  uses argv arrays; criteria text is inert data.
- **Log/Signal injection:** owner and module reject control chars / newlines;
  Signal payloads are single-line; the `operator_safe::ensure_operator_safe`
  denylist runs, fail-closed, before every send.
- **Snapshot compatibility:** only pre-existing fields are mutated; no serde
  shape change.
- **Data protection:** no secrets / PII / tokens in `description`, `assigned_to`,
  or Signal text. The state file's pre-existing world-readable `0o644` mode is out
  of scope and not relied upon for privacy.

## Contract tests

Located in
[`src/overseer/tests_escalation_triage.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_escalation_triage.rs)
plus inline `#[cfg(test)]` modules in `completion_gate.rs` and
`goal_board_store/mod.rs`:

- **Translation-not-passthrough:** no forbidden token appears in any Signal
  payload for the coverage-goal fixture.
- **Decision/escalate mutual exclusion:** `escalate` is non-null iff
  `decision == ask-operator-one-question`.
- **Threshold bounds:** thresholds outside `0..=100` divert to the operator
  question path.
- **Transition:** `Blocked(..) -> NotStarted` after a successful rewrite.

## See also

- [Triage & course-correct a blocked goal](../concepts/blocked-goal-escalation-triage.md)
- [Completion-evidence gate API](../reference/completion-evidence-gate-api.md)
- [Overseer operator notifications](../reference/overseer-operator-notifications.md)
- [Goal-board API](../reference/goal-board-api.md)
