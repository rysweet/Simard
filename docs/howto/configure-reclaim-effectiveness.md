---
title: Configure reclaim effectiveness (stop disk-reclaim churn)
description: >
  Operator guide for the disk-reclaim effectiveness gate — tuning the
  exponential cooldown that stops the OODA daemon from re-running reclamation
  every cycle when it keeps freeing nothing (#4809 / #4825 / #4810). Covers the
  cooldown base/multiplier/cap, the hard %-used ceiling that always bypasses the
  cooldown, how to read the new telemetry, how to diagnose held cycles, and how
  to disable the gate.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/reclaim-effectiveness-backoff.md
  - ../reference/reclaim-effectiveness-gate-api.md
  - ../reference/disk-reclaim-telemetry.md
  - ./configure-disk-reclamation.md
  - ../operations/reclaim-effectiveness-kill-switch.md
---

# Configure reclaim effectiveness

The OODA daemon reclaims disk when `%-used` crosses a threshold (see
[configure and run disk reclamation](./configure-disk-reclamation.md)). On a
host whose partition is *stuck* above the threshold with nothing safely
deletable, that trigger used to re-run reclamation every cycle and free 0 bytes
— wasteful churn that itself added disk pressure (#4809 / #4825 / #4810).

The **effectiveness gate** fixes this: after a streak of no-op runs it backs off
exponentially and skips over-threshold cycles, while still running immediately if
the disk crosses a hard ceiling. This guide shows how to tune and observe it.
For *why* it works this way, see
[reclaim effectiveness backoff](../concepts/reclaim-effectiveness-backoff.md).

## When to use this

- The daemon logs `disk reclaim held — N consecutive no-op run(s), cooling down`
  and you want to change how aggressively it backs off.
- You want genuine fill-ups to force reclamation sooner (lower the ceiling).
- Reclamation on your host *can* recover space and you think the cooldown is too
  long (shorten the base/cap) — or you want to disable the gate entirely.

## The knobs

All are environment variables read at daemon boot; all fail safe to the default
on invalid input. Set them the same way as the other daemon vars (systemd
drop-in, then `systemctl daemon-reload && systemctl restart simard-daemon`).

| Env var | Default | Effect |
| ------- | ------- | ------ |
| `SIMARD_DISK_RECLAIM_EFFECTIVENESS_GATE` | `on` | Master switch. `off` disables the cooldown (fire every over-threshold cycle). |
| `SIMARD_DISK_RECLAIM_COOLDOWN_BASE_SECS` | `900` | Cooldown after the **first** no-op run (15 min). |
| `SIMARD_DISK_RECLAIM_COOLDOWN_MULTIPLIER` | `2` | Growth per additional no-op run. Values `< 2` clamp to `2`. |
| `SIMARD_DISK_RECLAIM_COOLDOWN_MAX_SECS` | `14400` | Cooldown cap (4 h). |
| `SIMARD_DISK_RECLAIM_HARD_CEILING_PCT` | `97` | Locally-observed `%-used` at/above which the cooldown is **bypassed** and reclamation always runs. |

The pre-existing threshold `SIMARD_DISK_RECLAIM_PCT` (default `85`) still decides
*whether reclaim is even considered*; the effectiveness gate only decides
*whether to run given it is over threshold and hasn't been working*.

### Example: back off faster, bypass sooner

```ini
# /etc/systemd/system/simard-daemon.service.d/reclaim-effectiveness.conf
[Service]
Environment=SIMARD_DISK_RECLAIM_COOLDOWN_BASE_SECS=300
Environment=SIMARD_DISK_RECLAIM_COOLDOWN_MAX_SECS=3600
Environment=SIMARD_DISK_RECLAIM_HARD_CEILING_PCT=95
```

This arms a 5-minute base cooldown capped at 1 h, and bypasses the cooldown once
the disk hits 95% so an accelerating fill-up recovers promptly.

## Read the telemetry

The gate is fully observable through the unified telemetry facade (see
[disk-reclaim telemetry](../reference/disk-reclaim-telemetry.md)):

- `simard.disk.reclaim.suppressed_cycles` — counter, incremented every cycle the
  gate holds a run back.
- `noop_streak` / `suppressed_cycles` / `effective` — additive attributes on the
  existing `simard.disk.reclaim.*` series describing the current streak and
  whether the last run freed space.

```bash
simard status | grep 'disk.reclaim'
```

A rising `suppressed_cycles` with a flat `bytes_freed` is the expected healthy
signature of *"nothing to reclaim, so we're correctly holding back."* A rising
`bytes_freed` means reclamation is working and the streak keeps resetting.

## Diagnose a held cycle

1. Confirm the disk really has nothing safely reclaimable: run
   `simard disk-reclaim` (dry-run) and read the human-review list — if every
   candidate is skipped by a rail, the hold is correct.
2. If space *is* actually recoverable, the analysis agent isn't proposing it —
   that is a reclaim-analysis problem, not a gate problem; investigate the
   recipe, not the cooldown.
3. To force attempts while you investigate, either lower
   `SIMARD_DISK_RECLAIM_HARD_CEILING_PCT` or set the
   [kill switch](../operations/reclaim-effectiveness-kill-switch.md) `off`.

## Disable the gate

See the [reclaim-effectiveness kill switch](../operations/reclaim-effectiveness-kill-switch.md):

```ini
[Service]
Environment=SIMARD_DISK_RECLAIM_EFFECTIVENESS_GATE=off
```

This reverts to fire-every-over-threshold-cycle behavior. It does **not** change
the dry-run default or any deletion rail.
