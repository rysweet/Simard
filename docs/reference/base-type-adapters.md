---
title: Base Type Adapters
description: Reference for the pluggable agent execution substrates that Simard delegates work to — traits, shipped adapters, capability contracts, and topology support.
last_updated: 2026-03-31
owner: simard
doc_type: reference
---

# Base Type Adapters

A base type is the execution substrate that an agent identity builds on. Simard's runtime delegates actual work — running commands, calling LLMs, driving tools — to whichever base type the operator selects at bootstrap time. All adapters implement the same `BaseTypeFactory`/`BaseTypeSession` trait pair, so the runtime kernel does not know or care which backend is active.

## Trait Contract

### `BaseTypeFactory`

Creates sessions for a given base type.

```rust
pub trait BaseTypeFactory: Send + Sync {
    fn descriptor(&self) -> &BaseTypeDescriptor;
    fn open_session(&self, request: BaseTypeSessionRequest) -> SimardResult<Box<dyn BaseTypeSession>>;
}
```

### `BaseTypeSession`

A live session that executes turns.

```rust
pub trait BaseTypeSession: Send {
    fn descriptor(&self) -> &BaseTypeDescriptor;
    fn open(&mut self) -> SimardResult<()>;
    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome>;
    fn close(&mut self) -> SimardResult<()>;
}
```

### Session Lifecycle

```
Created → open() → run_turn() → ... → close()
```

- `open()` must be called exactly once before any turns
- `run_turn()` can be called multiple times while open
- `close()` ends the session; further calls are rejected
- Double-open, turn-before-open, and post-close calls return `InvalidBaseTypeSessionState`

### Capability Contract

Every adapter declares which capabilities it supports. The identity manifest requires specific capabilities, and the runtime refuses to instantiate an identity on an adapter that cannot satisfy them.

| Capability | Meaning |
|-----------|---------|
| `PromptAssets` | Can inject prompt assets into sessions |
| `SessionLifecycle` | Supports open/turn/close lifecycle |
| `Memory` | Can read/write memory during sessions |
| `Evidence` | Produces evidence records for audit |
| `Reflection` | Supports runtime reflection snapshots |
| `TerminalSession` | Drives real terminal PTY sessions |

## Shipped Adapters

### `local-harness` — `TestAdapter`

**Module:** `src/test_support.rs`

A lightweight adapter that returns canned results without spawning external processes or requiring API keys. Used as the default bootstrap base type and for integration tests.

| Property | Value |
|----------|-------|
| Capabilities | PromptAssets, SessionLifecycle, Memory, Evidence, Reflection |
| Topologies | SingleProcess |
| Memory enrichment | No |
| Knowledge enrichment | No |

### `terminal-shell` — `RealLocalHarnessAdapter`

**Module:** `src/base_type_harness.rs`

A PTY-backed shell adapter that runs a configurable local command through the terminal infrastructure. Supports all six capabilities including `TerminalSession`. Delegates turn execution to `terminal_session::execute_terminal_turn`.

| Property | Value |
|----------|-------|
| Capabilities | PromptAssets, SessionLifecycle, Memory, Evidence, Reflection, TerminalSession |
| Topologies | SingleProcess |
| Memory enrichment | No (done at caller level) |
| Knowledge enrichment | No (done at caller level) |

**Configuration** (`HarnessConfig`):
- `command` — shell command to run for each turn (optional; if absent, objective text passes directly to terminal session)
- `shell` — shell override (default: `/usr/bin/bash`)
- `working_directory` — working directory for command execution

### `rusty-clawd` — `RustyClawdAdapter`

**Module:** `src/base_type_rustyclawd.rs`

The RustyClawd session backend. Supports both single-process and multi-process topologies via the loopback mesh driver. Produces structured plan/execution/evidence outcomes.

| Property | Value |
|----------|-------|
| Capabilities | PromptAssets, SessionLifecycle, Memory, Evidence, Reflection |
| Topologies | SingleProcess, MultiProcess |
| Memory enrichment | At session level |
| Knowledge enrichment | At session level |

### `copilot-sdk` — `CopilotSdkAdapter`

**Module:** `src/base_type_copilot.rs`

Drives `amplihack copilot` through the PTY infrastructure with memory and knowledge context injection. Each turn:

1. Gathers relevant memory facts (up to 10, confidence ≥ 0.3) and procedures (up to 5) from `CognitiveMemoryBridge`
2. Queries `KnowledgeBridge` for domain knowledge relevant to the objective
3. Formats the enriched context via `base_type_turn::format_turn_input`
4. Executes through `terminal_session::execute_terminal_turn`
5. Parses structured output via `base_type_turn::parse_turn_output`

| Property | Value |
|----------|-------|
| Capabilities | PromptAssets, SessionLifecycle, Memory, Evidence, Reflection, TerminalSession |
| Topologies | SingleProcess |
| Memory enrichment | Yes — automatic per-turn injection |
| Knowledge enrichment | Yes — automatic per-turn injection |

**Configuration** (`CopilotAdapterConfig`):
- `command` — shell command to launch copilot (default: `amplihack copilot`)
- `working_directory` — working directory for the copilot session

**Security:** The command field is validated to reject shell metacharacters (`;`, `|`, `&`, `` ` ``, `$`) for defense-in-depth.

## Turn Context Enrichment

The `base_type_turn` module provides shared turn preparation for adapters that need memory and knowledge enrichment:

```
Objective → prepare_turn_context() → TurnContext → format_turn_input() → enriched prompt
                                                                             ↓
Raw LLM output ← terminal PTY ← enriched prompt
     ↓
parse_turn_output() → TurnOutput { actions, explanation, confidence }
```

**Honest degradation:** If a bridge call fails during enrichment, the failure is recorded in `TurnContext.degraded_sources` and the turn proceeds with partial context rather than failing entirely (Pillar 11).

## Bootstrap Wiring

Bootstrap registers all adapters via `register_builtin_base_type` in `bootstrap.rs`:

- `local-harness` → `TestAdapter` (lightweight canned-result adapter for tests)
- `terminal-shell` → `RealLocalHarnessAdapter` (PTY-backed shell execution)
- `rusty-clawd` → `RustyClawdAdapter` (rustyclawd-core SDK with process fallback)
- `copilot-sdk` → `CopilotSdkAdapter` (PTY + memory/knowledge enrichment)
- `claude-agent-sdk` → `PendingSdkAdapter` (structural — SDK not yet published)
- `ms-agent-framework` → `PendingSdkAdapter` (structural — integration not yet available)

## Base Type Selection at Bootstrap

The `bootstrap` module registers all adapters and the operator selects one via:

- `SIMARD_BASE_TYPE` environment variable
- CLI flag on `simard bootstrap`
- Default from the identity manifest

The runtime validates that the selected base type:
1. Is registered in the `BaseTypeRegistry`
2. Supports the requested topology
3. Satisfies all capabilities required by the identity manifest

Unsupported combinations fail with typed errors — never silent fallbacks.

## Planned Base Types

The original spec defines four agent runtime families. Two are shipped (`rusty-clawd`, `copilot-sdk`). The remaining two are planned:

| Base Type | Wraps | Status |
|-----------|-------|--------|
| `claude-agent-sdk` | Claude Agent SDK (Rust wrapper around the TypeScript/Python SDK) | Planned — structure only |
| `ms-agent-framework` | Microsoft Agent Framework (Rust wrapper) | Planned — structure only |

Each planned base type will follow the same pattern: its own `src/base_type_{name}.rs` file implementing the `BaseTypeFactory`/`BaseTypeSession` trait pair, with capabilities and topologies declared honestly in the descriptor.

## Adding a New Base Type

1. Create `src/base_type_{name}.rs` with a struct implementing `BaseTypeFactory`
2. Implement `BaseTypeSession` for the session type
3. Declare capabilities and supported topologies honestly
4. Register in `bootstrap::register_builtin_base_type` with a constant for the base type ID
5. Add the base type ID to identity manifests that should support it
6. Add tests for lifecycle, topology rejection, and turn execution
