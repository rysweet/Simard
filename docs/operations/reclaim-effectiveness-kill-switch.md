---
title: "Operations: reclaim-effectiveness kill switch (SIMARD_DISK_RECLAIM_EFFECTIVENESS_GATE)"
description: >
  The environment variable that disables the disk-reclaim effectiveness gate at
  daemon boot — what it does (and, critically, what it does NOT disable: the
  %-used trigger, every deletion safety rail, and the dry-run default all keep
  running), when to use it, how to set it via systemd, how to verify which mode
  the daemon is in, and how to remove it. Secure default is the gate ON.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/reclaim-effectiveness-backoff.md
  - ../reference/reclaim-effectiveness-gate-api.md
  - ../reference/disk-reclaim-telemetry.md
  - ../howto/configure-reclaim-effectiveness.md
  - resource-admission-kill-switch.md
  - index.md
---

# Reclaim-effectiveness kill switch (`SIMARD_DISK_RECLAIM_EFFECTIVENESS_GATE`)

> **Status: implemented.** This page describes the shipped kill switch in
> present tense. The gate it toggles lives in
> [`src/disk_reclaim/effectiveness.rs`](https://github.com/rysweet/Simard/blob/main/src/disk_reclaim/effectiveness.rs)
> and is wired into the daemon trigger in
> [`src/operator_commands_ooda/daemon/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_ooda/daemon/mod.rs).
> See [reclaim effectiveness backoff](../concepts/reclaim-effectiveness-backoff.md)
> and the [reclaim effectiveness gate reference](../reference/reclaim-effectiveness-gate-api.md).

This variable disables the **effectiveness cooldown** that stops the OODA daemon
from re-running disk-reclamation every cycle when reclamation keeps freeing
nothing (issues #4809 / #4825 / #4810).

> **The kill switch disables the cooldown REASONING, NOT any safety.** Turning
> the gate off reverts to the previous behavior — reclaim fires on **every**
> cycle where `%-used ≥ SIMARD_DISK_RECLAIM_PCT` — but it does **not** re-open
> any deletion path. The dry-run default, `SIMARD_DISK_RECLAIM_DAEMON_APPLY`
> apply-opt-in, and every candidate rail (protected paths, live processes,
> uncommitted/unpushed, active worktree, allow-root) all keep running unchanged.
> Disabling the gate only makes the daemon *churn again*; it never makes it
> *delete more*.

---

## What this variable does

| Value | Behavior |
|---|---|
| Unset, or any value other than `off` (case-insensitive) | **Gate ON (default).** Before firing reclaim, the daemon consults the `ReclaimEffectivenessGate`. After a streak of no-op runs it applies bounded exponential cooldown and **skips** over-threshold cycles, emitting a `WARN` line and incrementing `simard.disk.reclaim.suppressed_cycles`. Suppression is bypassed above `SIMARD_DISK_RECLAIM_HARD_CEILING_PCT`. |
| `off` (case-insensitive) | **Gate OFF.** The cooldown is skipped entirely. Reclaim fires on every cycle where `%-used ≥ SIMARD_DISK_RECLAIM_PCT`, exactly as before this feature. No `suppressed_cycles` metric is emitted and the `noop_streak` / `effective` attributes are absent. |

## When to use it

Set `off` only to **temporarily** diagnose or work around the gate — for
example, if you suspect the cooldown is holding back reclamation on a
legitimately recoverable disk and you want to force every-cycle attempts while
you investigate. In steady state, leave it ON: the whole point of the gate is to
stop the wasteful churn that #4809 / #4810 reported.

## Set it via systemd

```ini
# /etc/systemd/system/simard-daemon.service.d/reclaim-effectiveness.conf
[Service]
Environment=SIMARD_DISK_RECLAIM_EFFECTIVENESS_GATE=off
```

```bash
sudo systemctl daemon-reload
sudo systemctl restart simard-daemon
```

Remove the drop-in (or set any non-`off` value) and restart to return to the
secure default.

## Verify which mode the daemon is in

- **Logs:** with the gate ON, a held cycle logs
  `WARN: disk reclaim held — N consecutive no-op run(s), cooling down`.
  With the gate OFF you never see that line; you instead see the reclaim run (or
  the `under threshold` line) on every cycle.
- **Telemetry:** with the gate ON, `simard.disk.reclaim.suppressed_cycles`
  appears in `simard status` / the OTLP export and increments on held cycles.
  With the gate OFF the counter is absent. See
  [disk-reclaim telemetry](../reference/disk-reclaim-telemetry.md).

## Related knobs

Prefer **tuning** the gate over disabling it — see
[configure reclaim effectiveness](../howto/configure-reclaim-effectiveness.md):

- `SIMARD_DISK_RECLAIM_COOLDOWN_BASE_SECS` — shorten the initial cooldown.
- `SIMARD_DISK_RECLAIM_COOLDOWN_MAX_SECS` — cap the maximum cooldown.
- `SIMARD_DISK_RECLAIM_HARD_CEILING_PCT` — lower the ceiling so genuine
  fill-ups bypass the cooldown sooner.
