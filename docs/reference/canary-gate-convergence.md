---
title: Canary gate isolation and self-deploy convergence
description: Reference for the root-cause repair (#4440) that lets a healthy Overseer deploy candidate pass the relaunch canary and converge — the per-gate tracing/OTel spans emitted by verify_canary, the additive RelaunchConfig.canary_env narrow allow-list that supplies the environment a gate legitimately needs (scrub_gate_env), the preserved fail-closed gate ordering, and the self-deploy/DeployDrift loop advancing past a stuck target SHA once the canary goes green.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./self-deploy-source-prep.md
  - ./overseer-tick-self-healing.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../safe-self-update.md
  - ../../src/self_relaunch/gates.rs
  - ../../src/self_relaunch/types.rs
  - ../../src/self_deploy/source_prep.rs
  - ../../src/overseer/tests_deploy_drift.rs
---

# Canary gate isolation and self-deploy convergence

> **Status: implemented.** The per-gate tracing spans in
> [`verify_canary`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs),
> the additive `RelaunchConfig.canary_env` allow-list and `scrub_gate_env`
> helper
> ([`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs),
> [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)),
> and the canary build wiring
> ([`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs))
> ship the root-cause repair for the self-deploy red-canary convergence stall
> (#4440). The change is **additive and non-breaking**: `verify_canary`,
> `all_gates_passed`, `default_gates`, and the four-gate no-short-circuit
> sequence keep their signatures and semantics; `RelaunchConfig` gains one
> field that defaults to the pre-existing behavior.

## Why this exists

The [red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) (#4420)
made the reddening gate **visible** — the tick WARN and the `deploy_refused`
notification now name the specific gate and its detail. This feature acts on
that signal: it fixes the **root cause** that reddened the canary
deterministically on every Overseer tick, so a healthy candidate self-deploys
and `DeployDrift` returns to 0.

Before this change, the guarded deploy gate refused the same target commit
identically on every tick over many hours: the running binary fell several
commits behind merged `main`, the canary reddened on the *same* gate each cycle,
and the self-deploy loop re-queued the identical refusal without ever advancing
past the stuck target SHA. The reddening was not a genuine source regression —
it was a **gate that failed closed because the canary build context lacked
something the gate legitimately requires**. The gate was correct to fail closed
on a missing signal; the fix is to give a healthy candidate the environment it
needs so the gate can render a true verdict.

> **The specific reddening gate and its true dependency are identified
> empirically, not assumed.** Which gate reddens (and *why*) is read from the
> [#4420 diagnostics](./overseer-deploy-canary-diagnostics.md) — the named
> `failing_gate` / `failing_detail` — and the exact variable it needs is
> confirmed against a real canary run before the allow-list is populated. The
> `rpc-health` gate is used as the running illustration below because it dials a
> live endpoint (`simard memory stats`, a read-only RPC round-trip to the running
> memory daemon), but note that the concrete env dependency of the *actual*
> reddening gate must be established empirically rather than presumed to be
> `rpc-health`.

This feature does **not** weaken, skip, or disable any gate. It is the
"supply the missing signal correctly" repair, never the "bypass the check"
shortcut. Unhealthy candidates still redden; only a genuinely healthy candidate
now goes green.

## What changed

1. **Per-gate observability.** `verify_canary` wraps each gate in a `tracing`
   span so the exact gate that reddens — and its bounded, redacted detail — is
   emitted structurally as it runs, rather than only being reconstructed after
   the fact from the aggregate `TargetCanaryReport`.
2. **Narrow environment allow-list.** `RelaunchConfig` gains an additive
   `canary_env` field: an explicit allow-list of environment variable names that
   the gate subprocesses are permitted to inherit from the daemon's ambient
   environment. Every gate subprocess is spawned through `scrub_gate_env`, which
   `env_clear()`s and then re-injects only the always-required base variables
   plus the allow-listed names — mirroring
   [`scrub_git_env`](./self-deploy-source-prep.md) so a hostile ambient env can
   never hijack a gate.
3. **Convergence.** With a true green verdict available for a healthy candidate,
   the self-deploy / `DeployDrift` loop advances past the previously stuck target
   SHA. No loop logic changed — the loop was already correct; it was simply never
   handed a green canary.

## Data model

### `RelaunchConfig.canary_env` (additive field)

`RelaunchConfig` (in
[`src/self_relaunch/types.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/types.rs))
gains one field. The existing fields (`canary_target_dir`, `health_timeout`,
`manifest_dir`) are unchanged.

```rust
#[derive(Clone, Debug)]
pub struct RelaunchConfig {
    pub canary_target_dir: PathBuf,
    pub health_timeout: Duration,
    pub manifest_dir: PathBuf,
    /// Allow-list of environment variable NAMES that gate subprocesses may
    /// inherit from the daemon's ambient environment, on top of the always
    /// re-injected base set required for the gates to run at all (see
    /// `scrub_gate_env`). Names only — the values are read from the live
    /// environment at spawn time. Empty by default: gates then see only the
    /// deny-by-default base env floor.
    pub canary_env: Vec<String>,
}
```

`Default` sets `canary_env` to an empty `Vec`. Note this is **not** byte-for-byte
the pre-#4440 environment: before this change the gate subprocesses inherited the
daemon's *full* ambient environment (no scrubbing). Introducing `scrub_gate_env`
is a deliberate hardening — an empty `canary_env` is the new **deny-by-default
floor**, not a no-op. The base set re-injected by `scrub_gate_env` must therefore
be sized to keep every gate functional (in particular the `unit-test` gate shells
out to `cargo test`, which needs the Cargo/rustup toolchain env — see below);
otherwise a genuinely healthy candidate would falsely redden, defeating the fix.

| Field | Meaning | Default |
| --- | --- | --- |
| `canary_target_dir` | warm build/target dir for the canary | per-process temp dir |
| `health_timeout` | timeout passed to the `rpc-health` probe | `30s` |
| `manifest_dir` | crate root for the `unit-test` gate | `.` |
| `canary_env` | **new** — extra env var names gates may inherit | `[]` (empty) |

> **Names, not values.** `canary_env` carries variable *names*. The value is
> read from the live process environment at spawn time, never persisted in the
> config or logged. A name that is absent from the ambient environment is
> silently skipped (the gate then fails closed on the missing signal, exactly as
> before), so an allow-list entry can never inject an empty or attacker-supplied
> value.

### `scrub_gate_env` (gate subprocess env discipline)

Every gate subprocess (`smoke`, `unit-test`, `gym-baseline`, `rpc-health`) is
spawned through `scrub_gate_env`, which mirrors `scrub_git_env`:

```rust
/// `env_clear()` + selective re-injection of the always-required base
/// variables and any names in `config.canary_env`. Mirrors `scrub_git_env`,
/// but the base floor is broader than git's (PATH/HOME/SSH_AUTH_SOCK) because
/// the `unit-test` gate shells out to `cargo test`, which needs the Cargo/rustup
/// toolchain env to resolve a toolchain and registry. A hostile ambient env
/// (LD_PRELOAD, GIT_SSH_COMMAND, …) cannot reach a gate: only the base set plus
/// the explicit allow-list is forwarded. Names absent from the environment are
/// skipped. Nothing is logged from this function.
fn scrub_gate_env(cmd: &mut Command, config: &RelaunchConfig) {
    cmd.env_clear();
    // Universal floor: enough for ANY gate to run — the candidate binary's core
    // runtime plus the `cargo test` toolchain. Simard deploy-shape signals
    // (SIMARD_HOME, …) are NOT here; they arrive via `config.canary_env` (see
    // `canary_gate_env_allowlist`), keeping this floor minimal.
    const BASE: &[&str] = &[
        "PATH", "HOME",
        "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN",
        "SSH_AUTH_SOCK",
        "USER", "LOGNAME", "LANG", "LC_ALL", "TZ", "TERM",
    ];
    for var in BASE {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
    // Allow-list: names only, values read live at spawn time.
    for name in &config.canary_env {
        if let Ok(val) = std::env::var(name) {
            cmd.env(name, val);
        }
    }
}
```

The `canary_env` allow-list carries the Simard deploy-shape signals
([`canary_gate_env_allowlist`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs)
— `SIMARD_HOME`, `SIMARD_PROMPT_ASSETS_DIR`, `SIMARD_STATE_ROOT`), so the
`rpc-health` probe can reach the running daemon's socket and the candidate sees
the same environment the deployed systemd unit provides — without those signals
widening the universal floor. The base list is load-bearing (see the warning
below); confirm it against a real gate run before narrowing it.

> **The base set is load-bearing.** `env_clear()` on the `unit-test` gate is only
> safe if the Cargo/rustup toolchain variables are re-injected; the exact base
> list is whatever the four gates empirically require in the canary context.
> Confirm it against a real gate run before shipping — an over-narrow base set
> converts a healthy candidate into a false red (a self-inflicted stall), while
> an over-broad one erodes the deny-by-default guarantee.

**Invariants**

- **Deny by default.** Nothing outside the base set and `canary_env` is
  forwarded. `LD_PRELOAD`-class variables and `GIT_SSH_COMMAND` are never
  allow-listable through this path.
- **Names only.** The allow-list configures which names are read from the live
  environment; it never carries values.
- **Fail closed on absence.** A missing allow-listed name is skipped, so the gate
  proceeds with the missing signal and reddens if that signal is required.

## Behavior

### Per-gate tracing spans

`verify_canary` opens one span per gate and records the outcome. The span target
is `self_relaunch::gate`; each carries the gate name, pass/fail, and the bounded,
credential-redacted detail. Emission is `tracing` structured key=value only —
there are no `print!` / `println!` / `eprintln!` sinks.

```rust
for &gate in gates {
    let span = tracing::info_span!(
        target: "self_relaunch::gate", "canary_gate", gate = %gate
    );
    let _enter = span.enter();
    let result = run_gate(binary, gate, config);
    tracing::info!(
        target: "self_relaunch::gate",
        gate = %result.gate,
        passed = result.passed,
        detail = %bound_gate_detail(&result.detail),
        "canary gate evaluated"
    );
    results.push(result);
}
```

The sequence is unchanged: `Smoke → UnitTest → GymBaseline → RpcHealth`, run to
completion **without short-circuit**, so every gate's verdict is observable on a
single canary run even when an earlier gate has already reddened. Before this
change, gate details were only partially bounded and never redacted: the
`unit-test` gate truncated its `cargo` stderr to 200 bytes via a local
`truncate_output` helper, while the `smoke`, `gym-baseline`, and `rpc-health`
gates emitted untruncated, unredacted `stderr`. This feature routes **every**
gate's detail through `bound_gate_detail` before it reaches a span attribute:

- **Redaction first.** `bound_gate_detail` runs the detail through
  `redact_credentials` (the SEC-D2 URL-userinfo scrubber shared from
  [`self_deploy::source_prep`](./self-deploy-source-prep.md)) so a token-bearing
  remote URL embedded in a gate's stderr never reaches a span or OTel attribute.
- **Then a UTF-8-safe bound.** The redacted string is truncated to 512 bytes via
  the module's `truncate_output` helper (char-boundary-safe, appends `...`), so a
  pathological multi-megabyte stderr cannot bloat telemetry. Redact-then-bound is
  deliberate: truncation must never split a `://<userinfo>@` span in a way that
  leaves a live token visible.

A span attribute therefore never carries raw tokens, full stderr, or ambient env
values. This mirrors the credential redaction now applied to
`CanaryResult::refusal_reason` in
[`overseer::deploy`](./overseer-deploy-canary-diagnostics.md), so the reddening
gate's detail is bounded and clean on **both** the per-gate span and the composed
refusal reason.

| Attribute | Type | Present when | Example |
| --- | --- | --- | --- |
| `gate` | string | always (per span) | `rpc-health` |
| `detail` | string | a gate reddened (bounded, redacted) | `rpc health failed (exit 1): connection refused` |

Because emission stays on `tracing`, the spans **are** the OTel attributes
through Simard's existing tracing→OTel bridge; no exporter, subscriber, or
config change is required, and any redaction/retention already configured for
`self_relaunch::*` continues to apply.

### Supplying the missing signal (root-cause repair)

The canary is built and verified by `prepare_build_and_verify_canary`
([`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs)).
It constructs the `RelaunchConfig` for the gate suite and now populates
`canary_env` with the narrow allow-list of names the gates legitimately require
in the canary context:

```rust
let config = crate::self_relaunch::RelaunchConfig {
    manifest_dir: repo,
    canary_target_dir: target_dir.to_path_buf(),
    canary_env: canary_gate_env_allowlist(),
    ..Default::default()
};
```

`canary_gate_env_allowlist()` returns the minimal set of variable names required
for a healthy candidate's gates to render a true verdict. Those names are
**derived empirically** from the reddening gate that the
[#4420 diagnostics](./overseer-deploy-canary-diagnostics.md) actually name
(`failing_gate` / `failing_detail`) and confirmed against a real canary run —
not presumed. The RPC endpoint / socket path a probe like `rpc-health` would
dial is the *shape* of such a dependency, but the concrete variable must be
established from the observed failure rather than assumed. The list is a
compile-time constant of *names*; the values are read from the live daemon
environment at spawn time via `scrub_gate_env`. Nothing else is inherited.

This makes the previously deterministic red gate go **green for a genuinely
healthy candidate** — the gate was failing closed on an absent signal, and the
signal is now supplied through an audited allow-list rather than by broadening
inheritance or removing the check.

### Convergence

Once a healthy candidate produces a green canary, the guarded deploy gate no
longer returns `DeployRefusal::RedCanary`; the
[`OrchestratedBinaryDeployer`](./self-deploy-api.md) performs the swap, and the
next drift observation sees `DeployDrift == 0`. The self-deploy loop advances
past the previously stuck target SHA instead of re-queuing an identical refusal.
No loop, requeue, or drift logic changed.

## Fail-closed invariants (preserved)

This repair is bounded by the same rails as the diagnostics feature; none is
relaxed:

- **Canary is the authorization boundary.** The four gates still gate promotion
  and still fail closed. An unhealthy candidate reddens exactly as before.
- **No short-circuit.** All gates run to completion so every verdict is
  observable; `all_gates_passed` still requires every gate to pass.
- **Deny-by-default env.** `scrub_gate_env` forwards only the base set plus the
  explicit `canary_env` allow-list; no ambient variable reaches a gate
  otherwise.
- **`is_transient` guard unchanged.** A `deploy_gate` / `target_canary`
  capability failure is still **never** transient
  ([self-healing classifier](./overseer-tick-self-healing.md)); the #4420
  fatal-refusal semantics are untouched, so a now-healthy deploy is not treated
  as fatal and a genuine red is not retried away.
- **Promotion identity.** The artifact verified is the artifact that ships; no
  rebuild happens between verify and promote.
- **No new operator inputs.** No CLI flags, RPC, config keys, or "skip gate"
  controls. The trust boundary is unchanged.

## Regression tests

The change ships bidirectional tests proving the gate renders both verdicts and
the loop converges:

| Test surface | Asserts |
| --- | --- |
| [`src/self_relaunch/gates.rs`](https://github.com/rysweet/Simard/blob/main/src/self_relaunch/gates.rs) (unit) | An unhealthy candidate still reddens the previously-stuck gate (fail-closed preserved); `scrub_gate_env` forwards only the base set + allow-listed names and skips absent names; a gate span/detail is redacted and bounded. |
| [`src/overseer/tests_deploy_drift.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/tests_deploy_drift.rs) (integration) | A healthy candidate produces a green canary and the self-deploy / `DeployDrift` loop advances past the stuck target SHA instead of re-queuing the identical refusal (convergence). |

The unhealthy-candidate test reproduces the original red-canary condition; the
healthy-candidate test proves the green pass **and** the loop advance, so the fix
cannot regress into "gate green but loop still stuck" or "gate weakened to force
green."

## Compatibility

- **Additive field.** `RelaunchConfig` gains `canary_env: Vec<String>`,
  defaulting to empty — the deny-by-default env floor described above (note this
  is a deliberate hardening, not byte-identical to the pre-#4440 full-ambient
  inheritance). Existing construction sites are updated in the same change.
- **Signatures unchanged.** `verify_canary`, `all_gates_passed`,
  `default_gates`, `RelaunchGate`, and `GateResult` are unchanged.
- **Diagnostics preserved.** `CanaryResult.failing_gate` / `failing_detail`,
  `refusal_reason`, and the `overseer::deploy` WARN
  ([red-canary diagnostics](./overseer-deploy-canary-diagnostics.md)) are reused,
  not reimplemented. The enriched refusal reason still replaces the opaque
  aggregate string. The `DETAIL_CAP` truncation bound (`bound_detail`) is reused
  by being exposed/shared for the gate span emitter; **credential redaction over
  gate details is net-new** and added there, not inherited from #4420.
- **No `print`-family macros.** All new emission is `tracing` structured
  key=value at ≥ INFO; no silent fallbacks.
- **No `Bridge` naming.** New identifiers follow the
  [no-Bridge-naming guard](./no-bridge-naming-guard.md).

## See also

- [RPC-health canary gate probe](./rpc-health-canary-gate-probe.md) — how the
  `rpc-health` gate genuinely dials the daemon via `simard memory stats`
  (`RPC_HEALTH_PROBE_ARGS`), the socket-liveness pre-flight, the fail-closed
  timeout handling, and the #4646 regression guard.
- [Overseer deploy red-canary diagnostics](./overseer-deploy-canary-diagnostics.md) —
  the #4420 observability this repair acts on (`failing_gate` / `failing_detail`,
  `refusal_reason`, the `overseer::deploy` WARN, the `is_transient` guard).
- [How to converge a stuck red-canary self-deploy](../howto/converge-a-stuck-red-canary-self-deploy.md) —
  the operator runbook for diagnosing and confirming convergence.
- [Self-deploy API reference](./self-deploy-api.md) — the `GuardedDeployer`,
  `DeployRefusal`, and the `OrchestratedBinaryDeployer` swap path.
- [Self-deploy source preparation](./self-deploy-source-prep.md) — the
  cwd-independent source preparer, the build lock, and `scrub_git_env` (the model
  `scrub_gate_env` mirrors).
- [Overseer tick self-healing](./overseer-tick-self-healing.md) — the
  `is_transient` fail-closed classifier and the SR-1 latch invariant.
