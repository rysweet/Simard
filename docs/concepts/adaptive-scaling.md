---
title: Adaptive scaling — AIMD concurrency for the OODA cycle
description: Why and how Simard dynamically adjusts max_concurrent_actions using Additive Increase / Multiplicative Decrease, responding to CPU, memory, and API rate-limit pressure.
last_updated: 2026-06-02
owner: simard
doc_type: concept
related:
  - ../reference/adaptive-scaling-api.md
  - ../reference/ooda-coverage-parallelism-ceiling.md
  - ../howto/configure-adaptive-scaling.md
  - ../daemon-mode.md
  - ../reference/ooda-brain-api.md
---

# Adaptive scaling — AIMD concurrency for the OODA cycle

## The problem

Simard's OODA daemon dispatches up to `max_concurrent_actions` engineer
subprocesses per cycle. This value was previously a static configuration
constant in `OodaConfig`. On a lightly loaded host with ample API quota,
the static limit underutilizes available capacity. On a heavily loaded
host, or when the Copilot API starts returning 429 (rate limit) errors,
the same limit causes wasted cycles, failed dispatches, and cascading
retry pressure.

Operators could manually tune the value, but the optimal setting changes
throughout the day as host load varies and API quotas reset. There was
no feedback loop.

## The solution: AIMD

AIMD (Additive Increase / Multiplicative Decrease) is the congestion
control algorithm behind TCP's scalability. It is well-suited to this
problem because:

- **Probing behavior**: additive increase (+1 per cycle when pressure is
  low) slowly explores available capacity.
- **Fast retreat**: multiplicative decrease (halve on overload) rapidly
  backs off when pressure is detected, preventing sustained overload.
- **Convergence**: AIMD provably converges to a fair, efficient
  allocation point in the presence of multiple competing flows — useful
  if multiple Simard instances share the same host or API quota.
- **Simplicity**: the algorithm has two parameters (increase increment,
  decrease factor) and three thresholds, making it easy to reason about
  and debug.

### Pressure signals

The scaler reads three independent signals, each normalized to
`[0.0, 1.0]`:

| Signal | Source | Interpretation |
|--------|--------|----------------|
| CPU load | `/proc/stat` (Linux only) | `1 - idle_ratio` over the sample interval |
| Memory pressure | `/proc/meminfo` (Linux only) | `1 - MemAvailable/MemTotal` |
| API rate limits | `SimardError::AdapterInvocationFailed { status: 429 }` | Presence of any 429 in a 5-minute sliding window |

The aggregate pressure is `max(cpu, mem, 429_present ? 1.0 : 0.0)`.
Using `max` (rather than mean) ensures that a single saturated resource
triggers a decrease even if the others are idle.

On non-Linux platforms, CPU and memory signals return `None` (no
pressure detected), so the scaler holds steady unless 429 errors occur.
This makes the module compile and run everywhere while providing full
value on the Linux hosts where Simard typically runs.

### AIMD parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Additive increase | +1 per cycle | Conservative probing; one new engineer per cycle |
| Multiplicative decrease | ×0.5 (halve) | Standard AIMD; matches TCP's response |
| High pressure threshold | 0.8 | Above this, the host is under significant load |
| Low pressure threshold | 0.3 | Below this, there is ample headroom to grow |
| Error window | 300s (5 min) | Matches typical Copilot rate-limit reset periods |
| Floor | 1 | Always dispatch at least one action |
| Ceiling | `= configured base` (default 24) | The configured per-cycle max; prevents runaway |

### Decision flow

```
sample_cpu()     ──┐
sample_memory()  ──┼──→ max() ──→ aggregate pressure
check_429_window()─┘

if pressure > 0.8 or 429_count > 0:
    new = max(floor, current × 0.5)   ← multiplicative decrease
elif pressure < 0.3:
    new = min(ceiling, current + 1)   ← additive increase
else:
    new = current                     ← hold steady
```

## Trade-offs

| Gain | Cost |
|------|------|
| Automatic adaptation to host load | Adds ~250 LOC and a new module |
| Prevents cascading 429 failures | Reaction is per-cycle (60s default), not immediate |
| No external dependencies (reads /proc directly) | Linux-only for system signals |
| Opt-in via `SIMARD_SCALING=auto` | Operators must know the env var exists |

## Design decisions

**Why not PID control?** PID controllers are more responsive but harder
to tune (three parameters, integral windup risk) and harder to explain
in operator-facing documentation. AIMD is self-documenting: "it adds one
when things are calm, halves when things are stressed."

**Why `AtomicU32` + CAS instead of a mutex?** The concurrency limit is a
single integer read by the decide phase and written by the scaler. An
atomic with a CAS loop is lock-free and cheaper than a mutex for this
use case. `Relaxed` ordering is sufficient because no other shared state
depends on the visibility of this value.

**Why `Option<Arc<AdaptiveScaler>>` on `OodaConfig`?** `Option` because
the scaler is only present when `SIMARD_SCALING=auto`. `Arc` because the
scaler contains internal synchronization (`AtomicU32`, `Mutex` for the
error window) and may be referenced from multiple action-dispatcher
threads. The field uses `#[serde(skip)]` because the scaler is
runtime-only — it is reconstructed from the `SIMARD_SCALING` env var
on boot by `OodaConfig::default()` (which consults `SIMARD_SCALING` and
builds the `AdaptiveScaler`), not persisted.

**Why not mutate `OodaConfig` directly?** `OodaConfig` is immutable
after construction and may be shared. Cloning it into an
`effective_config` per cycle is cheap and avoids aliasing surprises.

## See also

- [OODA coverage parallelism ceiling](../reference/ooda-coverage-parallelism-ceiling.md)
  — the default-24 ceiling, the `SIMARD_OODA_MAX_CONCURRENT` override, and why
  the ceiling never bypasses the resource-admission gate.
- [Adaptive scaling API reference](../reference/adaptive-scaling-api.md)
  — Rust types, methods, and integration points.
- [How to configure adaptive scaling](../howto/configure-adaptive-scaling.md)
  — operator guide.
- [Daemon mode](../daemon-mode.md) — the OODA cycle.
