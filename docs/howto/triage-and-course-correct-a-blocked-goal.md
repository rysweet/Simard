---
title: Triage and course-correct a blocked goal (before it reaches you)
description: >
  Operator runbook for the Overseer escalation-triage step (#4276): what happens
  when a goal is marked blocked, how Simard restates the block in plain English,
  attempts a root cause, and course-corrects it agentically — rewriting an
  unmeasurable done-gate, completing a goal a merged PR already delivered, or
  asking you exactly ONE plain-English question — before it ever escalates to you.
  Includes the worked kgpacks-rs int8/PQ "already delivered" walkthrough, how to
  verify each outcome, and how to confirm no raw markers leaked.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/overseer-escalation-triage-course-correction.md
  - ../reference/escalation-triage-api.md
  - ./diagnose-a-no-progress-block.md
  - ./reinvestigate-bare-blocked-goals.md
  - ./unblock-stuck-ooda-goals.md
  - ../reference/overseer-operator-notifications.md
  - ../reference/simard-cli.md
---

# Triage and course-correct a blocked goal (before it reaches you)

## What this is

When the Overseer decides a goal is genuinely blocked, it no longer dumps a raw
diagnostic on you and calls it handled. It runs the **escalation-triage** step,
which restates the block in plain English, attempts a root cause, and
**course-corrects it agentically** — only asking you a question when a decision is
genuinely yours to make. You follow the reasoning as short, jargon-free Signal
updates.

For the design see
[the escalation-triage concept](../concepts/overseer-escalation-triage-course-correction.md);
for the exact contract see the
[escalation-triage API reference](../reference/escalation-triage-api.md).

## What you will see (and won't)

You will receive **plain-English** Signal messages, roughly one per reasoning step.
You will **not** see raw internal markers — Simard translates all of them. If you
ever see `[OODA-SAFEGUARD]`, `why=UNCLEAR-CRITERIA`, `GENUINELY-STUCK`,
`evidence=[…]`, the `🔒` lock token, or `health-review:stuck-goal` in a message,
that is a **bug** (see [Verify no markers leaked](#verify-no-markers-leaked)).

## The three possible outcomes

Simard picks exactly one, from the evidence:

1. **It rewrote the finish condition** so the goal can be certified automatically
   (`rewrite-done-gate`). Nothing needed from you.
2. **It marked the goal finished** because the work already shipped in a merged PR
   (`complete-delivered-goal`). Nothing needed from you.
3. **It asked you one specific question** because a call is genuinely yours to make
   (`ask-operator-one-question`). Answer the one question.

Only outcome 3 reaches you as an escalation; outcomes 1 and 2 resolve the block on
their own and just tell you what happened.

## Worked example: the kgpacks-rs int8/PQ embedding goal

**Symptom.** A goal such as
`fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-…` has failed several times in a
row and keeps restarting without completing. On the board it looks stuck; in the
journal it repeatedly hits the consecutive-failure cooldown with no progress over
hours.

**What Simard does.**

1. **Restates it plainly.** *"A task to finish some embedding work has been retrying
   for hours without getting anywhere — it looks stuck on something it can't get
   past."*
2. **Finds the root cause.** The work was **already delivered** by a **merged PR**.
   Simard verifies this from parsed GitHub state (not from prose), pinning the
   fully-qualified repo:

   ```bash
   gh issue view 17 --repo rysweet/agent-kgpacks-rs --json state,stateReason,url
   #   state = "CLOSED", stateReason = "COMPLETED"
   gh pr view 40 --repo rysweet/agent-kgpacks-rs --json state,mergedAt,url
   #   state = "MERGED", mergedAt set  (this PR closed issue #17)
   ```

   > **Watch the repo name.** `rysweet/agent-kgpacks-rs#17` is the real embedding
   > goal. The bare `rysweet/agent-kgpacks#17` is a *different*, unrelated closed
   > autocomplete bug — Simard rejects it as a false lead, and so should you if you
   > verify by hand.
3. **Decides `complete-delivered-goal`** — no human decision is required, because a
   merged PR already delivered the work.
4. **Marks the goal finished** through the intent-revealing, idempotent completion
   verb:

   ```bash
   simard goal complete fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca
   ```

   This marks the goal done, drops it from the active board and backlog, and
   writes a durable tombstone through the authoritative store, so it cannot
   re-stick and restart. (The goal id above is illustrative — the brain uses the
   blocked goal's own id.)

**The Signal messages you receive:**

```
"I looked at the stuck embedding goal — the work it's waiting on already shipped and was merged."
"So I'm going to mark that goal finished; nothing is actually left to do."
"Done — the goal is closed out and off the board. Nothing needed from you."
```

## Verify the outcome

### Outcome 2 — goal completed

```bash
simard goal list | grep -i int8      # the goal is gone (removed), not blocked
```

The goal should no longer appear as active or blocked. Re-running
`simard goal complete <goal_id>` is safe (idempotent) and changes nothing.

### Outcome 1 — done-gate rewritten

```bash
simard goal list | grep -i <goal-keyword>   # the goal's STATUS is no longer
                                             #   "blocked"; it is active again
```

The goal should no longer show a `blocked:` status — its rewritten finish
condition now references something the daemon can observe automatically (an issue
to observe `CLOSED`, a PR to observe `MERGED`, or a file/command to check), so the
goal can complete on its own next cycle. (`simard goal list` prints the board;
it takes only optional `--tag` filters, not a goal id.)

### Outcome 3 — you were asked one question

You will have received **exactly one** plain-English question. Reply on Signal;
your answer feeds the next cycle. If you were asked more than one question, or the
question contained jargon, that is a contract violation — file it (below).

## Verify no markers leaked

Every operator-visible string must be plain English. To spot a leak, scan the
operator-notification path / your Signal history for raw tokens:

```bash
# None of these should appear in any message sent to you:
#   OODA-SAFEGUARD   UNCLEAR-CRITERIA   GENUINELY-STUCK
#   why=   evidence=[   🔒   health-review:stuck-goal
```

If any appear, the translation step failed — see
[Report a bug](./report-a-bug-or-request-a-feature.md) and include the offending
message verbatim.

## When it still reaches you

If the outcome was `ask-operator-one-question`, the escalation flows through the
usual [per-goal escalation backoff](../concepts/blocked-goal-escalation-backoff.md)
(so you are not re-asked every tick) and the reliable two-channel
[operator-notification contract](../reference/overseer-operator-notifications.md).
Answer the single question; Simard takes it from there.

## Related

- [Escalation-triage concept](../concepts/overseer-escalation-triage-course-correction.md)
- [Escalation-triage API reference](../reference/escalation-triage-api.md)
- [Diagnose a no-progress block and read its WHY](./diagnose-a-no-progress-block.md)
- [Re-investigate bare-blocked OODA goals](./reinvestigate-bare-blocked-goals.md)
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md)
