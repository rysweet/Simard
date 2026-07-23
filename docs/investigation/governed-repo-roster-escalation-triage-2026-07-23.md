# Escalation Triage Record — move the governed-repo roster out of framework code (2026-07-23)

Goal id: `move-the-governed-repo-roster-out-of-framework-a8f57a50`
Repository: [`rysweet/Simard`](https://github.com/rysweet/Simard)
Playbook: [`prompt_assets/simard/overseer/escalation_triage.md`](../../prompt_assets/simard/overseer/escalation_triage.md)
Done-gate acceptance test: [`#4448`](https://github.com/rysweet/Simard/issues/4448)
Delivering change: [`#4440`](https://github.com/rysweet/Simard/pull/4440)

This is the durable record of the Overseer's agentic escalation triage of the
recurring "move Simard's governed-repo roster out of the framework and into her
own curated identity" goal after it was flagged as blocked. It captures the two
things the playbook requires that the code/state change does not otherwise
record:

1. the **plain-English Signal message** the operator received — one
   consolidated, jargon-free update covering the situation and the decision (no
   raw diagnostic markers surfaced); and
2. the **reconciled root cause**, including the secondary failure in the
   automatic "this goal is stuck" alert path.

---

## The recorded block (translated to plain English)

The goal had been stuck across multiple cycles. Every cycle hit the same wall
and stopped: Simard could not automatically tell when this goal was finished.
Its finish line was a free-text description of the outcome — a phrase no
automated check ever measured. With nothing machine-checkable to certify, each
cycle re-investigated and shipped nothing, and the goal kept re-blocking.

A second, quieter problem sat underneath it: when the no-progress breaker tried
to file its operator-facing "this goal is stuck" tracking issue, the filing
failed silently, so the stall never surfaced through the normal alert path.

## Ground truth (authoritative, from repository state)

| Artifact | State | Detail |
| --- | --- | --- |
| Done-gate acceptance test `#4448` | **OPEN**, concrete & machine-checkable | A single acceptance test with three falsifiable assertions: the roster is identity-seeded, runtime-mutable, and deploy-durable. Closes on merge of its delivering PR. |
| Delivering change `#4440` | **OPEN + MERGEABLE** | `refactor(roster): move governed-repo roster into identity-curated durable state`; body declares `Closes #4448`. Once merged, `#4448` closes and the done-gate can observe it. |
| `ooda-stuck` GitHub label | **was MISSING, now CREATED** | The no-progress breaker files its tracking issue with `gh issue create --label ooda-stuck`; the label did not exist, so every filing failed. Tracked by `#4394`, `#4472`, `#4474`; code hardening in flight in PRs `#4456` and `#4478`. |

The work the goal asks for is delivered by PR `#4440`. What was missing was never
the work — it was a finish line a machine could check, plus a working alert path
when the goal stalled.

## Root cause

The goal record carried an **unmeasurable free-text finish line** that no
automated check ever enforced, so the done-check could never certify completion
and the goal re-blocked every cycle. A secondary failure compounded it: the
no-progress breaker's tracking-issue filing hard-depends on a `ooda-stuck`
GitHub label that **did not exist in the repository**, so the breaker's
operator-facing escalation silently failed and the stall never surfaced.

## Course-correction (decision: rewrite the done-gate to be machine-checkable)

Per the playbook, the block was fixed agentically rather than dumped on a human.

1. **Machine-checkable finish line.** The unmeasurable prose finish line was
   replaced with a concrete, daemon-observable outcome: acceptance test issue
   `#4448` (three falsifiable assertions), bound to delivering PR `#4440` via
   `Closes #4448`. When `#4440` merges, `#4448` closes and an OODA cycle can
   certify this goal done by observing that closed issue — no operator decision
   required, so nothing was escalated to a person.

2. **Repaired the broken alert path (accounting for the escalation failure).**
   The missing `ooda-stuck` label — the exact reason the no-progress breaker
   could not file its operator-facing tracking issue — was **created** in
   `rysweet/Simard`, so the escalation path works again immediately. The
   durable code hardening (auto-ensuring the label before filing) is already in
   flight in PRs `#4456` and `#4478`; this triage does not duplicate that code
   change, it removes the live blocker they harden against.

Because the delivering PR `#4440` is not yet merged, the goal was **not** marked
complete prematurely; it now has a machine-checkable finish line and will
self-certify on merge.

## Signal message sent to the operator

Delivered to the allowlisted operator over the live signal-cli JSON-RPC channel
(send accepted, `type: SUCCESS`). Jargon-free; no raw diagnostic markers
surfaced.

> Update on one of my longer-running goals — moving my list of the repositories
> I look after out of the framework code and into my own curated settings, so I
> own it directly.
>
> It kept getting stuck for a simple reason: it had no clear "finished" line I
> could check on my own, so every time I looked at it I re-did the analysis and
> shipped nothing. It was never a hard problem — just an un-checkable goal.
>
> I fixed that myself. I gave it a concrete finish line: a single acceptance
> test (tracked as issue #4448) that proves the roster is owned by my identity,
> can be changed at runtime, and survives a redeploy. The change that delivers
> it (pull request #4440) is written and ready to merge, and it automatically
> closes that issue when it lands. Once it merges, this goal can certify itself
> as done — nothing is needed from you.
>
> I also cleared a small snag that had been silently blocking my automatic "this
> goal is stuck" alerts, so future stalls will surface properly.

## Verification

- `#4448` is a single acceptance test with three machine-checkable assertions;
  `#4440` is OPEN + MERGEABLE and declares `Closes #4448`.
- `ooda-stuck` label now exists in `rysweet/Simard` (`gh label list`), so the
  no-progress breaker's `gh issue create --label ooda-stuck` filing succeeds.
- The Signal send was accepted by the daemon with `type: SUCCESS` and contains
  none of the forbidden markers (`OODA-SAFEGUARD`, `UNCLEAR-CRITERIA`,
  `GENUINELY-STUCK`, `why=`, `evidence=[`, the lock token, or
  `health-review:blocked-unmeasurable-criteria`).
