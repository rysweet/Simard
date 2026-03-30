---
title: "How to configure bootstrap and inspect reflection"
description: Verify the current `simard` bootstrap entrypoint, inspect the truthful reflection snapshot exposed by the runtime, and understand the planned bootstrap subcommand.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ./carry-meeting-decisions-into-engineer-sessions.md
  - ../reference/runtime-contracts.md
  - ../concepts/truthful-runtime-metadata.md
---

# How to configure bootstrap and inspect reflection

Use this guide when you need to answer two questions:

- what bootstrap inputs did Simard actually use?
- what does the live runtime report through reflection?

## Status

Today, the real `simard` binary bootstraps directly from `SIMARD_*` environment variables.

The future `simard bootstrap run ...` subcommand is planned, not shipped yet.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet --` works locally when the required `SIMARD_*` variables are set
- [ ] You know which identity, base type, topology, and state root you want to inspect

## 1. Use explicit bootstrap configuration by default

Provide the prompt root, identity, base type, topology, objective, and state root yourself.

For the builtin identities in this repo, the current scaffold accepts `local-harness`, `rusty-clawd`, or `copilot-sdk` as explicit base-type choices everywhere, and `simard-engineer` additionally accepts `terminal-shell` for a real local PTY-backed shell session. `rusty-clawd` is a distinct session backend, `terminal-shell` is intentionally local-only, and `copilot-sdk` remains an explicit alias of `local-harness`.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_IDENTITY=simard-engineer \
SIMARD_BASE_TYPE=local-harness \
SIMARD_RUNTIME_TOPOLOGY=single-process \
SIMARD_OBJECTIVE="verify current reflection metadata" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
cargo run --quiet --
```

Look for output shaped like this:

```text
Simard local runtime executed successfully.
Bootstrap mode: explicit-config
Bootstrap selection: identity=simard-engineer, base_type=local-harness, topology=single-process
State root: /.../target/simard-state
Snapshot: state=ready, topology=single-process, base_type=local-harness
Adapter implementation: local-harness
Shutdown: stopped
```

Planned unified equivalent:

```bash
simard bootstrap run simard-engineer local-harness single-process \
  "verify current reflection metadata" \
  "$PWD/target/simard-state"
```

In the current bootstrap contract:

- missing required bootstrap inputs fail explicitly
- unsupported identity and base-type combinations fail explicitly
- unsupported topology and base-type combinations fail explicitly
- no missing value is replaced after startup unless you deliberately opted into builtin defaults

### Variation: exercise a non-default builtin base type

Use this when you want to prove that bootstrap is not silently snapping back to `local-harness`.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_IDENTITY=simard-engineer \
SIMARD_BASE_TYPE=copilot-sdk \
SIMARD_RUNTIME_TOPOLOGY=single-process \
SIMARD_OBJECTIVE="verify copilot-sdk bootstrap selection" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
cargo run --quiet --
```

Look for these lines:

```text
Bootstrap selection: identity=simard-engineer, base_type=copilot-sdk, topology=single-process
Snapshot: state=ready, topology=single-process, base_type=copilot-sdk
Adapter implementation: local-harness
```

That is the important contract boundary: the runtime records the explicit selection you asked for, and it also reports the honest implementation identity. Simard does not silently rewrite your selection, but it also does not pretend the alias is already a distinct backend.

## 2. Inspect the reflection fields

`ReflectionSnapshot` exposes the truth-bearing runtime metadata directly:

- `manifest_contract`
- `runtime_node`
- `mailbox_address`
- `agent_program_backend`
- `handoff_backend`
- `adapter_backend`
- `adapter_capabilities`
- `adapter_supported_topologies`
- `active_goal_count`
- `active_goals`
- `proposed_goal_count`
- `proposed_goals`
- `topology_backend`
- `transport_backend`
- `supervisor_backend`
- `memory_backend`
- `evidence_backend`
- `goal_backend`

For the current CLI bootstrap path, the manifest entrypoint is the bootstrap assembly boundary, not the thin binary wrapper.

```rust
use simard::{FreshnessState, ReflectiveRuntime};

let snapshot = runtime.snapshot()?;

assert_eq!(
    snapshot.manifest_contract.entrypoint,
    "simard::bootstrap::assemble_local_runtime"
);
assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
assert_eq!(
    snapshot.manifest_contract.freshness.state,
    FreshnessState::Current
);
assert_eq!(snapshot.runtime_node.to_string(), "node-local");
assert_eq!(snapshot.mailbox_address.to_string(), "inmemory://node-local");
assert_eq!(snapshot.agent_program_backend.identity, "agent-program::objective-relay");
assert_eq!(snapshot.handoff_backend.identity, "handoff::json-file-store");
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
assert_eq!(snapshot.topology_backend.identity, "topology::in-process");
assert_eq!(snapshot.transport_backend.identity, "transport::in-memory-mailbox");
assert_eq!(snapshot.supervisor_backend.identity, "supervisor::in-process");
assert_eq!(snapshot.memory_backend.identity, "memory::json-file-store");
assert_eq!(snapshot.evidence_backend.identity, "evidence::json-file-store");
```

If you launched with `copilot-sdk`, `snapshot.selected_base_type` still shows the alias you chose while `snapshot.adapter_backend.identity` remains `local-harness`. If you launched with `rusty-clawd`, reflection reports `snapshot.adapter_backend.identity == "rusty-clawd::session-backend"`. If you launch the engineer identity with `terminal-shell`, reflection reports `snapshot.adapter_backend.identity == "terminal-shell::local-pty"`, `snapshot.adapter_capabilities` includes `terminal-session`, and `snapshot.adapter_supported_topologies == ["single-process"]`.

## 3. Exercise the terminal-backed engineer path

Today, use the compatibility binary when you want to validate the terminal-backed engineer substrate directly:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  terminal-run single-process \
  $'working-directory: .\ncommand: pwd\ncommand: printf "terminal-foundation-ok\\n"'
```

Look for:

- `Mode: engineer`
- `Selected base type: terminal-shell`
- `Adapter implementation: terminal-shell::local-pty`
- `Terminal evidence: terminal-command-count=2`
- a transcript preview containing `terminal-foundation-ok`

Planned unified equivalent:

```bash
simard engineer terminal single-process \
  $'working-directory: .\ncommand: pwd\ncommand: printf "terminal-foundation-ok\\n"'
```

This is the honest terminal slice: Simard can drive a real local PTY-backed shell session through the runtime, but it does not claim remote hosts or distributed terminal control.

## 4. Exercise the bounded engineer path

Today, use the compatibility binary when you want Simard to inspect a repo, print a short plan with explicit verification steps, choose one bounded local engineering action, verify the outcome, and persist truthful local artifacts:

```bash
STATE_ROOT="$PWD/target/simard-state"
ENGINEER_OBJECTIVE=$'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory'

cargo run --quiet --bin simard_operator_probe -- \
  engineer-loop-run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for:

- `Mode: engineer`
- `Repo root: ...`
- `Active goals count: ...`
- `Execution scope: local-only`
- `Action plan: ...`
- `Verification steps: ...`
- `Selected action: cargo-metadata-scan` (or another explicit local-first action)
- `Action status: success`
- `Changed files after action: <none>` (or one expected repo-relative path for a bounded structured edit)
- `Verification status: verified`

When you pass the same explicit state root that an earlier `meeting` run used, this same command also prints `Carried meeting decisions: N` and up to the three most recent `Carried meeting decision <index>:` lines.

Planned unified equivalent:

```bash
simard engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

## 5. Opt in to builtin defaults only when you mean it

For local bootstrap, Simard supports explicit opt-in defaults.

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults cargo run --quiet --
```

In that mode:

- builtin prompt assets come from the repository prompt asset set
- builtin state root resolves to `target/simard-state`
- builtin identity resolves to `simard-engineer`
- builtin base type resolves to `local-harness`
- builtin topology resolves to `single-process`
- configuration sources are recorded as explicit opt-in, not silent recovery

Planned unified equivalent:

```bash
simard bootstrap run simard-engineer local-harness single-process "bootstrap the Simard engineer loop"
```

Builtin defaults are startup choices. They are not runtime recovery behavior.

## Troubleshooting

### Missing required bootstrap config

**Symptom**: startup fails before the runtime is composed.

**Solution**:

```bash
export SIMARD_PROMPT_ROOT="$PWD/prompt_assets"
export SIMARD_IDENTITY=simard-engineer
export SIMARD_BASE_TYPE=local-harness
export SIMARD_RUNTIME_TOPOLOGY=single-process
export SIMARD_OBJECTIVE="verify current reflection metadata"
cargo run --quiet --
```

Or opt in explicitly:

```bash
export SIMARD_BOOTSTRAP_MODE=builtin-defaults
```

### Base type or topology selection fails

**Symptom**: bootstrap resolves, but runtime composition returns `UnsupportedBaseType`, `AdapterNotRegistered`, `UnsupportedRuntimeTopology`, or `UnsupportedTopology`.

**Solution**: pick a base type the identity allows, make sure the base-type factory is registered for that identity, and choose a topology supported by both the injected runtime services and the selected backend. Simard does not substitute a different base type or downgrade the topology silently.

### Reflection metadata is truthful but incomplete

**Symptom**: the reflection values do not match the runtime you actually assembled.

**Solution**: inspect the bootstrap inputs and the selected base type. Reflection reports the active wiring, so incorrect metadata usually means the runtime was assembled differently than expected.

### Calls fail after stop

**Symptom**: `run()`, `start()`, or a repeated `stop()` returns `RuntimeStopped`.

**Solution**: compose a new runtime instance. Stopped runtimes remain inspectable, but they are not reusable.

## See also

- [Simard CLI reference](../reference/simard-cli.md)
- [Runtime contracts reference](../reference/runtime-contracts.md)
- [Concept: truthful runtime metadata](../concepts/truthful-runtime-metadata.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)
