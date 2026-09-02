---
title: Triage & course-correct a blocked goal before escalating to a human
description: >
  Why Simard's Overseer never ships a raw machine block marker to a person and
  counts it as an escalation. When a goal is marked blocked, an agentic
  escalation-triage brain (prompt_assets/simard/overseer/escalation_triage.md)
  first restates the block in plain English, attempts a root cause, and
  COURSE-CORRECTS it agentically — rewriting an unmeasurable done-gate,
  completing a goal already delivered by a merged PR, or asking the operator ONE
  specific plain-English question. It escalates to a human ONLY when a decision
  is genuinely theirs. The Rust seam is a thin structured trigger; the recipe —
  not a bare integer threshold — owns the escalate-vs-course-correct decision.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./agentic-recipes-first-principle.md
  - ./overseer-agentic-health-review.md
  - ./blocked-goal-escalation-backoff.md
  - ./ooda-reinvestigate-blocked-goals.md
  - ./no-progress-terminal-investigation.md
  - ../reference/escalation-triage-api.md
  - ../howto/triage-a-blocked-goal-escalation.md
---

# Triage & course-correct a blocked goal before escalating to a human

> **Status: implemented.** This page describes the shipped escalation-triage
> behaviour in present tense. The agentic brain lives in
> [`prompt_assets/simard/overseer/escalation_triage.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md);
> the thin Rust rails are the goal-completion seam
> [`operator_cli::goal::handle_complete`](https://github.com/rysweet/Simard/blob/main/src/operator_cli/goal.rs)
> and the dual-channel operator notifier
> [`overseer::notify`](https://github.com/rysweet/Simard/blob/main/src/overseer/notify.rs).

## The problem: a blocked goal that sat stuck and told no one

When the Overseer decides a goal is genuinely blocked, the naïve behaviour is to
forward the raw machine marker to a human and count it as "escalated":

```
🔒 [OODA-SAFEGUARD] … why=UNCLEAR-CRITERIA evidence=[…]
```

Two things go wrong with that:

1. **It dumps jargon on a non-engineer.** `OODA-SAFEGUARD`, `UNCLEAR-CRITERIA`,
   `GENUINELY-STUCK`, `evidence=[…]`, `why=`, and the 🔒 lock token are internal
   diagnostics. An operator cannot act on them.
2. **It gives up too early.** Many "blocks" are not real engineering walls. The
   done-gate may simply be unmeasurable, or the work may have *already shipped*
   in a merged PR. In those cases the right move is to fix the block, not to
   page a human.

The live trigger for this behaviour (#4904): the goal
`audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` was recorded as
**blocked** and left there. No person was ever told — the health-review pass
found `escalations=0` across every Overseer tick in the last 24h — so the goal
sat stuck with zero progress. Meanwhile the coverage charter it described had
**already been delivered** by a merged PR and a set of closed tracking issues.
The goal was not blocked on engineering at all; it was a stale block on
already-delivered work.

## The design: a thin rail + an agentic brain

Escalation-triage follows the same split as
[agentic health-review](./overseer-agentic-health-review.md) and the
[agentic-recipes-first principle](./agentic-recipes-first-principle.md): all
judgment lives in a recipe, and Rust provides only deterministic rails.

- **The brain — `escalation_triage.md`.** When a goal is marked blocked, the
  recipe receives the goal id and the internal diagnostic markers (the WHY and
  the reason marker) and, in order:

  1. **Restates the PROBLEM in plain English.** Every internal marker is
     translated. The operator never sees `OODA-SAFEGUARD`, `UNCLEAR-CRITERIA`,
     `GENUINELY-STUCK`, `evidence=[…]`, `why=`, or 🔒.
  2. **Recommends a concrete NEXT STEP** — the smallest, clearest action that
     unblocks the goal, in plain English.
  3. **Attempts a ROOT CAUSE and DECIDES the course-correction** (below).
  4. **Sends one jargon-free Signal message per step** so the operator can
     follow the reasoning in plain English as it happens.

- **The rails — Rust.** The escalation seam
  (`overseer::act_escalate_blocked_goal`) is only a thin structured trigger. The
  goal-state transition is `simard goal complete <id>`
  ([`handle_complete`](../reference/escalation-triage-api.md)), and every
  operator message goes out on both channels through
  [`notify`](../reference/overseer-operator-notification-dedup.md). Neither rail
  decides *whether* to escalate — the recipe does.

The escalate-vs-course-correct decision is **not** gated by a recurrence count or
any other bare integer threshold on the Rust side. Encoding a magic `N` would
freeze a guess where an agent should reason from the evidence.

## The three course-corrections

The brain attempts to fix the block itself before asking a human, and chooses
**exactly one** decision:

| Decision | When it applies | What the brain does |
| --- | --- | --- |
| `rewrite-done-gate` | The finish condition can't be measured automatically — the done-gate can never certify it. | Re-scopes the done-criteria to something machine-verifiable (a specific issue the daemon can see `CLOSED`, a specific PR it can see `MERGED`, or a file/command whose output the done-gate can check) and **applies** the rewrite. |
| `complete-delivered-goal` | The work the goal describes has **already shipped** in a merged PR. | Marks the goal complete via `simard goal complete <id>` rather than leaving it blocked. Idempotent; writes a durable tombstone so cycle-reconcile can't resurrect it. |
| `ask-operator-one-question` | A human decision is genuinely required — the intent is ambiguous, or a scope call is the operator's to make. | Asks **exactly one** crisp plain-English question. Never a wall of jargon, never more than one question. |

Only `ask-operator-one-question` sets a non-null `escalate`. The first two
resolve the block agentically and escalate nothing.

### Worked example (#4904)

For `audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a` the evidence — a
merged coverage charter PR and closed tracking issues — showed the objective was
**already satisfied**. So:

- **Root cause:** the goal was marked blocked and left silently un-escalated for
  24h even though the coverage objective was already delivered by merged work — a
  stale block on an already-delivered goal, not a true engineering blocker.
- **Decision:** `complete-delivered-goal`.
- **Action taken:** ran `simard goal complete audit-simard-s-test-coverage-and-raise-it-to-70-4d27c91a`, which removed it from the board and wrote a durable tombstone.
- **Escalate:** `null` — the merged charter answered every scope question, so no human decision remained.

The seed's suggested options (accept a lower target, allocate time, relax the CI
gate) are the *operator-facing next-step prose* — they do not bind the decision
enum. The evidence forces `complete-delivered-goal`, and the plain-English
next-step is phrased honestly ("the work appears already delivered; the goal can
be closed"), not as a mechanical restatement of options the evidence contradicts.

## The marker-scrub guarantee

Everything the operator ever sees — the six-key output JSON **and** every Signal
message — is plain English. Before emitting anything, the brain runs a hard
forbidden-token scan and blocks the emit on any hit. The scrub list covers:

```
OODA-SAFEGUARD   UNCLEAR-CRITERIA   GENUINELY-STUCK
health-review:blocked-terminal      why=       evidence=[      🔒
```

…plus raw typed-outcome ids (e.g. `blocked-terminal outcome 019f6c08-…`). If a
draft message would leak any of these, it is rewritten before it reaches a human.
This is a zero-leak requirement, not a best-effort filter.

## Why not just count and forward?

The tempting imperative "fix" is to keep a per-goal blocked-recurrence counter and
trip escalation at a threshold. We deliberately do **not**, for the same two
reasons health-review rejects a failure counter:

1. **It hard-codes judgment as a constant.** "Is this a real block? Can I rewrite
   the done-gate? Did a merged PR already deliver it? Or is this genuinely the
   operator's call?" does not reduce to an integer.
2. **It surfaces jargon.** Forwarding the raw marker is exactly the bug (#4276)
   this behaviour fixes. The operator gets a plain-English restatement and, at
   most, one specific question — never the raw diagnosis.

## Related

- [Agentic-recipes-first principle](./agentic-recipes-first-principle.md) — the
  governing guideline (`G3`) this behaviour applies at reasoning time.
- [Overseer agentic health-review](./overseer-agentic-health-review.md) — the
  pass that detects a blocked-terminal goal never escalated and hands it here.
- [Blocked-goal escalation backoff](./blocked-goal-escalation-backoff.md) — how
  repeat escalations for the same still-blocked goal are deduped.
- [Re-investigate blocked goals](./ooda-reinvestigate-blocked-goals.md) — the
  OODA-side pass for bare no-progress blocks.
- [Escalation-triage API reference](../reference/escalation-triage-api.md) — the
  exact I/O contract and rails.
- [How to triage a blocked-goal escalation](../howto/triage-a-blocked-goal-escalation.md)
  — operator playbook.
