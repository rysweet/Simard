---
title: Triage & course-correct a blocked goal before escalating to a human
description: >
  Why the Simard Overseer now TRIAGES a genuinely-blocked goal agentically —
  restating the machine markers in plain English, finding a root cause, and
  self-correcting the block — before it ever surfaces a person. The
  escalation_triage.md recipe owns the escalate-vs-course-correct DECISION and,
  when a goal's finish-line is unmeasurable, rewrites its done-criteria into a
  machine-checkable first slice (one under-tested module landed via a tracked
  PR/issue the completion gate can verify), assigns an owner, and returns the
  goal to the active list — all under one flock-guarded goal_board_store::mutate
  window. Every operator update is jargon-free: raw markers
  (OODA-SAFEGUARD, UNCLEAR-CRITERIA, GENUINELY-STUCK, why=, evidence=[…], the
  lock token, health-review:blocked-goal) are translated and never surfaced.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: concept
status: partially implemented
related:
  - ./blocked-goal-escalation-backoff.md
  - ./overseer-agentic-health-review.md
  - ./overseer-root-cause-why.md
  - ./deploy-aware-done-gate.md
  - ./self-diagnose-on-step-error.md
  - ../reference/escalation-triage-api.md
  - ../reference/completion-evidence-gate-api.md
---

# Triage & course-correct a blocked goal before escalating

> **Status: partially implemented (issue #4419).** When the Overseer decides a
> goal is genuinely blocked, it no longer ships a raw machine marker to a human
> and counts that as "handled". It first INSPECTS the block, restates it in plain
> English, attempts a root cause, and COURSE-CORRECTS it agentically — only
> escalating a person when a human decision is genuinely required. The reasoning
> lives in the recipe asset
> [`prompt_assets/simard/overseer/escalation_triage.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md);
> the thin Rust trigger
> [`overseer::act_escalate_blocked_goal`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
> launches it today. The validated Rust primitives that apply a course-correction
> — [`goal_board_store::rewrite_blocked_goal_done_gate`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs)
> and its fail-closed field validators, the
> [`overseer::triage`](https://github.com/rysweet/Simard/blob/main/src/overseer/triage.rs)
> escalate-vs-course-correct invariant, and the
> [`signal_conversation::operator_safe`](https://github.com/rysweet/Simard/blob/main/src/signal_conversation/operator_safe.rs)
> guard — ship tested and ready, but are **not yet wired into the live
> escalation path**; connecting them to their call sites is tracked by
> [#4427](https://github.com/rysweet/Simard/issues/4427). Once wired, state edits
> land through
> [`goal_board_store::mutate`](https://github.com/rysweet/Simard/blob/main/src/goal_board_store/mod.rs),
> the rewritten slice is certified by the existing
> [`goal_curation::completion_gate`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
> from a tracked PR/issue (a numeric coverage threshold would need new plumbing —
> see the [design note](#design-note-numeric-coverage-vs-existing-evidence)),
> and every operator update through
> [`signal_conversation::channel::notify`](https://github.com/rysweet/Simard/blob/main/src/signal_conversation/channel.rs),
> each guarded fail-closed by
> [`signal_conversation::operator_safe::ensure_operator_safe`](https://github.com/rysweet/Simard/blob/main/src/signal_conversation/operator_safe.rs)
> so no raw marker can leak.
> API details: [Escalation-triage API reference](../reference/escalation-triage-api.md).

## The defect this fixes

A goal can get stuck in a way that no counter can un-stick. The worked example is
goal `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` — *"raise
Simard's test coverage to 70%"*. Its trouble was not the work; it was the
**finish line**:

- The done-gate had **no measurable done condition**. "70% coverage" named no
  module, no command, and no numeric signal the daemon could read, so
  [`completion_gate::has_derivable_signal`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
  returned nothing to verify. Completion could never certify.
- After **5 consecutive no-progress cycles** the goal was demoted out of the
  active list into a cooldown / blocked state.
- **No engineer was assigned**, so nothing moved it forward.

Left alone, the OODA loop kept re-investigating the goal without shipping, and
the [backoff gate](./blocked-goal-escalation-backoff.md) merely spaced out the
duplicate escalations. The block needed a *decision*, not another tick.

Before this change, that decision defaulted to "surface the raw diagnosis to a
human". A non-engineer operator would have seen something like:

```text
🔒 [OODA-SAFEGUARD] goal blocked why=UNCLEAR-CRITERIA
evidence=[no done-gate; 5× no-progress; unassigned] health-review:blocked-goal
```

That is jargon, not a request a person can act on.

## The fix: an agentic triage step that decides and self-corrects

The Overseer now hands the blocked goal to the **escalation-triage brain**
(`escalation_triage.md`), the exact mirror of `self_diagnose.md` for a step
failure. The brain runs four steps in order and emits one plain-English Signal
message per step:

1. **Restate the PROBLEM in plain English.** Translate every internal marker.
2. **Recommend a concrete NEXT STEP** — the smallest clear action that unblocks.
3. **Attempt a ROOT CAUSE and DECIDE the course-correction** (one of three).
4. **Send a jargon-free Signal update** after each step.

### The three course-corrections

The brain attempts to fix the block itself before asking a human, choosing
exactly one:

| Decision | When it applies | What it does |
| --- | --- | --- |
| `rewrite-done-gate` | The finish condition can't be measured automatically | Rewrites the done-criteria into a machine-checkable first slice, assigns an owner, returns the goal to the active list |
| `complete-delivered-goal` | A merged PR already delivered the work | Marks the goal complete on the existing evidence |
| `ask-operator-one-question` | A human decision is genuinely required | Asks exactly ONE crisp plain-English question |

For the coverage goal the diagnostic is explicitly *"no measurable done-gate"*,
so the brain chooses **`rewrite-done-gate`**. It does **not** ask the operator:
the *intent* ("raise coverage") is clear; only the *done-gate* is unmeasurable,
and the recipe mandates self-correction before escalation.

### What "rewrite the done-gate" concretely does

The correction is a real state edit, not a proposal. It replaces the
unmeasurable *"raise Simard test coverage to 70%"* with a **machine-checkable
first slice**:

- **one named under-tested module** (a concrete first slice derived from the
  repo),
- **a machine-checkable finish signal for that slice** — a tracked PR the daemon
  can observe `MERGED` (or a linked issue it can observe `CLOSED`) that lands the
  module's tests. That is exactly the evidence
  [`completion_gate::has_derivable_signal`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
  already recognizes. A raw per-file line-% read from a coverage summary is the
  *intent*, but the current gate has **no** evidence field for a numeric coverage
  threshold — see the
  [design note](#design-note-numeric-coverage-vs-existing-evidence),
- **an owner** (`assigned_to`) so the goal re-enters the active list.

The done-criteria are stored in the goal's **`description`** field — `ActiveGoal`
has no separate `success_criteria` field (the verification layer clones
`description` as the success criteria). The whole read-modify-write happens inside
**one** `goal_board_store::mutate` closure under `flock`, transitioning the goal
`Blocked(..) -> NotStarted`. The change is additive — it edits only pre-existing
fields, so old snapshots stay byte-compatible and there is no serde-shape change.

```text
BEFORE  audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a
        status:        Blocked("… no measurable done-gate …")
        description:   "raise Simard test coverage to 70%"        ← unmeasurable
        assigned_to:   None
        wip_refs:      []                                         ← no signal to verify

AFTER   audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a
        status:        NotStarted
        description:   "add tests for src/goal_board_store to lift its line
                        coverage to ≥ 80%; land via one PR"       ← machine-checkable slice
        assigned_to:   Some("<owner>")
        wip_refs:      [pr/issue tracking the slice]              ← has_derivable_signal == true
```

Once rewritten, `has_derivable_signal` finds a signal (the tracked PR/issue), the
completion gate can certify the slice, and the goal stops churning.

### Design note: numeric coverage vs. existing evidence

The design spec asks the completion gate to *"certify done from a numeric
coverage signal (per-file line-% in `coverage-summary.json`)"* while also
*"reusing existing `CompletionEvidence` + `has_derivable_signal`; no new
`MissingEvidence` variant"*. **These two constraints conflict in the current
code:**

- [`has_derivable_signal`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs)
  inspects only `wip_refs` kinds (`pr` / `issue`) and self-affecting status. It
  has no notion of a coverage file or a numeric threshold.
- `CompletionEvidence` is **boolean-only** (`pr_merged`, `issue_closed`,
  `self_affecting`, `deployed`). There is no numeric field a line-% could land in.

Reading a coverage threshold therefore requires **new evidence plumbing** (a new
evidence field plus a new `has_derivable_signal` branch), which is *not* additive
and *does* touch the gate's serde shape. The recommended, fully-additive path is
therefore (a): make the slice verifiable through a **tracked PR/issue** using the
existing boolean evidence. Option (b) — a first-class numeric-coverage evidence
path — is a real design change and should be scoped explicitly, not smuggled in
under "reuse existing".

## The operator only ever sees plain English

Every Signal message goes through a fixed **translation table** and a
**forbidden-token denylist** that fails closed *before* each send. The raw
markers are inputs to translate, never text to forward:

| Internal marker | What the operator sees |
| --- | --- |
| opaque typed blocker / no measurable done-gate | "Simard can't automatically tell when this goal is finished, so it keeps re-checking without shipping." |
| demoted after 5 no-progress cycles | "It stalled repeatedly and dropped off the active list." |
| no engineer assigned | "No one is currently responsible for moving it." |

The denylist rejects any payload containing `OODA-SAFEGUARD`,
`UNCLEAR-CRITERIA`, `GENUINELY-STUCK`, `evidence=[`, `why=`, the 🔒 lock token, or
`health-review:blocked-goal`. A contract test asserts translation-not-passthrough
so a marker can never leak.

A typical cadence for the coverage goal:

```text
Signal → "I looked at the test-coverage goal — it's stuck because Simard can't
          automatically tell when it's finished, so it keeps re-checking without
          shipping. It stalled repeatedly and dropped off the active list, and
          no one is currently responsible for it."
Signal → "Next step: aim it at one under-tested module first, with a coverage
          number Simard can check automatically, and give it an owner."
Signal → "Done — I set its finish line to 'src/goal_board_store line coverage
          ≥ 80%' and assigned an owner, so it's back on the active list and can
          be certified automatically. Nothing needed from you."
```

## The decision is the recipe's, not a threshold's

Crucially, the escalate-vs-course-correct **decision is owned by the recipe**,
from the evidence — it is **not** gated by a recurrence count or any other bare
integer on the Rust side. The thin trigger
`overseer::act_escalate_blocked_goal` and the
[`RECURRENCE_ESCALATION_THRESHOLD`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)
are untouched: they still decide *when* to launch triage, but the triage brain
decides *what to do*.

## Output contract

The brain returns exactly this schema (see the
[recipe asset](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md)):

```json
{
  "problem": "plain-English statement of WHAT is wrong (no jargon, no markers)",
  "next_step": "plain-English recommended NEXT STEP",
  "root_cause": "one or two sentences on the true root cause, grounded in evidence",
  "decision": "rewrite-done-gate | complete-delivered-goal | ask-operator-one-question",
  "action_taken": "what you actually did, or the single question you asked",
  "escalate": "reason a human is genuinely required, or null"
}
```

Invariant: `escalate` is **non-null iff** `decision == ask-operator-one-question`.
When the block is self-corrected via `rewrite-done-gate` (as for the coverage
goal), `escalate` is `null`.

## Guardrails

- **Additive / non-breaking / CI-green.** Edits only pre-existing goal fields; no
  serde shape change; old snapshots stay byte-compatible.
- **No silent fallback.** Errors propagate as `SimardResult` with `?`; structured
  `tracing` + OTel only, no stray `print!`, no `Bridge` naming.
- **Fail closed on bad input.** If the first-slice module can't be derived, the
  threshold is out of `0..=100`, or the owner is malformed, the brain takes the
  `ask-operator-one-question` path rather than persisting a bad gate.
- **No TOCTOU.** The entire read-modify-write runs inside one `mutate` closure
  under `flock`; state read outside the lock is never persisted.

## See also

- [Blocked-goal escalation backoff](./blocked-goal-escalation-backoff.md) — how
  the same goal is not re-escalated every tick.
- [Overseer agentic health-review](./overseer-agentic-health-review.md) — the
  sibling self-diagnosis reflex for crash-loops.
- [Deploy-aware done-gate](./deploy-aware-done-gate.md) — the completion gate the
  rewritten done-criteria feed into.
- [Escalation-triage API reference](../reference/escalation-triage-api.md) — the
  seams, types, and validation rules.
