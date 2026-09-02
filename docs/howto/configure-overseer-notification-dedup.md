---
title: Configure and operate Overseer notification dedup
description: >
  Operator + developer guide for the Overseer notifier's signature-based dedup rail
  (#4579): why the operator no longer gets one identical self-deploy-refused notice
  per deploy cycle, tuning the cooldown/digest window with
  SIMARD_OVERSEER_NOTIFY_DEDUP_SECS, confirming suppression is working from the logs
  and overseer status, understanding exactly which notices are (and are not)
  deduped, and verifying the behaviour with the module's unit tests.
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: howto
status: reference
issues: ["#4579"]
related:
  - ../reference/overseer-operator-notification-dedup.md
  - ../reference/overseer-operator-notifications.md
  - ../reference/overseer-deploy-canary-diagnostics.md
  - ./configure-overseer-email-notifications.md
  - ./configure-overseer-signal-rpc-notifications.md
  - ./configure-overseer-gap-scan-backoff.md
  - ./watch-overseer-activity.md
  - ../design/overseer.md
---

# Configure and operate Overseer notification dedup

> **Status: implemented (#4579).** The signature-based dedup gate in
> `DualChannelNotifier::notify()` and the `SIMARD_OVERSEER_NOTIFY_DEDUP_SECS` knob
> ship in the Overseer notifier. For the typed surface and the full dispatch-vs-suppress
> contract, see the
> [notification dedup reference](../reference/overseer-operator-notification-dedup.md).

## What this rail fixes

While the running Overseer binary is behind `origin/main`, it re-attempts self-deploy
**every deploy cycle** (~15–25 min). Each attempt whose canary reds emits an
**identical** `deploy-refused` notice — before this rail, the operator got one such
`self-deploy refused … red canary (gate unit-test …) … Drop t…` message on **both**
Signal and email, per attempt, indefinitely.

This rail suppresses **identical repeats** of an already-sent failure notice, so the
operator gets:

- the **first** occurrence of a distinct failure **immediately**, then
- **silence** while that exact failure keeps recurring, then
- at most **one periodic reminder** per cooldown window (default 60 min) while it stays
  stuck — instead of one message per attempt.

**Nothing else is quieted.** Every attempt is still logged at **WARN** and still
reflected in the overseer **status** field — only the push notification is deduped.

## Always on (tune the window to disable)

The rail is **always active** — there is no enable/disable boolean. It can only ever
*reduce* push notifications (a suppressed notice is simply not sent on Signal/email),
and it is **fail-open**: any internal error dispatches rather than suppresses. To turn
it off, set the window to `0` (see below); do not look for a
`SIMARD_OVERSEER_NOTIFY_DEDUP` on/off switch — it does not exist.

## Tune the cooldown / digest window

One knob shapes the schedule. It falls back to its default on any invalid value, so a
bad setting cannot wedge the notifier:

| Env var | Default | Meaning | Fail-safe |
|---------|---------|---------|-----------|
| `SIMARD_OVERSEER_NOTIFY_DEDUP_SECS` | `3600` (60 min) | While a failure keeps producing the **same** signature, suppress repeats within this window; after it elapses, dispatch **one** reminder and restart the window. | Unset / empty / non-numeric → `3600`. |

Set it in the Overseer's systemd unit (or shell) and restart the daemon, e.g.:

```ini
# /etc/systemd/system/simard-overseer.service  (drop-in)
[Service]
Environment=SIMARD_OVERSEER_NOTIFY_DEDUP_SECS=1800   # 30-min reminders
```

```bash
sudo systemctl daemon-reload && sudo systemctl restart simard-overseer
```

Guidance:

- **Quieter operator, slower reminders** → larger value (e.g. `10800` = 3 h).
- **More frequent nudges while stuck** → smaller value (e.g. `900` = 15 min, roughly one
  reminder per deploy cycle).
- **Disable entirely** → `0` (every identical repeat dispatches; equivalent to the old
  one-per-attempt behaviour).

The value is read **per notification**, so the change takes effect on the next notice —
you do not strictly need a restart if you can set the env for the running process, but a
systemd `Environment=` change does require `daemon-reload` + `restart`.

## What is and isn't deduped

Only **pure-failure** kinds are suppressible. **State-change** notices always go through
immediately, so you never miss a start, a success, a recovery, or a genuinely new
problem:

| Notice | Deduped? |
|--------|----------|
| `deploy-refused` (red canary, self-deploy blocked) | **yes** |
| `goal-blocked` (needs-human escalation) | **yes** |
| `workstream-gap` (uncovered backlog) | **yes** |
| `merge-reasoning-disabled` | **yes** |
| `whisper` | **yes** |
| `deploy` **succeeded** (recovery) | no — always sent |
| `deploy-starting` | no — always sent |
| `merge` completed | no — always sent |

And even among the deduped kinds, a repeat is suppressed **only** when its *signature*
matches — same repo, same normalized headline + problem (spinner frames, durations,
timestamps, and whitespace are ignored; commit shortcodes, gate names, and failing-test
identifiers are **not**). So:

- a **different failing test** → new signature → **sent immediately**;
- a **new `target_commit`** → new signature → **sent immediately**;
- the canary going **green** / the deploy **succeeding** → non-suppressible → **sent
  immediately**.

## Confirm it's working

Dedup is never silent — it emits a structured `info` log each time it suppresses:

```bash
journalctl -u simard-overseer -f | grep 'overseer::notify'
```

You will see, per suppressed attempt:

```text
INFO overseer::notify: operator notification suppressed (dedup)
     suppressed=true kind="deploy-refused" signature_hash="9f3a1c7b" cooldown_secs=3600
```

and, for the **first** occurrence and each **digest reminder**, the normal dispatch
summary:

```text
INFO overseer::notify: operator notification dispatched
     dispatched=true all_sent=true kind="deploy-refused" channels="email=Sent signal=Sent"
```

Cross-check that suppression is *only* affecting the push, not your signal:

- **WARN logs still appear every attempt** — `journalctl -u simard-overseer | grep -i
  'self-deploy refused'` should show one WARN per cycle even while notices are
  suppressed.
- **Overseer status still reflects the block every tick** — the status field/endpoint
  keeps reporting the refused self-deploy; dedup does not touch it.

If you see the *dispatched* summary on **every** attempt (not just the first + hourly
reminder), check that `SIMARD_OVERSEER_NOTIFY_DEDUP_SECS` is not set to `0` and that the
signatures really are identical (a flapping/rotating failing test legitimately produces
a new signature each time).

## Verify with the tests

The behaviour is pinned by unit tests in `src/overseer/notify.rs`, serialized against
the process-global dedup `static`:

```bash
cargo test -p simard --lib overseer::notify::
```

These prove: (1) N identical `deploy-refused` within the window → exactly **one**
dispatched notice; (2) a changed signature or a `deploy-starting` / succeeded `deploy`
→ dispatched immediately; (3) after the window elapses a still-identical failure
dispatches again (digest reminder); (4) the dedup survives a simulated tick-rebuild;
plus a normalization test (spinner/duration-varied red-canary details hash equal while a
different failing test hashes distinct).

## Related

- [Notification dedup reference](../reference/overseer-operator-notification-dedup.md)
  — signature construction, normalization spec, fail-open guarantees, process-global
  state.
- [Operator-notification reliability reference](../reference/overseer-operator-notifications.md)
  — the two-channel (Signal + email) delivery contract the dedup gate sits in front of.
- [Deploy canary diagnostics](../reference/overseer-deploy-canary-diagnostics.md) — what
  produces the red-canary `deploy-refused` detail in the first place.
- [Gap-scan backoff](./configure-overseer-gap-scan-backoff.md) — the sibling
  "surface-once-then-back-off" rail for gap-scan issue filing.
