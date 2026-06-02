---
title: Adaptive scaling API reference
description: Rust API reference for the AIMD-based AdaptiveScaler that dynamically adjusts max_concurrent_actions based on system pressure and error signals.
last_updated: 2026-06-02
owner: simard
doc_type: reference
related:
  - ../concepts/adaptive-scaling.md
  - ../howto/configure-adaptive-scaling.md
  - ./ooda-brain-api.md
  - ../daemon-mode.md
---

# Adaptive scaling API reference

Module: `simard::ooda_loop::adaptive_scaling`

The `AdaptiveScaler` dynamically adjusts the OODA cycle's
`max_concurrent_actions` using an AIMD (Additive Increase /
Multiplicative Decrease) algorithm. It reads system pressure from
`/proc/stat` (CPU), `/proc/meminfo` (memory), and Copilot 429 error
signals to scale the action concurrency up or down.

---

## Configuration

The scaler is controlled by the `SIMARD_SCALING` environment variable:

| Value | Behavior |
|-------|----------|
| `auto` | AIMD scaling is active. `AdaptiveScaler` adjusts `max_concurrent_actions` each cycle. |
| `fixed` | Scaling is disabled. `max_concurrent_actions` uses the static value from `OodaConfig`. |
| (unset) | Defaults to `fixed` for backward compatibility. |

```bash
# Enable adaptive scaling
SIMARD_SCALING=auto simard ooda run

# Disable (use static config value)
SIMARD_SCALING=fixed simard ooda run
```

---

## Public API

### `AdaptiveScaler`

```rust
pub struct AdaptiveScaler {
    current: AtomicU32,
    floor: u32,
    ceiling: u32,
    // private fields: pressure history, error window, AIMD params
}
```

The `current` field uses `AtomicU32` with `Relaxed` ordering because
`max_concurrent_actions` is an independent numeric throttle with no
memory-visibility dependencies on other shared state.

### Construction

```rust
impl AdaptiveScaler {
    /// Creates a new scaler. Clamps arguments to valid ranges:
    /// - floor >= 1
    /// - ceiling >= floor
    /// - initial is clamped to [floor, ceiling]
    pub fn new(initial: u32, floor: u32, ceiling: u32) -> Self;
}
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `initial` | Value of `OodaConfig.max_concurrent_actions` | Starting concurrency level |
| `floor` | `1` | Minimum concurrency (never scales below this) |
| `ceiling` | `8` | Maximum concurrency (never scales above this) |

Bounds are validated on construction:

- `floor` is clamped to `max(1, floor)` — zero would disable action dispatch.
- `ceiling` is clamped to `max(floor, ceiling)`.
- `initial` is clamped to `[floor, ceiling]`.

### Core methods

```rust
impl AdaptiveScaler {
    /// Returns the current max_concurrent_actions value.
    /// Uses AtomicU32::load(Relaxed).
    pub fn current_max(&self) -> u32;

    /// Samples system pressure and adjusts the concurrency limit.
    /// Called once per OODA cycle, before the Decide phase.
    ///
    /// Returns the new max value after adjustment.
    pub fn adjust(&self) -> u32;

    /// Reports an action error. When the error carries an HTTP 429
    /// status (via AdapterInvocationFailed or similar), records a
    /// pressure signal that triggers multiplicative decrease on the
    /// next adjust() call.
    pub fn report_error(&self, error: &SimardError);
}
```

### AIMD algorithm

Each `adjust()` call:

1. **Sample pressure signals:**
   - CPU load from `/proc/stat` (computed as 1 − idle_ratio over the
     last sample interval)
   - Memory pressure from `/proc/meminfo` (computed as
     1 − MemAvailable/MemTotal)
   - 429 error count since the last `adjust()` call

2. **Compute aggregate pressure** as the maximum of the three signals
   (each normalized to `[0.0, 1.0]`; missing signals contribute `0.0`).

3. **Apply AIMD rule:**
   - If aggregate pressure > `HIGH_PRESSURE_THRESHOLD` (default `0.8`)
     **or** 429 count > 0:
     **Multiplicative decrease**: `new = max(floor, current * DECREASE_FACTOR)`
     where `DECREASE_FACTOR = 0.5` (halve).
   - Else if aggregate pressure < `LOW_PRESSURE_THRESHOLD` (default
     `0.3`):
     **Additive increase**: `new = min(ceiling, current + 1)`.
   - Else: **Hold steady**: `new = current`.

4. **Update** via `AtomicU32::fetch_update(Relaxed, Relaxed, ...)` —
   CAS loop ensures no lost updates if multiple threads call `adjust()`
   concurrently (though in practice only the OODA cycle thread calls it).

### Constants

```rust
const HIGH_PRESSURE_THRESHOLD: f64 = 0.8;
const LOW_PRESSURE_THRESHOLD: f64 = 0.3;
const DECREASE_FACTOR: f64 = 0.5;
const ERROR_WINDOW_SECS: u64 = 300; // 5-minute sliding window for 429s
```

---

## Signal parsers

### CPU pressure (`/proc/stat`)

```rust
/// Returns CPU pressure as a value in [0.0, 1.0], or None if /proc/stat
/// is unavailable (non-Linux) or unparseable.
#[cfg(target_os = "linux")]
fn sample_cpu_pressure() -> Option<f64>;
```

Reads the first `cpu` line from `/proc/stat`, computes
`1.0 - (idle_delta / total_delta)` between the current and previous
sample. Returns `None` on the first call (no previous sample) or on
parse failure.

On non-Linux platforms, this function is gated out and the CPU signal
is always `None`.

### Memory pressure (`/proc/meminfo`)

```rust
/// Returns memory pressure as a value in [0.0, 1.0], or None if
/// /proc/meminfo is unavailable or unparseable.
#[cfg(target_os = "linux")]
fn sample_memory_pressure() -> Option<f64>;
```

Reads `MemTotal` and `MemAvailable` from `/proc/meminfo`, computes
`1.0 - (available / total)`. Returns `None` if either field is missing
or the file is unreadable.

### 429 error detection

```rust
impl AdaptiveScaler {
    pub fn report_error(&self, error: &SimardError);
}
```

`report_error` inspects the `SimardError` variant:

- `AdapterInvocationFailed { status, .. }` where
  `status == Some(429)` → records a 429 event with timestamp.
- All other variants → ignored (no pressure signal).

The scaler maintains a sliding window of 429 events
(`ERROR_WINDOW_SECS = 300`). The `adjust()` method counts events in
the window and treats count > 0 as a pressure signal.

This keeps the scaler decoupled from error internals — it receives
`&SimardError` but only pattern-matches on the 429-bearing variant.

---

## Platform behavior

| Platform | CPU signal | Memory signal | 429 signal | Compiles |
|----------|-----------|---------------|------------|----------|
| Linux | `/proc/stat` | `/proc/meminfo` | Yes | Yes |
| macOS | `None` (hold steady) | `None` (hold steady) | Yes | Yes |
| Windows | `None` (hold steady) | `None` (hold steady) | Yes | Yes |

On non-Linux platforms, `/proc` parsing is `#[cfg(target_os = "linux")]`-gated.
With both system signals returning `None`, only 429 errors trigger
multiplicative decrease. In the absence of any pressure signal, the
scaler holds steady at its current value.

---

## Integration with the OODA cycle

The scaler is stored as `Option<Arc<AdaptiveScaler>>` on `OodaState`:

```rust
pub struct OodaState {
    // ... existing fields ...
    pub scaler: Option<Arc<AdaptiveScaler>>,
}
```

`Option` because the scaler is `None` when `SIMARD_SCALING=fixed` or
unset. `Arc` because the scaler contains atomics and a mutex and may be
shared across the action dispatcher threads.

### Per-cycle integration point

In `src/ooda_loop/cycle.rs`, before the Decide phase:

```rust
// Adjust scaling and build effective config
let mut effective_config = config.clone();
if let Some(ref scaler) = state.scaler {
    effective_config.max_concurrent_actions = scaler.adjust();
}

// Pass effective config to decide
let actions = decide_with_brain(&effective_config, &priorities, brain)?;
```

The original `OodaConfig` is never mutated. Each cycle clones the
config, overrides `max_concurrent_actions` with the scaler's current
value, and passes the effective config to `decide` / `decide_with_brain`.

### Error reporting

In the action dispatch loop, after an action fails:

```rust
if let Err(ref e) = outcome.result {
    if let Some(ref scaler) = state.scaler {
        scaler.report_error(e);
    }
}
```

This feeds 429 errors back to the scaler for the next cycle's
`adjust()` call.

---

## Observability

The scaler emits `eprintln!` lines prefixed with `[simard]` at each
adjustment:

```
[simard] adaptive scaling: cpu=0.72 mem=0.45 errors_429=0 pressure=0.72 → max_concurrent=3 (hold)
[simard] adaptive scaling: cpu=0.92 mem=0.81 errors_429=2 pressure=0.92 → max_concurrent=2 (decrease)
[simard] adaptive scaling: cpu=0.15 mem=0.22 errors_429=0 pressure=0.22 → max_concurrent=3 (increase)
```

Fields:

| Field | Description |
|-------|-------------|
| `cpu` | CPU pressure `[0.0, 1.0]` or `n/a` if unavailable |
| `mem` | Memory pressure `[0.0, 1.0]` or `n/a` if unavailable |
| `errors_429` | Count of 429 errors in the sliding window |
| `pressure` | Aggregate pressure (max of the three) |
| `max_concurrent` | New value after adjustment |
| `(hold\|increase\|decrease)` | Which AIMD branch was taken |

---

## Module layout

```
src/ooda_loop/
├── adaptive_scaling.rs   # AdaptiveScaler, signal parsers, AIMD logic  (~250 LOC)
├── mod.rs                # pub mod adaptive_scaling;
├── cycle.rs              # Integration: scaler.adjust() before decide
└── types.rs              # OodaState.scaler field
```

---

## See also

- [Adaptive scaling concept](../concepts/adaptive-scaling.md) — design
  rationale and theory.
- [How to configure adaptive scaling](../howto/configure-adaptive-scaling.md)
  — operator guide.
- [Daemon mode](../daemon-mode.md) — the OODA cycle that hosts the
  scaler.
