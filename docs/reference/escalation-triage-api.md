---
title: Escalation-triage API & output contract
description: >
  Reference for the blocked-goal escalation-triage brain
  (prompt_assets/simard/overseer/escalation_triage.md): its input envelope, the
  exact six-key output JSON contract, the decision enum, the marker-scrub
  forbidden-token list, and the two deterministic Rust rails it drives — the
  goal-completion seam simard goal complete / handle_complete (idempotent,
  tombstone-writing) and the dual-channel operator notifier notify() with its
  goal-blocked kind. Includes the goal_id validation rule and idempotency
  guarantees.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/escalation-triage-before-human.md
  - ../howto/triage-a-blocked-goal-escalation.md
  - ./overseer-operator-notification-dedup.md
  - ./tombstoned-goal-engineer-reaper-api.md
  - ./simard-cli.md
---

# Escalation-triage API & output contract

> **Status: implemented.** Canonical I/O contract for the escalation-triage
> brain and the rails it drives. The brain is
> [`escalation_triage.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md);
> the rails are
> [`operator_cli::goal::handle_complete`](https://github.com/rysweet/Simard/blob/main/src/operator_cli/goal.rs)
> and
> [`overseer::notify`](https://github.com/rysweet/Simard/blob/main/src/overseer/notify.rs).
> See the [concept](../concepts/escalation-triage-before-human.md) for the
> rationale.

## Input envelope

The recipe is invoked with the blocked goal's structured context. The markers
are **inputs to translate**, never text to forward verbatim.

```json
{
  "goal_id": "audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a",
  "problem_seed": "plain-English problem seed (refine it)",
  "next_step_seed": "recommended next-step seed (refine it)",
  "internal_why": "typed blocked-terminal outcome … on goal … never escalated (escalations=0 …)",
  "reason_marker": "health-review:blocked-terminal"
}
```

| Field | Meaning |
| --- | --- |
| `goal_id` | The blocked goal's id. The triage brain requires it to match `^[a-z0-9-]+$` before emitting the `simard goal complete` action (see validation below). |
| `problem_seed` | A plain-English problem seed to refine — not authoritative. |
| `next_step_seed` | A recommended next-step seed to refine — not authoritative. |
| `internal_why` | Internal diagnostic WHY. **Translate; never surface raw.** |
| `reason_marker` | Raw machine reason marker. **Translate; never surface raw.** |

## Output contract (exactly six keys)

```json
{
  "problem": "plain-English statement of WHAT is wrong (no jargon, no marker tokens)",
  "next_step": "plain-English recommended NEXT STEP",
  "root_cause": "one or two sentences on the true root cause, grounded in the evidence",
  "decision": "rewrite-done-gate | complete-delivered-goal | ask-operator-one-question",
  "action_taken": "what you actually did (the rewrite / completion), or the single question asked",
  "escalate": "reason a human is genuinely required, or null"
}
```

Rules:

- The object has **exactly** these six keys — no more, no fewer.
- Every string is plain English. No `OODA-SAFEGUARD`, `UNCLEAR-CRITERIA`,
  `GENUINELY-STUCK`, `evidence=[`, `why=`, or 🔒 anywhere.
- `escalate` is non-null **only** when `decision == "ask-operator-one-question"`.
  For `rewrite-done-gate` and `complete-delivered-goal` it is `null`.

### `decision` enum

| Value | Meaning | `escalate` | Rail invoked |
| --- | --- | --- | --- |
| `rewrite-done-gate` | The done-gate can't certify completion; re-scope it to a machine-verifiable condition and apply the rewrite. | `null` | goal/issue edit |
| `complete-delivered-goal` | A merged PR already delivered the work; mark the goal complete. | `null` | `simard goal complete <id>` |
| `ask-operator-one-question` | A genuine human decision is required; ask exactly ONE question. | reason string | `notify()` only |

### Worked output (#4904)

```json
{
  "problem": "Simard's work to lift automated test coverage above 70% was recorded as stuck and then left alone, so it made no further progress and nobody was told.",
  "next_step": "Close the goal: the coverage work it describes has already been delivered by merged changes, so there is nothing left to do and nothing is needed from you.",
  "root_cause": "The goal was marked blocked and left silently un-escalated for a day even though its coverage objective had already been delivered by merged work — a stale block on an already-delivered goal, not a real engineering blocker.",
  "decision": "complete-delivered-goal",
  "action_taken": "Marked the goal complete, which removed it from the active board and recorded a permanent record so it cannot be reopened by accident.",
  "escalate": null
}
```

## Marker-scrub gate

Before emitting the output JSON **or** any Signal message, the brain runs a
forbidden-token scan and blocks the emit on any hit. Forbidden substrings:

```
OODA-SAFEGUARD
UNCLEAR-CRITERIA
GENUINELY-STUCK
health-review:blocked-terminal
blocked-terminal outcome            (and any raw typed-outcome UUID)
why=
evidence=[
🔒
```

A draft that would leak any of these is rewritten in plain English before it
reaches a human. This is a zero-leak hard gate, not a best-effort filter.

## Rail: `simard goal complete <goal_id>`

The `complete-delivered-goal` decision is applied by the existing operator CLI
seam — no new entry point.

```bash
simard goal complete audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a
```

`goal_id` **validation:** the `^[a-z0-9-]+$` allowlist is enforced by the triage
brain (recipe step 2, described in
[the escalation-triage concept](../concepts/escalation-triage-before-human.md)),
which refuses to emit a `complete-delivered-goal` action for an id that fails the
pattern. `handle_complete` in Rust does **not** itself re-check the regex — it
treats `goal_id` as an opaque board key. Separately, the CLI passes `goal_id` as a
discrete argv element and never interpolates it into a `sh -c` string, so it
cannot be used for command injection regardless of contents.

### `handle_complete` outcomes

`handle_complete(goal_id)` runs under the shared goal-board flock and resolves to
one of three outcomes:

| Outcome | Condition | Effect |
| --- | --- | --- |
| `Reopened` | The goal exists and `is_perpetual()`. | Rolled to a fresh cycle; **not** terminated; **no** tombstone. Standing goals are never completed away. |
| `Completed` | A non-perpetual goal existed on the board. | Removed from `active` + `backlog`; a durable tombstone is written. |
| `Absent` | No matching goal on the board. | A tombstone is still recorded (idempotent re-run). |

Log lines (stderr), stable substrings:

```
[simard] goal complete: '<id>' marked done, removed from board, and tombstoned
[simard] goal complete: '<id>' not on board; recorded tombstone (idempotent)
[simard] goal complete: '<id>' is a standing goal — refused to terminate; reopened it for a fresh cycle (no tombstone)
```

**Idempotency.** Re-running `complete` on an already-completed goal takes the
`Absent` branch and re-records the tombstone without error. Triage may therefore
be re-run safely. Tombstones are the same durable record the
[tombstoned-goal engineer reaper](./tombstoned-goal-engineer-reaper-api.md) and
cycle-reconcile honour, so a completed goal cannot be resurrected.

## Rail: operator notifier `notify()`

Each triage step sends one plain-English update through the dual-channel notifier
([`DualChannelNotifier::notify`](./overseer-operator-notification-dedup.md)) as an
`OperatorNotification`:

```rust
pub struct OperatorNotification {
    pub kind: &'static str,   // "goal-blocked" for triage updates
    pub headline: String,     // one-line Signal first line / email subject core
    pub problem: String,      // plain-language problem/why
    pub next_step: String,    // plain-language recommended next step
    pub link: Option<String>, // optional PR/commit url
    pub repo: String,
    pub autonomous: bool,
}
```

- **`kind = "goal-blocked"`** renders the accurate *"Action needed — a goal is
  blocked in `<repo>`"* template (leading with the plain-English problem and
  next step), **not** the merge/deploy "Problem solved" template.
- `goal-blocked` is in the **suppressible** kind set, so identical repeats for
  the same still-blocked goal are deduped by the #4579 signature rail. The three
  triage updates carry **distinct content**, so all three dispatch.
- Every field is marker-scrubbed before it is handed to `notify()`.

### The three required updates

Triage sends at least three short, jargon-free messages, one per reasoning step:

1. **Problem restated** — what is wrong, in plain English.
2. **Recommended next step / root cause** — the plain-English recommendation.
3. **Decision + action taken** — what was decided and what was actually done
   (e.g. "the goal is closed; nothing is needed from you").

## Related

- [Concept: triage & course-correct a blocked goal](../concepts/escalation-triage-before-human.md)
- [How to triage a blocked-goal escalation](../howto/triage-a-blocked-goal-escalation.md)
- [Operator notification dedup](./overseer-operator-notification-dedup.md)
- [Tombstoned-goal engineer reaper API](./tombstoned-goal-engineer-reaper-api.md)
- [Simard CLI reference](./simard-cli.md)
