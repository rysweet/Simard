---
title: Runtime contracts reference
description: Reference for the current Simard executable surfaces, the in-process runtime contract, and the planned unified CLI shape.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ./simard-cli.md
  - ../howto/carry-meeting-decisions-into-engineer-sessions.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../concepts/truthful-runtime-metadata.md
---

# Runtime contracts reference

## Status

The in-process Rust runtime described here exists today.

The fully unified `simard` CLI described in the product architecture does **not** exist as a complete executable surface yet.

Today:

- `simard` is a bootstrap-from-environment entrypoint
- `simard_operator_probe` exposes the current operator-mode compatibility commands
- `simard-gym` exposes the current benchmark CLI

The mode contracts below are still real today because the probe and gym binaries exercise the same runtime code paths that the unified CLI will eventually dispatch.

## Public surfaces

Simard exposes three public surface classes:

- current executables:
  - `simard`
  - `simard_operator_probe`
  - `simard-gym`
- the planned unified operator CLI rooted at `simard`
- the in-process Rust runtime and bootstrap types in `src/bootstrap.rs`, `src/runtime.rs`, and related modules

Simard does **not** expose:

- an HTTP API
- a network service contract
- a database schema contract

## Current executable mappings

| Runtime behavior | Current executable surface | Planned unified surface |
| --- | --- | --- |
| bootstrap-configured local session | `simard` with `SIMARD_*` environment variables | `simard bootstrap run ...` |
| bounded engineer loop | `simard_operator_probe engineer-loop-run ...` | `simard engineer run ...` |
| terminal-backed engineer substrate | `simard_operator_probe terminal-run ...` | `simard engineer terminal ...` |
| meeting mode | `simard_operator_probe meeting-run ...` | `simard meeting run ...` |
| goal-curation mode | `simard_operator_probe goal-curation-run ...` | `simard goal-curation run ...` |
| improvement-curation mode | `simard_operator_probe improvement-curation-run ...` | `simard improvement-curation run ...` |
| review artifact persistence and readback | `simard_operator_probe review-run ...` and `review-read ...` | `simard review ...` |
| benchmark scenarios and suites | `simard-gym ...` | `simard gym ...` |

## [PLANNED] Canonical CLI surface

The canonical operator-facing command tree Simard is being built toward is:

- `simard engineer run <topology> <workspace-root> <objective> [state-root]`
- `simard engineer terminal <topology> <structured-objective>`
- `simard meeting run <base-type> <topology> <structured-objective> [state-root]`
- `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`
- `simard gym list`
- `simard gym run <scenario-id>`
- `simard gym run-suite <suite-id>`
- `simard review run <base-type> <topology> <objective> [state-root]`
- `simard review read <base-type> <topology> [state-root]`
- `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

Use the [Simard CLI reference](./simard-cli.md) for the exact command tree, examples, and current runnable mappings.

## Mode contracts

### Engineer mode

The bounded engineer contract already exists in the runtime today.

- current operator entrypoint: `simard_operator_probe engineer-loop-run <topology> <workspace-root> <objective> [state-root]`
- planned unified entrypoint: `simard engineer run <topology> <workspace-root> <objective> [state-root]`

The bounded engineer loop is intentionally narrow:

- it inspects the selected repo before acting
- it prints a short action plan and explicit verification steps
- it chooses one bounded local action
- it verifies the result explicitly
- it persists concise evidence and memory under the selected state root
- it surfaces active goals and up to the three most recent carried meeting records from the same state root

The current bounded engineer loop supports two honest action shapes:

- a read-only repo-native scan such as `cargo-metadata-scan` or `git-tracked-file-scan`
- one explicit structured text replacement on a clean repo when the objective includes all of:
  - `edit-file: <repo-relative path>`
  - `replace: <existing text>`
  - `with: <replacement text>`
  - `verify-contains: <required post-edit text>`

That structured edit path is intentionally narrow:

- the target path must stay inside the selected repo
- the repo must start clean so Simard does not overwrite unrelated user changes
- only one expected changed file is allowed
- verification must confirm both file content and git-visible change state

### Meeting mode

The meeting-mode contract already exists in the runtime today.

- current operator entrypoint: `simard_operator_probe meeting-run <base-type> <topology> <structured-objective> [state-root]`
- planned unified entrypoint: `simard meeting run <base-type> <topology> <structured-objective> [state-root]`

Meeting mode uses the facilitator agent program backend `agent-program::meeting-facilitator`.

Its contract is intentionally narrow:

- it persists a concise meeting record under the durable state root when the structured objective contains persistable outputs such as `update:`, `decision:`, `risk:`, `next-step:`, `open-question:`, or structured `goal:` lines
- later engineer runs against the same state root surface those carried meeting decisions explicitly, separate from the active top-goal list
- it can also persist structured goal updates into the durable goal register
- it does not mutate code paths or pretend implementation work happened

### Goal-curation mode

The goal-curation contract already exists in the runtime today.

- current operator entrypoint: `simard_operator_probe goal-curation-run <base-type> <topology> <structured-objective> [state-root]`
- planned unified entrypoint: `simard goal-curation run <base-type> <topology> <structured-objective> [state-root]`

The goal-curation path is intentionally narrow:

- it uses the dedicated `simard-goal-curator` identity and the `agent-program::goal-curator` backend
- it persists durable goal records under the selected state root
- reflection and later operator runs expose the active top 5 goals directly
- it does not claim implementation work or remote orchestration

### Improvement-curation mode

The improvement-curation contract already exists in the runtime today.

- current operator entrypoint: `simard_operator_probe improvement-curation-run <base-type> <topology> <structured-objective> [state-root]`
- planned unified entrypoint: `simard improvement-curation run <base-type> <topology> <structured-objective> [state-root]`

The improvement-curation path is intentionally narrow:

- it uses the dedicated `simard-improvement-curator` identity and the `agent-program::improvement-curator` backend
- it reads the latest persisted review artifact from the selected durable state root and turns explicit operator approvals into durable active or proposed priorities
- it preserves deferred proposals as durable decision memory rather than silently discarding them
- reflection and later operator runs expose both active and proposed improvement priorities directly
- it does not mutate code or silently promote unreviewed changes

### Review mode

The review contract already exists in the runtime today.

- current operator entrypoint: `simard_operator_probe review-run <base-type> <topology> <objective> [state-root]` and `simard_operator_probe review-read <base-type> <topology> [state-root]`
- planned unified entrypoint: `simard review run ...` and `simard review read ...`

The review path is intentionally narrow:

- it runs an ordinary bounded session first, then inspects the exported handoff offline
- it persists a concise JSON review artifact under `SIMARD_STATE_ROOT/review-artifacts/`
- it persists a concise decision-scoped review record so later sessions can reuse approved findings
- it emits concrete proposals tied to persisted evidence instead of silently changing prompts or policies
- it can read the latest persisted review artifact back in a later operator process through the current `review-read` entrypoint and the planned `simard review read` entrypoint

### Gym mode

The gym contract already exists in the runtime today.

- current operator entrypoint: `simard-gym list`, `simard-gym run <scenario-id>`, and `simard-gym run-suite <suite-id>`
- planned unified entrypoint: `simard gym list`, `simard gym run <scenario-id>`, and `simard gym run-suite <suite-id>`

The shipped benchmark surface supports:

- listing the shipped scenarios
- running one benchmark scenario
- running the `starter` suite

The starter suite is intentionally small and exercises:

- `local-harness`
- `terminal-shell`
- `copilot-sdk`
- `rusty-clawd`
- the dedicated `simard-gym` identity
- the composite `simard-composite-engineer` identity

Artifacts are written under `target/simard-gym/` as JSON and text reports plus a `review.json` artifact for each scenario run.

### Bootstrap mode

The bootstrap contract already exists in the runtime today, but its current entrypoint differs from the planned CLI shape.

- current operator entrypoint: `simard` with `SIMARD_*` environment variables
- current compatibility helper: `simard_operator_probe bootstrap-run <identity> <base-type> <topology> <objective> [state-root]`
- planned unified entrypoint: `simard bootstrap run <identity> <base-type> <topology> <objective> [state-root]`

The bootstrap utility exposes explicit runtime assembly rather than hidden defaults:

- unsupported or unregistered base types fail explicitly
- unsupported topology and base-type pairs fail explicitly
- builtin defaults are only used through explicit opt-in startup mode

## Configuration

### Environment variables

| Variable | Required for current `simard` entrypoint | Default | Description |
| --- | --- | --- | --- |
| `SIMARD_PROMPT_ROOT` | Yes in `explicit-config` bootstrap flows | none | Root directory for prompt assets. |
| `SIMARD_OBJECTIVE` | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults`; otherwise yes | none in `explicit-config`; `bootstrap the Simard engineer loop` in `builtin-defaults` | Objective passed to bootstrapped runs. Live execution keeps the real objective in memory while persisted scratch, summary, reflection, and exported handoff session text store objective metadata instead of the raw objective text. |
| `SIMARD_STATE_ROOT` | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults` | none in `explicit-config`; `target/simard-state` in `builtin-defaults` | Root directory for the durable local memory, goals, evidence, and latest handoff snapshot files written by the bootstrap path. |
| `SIMARD_BOOTSTRAP_MODE` | No | `explicit-config` | Startup mode. Accepted values: `explicit-config`, `builtin-defaults`. |
| `SIMARD_IDENTITY` | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults`; otherwise yes | none in `explicit-config`; `simard-engineer` in `builtin-defaults` | Identity to load before runtime composition. |
| `SIMARD_BASE_TYPE` | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults`; otherwise yes | none in `explicit-config`; `local-harness` in `builtin-defaults` | Base type selected for the runtime request. Unsupported or unregistered choices fail explicitly. |
| `SIMARD_RUNTIME_TOPOLOGY` | No when `SIMARD_BOOTSTRAP_MODE=builtin-defaults`; otherwise yes | none in `explicit-config`; `single-process` in `builtin-defaults` | Runtime topology selected for the runtime request. Accepted values: `single-process`, `multi-process`, `distributed`. |

### Operator carryover configuration

Meeting-to-engineer handoff currently uses the explicit trailing `state-root` argument on the probe commands:

- `simard_operator_probe meeting-run ... [state-root]`
- `simard_operator_probe engineer-loop-run ... [state-root]`

The planned unified CLI preserves the same carryover contract:

- `simard meeting run ... [state-root]`
- `simard engineer run ... [state-root]`

Passing the same explicit directory to both commands is the supported way to make meeting decisions visible in later engineer runs.

The carryover surface is intentionally bounded:

- engineer mode reports at most the three most recent persisted meeting records from that shared state root
- if you omit the shared state root, the commands still run, but there is no guaranteed cross-session carryover contract

### Current builtin base-type registrations

The builtin identities currently advertised by the loader are `simard-engineer`, `simard-meeting`, `simard-goal-curator`, `simard-improvement-curator`, `simard-gym`, and the composite `simard-composite-engineer`. All of them accept `local-harness`, `rusty-clawd`, and `copilot-sdk`; `simard-engineer` additionally accepts `terminal-shell` for the local terminal-backed path:

| Base type selection | Current session backend implementation | Supported topologies in this scaffold |
| --- | --- | --- |
| `local-harness` | `local-harness` single-process local process harness session backend | `single-process` |
| `terminal-shell` | `terminal-shell::local-pty` real local PTY-backed shell session backend (`simard-engineer` only) | `single-process` |
| `rusty-clawd` | `rusty-clawd::session-backend` real session backend | `single-process`, `multi-process` |
| `copilot-sdk` | `local-harness` single-process local process harness session backend (alias) | `single-process` |

Notes:

- bootstrap registers base-type factories from the manifest-advertised base-type list instead of assuming a single hardcoded local backend
- unsupported topology and base-type pairs still fail explicitly; for example, `local-harness + multi-process` returns `UnsupportedTopology`
- descriptors remain truthful: `selected_base_type` preserves the explicit choice, while `adapter_backend.identity` exposes the actual backend
- `MemoryPolicy.allow_project_writes=true` is rejected explicitly in v1 rather than being ignored

## Persisted session text

Simard keeps the live objective available while the run is executing, but persisted session text is redacted down to objective metadata.

- session scratch records store `objective-metadata(chars=..., words=..., lines=...)`
- reflection summaries describe completion with objective metadata instead of raw objective text
- persisted session summaries reuse sanitized plan and execution strings rather than copying the raw objective back out
- exported handoff snapshots preserve the session boundary while replacing `RuntimeHandoffSnapshot.session.objective` with the same objective metadata string
- bootstrap persists the latest exported handoff snapshot under `SIMARD_STATE_ROOT/latest_handoff.json`
- bootstrap persists durable goal state under `SIMARD_STATE_ROOT/goal_records.json`
- runtime reflection reports both `active_goal_count` / `active_goals` and `proposed_goal_count` / `proposed_goals`

## Identity metadata

### `ManifestContract`

```rust
pub struct ManifestContract {
    pub entrypoint: String,
    pub composition: String,
    pub precedence: Vec<String>,
    pub provenance: Provenance,
    pub freshness: Freshness,
}
```

Notes:

- the current CLI bootstrap path uses `simard::bootstrap::assemble_local_runtime` as the entrypoint
- provenance and freshness stay inside the contract so reflection carries a single source of truth
- invalid empty fields fail with `SimardError::InvalidManifestContract`

### `Provenance`

```rust
pub struct Provenance {
    pub source: String,
    pub locator: String,
}
```

Helpers currently provided:

| Helper | Meaning |
| --- | --- |
| `Provenance::new(source, locator)` | Build an explicit provenance value. |
| `Provenance::builtin(locator)` | Mark a builtin metadata source. |
| `Provenance::injected(locator)` | Mark an injected metadata source. |
| `Provenance::runtime(locator)` | Mark runtime-derived metadata. |

### `Freshness`

```rust
pub enum FreshnessState {
    Current,
    Stale,
}

pub struct Freshness {
    pub state: FreshnessState,
    pub observed_at_unix_ms: u64,
}
```

Notes:

- `Freshness::now()` returns `SimardResult<Freshness>` and fails explicitly if the system clock is before the Unix epoch
- freshness can explicitly represent stale metadata when a caller needs to surface it

## Session identity

### `SessionId`

```rust
pub struct SessionId(String);
```

Rules:

- `UuidSessionIdGenerator` emits `session-<uuid-v7>`
- `SessionId::parse(...)` accepts a bare UUID or a `session-<uuid>` value and canonicalizes to `session-<uuid>`
- invalid values fail with `SimardError::InvalidSessionId`
- custom `SessionIdGenerator` implementations must return valid `SessionId` values
- the session ID strategy is injected through `RuntimePorts`; the local bootstrap path opts into `UuidSessionIdGenerator` explicitly

## Runtime lifecycle

### `RuntimeState`

| State | Meaning |
| --- | --- |
| `Initializing` | Runtime has been composed but not started. |
| `Ready` | Prompt assets are loaded and the runtime can execute. |
| `Active` | Base-type session work is in progress. |
| `Reflecting` | Reflection metadata is being assembled. |
| `Persisting` | Memory and evidence summaries are being persisted. |
| `Failed` | Execution failed before completion. |
| `Stopping` | Shutdown is in progress. |
| `Stopped` | Runtime has been shut down and cannot accept more work. |

### Stopped-state behavior

After `stop()` succeeds:

- `snapshot()` remains valid
- `run()` fails with `RuntimeStopped { action: "run" }`
- `start()` fails with `RuntimeStopped { action: "start" }`
- a repeated `stop()` fails with `RuntimeStopped { action: "stop" }`
- callers must compose a new runtime instead of reusing the stopped one

### Failed-state behavior

After `run()` fails:

- `snapshot()` remains valid
- the last session remains visible with `session_phase == Some(Failed)`
- manifest freshness is reported as `Stale`
- `start()` fails with `RuntimeFailed`
- `run()` fails with `RuntimeFailed`
- `stop()` is still the explicit way to close the lifecycle boundary

## Reflection snapshot

### `ReflectionSnapshot`

```rust
pub struct ReflectionSnapshot {
    pub identity_name: String,
    pub identity_components: Vec<String>,
    pub selected_base_type: BaseTypeId,
    pub topology: RuntimeTopology,
    pub runtime_state: RuntimeState,
    pub runtime_node: RuntimeNodeId,
    pub mailbox_address: RuntimeAddress,
    pub session_phase: Option<SessionPhase>,
    pub prompt_assets: Vec<PromptAssetId>,
    pub manifest_contract: ManifestContract,
    pub evidence_records: usize,
    pub memory_records: usize,
    pub active_goal_count: usize,
    pub active_goals: Vec<String>,
    pub proposed_goal_count: usize,
    pub proposed_goals: Vec<String>,
    pub agent_program_backend: BackendDescriptor,
    pub handoff_backend: BackendDescriptor,
    pub adapter_backend: BackendDescriptor,
    pub adapter_capabilities: Vec<String>,
    pub adapter_supported_topologies: Vec<String>,
    pub topology_backend: BackendDescriptor,
    pub transport_backend: BackendDescriptor,
    pub supervisor_backend: BackendDescriptor,
    pub memory_backend: BackendDescriptor,
    pub evidence_backend: BackendDescriptor,
    pub goal_backend: BackendDescriptor,
}
```

For `simard-meeting`, reflection reports `agent_program_backend.identity == "agent-program::meeting-facilitator"`. For `simard-goal-curator`, it reports `agent_program_backend.identity == "agent-program::goal-curator"`. For `simard-improvement-curator`, it reports `agent_program_backend.identity == "agent-program::improvement-curator"`.

### `BackendDescriptor`

```rust
pub struct BackendDescriptor {
    pub identity: String,
    pub provenance: Provenance,
    pub freshness: Freshness,
}
```

Reflection rules:

- `manifest_contract` carries entrypoint, provenance, and freshness together
- `agent_program_backend` comes from the injected agent-program contract, not from hardcoded runtime logic
- `handoff_backend` comes from the injected handoff store used for export/import
- `adapter_backend` comes from the selected base-type factory or session descriptor, not from a bootstrap shortcut
- `runtime_node` and `mailbox_address` come from the injected topology and transport services
- `topology_backend`, `transport_backend`, and `supervisor_backend` come from the live runtime services
- `memory_backend` comes from the live memory store descriptor
- `evidence_backend` comes from the live evidence store descriptor
- `goal_backend` comes from the live goal store descriptor
- `active_goal_count` and `active_goals` expose the active top-goal state derived from the durable goal store
- `proposed_goal_count` and `proposed_goals` expose durable proposed priorities, including approved improvement proposals that have not been activated yet
- reflection reports live wiring, not placeholder labels
- stopped or failed snapshots mark manifest freshness as `Stale`

## Errors

### Configuration and identity errors

| Error | Meaning |
| --- | --- |
| `MissingRequiredConfig` | Required startup configuration is absent. |
| `NonUnicodeConfigValue` | A configuration value exists but cannot be decoded as UTF-8. |
| `InvalidConfigValue` | A configuration value is present but invalid. |
| `UnknownIdentity` | Requested identity is not registered. |
| `InvalidIdentityComposition` | A composite identity definition is internally inconsistent. |
| `InvalidManifestContract` | Manifest contract metadata is incomplete or untruthful. |
| `InvalidGoalRecord` | A structured goal update is malformed or incomplete. |
| `ClockBeforeUnixEpoch` | Runtime metadata could not record a truthful observation time. |
| `InvalidSessionId` | A supplied session ID is not a valid distributed-safe identifier. |
| `UnsupportedBaseType` | The chosen identity does not allow the requested base type. |
| `AdapterNotRegistered` | No base-type factory has been registered for the requested base type. |
| `MissingCapability` | The selected base-type backend exists but does not satisfy the manifest's required capabilities. |
| `UnsupportedRuntimeTopology` | The injected topology driver does not support the requested topology. |
| `UnsupportedTopology` | The selected base-type backend does not support the requested topology. |

### Lifecycle and session errors

| Error | Meaning |
| --- | --- |
| `InvalidRuntimeTransition` | A runtime lifecycle transition is invalid. |
| `InvalidBaseTypeSessionState` | A base-type session was opened, used, or closed out of order. |
| `AdapterInvocationFailed` | The selected base-type backend failed while executing a turn. |
| `BaseTypeSessionCleanupFailed` | A base-type session failed during execution and then also failed to close cleanly. |
| `InvalidHandoffSnapshot` | A handoff snapshot could not be restored into the requested runtime. |
| `RuntimeStopped` | Caller attempted `start`, `run`, or `stop` after shutdown was already in effect. |
| `RuntimeFailed` | Caller attempted `start` or `run` after a failed execution but before shutdown. |
| `InvalidSessionTransition` | A session phase transition is invalid. |

### Prompt and storage errors

| Error | Meaning |
| --- | --- |
| `PromptAssetMissing` | A referenced prompt asset was not found. |
| `PromptAssetRead` | A prompt asset could not be read. |
| `StoragePoisoned` | A durable store lock was poisoned before Simard could read or persist state. |

## Example: truthful fields

```rust
use simard::{FreshnessState, ReflectiveRuntime};

let snapshot = runtime.snapshot()?;

assert_eq!(snapshot.runtime_state.to_string(), "ready");
assert_eq!(
    snapshot.manifest_contract.entrypoint,
    "simard::bootstrap::assemble_local_runtime"
);
assert_eq!(snapshot.manifest_contract.provenance.source, "bootstrap");
assert_eq!(snapshot.manifest_contract.freshness.state, FreshnessState::Current);
assert_eq!(snapshot.runtime_node.to_string(), "node-local");
assert_eq!(snapshot.mailbox_address.to_string(), "inmemory://node-local");
assert_eq!(snapshot.adapter_backend.identity, "local-harness");
```

## See also

- [Simard CLI reference](./simard-cli.md)
- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [How to carry meeting decisions into engineer sessions](../howto/carry-meeting-decisions-into-engineer-sessions.md)
- [Concept: truthful runtime metadata](../concepts/truthful-runtime-metadata.md)
