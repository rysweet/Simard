---
title: "Operations: resource-admission kill switch (SIMARD_RESOURCE_ADMISSION)"
description: >
  The environment variable that disables the resource-aware admission REASONING
  at daemon boot — what it does (and, critically, what it does NOT disable: the
  deterministic disk-ceiling rail and the byte-level MIN_FREE_GB precheck both
  keep running), when to use it, how to set it via systemd, how to verify which
  mode the daemon is in, and how to remove it. Secure default is reasoning ON.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/resource-aware-engineer-admission.md
  - ../reference/resource-admission-api.md
  - ../howto/configure-resource-aware-admission.md
  - engineer-admission-kill-switch.md
  - outcome-verification-kill-switch.md
---

# Resource-admission kill switch (`SIMARD_RESOURCE_ADMISSION`)

> **Status: implemented.** This page describes the shipped kill-switch in present
> tense. The gate it toggles lives in
> [`src/ooda_actions/advance_goal/resource_admission.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/resource_admission.rs)
> — see [resource-aware engineer admission](../concepts/resource-aware-engineer-admission.md)
> and the [resource-admission API](../reference/resource-admission-api.md).

This page documents the environment variable that disables the resource-aware
admission **reasoning** at daemon boot. The gate is described conceptually in
[resource-aware engineer admission](../concepts/resource-aware-engineer-admission.md).

> **The kill switch disables the reasoning, NOT the ENOSPC guarantee.** Unlike the
> [overlap-admission kill switch](engineer-admission-kill-switch.md) — which
> disables an entire fail-open gate — this switch turns off only the *agentic
> reasoning* (the brain call and the defer/reclaim decisions). The **deterministic
> disk-ceiling rail** and the byte-level **`MIN_FREE_GB` precheck** at worktree
> allocation **keep running**. Disabling the reasoning reverts admission to
> today's count-only behavior *plus* the hard ceiling — it never re-opens the path
> to `ENOSPC`.

---

## What this variable does

| Value | Behavior |
|---|---|
| Unset, or any value other than `off` (case-insensitive) | The reasoning gate runs in `dispatch_spawn_engineer`: before allocating a worktree for a **new, unassigned** goal, Simard gathers the resource picture, calls `decide_resource_admission`, and applies the hard rail. Each decision emits a `resource_admission_decision` metric and a `ResourceAdmission` judgment record. |
| `off` (case-insensitive) | The **reasoning** is **skipped** — no gather, no brain call, no defer/reclaim. Every candidate proceeds to the **hard disk-ceiling rail** and then to worktree allocation. The rail still blocks admission at/above `SIMARD_DISK_ADMISSION_CEILING_PCT`, and the `MIN_FREE_GB` precheck still guards the allocation. **No `resource_admission_decision` metric or `ResourceAdmission` record is emitted.** |

> **Unknown values keep reasoning ENABLED.** Only the exact documented value
> `off` disables the reasoning. A typo (`false`, `0`, `no`) leaves it **on** — the
> secure default is never silently disabled by a mis-spelled value.

The variable is read once, at daemon startup. Changing it during a run has no
effect — restart the daemon to pick up a new value.

---

## When to use `off`

Because the hard rail survives the switch, the legitimate uses are narrow:

1. **The reasoning recipe is persistently broken.** If a bad prompt edit or a
   `recipe-runner-rs` fault makes `decide_resource_admission` error every cycle,
   every new-engineer admission fails closed to `defer` and the fleet stops
   spawning. Set `off` to revert to count-only admission (still ceiling-guarded)
   while you fix the recipe, then re-enable.
2. **Isolating the reasoning during an investigation.** To rule the resource
   reasoning out as one variable when engineers are not starting, toggle it off on
   a **non-production** daemon for a side-by-side comparison, then re-enable.
3. **A defect is wrongly deferring at low disk.** If a probe bug makes the gate
   defer healthy spawns while disk is genuinely low, disable the reasoning to
   restore spawning while you fix it. The ceiling still protects you.

---

## When NOT to use `off`

- **"To spawn more engineers under disk pressure."** The reasoning exists to keep
  parallel builds from accumulating toward `ENOSPC`. Turning it off removes the
  proactive backpressure and leaves only the hard ceiling — exactly the
  reactive-only posture that let the disk reach 91% in the first place. A `defer`
  costs one cycle; an `ENOSPC` costs a crashed cycle and corrupted subprocesses.
- **"Because a spawn keeps deferring."** Repeated deferral usually means the disk
  is genuinely tight. The fix is to reclaim (`simard worktree-gc --apply`) or let
  a `reclaim-first` decision run — not to silence the gate. See
  [diagnose a resource-deferred spawn](../howto/configure-resource-aware-admission.md#diagnosing-a-resource-deferred-spawn).
- **To "turn off the disk ceiling."** This switch does not do that — the ceiling
  is `SIMARD_DISK_ADMISSION_CEILING_PCT` and is independent. Raise the ceiling
  (e.g. to `99`) if you truly need to, rather than disabling the reasoning.

---

## How to set it

### One-shot for an interactive run

```bash
SIMARD_RESOURCE_ADMISSION=off simard daemon
```

### Persistent across daemon restarts (systemd unit)

The Simard daemon ships with a reference unit file at
[`scripts/simard-ooda.service`](https://github.com/rysweet/Simard/blob/main/scripts/simard-ooda.service)
and is typically installed as a **user-level** unit at
`~/.config/systemd/user/simard-ooda.service`. Operators who install it
system-wide (`/etc/systemd/system/`) should drop the `--user` flag from every
command below.

Add the override to the unit's `[Service]` section:

```ini
[Service]
Environment="SIMARD_RESOURCE_ADMISSION=off"
```

Then reload and restart:

```bash
systemctl --user daemon-reload
systemctl --user restart simard-ooda
```

To remove the override, delete the `Environment=` line, `daemon-reload`, and
restart.

For system-level installs, prefer `systemctl edit simard-ooda` (with `sudo`) so
the override lands in
`/etc/systemd/system/simard-ooda.service.d/override.conf` rather than being
merged into the upstream unit file:

```bash
sudo systemctl edit simard-ooda
# add the same [Service] / Environment= snippet
sudo systemctl daemon-reload
sudo systemctl restart simard-ooda
```

---

## Verifying which mode the daemon is running in

The daemon logs the active mode at boot, and audits every degradation to
count-only admission. The `resource-admission:` substring and the `enabled` /
`DISABLED` words are stable; the parenthetical detail may evolve.

```
[simard] resource-admission: enabled (reasoning + disk-ceiling rail @ 90%)
```

Or:

```
[simard] resource-admission: DISABLED (reasoning off — SIMARD_RESOURCE_ADMISSION=off; disk-ceiling rail @ 90% STILL ACTIVE) [AUDIT: degraded to count-only admission]
```

Grep the daemon log to confirm (drop `--user` for a system-level install):

```bash
journalctl --user -u simard-ooda -n 200 | grep 'resource-admission:'
```

You can also probe live behavior via the metrics stream: when the reasoning is
enabled there is a `resource_admission_decision` metric entry per new-engineer
admission. When disabled, zero such entries are emitted (but the hard rail may
still block at the ceiling).

```bash
simard metrics query --name resource_admission_decision | tail -n 5
```

---

## Removing the kill switch

When the underlying issue is resolved, remove the environment variable and
restart the daemon. Confirm via the boot log line above that the gate is
`enabled`. The next cycle that admits a new engineer should emit a
`resource_admission_decision` metric and a `ResourceAdmission` judgment record.

---

## Related

- [Resource-aware engineer admission (concept)](../concepts/resource-aware-engineer-admission.md)
- [Resource-admission API (reference)](../reference/resource-admission-api.md)
- [How to configure and diagnose resource-aware admission (how-to)](../howto/configure-resource-aware-admission.md)
- [Engineer-admission kill switch](engineer-admission-kill-switch.md) — the sibling overlap gate's switch (fail-open, whole-gate).
- [Outcome-verification kill switch](outcome-verification-kill-switch.md) — the completion-moment sibling gate (fail-closed).
