---
title: "Operations: Overseer cadence-watchdog tuning (SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER)"
description: >
  The single environment variable that tunes the Overseer cadence watchdog —
  SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER (cadence periods a hung tick may run
  before the overlap guard is force-cleared; default 3, floor 2). What it does,
  why there is no off switch, when (and when not) to change it, how to set it via
  systemd, how to verify the watchdog fired (WARN span +
  overseer_tick_watchdog_rearm_total), and how to revert.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/overseer-cadence-watchdog.md
  - ../reference/overseer-cadence-watchdog-api.md
  - ../reference/overseer-tick-details.md
  - ../howto/simard-status.md
  - ../howto/watch-overseer-activity.md
  - claim-reaper-kill-switch.md
---

# Overseer cadence-watchdog tuning (`SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER`)

> **Status: implemented.** This page describes the shipped configuration in
> present tense. The watchdog it tunes lives in the daemon loop in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs);
> the resolver lives in
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs).
> See the [concept](../concepts/overseer-cadence-watchdog.md) and the
> [API reference](../reference/overseer-cadence-watchdog-api.md).

The cadence watchdog keeps the acting Overseer's OODA cadence advancing when a
single tick hangs indefinitely on an external call. It is **always on** — there
is deliberately **no kill switch**, because disabling it would re-introduce the
exact stall it prevents (a hung tick pins the overlap guard, no further tick is
scheduled, and `simard status` flips to `(stale)`). One variable tunes *how
patient* it is before it acts.

---

## The variable

| Variable | Default | Floor | Effect |
| --- | --- | --- | --- |
| `SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER` | `3` | `2` | The watchdog force-clears the overlap guard once the in-flight tick has been outstanding for **more than `multiplier × cadence`**. At the 15-minute default cadence, `3` ⇒ re-arm after ~45 minutes. |

Fail-safe resolution (see the
[API reference](../reference/overseer-cadence-watchdog-api.md#configuration)):

- Unset / empty / non-numeric ⇒ default `3`.
- A value **below `2`** ⇒ clamped up to `2` with a `WARN`.
- No value disables the watchdog. Secure default = self-heal ON.

## What it does **not** do

- It does **not** change the cadence. The 15-minute
  `DEFAULT_OVERSEER_INTERVAL_SECS = 900` is unaffected; this knob only sets how
  long a *hung* tick may block before the next one is allowed to start.
- It does **not** kill the hung thread or take any external action. It flips one
  in-process boolean so the next scheduled tick can run.

## When to change it

Most deployments should leave it at `3`. Consider tuning only if:

- **Lower it toward `2`** if a specific deployment sees ticks that legitimately
  hang on flaky upstreams and you want the cadence to recover faster. `2` is the
  floor: recover after ~2 cadence periods (~30 min at default cadence).
- **Raise it above `3`** if your Overseer routinely runs long, expensive ticks
  (e.g. big gap-scans) that you do not want the watchdog to interrupt
  prematurely. Raising it trades faster stall recovery for a wider safety margin
  against interrupting a genuinely-slow-but-healthy tick.

Do **not** set it to `1` or `0` expecting to "disable" or "instantly fire" the
watchdog — both clamp to `2`.

## Set it via systemd

The daemon ships as the `simard-ooda` **user** service
([`scripts/simard-ooda.service`](https://github.com/rysweet/Simard/blob/main/scripts/simard-ooda.service),
typically `~/.config/systemd/user/simard-ooda.service`). Set the knob with a
drop-in via `systemctl --user edit simard-ooda`:

```ini
[Service]
Environment="SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER=2"
```

```bash
systemctl --user daemon-reload
systemctl --user restart simard-ooda
```

For a system-wide install, drop `--user` and prefer `systemctl edit simard-ooda`
so the override lands in a drop-in. Or inline for a foreground run:

```bash
SIMARD_OVERSEER_TICK_WATCHDOG_MULTIPLIER=2 simard ooda run
```

## Verify it is working

Confirm the cadence is advancing (not `(stale)`):

```bash
simard status | grep -i overseer
# Overseer: enabled, 305 interventions   ← no "(stale)" marker
```

When the watchdog fires, it is **loud**. Look for the WARN span and the counter:

```bash
journalctl --user -u simard-ooda | grep overseer.cadence_watchdog
# WARN overseer.cadence_watchdog elapsed_secs=2701 cadence_secs=900 multiplier=3
#      overseer tick guard held past watchdog threshold; re-arming cadence
```

The counter (internal OTel name `simard.overseer.tick_watchdog_rearm`, exported
to Prometheus as `simard_overseer_tick_watchdog_rearm_total`) is emitted through
the unified [telemetry facade](../reference/telemetry-metrics.md) and visible in
your OTLP metrics backend. **A rising re-arm count is a signal**, not just noise:
it means some dependency is repeatedly hanging the Overseer tick and deserves
root-cause investigation even though the cadence keeps recovering.

## Revert

Remove the override and restart:

```bash
systemctl --user edit simard-ooda   # delete the Environment= line, save
systemctl --user daemon-reload
systemctl --user restart simard-ooda
```

The watchdog returns to the default `3× cadence` patience. There is nothing to
"turn back on" — it is never off.
