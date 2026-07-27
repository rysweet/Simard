---
title: Escalation-triage recipe reference (blocked-goal course-correction)
description: >
  Reference for the Overseer escalation-triage recipe
  (prompt_assets/simard/overseer/escalation_triage.md, #4276): the thin
  act_escalate_blocked_goal trigger contract, the exact INPUT and OUTPUT JSON
  schemas, the three permitted decisions (rewrite-done-gate,
  complete-delivered-goal, ask-operator-one-question) and how each is applied, the
  marker-translation rules (never surface OODA-SAFEGUARD / UNCLEAR-CRITERIA /
  GENUINELY-STUCK / why= / evidence=[…] / 🔒 / health-review:stuck-goal), the
  per-step Signal cadence, the machine-checkable evidence gate for completion, and
  the change/safety guardrails.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#4276"]
related:
  - ../concepts/overseer-escalation-triage-course-correction.md
  - ../concepts/agentic-recipes-first-principle.md
  - ./overseer-operator-notifications.md
  - ./overseer-backoff-gate-api.md
  - ./overseer-root-cause-why-api.md
  - ./goal-board-api.md
  - ./simard-cli.md
  - ../howto/triage-and-course-correct-a-blocked-goal.md
---

# Escalation-triage recipe reference

The escalation-triage recipe is the agentic reasoning step behind the Overseer's
blocked-goal escalation. This reference documents its exact contract: the trigger,
the input it receives, the output it must produce, the three decisions, and the
translation and safety rules. For the rationale see
[The Overseer triages and course-corrects a blocked goal before escalating](../concepts/overseer-escalation-triage-course-correction.md).

- **Recipe asset:** [`prompt_assets/simard/overseer/escalation_triage.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md)
- **Thin Rust trigger:** `overseer::act_escalate_blocked_goal`
  ([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs))
- **Owns the decision:** the recipe, not a Rust threshold.

## Trigger contract (`act_escalate_blocked_goal`)

`act_escalate_blocked_goal` is a **thin structured trigger**. Its sole
responsibilities are:

1. Gather the blocked goal's structured context — goal id, the internal diagnostic
   markers (`internal_why`, `reason_marker`), and a seed problem/next-step.
2. Launch the escalation-triage recipe via the recipe runner with that context.
3. Route the recipe's `action_taken` to the correct effecting capability (goal
   completion, goal/issue edit, or an operator question), and — only when
   `escalate` is non-null — hand the single question to the
   [operator-notification path](./overseer-operator-notifications.md) behind the
   [escalation backoff gate](./overseer-backoff-gate-api.md).

It does **not** decide whether to escalate, and it does **not** implement a
"Nth consecutive failure ⇒ human" counter. The escalate-vs-course-correct decision
is the recipe's.

## Input schema

The recipe receives exactly:

```json
{
  "goal_id": "{goal_id}",       // the blocked goal's id
  "problem_seed": "{problem}",  // plain-English problem seed (to refine)
  "next_step_seed": "{next_step}", // recommended next-step seed (to refine)
  "internal_why": "{why}",      // internal diagnostic WHY — TRANSLATE, never surface raw
  "reason_marker": "{reason}"   // raw machine marker — TRANSLATE, never surface raw
}
```

`internal_why` and `reason_marker` are **evidence for the brain's own reasoning**,
not text to forward. They are inputs to translate, never to echo verbatim.

## Output schema

The recipe emits exactly these six fields:

```json
{
  "problem": "plain-English statement of WHAT is wrong (no jargon, no marker tokens)",
  "next_step": "plain-English recommended NEXT STEP",
  "root_cause": "one or two sentences on the true root cause, grounded in the evidence",
  "decision": "rewrite-done-gate | complete-delivered-goal | ask-operator-one-question",
  "action_taken": "what you actually did (the rewrite / completion), or the single question you asked",
  "escalate": "reason a human is genuinely required, or null"
}
```

| Field | Type | Notes |
| --- | --- | --- |
| `problem` | string | Plain English only. Must contain **no** marker tokens. |
| `next_step` | string | The smallest, clearest unblocking action, plain English. |
| `root_cause` | string | 1–2 sentences, grounded in observable evidence. |
| `decision` | enum | Exactly one of the three values below. |
| `action_taken` | string | The effected change or the single question — never a proposal. |
| `escalate` | string \| null | Non-null **only** for `ask-operator-one-question`. `null` for the two self-resolving decisions. |

### Invariant: `escalate` ↔ `decision`

- `decision = "complete-delivered-goal"` ⇒ `escalate = null`.
- `decision = "rewrite-done-gate"` ⇒ `escalate = null`.
- `decision = "ask-operator-one-question"` ⇒ `escalate` is a non-null plain-English
  reason a human is genuinely required.

## The three decisions

### `rewrite-done-gate`

Use when the goal's finish condition cannot be measured automatically — the
done-gate can never certify it. The brain re-scopes the done-criteria to something
**machine-verifiable** and **applies** it (edits the goal / its tracking issue via
Simard's agentic capabilities — not merely proposed):

- a specific issue the daemon can observe `CLOSED`, or
- a specific PR the daemon can observe `MERGED`, or
- a specific file or command whose presence / output the done-gate can check.

`escalate = null`.

### `complete-delivered-goal`

Use when the work the goal describes has **already shipped** in a merged PR. The
brain marks the goal complete, citing machine-checkable proof.

**Evidence gate (required before completion).** Completion must be justified by
**parsed** GitHub state, never inferred from free-text bodies:

```bash
# Both facts must hold; pin the fully-qualified repo.
gh issue view 17 --repo rysweet/agent-kgpacks-rs --json state,stateReason
#   -> state == "CLOSED"  (stateReason "COMPLETED")
gh pr view 40 --repo rysweet/agent-kgpacks-rs --json state,mergedAt
#   -> state == "MERGED"  (mergedAt non-null)
```

> **The `17` / `40` / `rysweet/agent-kgpacks-rs` values above are illustrative**
> (the kgpacks-rs worked case). At runtime the brain resolves the goal's own
> fully-qualified `owner/repo#number` from the goal context — do not hard-code
> these literals into any real check.

Completion is effected through the **intent-revealing completion verb**:

```bash
simard goal complete <goal_id>   # marks the goal done, removes it, and tombstones it so nothing re-seeds it
```

`simard goal complete` (see
[CLI reference](./simard-cli.md#simard-goal-complete-goal-id)) marks the goal
finished, removes it from the active board and backlog, and writes a **durable
tombstone** so no path (default seeding, memory recall, meeting handoff, or the
daemon's cycle reconcile) can resurrect it. It is idempotent, and it refuses
standing/perpetual goals (auto-reopening them for a fresh cycle instead of
terminating). The brain never writes the goal-board store directly.
`escalate = null`.

> **Target discipline.** Resolve the fully-qualified `owner/repo#number`. For the
> kgpacks-rs case that is `rysweet/agent-kgpacks-rs#17`; the bare
> `rysweet/agent-kgpacks#17` is an **unrelated closed autocomplete bug** and must
> be rejected as a false lead.

### `ask-operator-one-question`

Use **only** when a human decision is genuinely required (ambiguous intent, or a
scope call that is the operator's to make). The brain asks **exactly one** crisp,
plain-English question — never a wall of jargon, never more than one. `escalate` is
the non-null reason.

## Marker-translation rules

Everything the operator sees is **plain English**. The recipe MUST translate every
internal marker and MUST NOT surface any of these raw tokens to a human — in the
output `problem`/`next_step`/`action_taken` fields **or** in any Signal message:

| Raw token (never surfaced) | Plain-English translation (example) |
| --- | --- |
| `🔒` lock token | *(dropped — no symbol shown)* |
| `[OODA-SAFEGUARD]` | "Simard's own safety check on a stuck goal" |
| `why=UNCLEAR-CRITERIA` | "Simard can't automatically tell when this goal is finished" |
| `why=GENUINELY-STUCK` | "the goal is stuck on something it can't get past on its own" |
| `evidence=[…]` | *(summarize the underlying facts in plain words)* |
| `health-review:stuck-goal` | "this goal keeps retrying for hours without making progress" |
| consecutive-failure cooldown / demotion | "the goal keeps failing and backing off, so it isn't getting anywhere" |

A conforming run leaves **no** marker token in any operator-visible string.

## Signal cadence

After each reasoning step the recipe sends the operator one short, jargon-free
Signal message describing what it found or decided. The canonical three-message
cadence for a self-resolving course-correction is **found → decided → done**:

```
"I looked at the stuck embedding goal — the work it's waiting on already shipped and was merged."
"So I'm going to mark that goal finished; nothing is actually left to do."
"Done — the goal is closed out and off the board. Nothing needed from you."
```

For `ask-operator-one-question`, the final message is the single question itself.
Signal delivery uses the reliable two-channel
[operator-notification contract](./overseer-operator-notifications.md).

## Change & safety guardrails

Any code or config change the brain makes as a course-correction MUST be:

- **additive / non-breaking**, CI-green, and merge-ready;
- free of `Bridge` naming;
- free of stray `print!` in new code — structured `tracing` + OTel only;
- free of silent fallbacks (failures are surfaced, not swallowed).

Mutation safety:

- `simard goal complete <goal-id>` is the authorized goal-completion path; it is
  idempotent (completing an absent id still records the tombstone), refuses
  standing/perpetual goals (auto-reopening instead of terminating), and persists
  under the shared `goal-board.lock` flock, writing a durable tombstone via
  `tombstone_goals` so nothing re-seeds the goal (see
  [CLI reference](./simard-cli.md#simard-goal-complete-goal-id) and
  [goal-board API](./goal-board-api.md)).
- GitHub access for the evidence gate is **read-only** (`gh … view/list`); the
  brain never closes/merges anything to "make the evidence true."
- The goal id and `owner/repo#number` are passed as **validated literal
  arguments**, never string-interpolated into a shell.
- No secret egress: the output JSON and `action_taken` contain no tokens,
  credential paths, or environment secrets.

## Related

- [Escalation-triage concept](../concepts/overseer-escalation-triage-course-correction.md)
- [Overseer operator-notification reliability reference](./overseer-operator-notifications.md)
- [Overseer BackoffGate & gap-scan dedup](./overseer-backoff-gate-api.md)
- [Goal-board API](./goal-board-api.md)
- [simard CLI reference](./simard-cli.md)
- [How to triage and course-correct a blocked goal](../howto/triage-and-course-correct-a-blocked-goal.md)
