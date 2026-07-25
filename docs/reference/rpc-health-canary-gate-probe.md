---
title: RPC-health canary gate probe
description: Reference for the rpc-health self-relaunch canary gate — how it genuinely dials the running memory daemon via the read-only `simard memory stats` operator-cli subcommand (RPC_HEALTH_PROBE_ARGS), the liveness pre-flight that refuses to green a dead daemon, the fail-closed timeout/spawn/exit handling, the SIMARD_STATE_ROOT-driven socket resolution, and the regression guards that keep the probe pointed at a real dispatched subcommand rather than the `unsupported command 'probe'` no-op it replaced.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./canary-gate-convergence.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./simard-memory-cli.md
  - ../howto/converge-a-stuck-red-canary-self-deploy.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
---

# RPC-health canary gate probe

> **Status: implemented.** The `rpc-health` gate
> ([`run_rpc_health_gate`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs))
> dials the running memory daemon with the read-only `simard memory stats`
> operator-cli subcommand (`RPC_HEALTH_PROBE_ARGS = ["memory", "stats"]`),
> guarded by a socket-liveness pre-flight and a fail-closed
> `run_probe_with_timeout`. It resolves #4646 — the deterministic red where the
> gate ran the non-existent `probe rpc` subcommand and therefore reddened
> **every** self-deploy candidate. The change is **additive and non-breaking**:
> `RelaunchGate::RpcHealth` keeps its position and semantics; no public signature
> changed; `RelaunchConfig` is unchanged.

## Why this exists

The `rpc-health` gate is the **only** canary that proves a relaunched candidate
can actually reach the live memory daemon over RPC. If it silently passes a
candidate that cannot dial the daemon, a broken binary can promote itself — the
exact failure the gate exists to catch.

Before #4639, the gate ran the argument vector `["probe", "rpc", "--timeout",
N]`. `probe` is **not** a dispatched operator-cli subcommand:
`dispatch_operator_cli`'s default arm returns `unsupported command 'probe'`. So
the gate never dialed anything — it errored on an unknown subcommand for *every*
candidate regardless of daemon health, reddening the canary deterministically on
every Overseer tick. The running binary fell behind merged `main`, `DeployDrift`
climbed, and the self-deploy loop re-queued the identical red refusal without
ever advancing past the stuck target SHA (#4646).

The fix points the probe at a **real, read-only, dispatched** subcommand —
`simard memory stats` — so a healthy candidate that can reach the daemon goes
green while a candidate that cannot still reddens. This does **not** weaken,
skip, or disable the gate: it makes the gate render a *true* verdict instead of
a constant false red.

## The probe: `simard memory stats`

`RPC_HEALTH_PROBE_ARGS` is a compile-time constant in
[`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs):

```rust
/// Argument vector the RpcHealth canary gate runs against the candidate binary
/// to genuinely dial the running memory daemon.
const RPC_HEALTH_PROBE_ARGS: &[&str] = &["memory", "stats"];
```

`simard memory stats` routes through `dispatch_operator_cli` →
`dispatch_memory_command` → `run_stats` → `open_reader_client`. When the live
daemon's socket is present — the self-deploy scenario, where the current daemon
is running while the candidate is verified — `open_reader_client` performs a real
stats **RPC round-trip** over that socket.

Why `memory stats` specifically:

- **Real dispatched subcommand.** Unlike `probe`, `memory` is a genuine arm of
  `dispatch_operator_cli`, so the gate actually executes a code path that dials
  the daemon.
- **Read-only, side-effect-free.** `stats` only reads. Unlike a write probe such
  as `memory remember` (which could pollute or quarantine the store), it verifies
  reachability without touching live state.
- **Fails closed on an unconnectable socket.** A socket that is present but
  cannot be connected surfaces as `SimardError::RpcSpawnFailed` (bug #2896) → a
  non-zero exit → the gate reddens. That is exactly the reachability signal the
  gate must assert.

## Liveness pre-flight

`memory stats` transparently falls through to a **tier-2 on-disk store** when the
daemon socket is **absent**. Left unchecked, an absent daemon would make the
probe exit 0 and *green* the gate without ever proving reachability — defeating
its entire purpose.

To prevent that, `run_rpc_health_gate` performs a socket-liveness pre-flight
**before** running the probe: it resolves the exact socket the candidate would
dial (see [Socket resolution](#socket-resolution)) and requires it to exist. A
genuinely absent daemon reddens here with an explicit detail:

```text
rpc health failed: no live daemon socket at <state_root>/memory.sock — the
candidate would fall through to a tier-2 on-disk store and pass without proving
reachability; refusing to green a dead daemon
```

A socket that is **present but unconnectable** passes the pre-flight, reaches the
probe, and reddens via the #2896 fail-closed path — the correct outcome.

## Socket resolution

The pre-flight and the probe target the **same** socket the candidate itself
would resolve under the scrubbed gate environment, so the check is never pointed
at a different socket than the one the daemon listens on.

- The gate re-injects the allow-listed `SIMARD_STATE_ROOT` (from
  [`canary_gate_env_allowlist()`](./canary-gate-convergence.md)), so
  `simard_state_root()` in the gate agrees with what the candidate resolves.
- `SIMARD_MEMORY_SOCKET` is honored **only** when the gate re-injects it (i.e.
  it is present in `config.canary_env`). Otherwise the candidate never sees an
  ambient override and resolves the default `<state_root>/memory.sock`; the
  pre-flight replicates that decision rather than honoring an ambient override
  the candidate cannot see.

| Input | Source | Effect |
| --- | --- | --- |
| `SIMARD_STATE_ROOT` | allow-listed via `canary_gate_env_allowlist()` | selects the state root → `<state_root>/memory.sock` |
| `SIMARD_MEMORY_SOCKET` | honored only if in `config.canary_env` | overrides the socket path when re-injected |
| `config.health_timeout` | `RelaunchConfig` (default `30s`) | maximum wait before the probe is killed and reddened |

## Fail-closed disposition

The probe subprocess is spawned through `run_probe_with_timeout`, which returns a
`ProbeOutcome`:

```rust
enum ProbeOutcome {
    Exited { status: ExitStatus, stderr: String },
    TimedOut,
    SpawnFailed(std::io::Error),
}
```

`run_rpc_health_gate` maps those to a `GateResult` **fail-closed** — only a clean
exit (a genuine round-trip) yields `passed: true`:

| Condition | `passed` | `detail` (bounded + credential-redacted) |
| --- | --- | --- |
| socket absent (pre-flight) | `false` | `rpc health failed: no live daemon socket at <path> …` |
| clean exit (status 0) | `true` | `rpc health check passed (memory stats round-trip)` |
| non-zero exit | `false` | `rpc health failed (exit <status>): <bounded stderr>` |
| timeout elapsed | `false` | `rpc health timed out after <N>s (memory stats did not return)` |
| spawn error | `false` | `rpc health probe failed to run: <io error>` |

`run_probe_with_timeout` also:

- **Drains the child's stderr concurrently** on a joined thread, so a probe that
  fills the ~64KB stderr pipe before exiting can never wedge (`try_wait` would
  otherwise never observe the exit — #4639 review F3).
- **Kills and reaps** a probe that exceeds `health_timeout`, then joins the drain
  thread, so a hung dial neither blocks the deploy loop forever nor leaks a child
  or thread.
- Routes every `detail` through `bound_gate_detail`, which **redacts credentials**
  (the shared URL-userinfo scrubber) and then truncates to 512 bytes on a UTF-8
  char boundary, so a token-bearing or multi-megabyte stderr never reaches a span
  or OTel attribute.

## Contract (studs)

- **Input:** the candidate binary path + a `RelaunchConfig`. The probe uses
  `config.health_timeout` and the socket resolved from the scrubbed gate env
  (`SIMARD_STATE_ROOT` / optionally `SIMARD_MEMORY_SOCKET`).
- **Environment boundary:** the child inherits only the `scrub_gate_env` base
  floor plus the `canary_env` allow-list; no live-socket or ambient env leaks
  into the canary beyond the audited names.
- **Output:** `GateResult { gate: RpcHealth, passed, detail }`. `passed == true`
  ⇒ a real `memory stats` round-trip succeeded; `passed == false` on an absent
  socket, non-zero exit, spawn error, or timeout (all fail-closed).
- **Invariants (regression-locked):**
  - `RPC_HEALTH_PROBE_ARGS` MUST resolve to a **dispatched** subcommand
    (`["memory", "stats"]`); the old `probe rpc` argv MUST remain
    `unsupported command`.
  - `RelaunchGate::RpcHealth` MUST remain in `default_gates()` (do-not-remove
    guard) — it is the only canary that proves live daemon reachability.
  - All emission is structured `tracing`/OTel; there are no `print!` /
    `println!` / `eprintln!` sinks in the gate or probe path.

## Regression tests

The behavior is pinned by unit tests in
[`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs):

| Test | Asserts |
| --- | --- |
| `rpc_health_probe_args_resolve_to_a_dispatched_subcommand` | `RPC_HEALTH_PROBE_ARGS == ["memory","stats"]`; that argv dispatches and **succeeds** against a hermetic tier-2 store; and the old `probe rpc` argv is still rejected as `unsupported command`. This is the #4646 reproduction guard: it goes red the moment the probe reverts to a non-dispatched subcommand. |
| `rpc_health_gate_fails_closed_on_missing_binary` | the gate reddens (never silently passes) for a bad candidate under `RelaunchConfig::default()` — a fail-closed end-to-end assertion. (With the default config's absent socket, the liveness pre-flight reddens first, so the missing binary need never be reached; the test pins the red verdict, not the specific fail path.) |
| `rpc_health_gate_fails_closed_on_timeout` | a probe that never returns within `health_timeout` is killed and reddened (`sleep 30` vs a 1s timeout). |
| `rpc_health_gate_fails_closed_when_daemon_socket_absent` | the liveness pre-flight reddens when no live daemon socket exists. |
| `rpc_health_gate_fails_closed_and_surfaces_stderr_on_nonzero_exit` | a non-zero probe exit reddens and surfaces the bounded, redacted stderr. |
| `rpc_health_stays_in_default_gates` | the gate is never silently dropped from the default set. |

The tests are hermetic: gate-behavior cases use the fake-candidate `#!/bin/sh`
pattern (no live daemon dependency), and the argv-dispatch case opens a tier-2
store in a fresh `TempDir` with no live socket present.

## Verification

```bash
# Reproduction + regression guard for #4646:
cargo test -p simard --lib \
  self_relaunch::gates::tests::rpc_health_probe_args_resolve_to_a_dispatched_subcommand

# Full rpc-health gate suite (fail-closed + isolation guards):
cargo test -p simard --lib self_relaunch::gates::tests
```

You can also run the probe by hand against a running daemon to confirm the shape
the gate exercises:

```bash
# Green when the daemon is reachable (real RPC round-trip):
SIMARD_STATE_ROOT=/var/lib/simard simard memory stats

# The gate reddens (pre-flight) when this socket is absent:
ls -l "${SIMARD_STATE_ROOT:-$SIMARD_HOME}/memory.sock"
```

## Compatibility

- **No API change.** `RelaunchConfig`, `RelaunchGate`, `GateResult`,
  `verify_canary`, `all_gates_passed`, and `default_gates` are unchanged. Only the
  private `RPC_HEALTH_PROBE_ARGS` constant and `run_rpc_health_gate` internals
  changed.
- **No new operator inputs.** No CLI flags, RPC, config keys, or "skip gate"
  controls. The trust boundary is unchanged.
- **Structured emission only.** All new emission is `tracing` structured
  key=value at ≥ INFO; no `print`-family macros, no silent fallbacks.
- **No `Bridge` naming.** New identifiers follow the no-Bridge-naming guard.

## See also

- [Canary gate isolation and self-deploy convergence](./canary-gate-convergence.md) —
  the `scrub_gate_env` / `canary_env` allow-list that supplies `SIMARD_STATE_ROOT`
  to this probe, and the per-gate tracing spans.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  how the reddening gate (`failing_gate` / `failing_detail`) is surfaced up the
  deploy path.
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md) —
  the operator runbook for diagnosing and confirming convergence.
- [`simard memory` CLI](./simard-memory-cli.md) — the `memory stats` subcommand
  the probe invokes.
- [Self-deploy API reference](./self-deploy-api.md) — the deployer and
  `DeployRefusal::RedCanary` path this gate feeds.
