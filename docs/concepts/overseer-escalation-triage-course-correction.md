---
title: The Overseer triages and course-corrects a blocked goal before escalating to a human
description: >
  Why the Simard Overseer no longer ships a raw machine marker to a person the
  moment a goal looks blocked. Behind the thin `act_escalate_blocked_goal` rail,
  an agentic escalation-triage recipe (prompt_assets/simard/overseer/escalation_triage.md,
  #4276) INSPECTS the block, restates it in plain English, attempts a root cause,
  and COURSE-CORRECTS it agentically — rewriting an unmeasurable done-gate to be
  machine-checkable, completing a goal a merged PR already delivered, or asking the
  operator exactly ONE plain-English question — escalating to a person only when a
  human decision is genuinely required. The operator hears the reasoning as
  jargon-free Signal updates and never sees a raw marker.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
issues: ["#4276"]
related:
  - ./agentic-recipes-first-principle.md
  - ./overseer-agentic-health-review.md
  - ./blocked-goal-escalation-backoff.md
  - ./ooda-reinvestigate-blocked-goals.md
  - ./overseer-root-cause-why.md
  - ../reference/escalation-triage-api.md
  - ../reference/overseer-operator-notifications.md
  - ../howto/triage-and-course-correct-a-blocked-goal.md
---

# The Overseer triages and course-corrects a blocked goal before escalating

> **Status: implemented (issue #4276).** When the Overseer decides a goal is
> genuinely blocked, it no longer forwards a bare machine marker to a human and
> counts it done. It first runs the **escalation-triage** reasoning recipe
> ([`prompt_assets/simard/overseer/escalation_triage.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/escalation_triage.md)),
> which restates the block in plain English, attempts a root cause, and
> **course-corrects it agentically** — escalating to a person only when a human
> decision is genuinely theirs to make. The Rust seam
> `overseer::act_escalate_blocked_goal` is only a thin structured trigger; the
> "WHY + remedy + decide" reasoning lives in the recipe. API details:
> [escalation-triage API reference](../reference/escalation-triage-api.md).

## The defect this fixes

The Overseer marks a goal **blocked** when its own OODA loop can make no progress.
Before this change, "blocked" meant one thing: fire `EscalateBlockedGoal`, hand a
human the raw diagnostic — something like

```
🔒 [OODA-SAFEGUARD] recurring blocked goal why=UNCLEAR-CRITERIA evidence=[…]
```

— and consider the goal "handled." Two problems compounded:

1. **The operator got jargon, not a decision.** A non-engineer cannot act on
   `OODA-SAFEGUARD … why=UNCLEAR-CRITERIA`. It names an internal safeguard, not
   what is actually wrong or what to do about it.
2. **Simard gave up too early.** Many "blocked" goals are machine-fixable: the
   finish condition simply can't be checked automatically, or the work has
   *already shipped* in a merged PR and only the tracking state never caught up.
   Escalating those to a human is busywork Simard should absorb itself.

The kgpacks-rs int8/PQ embedding goal is the canonical case. The underlying work
([`rysweet/agent-kgpacks-rs#17`](https://github.com/rysweet/agent-kgpacks-rs/issues/17))
was **already delivered by a merged PR**, but the goal's tracking state never
reconciled against the closed issue, so its OODA loop kept restarting, hit the
consecutive-failure cooldown over and over for hours, and produced a bare
"needs human review" marker — for work that was **done**.

## The principle: reason, then course-correct, then (only maybe) escalate

This capability is the escalation-time application of the
[agentic-recipes-first principle](./agentic-recipes-first-principle.md) and the
exact mirror, for a *blocked goal*, of what the
[agentic health-review](./overseer-agentic-health-review.md) does for a
*crash-loop*. The decision to escalate-vs-course-correct belongs to the **recipe**,
not to a bare integer threshold on the Rust side.

When a goal is handed to the triage brain, it works in this order:

1. **Restate the problem in plain English.** Every internal marker is translated
   for a non-engineer. The operator never sees `OODA-SAFEGUARD`,
   `UNCLEAR-CRITERIA`, `GENUINELY-STUCK`, `why=`, `evidence=[…]`, the `🔒` lock
   token, or a `health-review:stuck-goal` reason marker. Instead the operator
   hears, e.g., *"Simard can't automatically tell when this goal is finished, so
   it keeps re-investigating without shipping anything."*
2. **Recommend the smallest concrete next step** that unblocks the goal, in plain
   English.
3. **Attempt a root cause and decide the course-correction** (below).
4. **Send one jargon-free Signal update per step** so the operator can follow the
   reasoning as it happens.

## The three course-corrections

The brain attempts to fix the block itself before asking a human, and chooses
**exactly one** of:

| Decision | When it applies | What Simard does |
| --- | --- | --- |
| **`rewrite-done-gate`** | The goal's finish condition can't be measured automatically — the done-gate can never certify it. | Re-scope the done-criteria so completion is **machine-verifiable**: a specific issue the daemon can observe `CLOSED`, a specific PR it can observe `MERGED`, or a specific file/command whose presence or output the done-gate can check. The rewrite is *applied* via Simard's agentic capabilities (editing the goal / its tracking issue), not merely proposed. |
| **`complete-delivered-goal`** | The work the goal describes has **already shipped** — a merged PR already delivered it. | Mark the goal complete rather than leaving it blocked, citing the merged PR / closed issue as machine-checkable proof. |
| **`ask-operator-one-question`** | A human decision is **genuinely required** — the intent is ambiguous, or a scope call is the operator's to make. | Ask exactly **one** crisp plain-English question. Never a wall of jargon; never more than one question. |

The decision is made **from the evidence**, not gated by a recurrence count or any
bare threshold. Only the third branch escalates to a person; the first two resolve
the block without human involvement.

### Worked example: the kgpacks-rs int8/PQ goal (`complete-delivered-goal`)

- **Symptom.** Goal `fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-…` failed five
  times in a row and kept restarting without completing; over 6h it repeatedly hit
  the consecutive-failure cooldown with no progress.
- **Plain-English problem** (what the operator hears): *"A task to finish some
  embedding work has been retrying for hours without getting anywhere — it looks
  stuck on something it can't get past."*
- **Root cause.** The work was **already delivered** by a **merged PR**
  ([#40](https://github.com/rysweet/agent-kgpacks-rs/pull/40), which closed
  [issue #17](https://github.com/rysweet/agent-kgpacks-rs/issues/17)). The goal's
  tracking state never reconciled against the closed issue, so it kept restarting.
  > **Target discipline.** Pin the fully-qualified `rysweet/agent-kgpacks-rs#17`.
  > The bare `rysweet/agent-kgpacks#17` is an **unrelated closed autocomplete bug**
  > — a false lead the triage brain must reject.
- **Decision.** `complete-delivered-goal`. `escalate = null` — no human decision
  is required, because the asset says to mark a goal complete when a merged PR
  already delivered it, and that condition is objectively met.
- **Signal cadence the operator sees.** *"I looked at the stuck embedding goal — the
  work it's waiting on already shipped and was merged a couple of weeks ago."* →
  *"So I'm going to mark that goal finished; nothing is actually left to do."* →
  *"Done — the goal is closed out and off the board. Nothing needed from you."*

## Where the reasoning lives (and where it does not)

Imperative Rust owns only the **thin deterministic rails** — dispatch, I/O,
storage, and the scheduling tick. `overseer::act_escalate_blocked_goal` is the
structured trigger that gathers the goal id, the internal diagnostic markers, and
a seed problem/next-step, then launches the triage recipe. It does **not** decide
whether to escalate.

The **judgment** — translate the markers, find the root cause, pick one of the
three course-corrections, apply it, and phrase the operator updates — lives in the
agentic recipe. This is deliberate:

- **No new threshold counter.** We do not add a "Nth consecutive failure ⇒
  escalate to human" integer gate. The evidence, read by an agent, decides.
- **No brittle heuristics.** The three-way decision is reasoning over observable
  state (the issue's `CLOSED`/open status, a PR's `MERGED` status, whether the
  done-gate can certify completion), not a string-match rule.

This mirrors [`self_diagnose.md`](https://github.com/rysweet/Simard/blob/main/prompt_assets/simard/overseer/self_diagnose.md)
for a step failure: the Rust side detects and structures; the recipe reasons and
remedies.

## Guardrails on any change the brain makes

When the course-correction edits code or config (e.g. hardening a done-gate), the
change must be **additive / non-breaking, CI-green, and merge-ready**. No `Bridge`
naming. No stray `print!` in new code — structured `tracing` + OTel only. No silent
fallbacks. Completing a delivered goal uses the intent-revealing completion verb
`simard goal complete`, which marks the goal done, removes it, and writes a durable
tombstone so it cannot re-stick or be re-seeded; the brain never writes the
goal-board store directly, and verifies the delivered state from **parsed** GitHub
state (issue `CLOSED` + PR `MERGED`), never inferred from free text.

## Interaction with the escalation backoff

This triage step runs **before** the operator-notification path. When the outcome
*is* `ask-operator-one-question`, the resulting escalation still flows through the
[per-signature exponential backoff](./blocked-goal-escalation-backoff.md) so the
same goal is not re-asked every tick, and through the reliable two-channel
[operator-notification contract](../reference/overseer-operator-notifications.md).
When the outcome is `rewrite-done-gate` or `complete-delivered-goal`, no human is
notified as an escalation at all — the operator instead receives the plain-English
"found / decided / done" progress updates.

## Related

- [Agentic-recipes-first reasoning principle](./agentic-recipes-first-principle.md)
- [Overseer agentic health-review](./overseer-agentic-health-review.md)
- [Blocked-goal escalation backoff](./blocked-goal-escalation-backoff.md)
- [Re-investigating already-blocked OODA goals](./ooda-reinvestigate-blocked-goals.md)
- [Escalation-triage API reference](../reference/escalation-triage-api.md)
- [How to triage and course-correct a blocked goal](../howto/triage-and-course-correct-a-blocked-goal.md)
