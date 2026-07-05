---
title: Configure and observe the Simard Whisperer
description: Operator guide for the Overseer's lightweight whisper steering — enabling/disabling with SIMARD_OVERSEER_WHISPER, tuning the dedup window, per-hour cap, and meeting-escalation threshold, reading whisper tracing and operator notifications, understanding how a whisper reaches Simard's next OODA cycle, confirming the advisory-not-command and fail-closed-identity guarantees, and verifying the feature end-to-end with injected fakes.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/simard-whisperer.md
  - ../reference/simard-whisperer-api.md
  - ../design/overseer.md
  - ./configure-self-quality-audit.md
---

# Configure and observe the Simard Whisperer

> **Status: design specification — not yet implemented (issue
> [#2605](https://github.com/rysweet/Simard/issues/2605), open).**
>
> This guide documents the **intended** operator surface. The
> `SIMARD_OVERSEER_WHISPER*` environment variables, the `overseer::whisper`
> traces, the `whisper` operator notifications, and the `cargo test` targets
> below do **not** exist in a running daemon yet — they describe how the feature
> will be configured and observed once the implementation PR lands. Until then,
> setting these variables has no effect.

The **Overseer** can steer Simard (the main OODA loop) with a lightweight,
**advisory** *whisper* — a short steering note injected onto Simard's meeting-handoff
inbox and folded into her reasoners' context at the start of her next cycle. Whispers
fire when the Overseer sees Simard **looping** (repeated no-action) or **drifting**
from a goal's intent. The whisper never takes the action for her; her reasoners still
decide.

This guide shows how to enable, tune, observe, and verify whispering. For the concept
see [The Simard Whisperer](../concepts/simard-whisperer.md); for the API contract see
the [Simard Whisperer API](../reference/simard-whisperer-api.md).

!!! note "Activation requires a redeploy"
    These knobs are read from the environment at daemon start. Set them **before**
    launching (or relaunching) the daemon. After merging a Whisperer change, the
    operator redeploys the binary to activate it — the running daemon does not pick up
    new code until then.

## When to use this

Use this guide when:

- You want to turn whispering on or off, independently of the rest of the Overseer
- You want to change how aggressively the Overseer whispers (window, per-hour cap)
- You want the Overseer to escalate to a full meeting sooner or later
- The daemon emitted `overseer::whisper` traces, or you received a `[Overseer]
  whisper: …` operator notification, and you want to understand it
- You need to confirm a whisper actually reached Simard's next cycle
- You need to verify the advisory / fail-closed guarantees

## Enable, disable, and tune

All knobs are environment variables, read once at daemon start.

| Knob | Env var | Default | What it controls |
| --- | --- | ---: | --- |
| On/off | `SIMARD_OVERSEER_WHISPER` | enabled with Overseer | Master gate. Opt-**out**: explicit falsey disables. |
| Dedup window | `SIMARD_OVERSEER_WHISPER_WINDOW_SECS` | `900` | Suppress an identical whisper seen within this many seconds. |
| Per-hour cap | `SIMARD_OVERSEER_WHISPER_CAP_PER_HOUR` | `5` | Max whispers admitted per rolling hour across all signatures. |
| Escalate after | `SIMARD_OVERSEER_WHISPER_ESCALATE_AFTER` | `3` | Same-signature whispers before escalating to a full meeting. |

### On/off — opt-out semantics

`SIMARD_OVERSEER_WHISPER` follows the same **opt-out** convention as the acting
Overseer: it is **enabled by default whenever the Overseer is enabled**, and only an
explicit falsey value disables it.

```bash
# Disable whispering (Overseer still runs, just never whispers)
export SIMARD_OVERSEER_WHISPER=off      # or: 0 | false | no

# Explicitly enable (default when the Overseer is enabled)
export SIMARD_OVERSEER_WHISPER=on       # or: 1 | true | yes

# Restore the default (enabled-with-Overseer) — just unset it
unset SIMARD_OVERSEER_WHISPER
```

Recognised **falsey** values (case-insensitive, trimmed): `0`, `false`, `no`, `off`.
Everything else — unset, empty, `1`, `true`, `yes`, `on`, or an unrecognised string —
leaves whispering **enabled**, provided the Overseer itself is enabled
(`SIMARD_OVERSEER_ENABLED` is not falsey). If the Overseer is disabled, whispering is
disabled too, regardless of this flag.

### Tuning aggressiveness

```bash
# Whisper about the same condition at most once every 30 min (less chatty)
export SIMARD_OVERSEER_WHISPER_WINDOW_SECS=1800

# Allow at most 3 whispers per hour total
export SIMARD_OVERSEER_WHISPER_CAP_PER_HOUR=3

# Escalate to a full meeting after 2 repeats instead of 3
export SIMARD_OVERSEER_WHISPER_ESCALATE_AFTER=2
```

- **Window** larger ⇒ fewer repeated nudges about the same thing.
- **Cap** smaller ⇒ a hard ceiling on total whispers per hour (dedup runs first, the
  cap second).
- **Escalate-after** smaller ⇒ the Overseer opens a full meeting sooner when a
  condition will not clear. High-urgency whispers escalate immediately regardless of
  this count.

## How a whisper reaches Simard

You do not deliver whispers manually — the Overseer composes and delivers them. The
path is:

1. **Overseer tick.** The Overseer observes `consecutive_no_action` and drift for the
   active goal. At **2** consecutive no-action cycles (one below the no-progress
   breaker at **3**), or on detected drift, it composes a one- or two-sentence note.
2. **Gate.** The `WhisperGate` suppresses duplicates within the window and enforces
   the per-hour cap; `RecursionGuard` refuses if the steward identity is unconfigured.
3. **Deliver.** The whisper is written as a real
   `<state_root>/meeting_handoffs/handoff-<rfc3339>.json` with `themes:
   ["overseer-whisper"]`, the note in `open_questions`, and **empty** `decisions` /
   `action_items`.
4. **Pickup.** At the **start of Simard's next OODA cycle**, her inbox scan
   (`check_meeting_handoffs` + `observe()`) folds the note into her reasoners' Observe
   context as advisory input, then marks the handoff processed. Because the handoff
   carries no decisions or action items, it **never becomes a goal or backlog item**.

Delivery is asynchronous: a whisper written during Overseer tick *T* appears in
Simard's context at cycle *T+1*.

## Observe whispers

### Structured tracing

Every whisper emits one `tracing` event on target `overseer::whisper`:

```
INFO overseer::whisper trigger="loop_detected" note="No action for 2 cycles on g-42; reconsider or switch sub-task." urgency=Normal signature="loop:g-42:…" delivered=true suppressed="" path="…/meeting_handoffs/handoff-2026-07-05T15-40-12Z.json" overseer whisper
```

Key fields:

- `trigger` — `loop_detected` or `drift_correction`
- `note` — the exact steering text delivered
- `urgency` — `Low` | `Normal` | `High`
- `delivered` — `true` when written, `false` when suppressed/refused
- `suppressed` — `""`, `duplicate`, or `cap`
- `signature` — the dedup signature
- `path` — the written handoff file (when delivered)

The per-tick summary event (`overseer::tick`) additionally carries `whispers` and
`whispers_suppressed` counts.

### Operator notifications

Each **delivered** whisper is surfaced to the operator through the mandatory
dual-channel notifier as an `OperatorNotification` of kind `"whisper"`:

- **Subject:** `[Overseer] whisper: <headline>`
- **Body:** the steering note, the trigger, and the affected goal

Suppressed whispers are traced but **not** notified, to keep the operator channels
quiet.

### On disk

You can see pending whispers as handoff files in the inbox:

```bash
ls -1 "$SIMARD_STATE_ROOT/meeting_handoffs/"
# handoff-2026-07-05T15-40-12Z.json   ← a whisper (themes: ["overseer-whisper"])

# Inspect one:
jq '{themes, decisions, action_items, open_questions, participants, processed}' \
  "$SIMARD_STATE_ROOT/meeting_handoffs/handoff-2026-07-05T15-40-12Z.json"
```

A whisper handoff always shows `themes` containing `"overseer-whisper"`, **empty**
`decisions` and `action_items`, the note in `open_questions`, and the Overseer's
steward login in `participants`. Once Simard's next cycle consumes it, `processed`
flips to `true`.

## Confirm the guarantees

### Advisory, not command

A whisper cannot create work or fake a decision. Because the whisper handoff carries
empty `decisions` and empty `action_items`, Simard's curation step
(`check_meeting_handoffs`) has nothing to promote — it folds the note into context and
marks the handoff processed without creating any goal, backlog item, or planned
action. Simard's reasoners produce their **own** next decision with the whisper as one
additional input.

### Distinct identity and no self-whisper

Whispers are authored under the Overseer's **steward login**
(`SIMARD_OVERSEER_AUTHOR_LOGIN`, default `simard-overseer[bot]`) — never the operator's
login and never Simard's. Simard's observation ignores overseer-authored handoffs, so
the Overseer cannot observe its own whisper and whisper about it.

If the steward identity is unconfigured, the whisper is **refused** (fail-closed) and
nothing is written — you will see a trace with `delivered=false` and an
`anti-recursion` refusal, and no `handoff-*.json` appears. To configure the identity:

```bash
export SIMARD_OVERSEER_AUTHOR_LOGIN="simard-overseer[bot]"
```

### Escalation to a meeting

When the same condition keeps recurring (same-signature whispers reach
`SIMARD_OVERSEER_WHISPER_ESCALATE_AFTER`) or a whisper is `High` urgency, the Overseer
stops whispering and opens a **full meeting** with Simard via the existing
`TransferGoal`/`MeetingHost` path — the heavier steering channel. The lightweight
whisper remains the default; escalation is the exception. You will see the
`overseer::tick` report show a goal transfer rather than a whisper for that cycle.

## Verify

The Whisperer is fully covered by tests that inject fakes for observed-state, the
whisper sink, the clock, and the identity — **no network**. Run them with:

```bash
# Whisperer unit + integration tests
cargo test -p simard overseer::whisper
cargo test -p simard whisper
```

The suite proves, at minimum:

- **Delivery reaches the next cycle.** A loop/drift condition produces a whisper
  written as a real `handoff-*.json`; a subsequent OODA `observe()`/`check_meeting_handoffs`
  finds the note in the next cycle's reasoner context (via a `tempfile` inbox).
- **Advisory.** The delivered handoff has empty `decisions` and `action_items`; no
  goal/backlog item/planned action is fabricated.
- **Dedup + cap.** An identical whisper on the next tick is suppressed within the
  window (single delivery); a 6th whisper in an hour is capped.
- **Fail-closed identity.** With the steward identity unset, the whisper is refused
  and the sink is never called.
- **No self-whisper.** An overseer-authored handoff is ignored by `signals_from`.
- **Escalation.** Repeated or `High`-urgency conditions transfer a goal via
  `MeetingHost`; the default path stays a whisper.
- **Config.** A falsey `SIMARD_OVERSEER_WHISPER` yields no whisper; the default yields
  one when the condition holds.
- **Isolation.** A whisper-sink error or panic is caught by the isolated tick; the
  report records it (`errors` / `panicked`) and the daemon continues.
- **Observability.** The tracing fields and the operator `whisper` notification are
  emitted for each delivered whisper.

## Troubleshoot

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| No whispers ever, condition clearly holds | `SIMARD_OVERSEER_WHISPER` falsey, or the Overseer disabled (`SIMARD_OVERSEER_ENABLED` falsey) | Unset the flag or set a truthy value; ensure the Overseer is enabled; redeploy. |
| Trace shows `delivered=false`, `anti-recursion` refusal | Steward identity unconfigured | Set `SIMARD_OVERSEER_AUTHOR_LOGIN`; redeploy. |
| Same whisper repeats every cycle | Window too small | Increase `SIMARD_OVERSEER_WHISPER_WINDOW_SECS`. |
| Whispers stop after a few per hour | Per-hour cap reached | Increase `SIMARD_OVERSEER_WHISPER_CAP_PER_HOUR` (or accept the ceiling). |
| Overseer opens meetings instead of whispering | Escalation threshold reached, or `High` urgency | Raise `SIMARD_OVERSEER_WHISPER_ESCALATE_AFTER` if you want more whispers before a meeting. |
| A whisper became a goal/backlog item | Should be impossible — whisper handoffs carry empty `decisions`/`action_items` | File a bug: inspect the offending `handoff-*.json`; a non-empty `decisions`/`action_items` on an `overseer-whisper` handoff is the defect. |

## See also

- Concept: [The Simard Whisperer](../concepts/simard-whisperer.md)
- API reference: [Simard Whisperer API](../reference/simard-whisperer-api.md)
- Design: [Overseer — operator/observer co-process](../design/overseer.md)
- Related: [Configure the monthly self-quality-audit](./configure-self-quality-audit.md)
