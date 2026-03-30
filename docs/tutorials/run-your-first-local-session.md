---
title: "Tutorial: Run your first local session"
description: Learn the Simard local runtime flow, from bootstrap through reflection, goal stewardship, and shutdown.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: tutorial
related:
  - ../index.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../reference/runtime-contracts.md
---

# Tutorial: Run your first local session

This tutorial follows the runtime path that exists in the repository today.

## What you'll learn

- How the local runtime starts with explicit configuration
- How explicit opt-in defaults behave
- What reflection reports after a run
- How durable goal stewardship flows into later sessions
- How runtime node, mailbox, and backend wiring appear in reflection
- What stop semantics look like in practice

## Prerequisites

- Rust and Cargo installed
- A shell in the repository root

## Step 1: Run the current local runtime with explicit configuration

From the repository root, start Simard with a real prompt asset directory, an explicit objective, and an explicit durable state root.

For the builtin identities in this repo, you can currently choose `local-harness`, `rusty-clawd`, or `copilot-sdk` everywhere, and `simard-engineer` additionally accepts `terminal-shell` for a real local PTY-backed shell session. `rusty-clawd` is a distinct backend, `terminal-shell` is intentionally local-only, and `copilot-sdk` remains an explicit alias of the local harness implementation. The default bootstrap path still opts into `single-process`, but the runtime can now inject a loopback `multi-process` topology when you request a supported pairing such as `rusty-clawd + multi-process`.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="exercise the local runtime" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="local-harness" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

You should see output shaped like this:

```text
Simard local runtime executed successfully.
Bootstrap mode: explicit-config
Config sources: prompt_root=env:SIMARD_PROMPT_ROOT, objective=env:SIMARD_OBJECTIVE, state_root=env:SIMARD_STATE_ROOT, base_type=env:SIMARD_BASE_TYPE, topology=env:SIMARD_RUNTIME_TOPOLOGY
Bootstrap selection: identity=simard-engineer, base_type=local-harness, topology=single-process
State root: /.../target/simard-state
Snapshot: state=ready, topology=single-process, base_type=local-harness
Adapter implementation: local-harness
Shutdown: stopped
```

**Checkpoint**: this is the real CLI path. `src/main.rs` is the thin wrapper; `bootstrap::run_local_session` owns the run loop, and `simard::bootstrap::assemble_local_runtime` remains the reflected assembly boundary.

## Step 2: Switch to another built-in base type

Run the same bootstrap path again, but select `copilot-sdk` explicitly.

```bash
SIMARD_PROMPT_ROOT="$PWD/prompt_assets" \
SIMARD_OBJECTIVE="exercise the copilot-sdk runtime path" \
SIMARD_STATE_ROOT="$PWD/target/simard-state" \
SIMARD_IDENTITY="simard-engineer" \
SIMARD_BASE_TYPE="copilot-sdk" \
SIMARD_RUNTIME_TOPOLOGY="single-process" \
cargo run --quiet
```

Look for these lines:

```text
Bootstrap selection: identity=simard-engineer, base_type=copilot-sdk, topology=single-process
Snapshot: state=ready, topology=single-process, base_type=copilot-sdk
Adapter implementation: local-harness
```

**Checkpoint**: the runtime contract is explicit. `copilot-sdk` is selectable now, but its underlying implementation still stays `local-harness`. Simard preserves the selected base type without pretending the alias is already a distinct backend integration.

### Variation: exercise the terminal-backed engineer path

Use the shipped operator probe to drive a real local PTY-backed shell session through the runtime:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  terminal-run single-process \
  $'working-directory: .\ncommand: pwd\ncommand: printf "terminal-foundation-ok\\n"'
```

Look for these lines:

```text
Probe mode: terminal-run
Selected base type: terminal-shell
Adapter implementation: terminal-shell::local-pty
Terminal evidence: terminal-command-count=2
```

**Checkpoint**: this path is no longer synthetic. The runtime is actually allocating a local PTY-backed shell session and preserving a transcript preview in evidence, while still honestly limiting the feature to local single-process execution.

### Variation: exercise the local-first engineer loop

Use the shipped operator probe to inspect the repo, run one explicit safe engineering action, and verify the result:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  engineer-loop-run single-process . \
  $'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory'
```

Look for these lines:

```text
Probe mode: engineer-loop-run
Repo root: /path/to/repo
Active goals count: 0
Execution scope: local-only
Selected action: cargo-metadata-scan
Verification status: verified
```

**Checkpoint**: Simard is now doing more than opening a shell. It is inspecting repo state, choosing a bounded repo-native action, verifying that repo grounding stayed stable, and persisting truthful memory/evidence for the loop. When a shared state root already contains durable goals, the same probe also reports the active top-goal set.

## Step 3: Curate durable top goals and reuse them in later sessions

Use the goal-curation probe to persist a truthful top-5 goal set:

```bash
STATE_ROOT="$PWD/target/simard-goal-demo"

cargo run --quiet --bin simard_operator_probe -- \
  goal-curation-run local-harness single-process \
  "$(cat <<'EOF'
goal: Keep Simard's top 5 goals current | priority=1 | status=active | rationale=long-horizon stewardship is now a shipped product responsibility
goal: Preserve meeting-to-engineer continuity | priority=2 | status=active | rationale=meeting outputs should shape later engineer sessions
EOF
)" \
  "$STATE_ROOT"
```

Then point the engineer loop at the same state root:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  engineer-loop-run single-process . \
  $'inspect the repository state\nrun one safe local engineering action\nverify the outcome explicitly\npersist truthful local evidence and memory' \
  "$STATE_ROOT"
```

Look for:

- `Probe mode: goal-curation-run`
- `Identity: simard-goal-curator`
- `Active goal 1: p1 [active] Keep Simard's top 5 goals current`
- the later engineer-loop run reporting the same active goals

**Checkpoint**: this is the current honest backlog-stewardship slice. Simard can now preserve durable top goals and feed them into later engineer sessions without pretending it already has a full remote PM agent.

## Step 4: Exercise a composite identity and loopback multi-process runtime

Use the shipped operator probe to validate the broader runtime seams like an operator would.

```bash
cargo run --quiet --bin simard_operator_probe -- \
  bootstrap-run simard-composite-engineer local-harness single-process \
  "exercise the composite engineer loop"
```

Then run the multi-process path:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  bootstrap-run simard-engineer rusty-clawd multi-process \
  "exercise loopback multi-process runtime"
```

Look for:

- `Identity components: simard-engineer, simard-meeting, simard-gym, simard-goal-curator, simard-improvement-curator`
- `Topology: multi-process`
- `Topology backend: topology::loopback-mesh`
- `Transport backend: transport::loopback-mailbox`
- `Adapter implementation: rusty-clawd::session-backend`

**Checkpoint**: composition and topology are now visible runtime facts, not just architecture aspirations.

## Step 5: Promote a persisted review into durable improvement priorities

First, generate a persisted review artifact:

```bash
STATE_ROOT="$PWD/target/simard-improvement-demo"

cargo run --quiet --bin simard_operator_probe -- \
  review-run local-harness single-process \
  "inspect the current Simard review surface and preserve concrete proposals" \
  "$STATE_ROOT"
```

Then promote explicit operator-approved proposals into durable priorities in the same state root:

```bash
cargo run --quiet --bin simard_operator_probe -- \
  improvement-curation-run local-harness single-process \
  "$(cat <<'EOF'
approve: Capture denser execution evidence | priority=1 | status=active | rationale=operators need denser execution evidence now
approve: Promote this pattern into a repeatable benchmark | priority=2 | status=proposed | rationale=carry this into the next benchmark planning pass
EOF
)" \
  "$STATE_ROOT"
```

Look for:

- `Probe mode: improvement-curation-run`
- `Identity: simard-improvement-curator`
- `Approved proposals: 2`
- `Active goal 1: p1 [active] Capture denser execution evidence`
- `Proposed goal 1: p2 [proposed] Promote this pattern into a repeatable benchmark`

**Checkpoint**: Simard now has an honest evidence-to-priority loop. Review findings stay operator-reviewable, and approved improvements become durable backlog state instead of dead-end artifacts.

## Step 6: Opt in to builtin defaults

Builtin defaults exist for local bootstrap convenience, but they are only used when startup opts in.

```bash
SIMARD_BOOTSTRAP_MODE=builtin-defaults \
cargo run --quiet
```

You should see:

- `Bootstrap mode: builtin-defaults`
- `prompt_root=opt-in:SIMARD_BOOTSTRAP_MODE`
- `objective=opt-in:SIMARD_BOOTSTRAP_MODE`
- `state_root=opt-in:SIMARD_BOOTSTRAP_MODE`
- `base_type=opt-in:SIMARD_BOOTSTRAP_MODE`
- `topology=opt-in:SIMARD_BOOTSTRAP_MODE`
- the builtin identity `simard-engineer`

**Checkpoint**: defaults are a startup choice, not a recovery path. This part of the audited contract already exists.

## Step 7: Observe stopped-state behavior

The runtime preserves its snapshot after shutdown and surfaces a dedicated stopped-state error:

```rust
use simard::{RuntimeState, SimardError};

runtime.stop()?;

let snapshot = runtime.snapshot()?;
assert_eq!(snapshot.runtime_state, RuntimeState::Stopped);

let error = runtime.run("should fail after stop").unwrap_err();
assert_eq!(
    error,
    SimardError::RuntimeStopped {
        action: "run".to_string(),
    }
);
```

**Checkpoint**: stop is an observable lifecycle boundary. Snapshot inspection still works, but execution does not resume.

After shutdown, the reflected manifest freshness becomes `Stale` so callers can tell they are looking at post-stop metadata instead of a live runtime.

## Step 7: Inspect truthful reflection metadata

After a successful run, reflection reports the assembled contract and backend descriptors:

```rust
use simard::{FreshnessState, ReflectiveRuntime};

let snapshot = runtime.snapshot()?;

assert_eq!(
    snapshot.manifest_contract.entrypoint,
    "simard::bootstrap::assemble_local_runtime"
);
assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
assert_eq!(snapshot.manifest_contract.freshness.state, FreshnessState::Current);
assert_eq!(snapshot.runtime_node.to_string(), "node-local");
assert_eq!(snapshot.mailbox_address.to_string(), "inmemory://node-local");
assert_eq!(snapshot.active_goal_count, 0);
assert_eq!(snapshot.agent_program_backend.identity, "agent-program::objective-relay");
assert_eq!(snapshot.handoff_backend.identity, "handoff::json-file-store");
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
assert_eq!(snapshot.transport_backend.identity, "transport::in-memory-mailbox");
assert_eq!(snapshot.goal_backend.identity, "goals::json-file-store");
assert_eq!(snapshot.memory_backend.identity, "memory::json-file-store");
assert_eq!(snapshot.evidence_backend.identity, "evidence::json-file-store");
```

If you launched with `SIMARD_BASE_TYPE="copilot-sdk"`, `snapshot.selected_base_type` still shows the explicit selection while `snapshot.adapter_backend.identity` remains `local-harness`. If you launched with `SIMARD_BASE_TYPE="rusty-clawd"`, reflection now reports `rusty-clawd::session-backend`. The runtime-side wiring is explicit too: single-process runs report `node-local` / `inmemory://node-local`, while loopback multi-process runs report `node-loopback-mesh` / `loopback://node-loopback-mesh`. Composite identities also expose `snapshot.identity_components`.

## Summary

You now know:

- how to run the local runtime with explicit config
- how to switch between built-in base types without hidden inference
- how `copilot-sdk` still aliases `local-harness` while `rusty-clawd` now reports a distinct backend honestly
- how `simard-meeting` uses a facilitator program to persist concise decision records instead of acting like an engineer session
- how `simard-goal-curator` persists durable top-goal state that later engineer runs can read back
- how composite identities surface their assembled components explicitly
- how loopback multi-process execution reuses the same runtime contracts
- how opt-in defaults are recorded
- how the bootstrap path persists durable local state under the configured state root
- how reflection reports truthful runtime metadata
- how stop semantics behave after shutdown

## Next steps

- Use the [bootstrap and reflection how-to](../howto/configure-bootstrap-and-inspect-reflection.md) to inspect the reflection surface in more detail.
- Use the [runtime contracts reference](../reference/runtime-contracts.md) when you need exact API details.
- Read [truthful runtime metadata](../concepts/truthful-runtime-metadata.md) for the design rationale behind the contract.

See the [documentation index](../index.md) for the rest of the Simard docs.
