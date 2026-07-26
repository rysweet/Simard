---
title: "Reference: RPC-Health Gate Timeout, Retry & Diagnostics"
description: >
  How the self-deploy rpc-health gate becomes robust and diagnosable: a
  configurable memory-stats probe timeout, bounded retry with backoff, and three
  distinct fail-closed ProbeOutcomes — TimedOut, EmptyStats, and Unreachable — so
  deploy drift is actionable. Fail-closed strictness is preserved by default;
  host disk pressure is explicitly out of scope (dedup key
  process:self_deploy_blocked).
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./self-deploy-api.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./canary-gate-convergence.md
  - ./deterministic-canary-unit-test-gate.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
---

# Reference: RPC-Health Gate Timeout, Retry & Diagnostics

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary sources:
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs),
> [`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs).
> Tracked by dedup key `process:self_deploy_blocked`.

## Overview

The final self-deploy gate, `rpc-health`, verifies the candidate binary can dial
the running memory daemon by executing `simard memory stats` against it. A canary
deploy failed with an opaque message — `rpc health timed out after 30s (memory
stats did not return)` (deploy `953d5a9d407a`) — while self-deploy drift grew
(daemon `0.37.0` vs. main building `0.38.0`, 3 commits behind). The memory-stats
RPC could exceed the fixed 30-second window, or return nothing, with **no retry**
and **one opaque error** that did not say *why*.

The gate now has:

1. a **configurable timeout** for the memory-stats probe (no longer a hardcoded
   30 s);
2. **bounded retry with backoff** for a probe that transiently times out or
   fails to connect; and
3. **three distinct, structured fail-closed outcomes** — `TimedOut`,
   `EmptyStats`, and `Unreachable` — so an operator can tell *which* failure
   mode occurred and act on it.

**Fail-closed strictness is preserved.** Every one of the three failure outcomes
still reddens the gate (`passed: false`) and blocks the deploy by default. Only a
clean round-trip that returns memory stats yields `passed: true`.

> **Out of scope.** The earlier `No space left on device (os error 28)` failure
> is **host disk pressure**, tracked separately as `resource:host_disk_load`.
> This gate does not attempt to reclaim disk.

## Diagnostic outcomes

`ProbeOutcome` classifies the terminal disposition of the probe subprocess. Each
non-success outcome maps to a distinct, log-safe diagnostic and a **fail-closed,
non-relaunch** posture.

```rust
// src/self_relaunch/gates.rs
enum ProbeOutcome {
    /// Clean exit; stderr carried for a red verdict's detail.
    Exited { status: ExitStatus, stderr: String },
    /// The probe exhausted `health_timeout` (a wedged daemon that accepted the
    /// connection but never answered). Killed and reaped; fail-closed.
    TimedOut,
    /// The probe exited 0 but produced no memory stats on stdout — the daemon
    /// answered but returned nothing usable. Fail-closed (a hollow success).
    EmptyStats,
    /// The probe could not reach an endpoint at all: spawn/connect failure, or
    /// the socket the candidate would dial is absent. Fail-closed.
    Unreachable(std::io::Error),
}
```

### Classification rules

| Observation | Outcome | Gate verdict |
| --- | --- | --- |
| Exit 0 **with** memory stats on stdout | `Exited` (success) | `passed: true` |
| Exit 0 **with empty** stdout | `EmptyStats` | `passed: false` |
| Non-zero exit | `Exited` (failure) | `passed: false` |
| Exceeded `health_timeout` | `TimedOut` | `passed: false` |
| Spawn/connect failure, or absent socket | `Unreachable` | `passed: false` |

> **Refactor note.** `Unreachable` supersedes the prior `SpawnFailed(io::Error)`
> variant (same payload, clearer name that also covers an absent socket);
> `EmptyStats` is net-new. Both changes are exhaustive-`match` sites in
> `run_rpc_health_gate` — every existing `ProbeOutcome::SpawnFailed` arm is
> renamed and a new `EmptyStats` arm added, so the compiler enforces coverage.

Because a plain exit-0 no longer proves health, the probe now **captures** stdout
(previously discarded) to confirm memory stats were actually returned
(distinguishing `EmptyStats` from a genuine round-trip). The existing dedicated
**drain thread** design is preserved — now for both stdout and stderr — so a full
pipe buffer can never wedge the child and be misclassified as `TimedOut`
(#4639 review F3).

## Configuration

`RelaunchConfig` gains additive fields
([`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs)).
The struct is constructed via `..RelaunchConfig::default()` at every call site, so
adding fields with defaults is non-breaking.

```rust
pub struct RelaunchConfig {
    // ...existing fields...

    /// Per-attempt timeout for the memory-stats rpc-health probe.
    /// Default: 30s (preserves prior behaviour). Enforced as a spawn + bounded
    /// wait inside the gate.
    pub health_timeout: Duration,

    /// Maximum number of probe attempts before the gate reddens.
    /// Bounded and positive; default 3.
    pub health_probe_max_attempts: usize,

    /// Base backoff between probe attempts (capped exponential). Default 2s.
    pub health_probe_backoff: Duration,
}
```

| Field | Default | Meaning |
| --- | --- | --- |
| `health_timeout` | `30s` | Per-attempt probe timeout (was a hardcoded 30 s) |
| `health_probe_max_attempts` | `3` | Total attempts before fail-closed |
| `health_probe_backoff` | `2s` | Base backoff, capped exponential (`base << (attempt-1)`, clamped to `base * 8`) between attempts |

Retry applies **only** to the **transient** outcomes (`TimedOut`, `Unreachable`);
once attempts are exhausted the gate reddens fail-closed with the last outcome's
diagnostic. `EmptyStats` is **not** retried: a daemon that answers exit-0 with no
stats is exhibiting a deterministic (non-transient) fault, so the gate reddens
immediately rather than re-probing a condition a retry is unlikely to clear.
Backoff is bounded (capped exponential) so retries never stall the deploy loop.

## Structured diagnostics

Each outcome emits a precise, structured tracing event (and OTel span) —
**never** `print!`/`println!`. Diagnostics carry the outcome classification, the
attempt count, and a bounded, sanitized stderr snippet. They **never** leak
secrets, tokens, absolute host paths, or raw subprocess stdout: only length,
classification, and a bounded sanitized snippet are logged.

```text
gate=rpc-health outcome=timed_out attempt=2/2 timeout=30s → FAIL (fail-closed)
gate=rpc-health outcome=empty_stats attempt=1/2 → FAIL (daemon answered, no stats)
gate=rpc-health outcome=unreachable attempt=2/2 detail="socket absent" → FAIL
```

This turns the previously opaque `memory stats did not return` into an
actionable classification, so deploy drift can be diagnosed and converged.

## Examples

### A transient timeout retries, then reddens

```text
gate=rpc-health attempt=1 → timed_out (30s)
  backoff …
gate=rpc-health attempt=2 → timed_out (30s)
→ FAIL CLOSED: rpc-health exhausted 2 attempts (last: timed_out)
```

### An answering-but-empty daemon is distinguished from a healthy one

```text
gate=rpc-health attempt=1 → exit 0, stdout empty → empty_stats
→ FAIL CLOSED: daemon reachable but returned no memory stats
```

## Fail-closed guarantees

- All three failure outcomes (`TimedOut`, `EmptyStats`, `Unreachable`) yield
  `passed: false` and **block the deploy** by default.
- **Only** a clean exit that returns memory stats yields `passed: true`.
- Retry/backoff is **bounded** — it never loops forever; the gate always reaches
  a terminal pass/fail within `health_probe_max_attempts`.
- No secret/PII/host-path leakage in diagnostics.

## Regression tests

Co-located in
[`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
and
[`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs):

- `probe_timeout_is_transient_and_fail_closed` — a wedged probe yields `TimedOut`,
  is killed and reaped, and is classified transient.
- `probe_empty_stats_is_fail_closed` — exit-0-empty-stdout yields `EmptyStats`
  and reddens (hollow success rejected); not transient.
- `probe_nonempty_stdout_is_a_genuine_round_trip` — exit-0 WITH stats on stdout is
  a successful `Exited` round-trip (the only healthy outcome).
- `probe_unreachable_is_fail_closed` — a spawn/connect failure yields
  `Unreachable` and is classified transient.
- `empty_stats_is_not_retried` — `EmptyStats` is deterministic (not transient) so
  the bounded-retry loop never retries it; `TimedOut` is transient.
- `rpc_health_gate_reddens_when_unreachable_after_bounded_retry` — the gate fails
  closed for an unreachable daemon even after the bounded retry budget, and the
  attempt budget is positive.
- `default_health_timeout_preserves_prior_30s_behaviour` — the per-attempt probe
  timeout default is the prior fixed 30s.
- `default_health_probe_attempts_is_bounded_positive` /
  `default_health_probe_backoff_is_set` — the retry budget and base backoff have
  bounded, positive defaults.

## Related

- [Self-Deploy API](./self-deploy-api.md)
- [Overseer Deploy Red-Canary Diagnostics](./overseer-deploy-canary-diagnostics.md)
- [Canary Gate Isolation & Self-Deploy Convergence](./canary-gate-convergence.md)
- How-to: [Converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md)
