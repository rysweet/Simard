---
title: Configure and operate Overseer gap-scan backoff
description: >
  Operator + developer guide for the Overseer's gap-scan duplicate-suppression
  rail (#4186): tuning the exponential window (SIMARD_OVERSEER_BACKOFF_BASE_SECS /
  _MULTIPLIER / _MAX_SECS), confirming the in-process BackoffGate is doing its
  job, diagnosing an over- or under-silenced gap, and verifying the behaviour
  with the virtual-clock unit tests and the gap-scan integration tests.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: howto
status: reference
related:
  - ../concepts/gap-scan-backoff-dedup.md
  - ../reference/overseer-backoff-gate-api.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../reference/overseer-recipe-launch-idempotency.md
  - ./review-overseer-workstream-gaps.md
  - ./diagnose-recurring-cognitive-memory-signature.md
  - ./watch-overseer-activity.md
  - ./configure-overseer-goal-board-health.md
  - ../design/overseer.md
---

# Configure and operate Overseer gap-scan backoff

> **Status: implemented (#4186).** The `SIMARD_OVERSEER_BACKOFF_*` tuning knobs,
> the `BackoffGate` primitive, and its wiring into the gap-scan `gate()`/`act()`
> path ship in the Overseer. For the rationale see
> [gap-scan dedup & backoff](../concepts/gap-scan-backoff-dedup.md); for the
> typed surface, the
> [BackoffGate reference](../reference/overseer-backoff-gate-api.md).

The gap-scan backoff rail stops the Overseer from re-filing the **same**
*"Cover uncovered backlog workstream(s)"* issue and re-emitting the **same**
recurring cognitive-memory signature on every tick. It surfaces a gap **once**,
then backs off exponentially while the gap stays open — without ever permanently
silencing a genuinely recurring gap. This page covers tuning the window and
verifying it end-to-end.

## Always on (no enable flag)

The rail is **always active** — there is no enable/disable environment flag. The
gate can only ever *reduce* actions (a suppressed launch is simply not taken), so
there is no safety reason to switch it off; disabling it would just restore the
duplicate-issue behaviour of #4186. Tune the window instead (below); do not look
for a `SIMARD_OVERSEER_BACKOFF` on/off switch — it does not exist.

The in-process gate's state lives for the lifetime of the daemon process and is
reset by a restart. A cross-process open-issue equivalence check (to also catch
the cold-start case) is planned *future work*, not part of this rail today.

## Tune the exponential window

Three knobs shape the backoff schedule. Each is resolved independently and falls
back to its own default on any invalid value (so a bad knob cannot disable
suppression or overflow the window):

| Env var | Default | Meaning | Fail-safe |
|---------|---------|---------|-----------|
| `SIMARD_OVERSEER_BACKOFF_BASE_SECS` | `900` (15 min) | First window after the initial admit | `> 0`; bad value ⇒ default |
| `SIMARD_OVERSEER_BACKOFF_MULTIPLIER` | `2` | Factor the window grows by each admit | `> 1`; `<= 1` ⇒ default |
| `SIMARD_OVERSEER_BACKOFF_MAX_SECS` | `86400` (24 h) | Hard cap on the window | `> 0`; bad value ⇒ default |

With the defaults, a gap that keeps recurring is suppressed for 15 min, then
30 min, 1 h, 2 h, … doubling up to a 24 h ceiling. After `2 × current_window` of
silence the key resets to 15 min, so a fixed-then-recurring gap surfaces promptly
again.

**Example — a noisier ecosystem that wants slower re-surfacing:**

```bash
export SIMARD_OVERSEER_BACKOFF_BASE_SECS=1800     # start at 30 min
export SIMARD_OVERSEER_BACKOFF_MULTIPLIER=3       # grow x3
export SIMARD_OVERSEER_BACKOFF_MAX_SECS=172800    # cap at 48 h
```

**Example — a tighter loop that wants faster re-surfacing (dev/testing):**

```bash
export SIMARD_OVERSEER_BACKOFF_BASE_SECS=60       # start at 1 min
export SIMARD_OVERSEER_BACKOFF_MULTIPLIER=2
export SIMARD_OVERSEER_BACKOFF_MAX_SECS=3600      # cap at 1 h
```

> **Do not try to disable suppression through the knobs.** A `multiplier <= 1`,
> a zero/negative base, or an unparseable value each falls back to that field's
> default at load, precisely so a misconfiguration cannot silently re-open the
> #4186 duplicate-issue loop. There is no on/off flag; the rail is always on.
> Each knob validates only its **own** field — there is no cross-field coercion
> (e.g. `max` is not raised relative to `base`), so keep `max >= base` yourself
> if you want the exponential ramp to actually grow.

> **Not to be confused with `SIMARD_OVERSEER_TRANSIENT_BACKOFF_CEILING`.**
> That pre-existing knob (#893) is the consecutive-transient **self-heal**
> ceiling — an unrelated "backoff". The `SIMARD_OVERSEER_BACKOFF_*` flags on
> this page only govern gap-scan duplicate suppression.

## Observe suppression

Suppression is surfaced through the tick's **held-plan** output, not a dedicated
trace target. When the gate suppresses a coverage relaunch, the plan is held with
the reason string (visible in the tick's `action_details`):

> `held: an equivalent coverage was launched recently (backoff window)`

Watch the Overseer's activity with the
[activity how-to](./watch-overseer-activity.md); a healthy, already-covered gap
shows this held reason each tick instead of a new issue. A dedicated
`overseer::gap_scan` structured trace/counter, an open-issue equivalence-check
skip, and an over-silence alert are **future work** (see the
[reference](../reference/overseer-backoff-gate-api.md#observability)), aligned
with the planned cross-process layer 2.

## Confirm it is doing its job

1. **One open issue per gap.** For a known uncovered workstream, confirm the
   backlog holds exactly **one** open *"Cover uncovered backlog workstream(s)"*
   issue — not the seven of #4186. New ticks should show the held-plan *backoff
   window* reason, not a new issue.
2. **Prompt first surfacing.** A genuinely **new** gap (a fresh `dedup_key`) is
   admitted on its first tick — verify the covering launch/issue appears without
   a 15-minute delay.
3. **Recurrence after silence.** If a covered gap is resolved and then genuinely
   recurs after a long quiet period, confirm it surfaces again (the key reset
   after `2 × window` of silence).
4. **Stable count keys.** Confirm a signature whose human text changes from
   *"seen 3×"* to *"seen 4×"* is still treated as the **same** key (it collapses
   on the stable signature hash), so the incrementing count does not defeat
   suppression.

## Diagnose

**A gap is over-silenced (a real, worsening gap stays quiet too long).**
Lower `SIMARD_OVERSEER_BACKOFF_MAX_SECS` so the window ceiling is shorter, or
lower the base/multiplier. Remember the gap is not lost — it is rate-limited; the
covering issue is already open.

**Duplicate issues still appear.** Confirm the daemon is on a post-#4186 binary.
Because the gate's state is in-memory, duplicates **immediately after a restart**
are expected until the gap is admitted once on the new process (the cold-start
case the planned layer-2 equivalence check would close).

**A brand-new gap is not surfacing.** A first-seen `dedup_key` must return
`Admit`. If it does not, the key is colliding with an existing one — the
`dedup_key` should be `"{signal_kind}:{stable_signature}"` and unique per
distinct gap.

## Verify with the tests

The behaviour is pinned by the same test files the feature ships with:

```bash
# Virtual-clock unit tests: suppress-within-window, exponential growth,
# reset-after-silence, key independence, overflow / clock-regression safety.
cargo test -p simard --lib overseer::tests_whisper

# Integration: a completed WorkstreamCoverage launch does not create a second
# issue next tick; relaunch after the window elapses; distinct signatures
# unaffected.
cargo test -p simard --lib overseer::tests_gap_scan
```

## What you should *not* do

- **Do not** widen the window to "make the signal go away" — a persistent gap is
  a real signal; suppress its *duplication*, not the gap itself. If it is noise,
  fix the gap, do not mute it.
- **Do not** look for an on/off flag to force a gap to re-appear every tick — the
  rail is always on; use the held-plan reason or the covering issue to track it.
- **Do not** mutate or auto-close the covering issue — the rail is
  suppress-only by design, and does not touch existing issues.

## Related reading

- [Gap-scan dedup & backoff](../concepts/gap-scan-backoff-dedup.md) — the concept.
- [Overseer BackoffGate & gap-scan dedup reference](../reference/overseer-backoff-gate-api.md)
  — the typed API and config accessors.
- [Review the Overseer's workstream gaps](./review-overseer-workstream-gaps.md) —
  reading the gaps this rail guards.
- [Diagnose a recurring cognitive-memory signature](./diagnose-recurring-cognitive-memory-signature.md)
  — the operator playbook for the #4186 symptom.
