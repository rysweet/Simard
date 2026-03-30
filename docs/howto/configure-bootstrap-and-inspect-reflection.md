---
title: "How to configure bootstrap and inspect reflection"
description: Bootstrap an explicit runtime selection through `simard bootstrap run`, inspect the truthful reflection snapshot, and validate the bounded engineer surfaces that hang off the same runtime contract.
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

The canonical bootstrap surface is now `simard bootstrap run ...`.

The old zero-argument `simard` bootstrap fallback is gone. Operators must pass the runtime selection explicitly. The terminal-backed engineer substrate now also lives on the canonical CLI through `simard engineer terminal ...`, while `simard_operator_probe terminal-run ...` remains as a compatibility alias.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet -- bootstrap run ...` works locally
- [ ] You know which identity, base type, topology, and state root you want to inspect

## 1. Bootstrap explicitly by default

Provide the identity, base type, topology, objective, and state root yourself.

For the builtin identities in this repo, the current scaffold accepts `local-harness`, `rusty-clawd`, or `copilot-sdk` as explicit base-type choices everywhere, and `simard-engineer` additionally accepts `terminal-shell` for a real local PTY-backed shell session. `rusty-clawd` is a distinct session backend, `terminal-shell` is intentionally local-only, and `copilot-sdk` remains an explicit alias of `local-harness`.

```bash
cargo run --quiet --   bootstrap run simard-engineer local-harness single-process   "verify current reflection metadata"   "$PWD/target/simard-state"
```

Look for output shaped like this:

```text
Probe mode: bootstrap-run
Identity: simard-engineer
Selected base type: local-harness
Topology: single-process
State root: /.../target/simard-state
Execution summary: ...
Reflection summary: ...
```

In the current bootstrap contract:

- missing required bootstrap inputs fail explicitly
- unsupported identity and base-type combinations fail explicitly
- unsupported topology and base-type combinations fail explicitly
- state roots are validated before persistence is touched
- no missing value is replaced through a hidden bootstrap fallback

### Variation: exercise a non-default builtin base type

Use this when you want to prove that bootstrap is not silently snapping back to `local-harness`.

```bash
cargo run --quiet --   bootstrap run simard-engineer copilot-sdk single-process   "verify copilot-sdk bootstrap selection"   "$PWD/target/simard-state"
```

Look for these lines:

```text
Selected base type: copilot-sdk
Topology: single-process
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

## 3. Exercise the terminal-backed engineer substrate

Use the canonical CLI when you want the real local PTY-backed engineer substrate:

```bash
cargo run --quiet --   engineer terminal single-process   $'working-directory: .
command: pwd
command: printf "terminal-foundation-ok\n"'   "$PWD/target/simard-state"
```

Look for:

- `Selected base type: terminal-shell`
- `Adapter implementation: terminal-shell::local-pty`
- `Terminal steps count: 2`
- `Terminal step 1: input: pwd`
- `Terminal last output line: terminal-foundation-ok`
- a transcript preview containing `terminal-foundation-ok`

This is the honest terminal slice: Simard can drive a real local PTY-backed shell session through the runtime, but it does not claim remote hosts or distributed terminal control.

## 4. Exercise the bounded engineer path

Use the canonical CLI when you want Simard to inspect a repo, print a short plan with explicit verification steps, choose one bounded local engineering action, verify the outcome, and persist truthful local artifacts:

```bash
STATE_ROOT="$PWD/target/simard-state"
ENGINEER_OBJECTIVE=$'inspect the repository state
run one safe local engineering action
verify the outcome explicitly
persist truthful local evidence and memory'

cargo run --quiet --   engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for:

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

## 5. Builtin defaults are no longer an operator CLI path

`BootstrapConfig` still understands `builtin-defaults` internally, but the operator-facing CLI no longer exposes a hidden zero-argument startup mode. If you want a local session through the CLI, pass the selection explicitly with `simard bootstrap run ...`.

That keeps the public surface honest:

- startup choices stay visible at the call site
- durable state roots stay explicit
- help output remains stable when `simard` is launched with no arguments

## Troubleshooting

### Missing required bootstrap config

**Symptom**: the command fails before the runtime is composed.

**Solution**: pass the missing positional bootstrap arguments explicitly:

```bash
cargo run --quiet --   bootstrap run simard-engineer local-harness single-process   "verify current reflection metadata"   "$PWD/target/simard-state"
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
