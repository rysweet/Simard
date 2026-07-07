---
title: "Operations: engineer-admission kill switch (SIMARD_ENGINEER_ADMISSION)"
description: >
  The environment variable that disables the dependency/overlap-aware engineer
  admission gate at daemon boot — what it does, when (and when not) to use it,
  how to set it via systemd, how to verify which mode the daemon is in, and how
  to remove it. Secure default is scheduling ON; the gate is already fail-open,
  so this is an incident lever, not a safety necessity.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/dependency-overlap-aware-scheduling.md
  - ../reference/engineer-admission-api.md
  - ../howto/diagnose-a-deferred-engineer-spawn.md
  - outcome-verification-kill-switch.md
---

# Engineer-admission kill switch (`SIMARD_ENGINEER_ADMISSION`)

> **Status: implemented.** This page describes the shipped
> kill-switch in present tense. The gate
> it toggles lives in
> [`src/ooda_actions/advance_goal/admission.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/admission.rs)
> — see [dependency/overlap-aware engineer scheduling](../concepts/dependency-overlap-aware-scheduling.md)
> and the [engineer-admission API](../reference/engineer-admission-api.md).

This page documents the environment variable that disables the overlap-aware
engineer-admission gate at daemon boot. The gate is described conceptually in
[dependency/overlap-aware engineer scheduling](../concepts/dependency-overlap-aware-scheduling.md).

> The gate is **enabled by default** on all deployments. Secure default =
> scheduling ON. Unlike the [outcome verifier](outcome-verification-kill-switch.md)
> — which is fail-closed and can wedge archival — this gate is **fail-open**: a
> broken scheduler already degrades to admitting every candidate on its own. The
> kill switch therefore exists for incident isolation and short-lived debugging,
> not for steady-state operation, and you will rarely need it.

---

## What this variable does

| Value | Behavior |
|---|---|
| Unset, or any value other than `off` (case-insensitive) | The admission gate runs in `dispatch_spawn_engineer`: before allocating a worktree for a **new, unassigned** goal, Simard gathers the candidate's predicted file footprint and every live engineer's changed-file set, runs the exact-path rail, and calls `decide_engineer_admission`. Overlapping work is **deferred** or **serialized**; independent work is **admitted**. Each decision emits an `engineer_admission_decision` metric and an `EngineerAdmission` cycle-report entry. |
| `off` (case-insensitive) | The gate is **skipped entirely** — no gather, no overlap computation, no brain call, no exact-path rail. Every candidate is admitted straight to worktree allocation + spawn (the pre-#2690 collision-blind behaviour). **No `engineer_admission_decision` metric or `EngineerAdmission` cycle-report entry is emitted.** |

> **Unknown values keep scheduling ENABLED.** Only the exact documented value
> `off` disables the gate. A typo (`SIMARD_ENGINEER_ADMISSION=false`, `0`, `no`)
> leaves the gate **on** — the secure default is never silently disabled by a
> mis-spelled value.

The variable is read once, at daemon startup, in the client-construction path.
Changing it during a daemon run has no effect — restart the daemon to pick up a
new value.

---

## When to use `off`

Because the gate is fail-open, the legitimate uses are narrow:

1. **A defect in the gate is wrongly deferring genuinely independent work.** If a
   bug in scope prediction or overlap computation causes correct, non-overlapping
   goals to defer every cycle, disable the gate to restore unconditional spawning
   while you fix it, then re-enable.
2. **Isolating the gate during an investigation.** If engineers are not starting
   as expected and you want to rule the admission gate out as one variable, toggle
   it off on a **non-production** daemon for a side-by-side comparison, then
   re-enable.
3. **Bisecting a spawn-path bug unrelated to scheduling.** Removing the gate
   eliminates one variable. Restore it as soon as bisection completes.

---

## When NOT to use `off`

- **"To force two engineers onto overlapping goals faster."** That re-introduces
  exactly the merge collisions this gate prevents — the duplicate PRs (#2698 /
  #2696) and the broken-main Adapter-rename class. A `defer` costs one cycle of
  latency and self-clears when the overlapping engineer finishes; a collision
  costs a rebase or a broken `main`.
- **"Because a goal keeps deferring."** Repeated deferral means a long-running
  engineer is holding the overlapping files. The fix is to look at the *blocker*
  (is it wedged? reclaim it), not to silence the gate. See
  [diagnose a deferred engineer spawn](../howto/diagnose-a-deferred-engineer-spawn.md).
- **"For a single wrong prediction."** The gate is fail-open; one mis-predicted
  `defer` self-corrects next cycle. Improve the goal's `wip_ref` signal instead.

---

## How to set it

### One-shot for an interactive run

```bash
SIMARD_ENGINEER_ADMISSION=off simard daemon
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
Environment="SIMARD_ENGINEER_ADMISSION=off"
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
collision-blind spawning. The `engineer-admission:` substring and the `enabled` /
`DISABLED` words are stable; the parenthetical detail may evolve.

```
[simard] engineer-admission: enabled (overlap-aware scheduling)
```

Or:

```
[simard] engineer-admission: DISABLED (collision-blind spawn — SIMARD_ENGINEER_ADMISSION=off) [AUDIT: degraded to no overlap check]
```

Grep the daemon log to confirm (drop `--user` for a system-level install):

```bash
journalctl --user -u simard-ooda -n 200 | grep 'engineer-admission:'
```

You can also probe live behavior via the metrics stream: when the gate is enabled
there is an `engineer_admission_decision` metric entry per new-engineer admission.
When disabled, zero such entries are emitted.

```bash
simard metrics query --name engineer_admission_decision | tail -n 5
```

---

## Removing the kill switch

When the underlying issue is resolved, remove the environment variable and
restart the daemon. Confirm via the boot log line above that the gate is
`enabled`. The next cycle that admits a new engineer should emit an
`engineer_admission_decision` metric and an `EngineerAdmission` cycle-report
entry.

---

## Related

- [Dependency/overlap-aware engineer scheduling (concept)](../concepts/dependency-overlap-aware-scheduling.md)
- [Engineer-admission API (reference)](../reference/engineer-admission-api.md)
- [Diagnose a deferred engineer spawn (how-to)](../howto/diagnose-a-deferred-engineer-spawn.md)
- [Outcome-verification kill switch](outcome-verification-kill-switch.md) — the sibling brain-seam gate at the completion moment (fail-closed, opposite polarity).
