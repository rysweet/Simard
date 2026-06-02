---
title: How to configure adaptive scaling
description: Enable and tune Simard's AIMD-based adaptive scaling for max_concurrent_actions.
last_updated: 2026-06-02
owner: simard
doc_type: howto
related:
  - ../concepts/adaptive-scaling.md
  - ../reference/adaptive-scaling-api.md
  - ../daemon-mode.md
  - ./run-ooda-daemon.md
---

# How to configure adaptive scaling

Simard can dynamically adjust how many engineer subprocesses it
dispatches per OODA cycle based on host CPU load, memory pressure, and
API rate-limit signals. This guide covers enabling, monitoring, and
tuning the feature.

## Prerequisites

- Simard is installed and the OODA daemon can be started
  (`simard ooda run`).
- You have SSH access to the host.

---

## 1. Enable adaptive scaling

Set the `SIMARD_SCALING` environment variable to `auto`:

```bash
SIMARD_SCALING=auto simard ooda run
```

Or in a systemd unit:

```ini
[Service]
Environment=SIMARD_SCALING=auto
ExecStart=/usr/local/bin/simard ooda run
```

When `SIMARD_SCALING` is unset or set to `fixed`, the daemon uses the
static `max_concurrent_actions` value from `OodaConfig` (default: `3`).

---

## 2. Monitor scaling decisions

The scaler logs one line per cycle to stderr:

```
[simard] adaptive scaling: cpu=0.72 mem=0.45 errors_429=0 pressure=0.72 → max_concurrent=3 (hold)
```

To watch in real time:

```bash
journalctl -u simard -f | grep 'adaptive scaling'
```

Key fields:

| Field | What to look for |
|-------|-----------------|
| `cpu` | Sustained > 0.8 means the host is CPU-bound |
| `mem` | Sustained > 0.8 means the host is memory-bound |
| `errors_429` | Any non-zero value triggers immediate decrease |
| `pressure` | The max of the three signals |
| `(increase\|decrease\|hold)` | Which AIMD branch was taken |

---

## 3. Understand the scaling behavior

The scaler uses AIMD (Additive Increase / Multiplicative Decrease):

- **Low pressure** (< 0.3): increase by 1 per cycle (conservative
  probing).
- **High pressure** (> 0.8) or **any 429 errors**: halve the concurrency
  (rapid retreat).
- **Medium pressure** (0.3–0.8, no 429s): hold steady.

The concurrency is bounded by a floor (default: 1) and ceiling
(default: `max_concurrent_actions × 4`, e.g. 20 when the base is 5).
The floor guarantees at least one engineer dispatch per cycle even
under maximum pressure.

---

## 4. Override the static fallback

When scaling is disabled (`SIMARD_SCALING=fixed` or unset), the daemon
uses the static `max_concurrent_actions` from `OodaConfig`. This value
defaults to `3` and can be adjusted in the config if needed.

---

## 5. Verify on non-Linux hosts

On macOS or Windows, the `/proc`-based CPU and memory signals are
unavailable. The scaler compiles and runs but only responds to 429
errors. Without system pressure signals:

- The scaler holds steady at its initial value unless API errors occur.
- This is intentional — holding steady is safer than guessing system
  load on platforms without `/proc`.

To test that 429 detection works on a non-Linux host:

```bash
# Start the daemon with adaptive scaling
SIMARD_SCALING=auto simard ooda run --cycles=5

# In another terminal, watch for scaling lines
journalctl -u simard | grep 'adaptive scaling'
# On non-Linux: expect cpu=n/a mem=n/a
```

---

## Troubleshooting

### Concurrency stays at 1 and never increases

Check:

1. Is `SIMARD_SCALING=auto` in the daemon's environment?
   ```bash
   cat /proc/$(pgrep simard)/environ | tr '\0' '\n' | grep SIMARD_SCALING
   ```

2. Is system pressure consistently above 0.8?
   ```bash
   journalctl -u simard | grep 'adaptive scaling' | tail -20
   ```
   If `pressure` is always > 0.8, the host is genuinely overloaded. The
   scaler is working correctly. Consider reducing other workloads or
   raising the floor.

3. Are 429 errors occurring every cycle? Check `errors_429` in the log
   lines. Persistent 429s will keep the scaler at floor. Wait for the
   rate-limit window to reset (5 minutes) or reduce the ceiling.

### Concurrency oscillates rapidly

This can happen when pressure hovers near the 0.8 threshold. The
scaler increases by 1, tips over 0.8, halves, drops below 0.3,
increases again. This "sawtooth" is normal AIMD behavior and is
self-correcting — the concurrency converges to the sustainable level.

If the oscillation is disruptive, switch to `SIMARD_SCALING=fixed` and
set a static value.

### Want to change the floor or ceiling

The floor is always 1. The ceiling defaults to `max_concurrent_actions × 4`
(computed from `OodaConfig`). To change the effective ceiling, adjust
`SIMARD_MAX_CONCURRENT_ACTIONS`:

```bash
# Base=3 → ceiling=12, base=10 → ceiling=40
SIMARD_MAX_CONCURRENT_ACTIONS=10 SIMARD_SCALING=auto simard ooda run
```

Environment-variable overrides for independent floor and ceiling values
are a candidate for a follow-up issue.

---

## See also

- [Adaptive scaling concept](../concepts/adaptive-scaling.md) — design
  rationale.
- [Adaptive scaling API reference](../reference/adaptive-scaling-api.md)
  — Rust types and methods.
- [Daemon mode](../daemon-mode.md) — the OODA cycle.
- [How to run the OODA daemon](./run-ooda-daemon.md).
