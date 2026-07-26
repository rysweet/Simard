---
title: How to configure graceful OODA completion and the reflection bound
description: Procedure for tuning the issue #1025 graceful-completion layer — keep Simard perpetual by default, optionally let an all-ACHIEVED board idle the daemon, and cap self-inflicted no-progress reflection spin with a bounded safeguard.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: howto
issues: ["#1025"]
related:
  - ../concepts/graceful-ooda-completion.md
  - ../reference/ooda-graceful-completion-api.md
  - ./run-ooda-daemon.md
  - ./diagnose-perpetual-completion-recuration.md
  - ../reference/completion-evidence-gate-api.md
---

# How to configure graceful OODA completion and the reflection bound

The graceful-completion layer (issue #1025) needs no configuration to be safe:
its defaults keep Simard perpetual and only stop the *reflection spin* on goals
that are already gate-verified ACHIEVED. This guide covers the two knobs you can
turn when you want different behavior.

All settings are environment variables read at daemon start (and re-read on the
daemon's `exec()` self-reload). For a systemd deployment, set them in the
`simard-ooda.service` unit environment; for a manual run, export them before
`simard ooda`.

## Defaults at a glance

| Variable | Default | Meaning |
| --- | --- | --- |
| `SIMARD_OODA_STOP_WHEN_ACHIEVED` | `0` (off) | When `1`, an all-ACHIEVED goal board lets the daemon loop idle instead of staying perpetual. |
| `SIMARD_OODA_MAX_REFLECTION_CYCLES` | `0` (disabled) | Consecutive no-progress reflection cycles a **non-perpetual** goal may burn before the loop yields with a recorded blocker. `0` = no cap. |

With both at their defaults, per-goal graceful completion is **always active**
(a goal whose deliverable PR is green with all criteria met stops re-reflecting),
the daemon **stays perpetual**, and there is **no** no-progress cap beyond the
existing no-progress breaker.

## 1. Keep the perpetual default (recommended)

Do nothing. A goal that reaches a gate-verified `Complete` verdict with all
success criteria satisfied is marked ACHIEVED and its reflection loop closes;
the daemon frees the goal and continues with the rest of the board and its
standing research goal. This is the behavior issue #1025 fixes — no more endless
re-reflection over an already-green deliverable — without turning Simard into a
run-once process.

Verify from the daemon log:

```bash
journalctl --user -u simard-ooda -f | grep -i "ACHIEVED (gate-verified)"
```

You should see one terminal line per completed goal, not a stream of repeated
reflection ticks.

## 2. Let an idle board pause the daemon loop

Only if you *want* the daemon to stop cycling once every goal is ACHIEVED (for
example, a bounded batch host rather than a standing autonomous instance):

```bash
export SIMARD_OODA_STOP_WHEN_ACHIEVED=1
```

With this set, when `goals_all_achieved` holds for the whole board the daemon
loop idles. Leave it **unset/`0`** for any standing instance — a perpetual
Simard should keep her standing research goal moving even when the delivery
board is empty.

## 3. Cap self-inflicted no-progress spin

To bound how long a genuinely stuck, **non-perpetual** goal may reflect without
producing shippable progress:

```bash
export SIMARD_OODA_MAX_REFLECTION_CYCLES=25
```

After 25 *consecutive* no-progress reflection cycles the loop yields that goal
with a recorded blocker (it never fabricates completion). Any cycle that makes
shippable progress resets the counter, so an actively moving goal is never
capped. Perpetual/standing goals are exempt from this bound by design.

Choose a value comfortably above your normal cycle count for a healthy goal.
`0` disables the cap entirely.

## 4. Apply the change

For a systemd deployment, add the variables to the service unit and restart:

```bash
systemctl --user set-environment SIMARD_OODA_MAX_REFLECTION_CYCLES=25
systemctl --user restart simard-ooda.service
```

Confirm the daemon logged the effective policy at startup:

```bash
journalctl --user -u simard-ooda | grep -i "reflection bound"
```

## Troubleshooting

- **A green, done goal still re-reflects.** Confirm the done-gate is actually
  returning `Complete` for it — graceful completion consumes that verdict and
  will not fire until the merged-PR / closed-issue / deployed clauses hold. See
  [the completion-evidence gate](../reference/completion-evidence-gate-api.md)
  and [diagnose perpetual completion re-curation](./diagnose-perpetual-completion-recuration.md).
- **A goal yielded `BoundExceeded` too early.** Raise
  `SIMARD_OODA_MAX_REFLECTION_CYCLES` or set it to `0`; check the recorded
  blocker for the WHY.
- **The daemon stopped cycling unexpectedly.** Ensure
  `SIMARD_OODA_STOP_WHEN_ACHIEVED` is not set to `1` on a standing instance.
- **A malformed value.** Non-numeric or unparseable settings fall back to the
  safe default and log a `tracing::warn!`; the daemon never panics on bad input.
