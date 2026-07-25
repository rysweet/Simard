---
title: Gap-scan dedup & exponential backoff
description: >
  Why the Overseer suppresses duplicate gap-cover work with an in-process
  exponential-backoff gate (#4186) instead of re-emitting the same "Cover
  uncovered backlog workstream(s)" issue and the same recurring cognitive-memory
  signature on every tick. Explains the observed self-amplifying loop, why
  fixed-window dedup was insufficient, how exponential backoff rate-limits
  without ever permanently silencing a genuinely recurring gap, how this makes
  the Overseer ACT on a gap once rather than observe it forever (meta bugs
  #4255 / #4126), and the durable cross-process open-issue check — enabled by a
  stable, content-addressed signature — that now backstops the in-process gate
  across daemon restarts.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: concept
status: reference
related:
  - ../reference/overseer-backoff-gate-api.md
  - ../reference/overseer-gap-durable-dedup.md
  - ../howto/configure-gap-durable-dedup.md
  - ../howto/configure-overseer-gap-scan-backoff.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../reference/overseer-recipe-launch-idempotency.md
  - ../reference/overseer-self-observation-stability.md
  - ../howto/diagnose-recurring-cognitive-memory-signature.md
  - ../howto/review-overseer-workstream-gaps.md
  - ./operational-autonomy-model.md
  - ../design/overseer.md
---

# Gap-scan dedup & exponential backoff

> **Operator symptom (issue [#4186](https://github.com/rysweet/Simard/issues/4186)):**
> the Overseer observe loop kept re-emitting *"ProcessHealth — recurring
> signature seen 3× in cognitive memory"* every tick, and kept opening a fresh
> *"Cover uncovered backlog workstream(s)"* issue for the **same** gap — seven
> identical issues
> ([#4190](https://github.com/rysweet/Simard/issues/4190),
> [#4191](https://github.com/rysweet/Simard/issues/4191),
> [#4198](https://github.com/rysweet/Simard/issues/4198),
> [#4201](https://github.com/rysweet/Simard/issues/4201),
> [#4203](https://github.com/rysweet/Simard/issues/4203),
> [#4206](https://github.com/rysweet/Simard/issues/4206))
> plus duplicate recurring-signature issues
> ([#4108](https://github.com/rysweet/Simard/issues/4108),
> [#4124](https://github.com/rysweet/Simard/issues/4124)).

The [workstream gap-scan](../reference/overseer-workstream-gap-scan.md) is the
Overseer step that asks *"what workstreams are we missing?"* and files a covering
issue or launches a covering recipe for each uncovered gap. That step was
**correct once** and **wrong every tick after**: it re-detected the same
still-open gap on the next cycle and acted on it again, because the observe/decide
path had no memory that it had *already acted*. The result was a self-amplifying
loop that turned a single real gap into a stream of duplicate issues and a
never-quieting recurring signal.

This is the concrete instance of meta bugs
[#4255](https://github.com/rysweet/Simard/issues/4255)
(*recurring-blocked-goal cluster + dedup/backoff gap*) and
[#4126](https://github.com/rysweet/Simard/issues/4126)
(*make the Overseer ACT on the gaps it detects instead of just observing*).

## Why a fixed window is not enough

The Overseer already has fixed-window dedup for **whispers**
(the [`WhisperGate`](../reference/overseer-memory-recall-api.md)) and
in-flight-process dedup for **recipe launches**
(the [recipe-launch idempotency rail](../reference/overseer-recipe-launch-idempotency.md)).
Neither closes this seam:

- The `WhisperGate` suppresses within a **single fixed window**. For a condition
  that legitimately persists for days, a fixed window either re-fires too often
  (window too short) or hides a genuinely worsening situation (window too long).
- The idempotency rail suppresses a launch **only while a prior launch is still
  running**. Once a gap-cover launch *completes*, nothing stops the next tick
  from launching an identical one.

What the gap-scan needs is a window that **starts short and grows** the longer a
gap stays unresolved: surface it promptly the first time, then back off so the
same unresolved gap does not flood the backlog — while never silencing it
permanently, because a gap that is still open **is** still a real signal.

## The backoff rail

The fix
([BackoffGate reference](../reference/overseer-backoff-gate-api.md)) is
deliberately additive — it adds a new primitive rather than mutating the
existing `WhisperGate`, so every current caller is untouched. An in-process
exponential-backoff gate guards the gap-cover act path, with a durable
cross-process open-issue equivalence check layered in front of it so the
guarantee survives daemon restarts.

### Exponential BackoffGate (in-process) — implemented

A new `BackoffGate` keyed by the gap's stable signature. The **first** occurrence
of a gap is admitted immediately (surface promptly). Each subsequent occurrence
of the **same** signature within its current window is suppressed, and every
admit **doubles** the window (`900s → 1800s → 3600s → …`, hard-capped at 24 h).
After a silence of `2 × current_window`, the key resets to the 15-minute base —
so if a gap is fixed and then genuinely recurs later, it surfaces promptly again.

Two properties make this safe:

- **Never permanently silent.** The window is capped and resets after silence;
  the gate *rate-limits*, it does not mute.
- **Fail toward surfacing.** A clock regression or any ambiguity resolves to
  *admit*, and the gate can only ever *reduce* actions — it never triggers a
  launch or an issue write.

Because the gate lives in the decide/act seam and only holds a suppressed plan,
it gives a clean guarantee **within one running daemon**: at most one open
covering issue per distinct gap for the life of the process.

### Open-issue equivalence check (cross-process) — implemented

The BackoffGate is in-memory, so a daemon restart forgets its state and a cold
gate could re-file a duplicate that is already open on GitHub. The deeper cause
of the observed `[stewardship] workstream_gap:*` flood (e.g. #4671, #4680,
#4685; OODA-stuck #4689) was that the filed `stewardship-signature:` was keyed
per **run** (`originating-run: overseer-<hash>`), so the already-existing
open-issue search never matched across runs. The fix makes the signature a
stable, content-addressed slug so the durable check closes this: on
the gap-filing path, **before** filing, the Overseer runs a GitHub query
(reusing the existing `stewardship::dedup` helpers and `find_existing` on the
`stewardship-signature:` body marker) for an already-open **equivalent** issue
and **reuses / skips** if one exists. It **fails loud** — a `gh` search error
propagates and files nothing, never a blind create. Because GitHub is the source
of truth, the guarantee is now *at most one open issue per distinct gap
signature across restarts and daemons*. The in-process `WhisperGate` remains the
fast pre-filter in front of it. See the
[durable gap-filing dedup reference](../reference/overseer-gap-durable-dedup.md)
and the [how-to](../howto/configure-gap-durable-dedup.md).

## How this makes the Overseer ACT (not just observe)

Meta bug [#4126](https://github.com/rysweet/Simard/issues/4126) framed the real
defect as *observation without action*: the Overseer saw the gap every tick but
its response — file another issue — was **noise**, not progress. Backoff changes
the behaviour from *"re-announce the gap forever"* to *"surface it once, act on
it, and stay quiet unless it changes or persists past the backoff horizon."* The
gap-cover launch still happens; what stops is the **duplication** of announcing
it. That is the difference between an Overseer that acts and an Overseer that
merely echoes its own observations — the same self-referential failure mode the
[self-observation stability](../reference/overseer-self-observation-stability.md)
work addresses on the recall side.

## Relationship to the autonomy model

Suppressing duplicate gap-cover work is a **preserved safety gate**, not a
weakening of autonomy. As with every gate in the
[operational autonomy model](./operational-autonomy-model.md), the BackoffGate
only ever *removes* an action from the routine path — it can never authorise a
launch, a merge, or an issue write that the objective gates would otherwise
block. It reduces backlog noise so a human (or the Overseer's own decide path)
can see the *real* signal, which is precisely what autonomy is supposed to
protect.

## Invariants

- **One open covering issue per distinct gap, across restarts.** The in-process
  gate holds duplicate coverage plans across ticks; the cross-process case
  (restarts / multiple daemons) is closed by the durable
  [open-issue check](../reference/overseer-gap-durable-dedup.md).
- **Never permanently silent.** The window is capped and resets after silence; a
  genuinely recurring gap always re-surfaces.
- **Additive / non-breaking.** A new `BackoffGate` primitive; existing
  `WhisperGate` callers and the recipe-idempotency rail are unchanged.
- **Fail toward surfacing.** Clock regressions resolve to *admit*, never to
  *silence*.
- **Signature-stable keys.** *"seen 3×"* and *"seen 4×"* collapse to one dedup
  key, so an incrementing count cannot defeat suppression.

## Related reading

- [Overseer BackoffGate & gap-scan dedup reference](../reference/overseer-backoff-gate-api.md)
  — the typed API, config accessors, and wiring.
- [Configure Overseer gap-scan backoff](../howto/configure-overseer-gap-scan-backoff.md)
  — tune the window and verify end-to-end.
- [Overseer workstream gap-scan](../reference/overseer-workstream-gap-scan.md) —
  the step this rail guards.
- [Diagnose a recurring cognitive-memory signature](../howto/diagnose-recurring-cognitive-memory-signature.md)
  — the operator playbook for the symptom.
