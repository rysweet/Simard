---
title: Base Type Adapters
description: Reference for the pluggable agent execution substrates that Simard delegates work to — traits, shipped adapters, capability contracts, and topology support.
last_updated: 2026-07-03
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

    // Normalized memory + knowledge enrichment (issue #1665).
    // Provided methods — default to "no enrichment configured"; adapters that
    // support enrichment override `enrichment`/`enrichment_mut`.
    fn enrichment(&self) -> Option<&EnrichmentClients> { None }
    fn enrichment_mut(&mut self) -> Option<&mut EnrichmentClients> { None }
    fn enrich_input(&self, input: &BaseTypeTurnInput) -> SimardResult<BaseTypeTurnInput> { /* shared */ }
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
| Memory enrichment | Exposed via shared `enrich_input` (not folded into shell command stream) |
| Knowledge enrichment | Exposed via shared `enrich_input` (not folded into shell command stream) |

**Configuration** (`HarnessConfig`):
- `command` — shell command to run for each turn (optional; if absent, objective text passes directly to terminal session)
- `shell` — shell override (default: `/usr/bin/bash`, falling back to `$SHELL`/`/bin/bash`/`/bin/sh` when that path is absent)
- `working_directory` — working directory for command execution

> **Enrichment note (#1665):** The harness exposes the normalized
> `BaseTypeSession::enrich_input` entry point and stores `EnrichmentClients`,
> so it participates in the shared enrichment contract. Because it executes
> *literal shell commands* rather than natural-language LLM prompts, it does
> not fold memory/knowledge markdown into its command stream.

**Failure diagnostics:** when the PTY shell exits non-zero — or a `wait-for` checkpoint fails because the command was missing — the adapter returns an actionable error rather than a bare status. Exit code `127` is reported as a command that could not be resolved on `PATH` (use an absolute path or install the command), `126` as a found-but-not-executable command, and the shell's own diagnostic line (e.g. `bash: say: command not found`) is included so the offending command is named. The child PTY is launched with a usable `PATH` (falling back to the standard system bin directories when the inherited environment has none).

### `rusty-clawd` — `RustyClawdAdapter`

**Module:** `src/base_type_rustyclawd/` (`RustyClawdAdapter` in `adapter.rs`)

The RustyClawd session backend. Supports both single-process and multi-process topologies via the loopback mesh driver. Produces structured plan/execution/evidence outcomes.

| Property | Value |
|----------|-------|
| Capabilities | PromptAssets, SessionLifecycle, Memory, Evidence, Reflection |
| Topologies | SingleProcess, MultiProcess |
| Memory enrichment | Yes — automatic per-turn via shared `enrich_input` (#1665) |
| Knowledge enrichment | Yes — automatic per-turn via shared `enrich_input` (#1665) |

> **Enrichment note (#1665, #2383):** Each `run_turn` routes the input through
> the shared `BaseTypeSession::enrich_input` entry point before dispatching to
> the RustyClawd client, so recalled memory facts/procedures and domain
> knowledge are injected into the turn's system prompt (the objective stays the
> bare user message, keeping the conversation history clean). Prior to #1665
> only the Copilot adapter enriched its turns. The entry point alone is not
> enough, though: the session's `EnrichmentClients` must also be **populated**.
> Until #2383, `SessionBuilder`'s RustyClawd arm built sessions with empty
> clients, so production enrichment was a permanent no-op. The production path
> now opts in via `RustyClawdAdapter::with_enrichment(default_state_root())`
> (mirroring the Copilot wiring from #1664), so live RustyClawd turns recall
> real memory + knowledge. See [Production wiring](#production-wiring-1664-2383).

**Bash-tool idle-liveness (#2607):** The RustyClawd Bash tool has no wall-clock
cap — a long-but-productive command runs unbounded as long as it keeps producing
output. Only a command silent for the whole idle window (default 120 s,
configurable via `SIMARD_RUSTYCLAWD_IDLE_LIVENESS_SECS`; `0` = fully unbounded)
is reaped, killing the whole process group so no orphan survives. See the
reference:
[RustyClawd Bash-tool idle-liveness](./rustyclawd-bash-tool-idle-liveness.md).

### `copilot-sdk` — `CopilotSdkAdapter`

**Module:** `src/base_type_copilot/` (`CopilotSdkAdapter` in `mod.rs`)

Drives `amplihack copilot` through the PTY infrastructure with memory and knowledge context injection. Each turn:

1. Routes the input through the shared `BaseTypeSession::enrich_input` entry point, which gathers relevant memory facts (up to 10, confidence ≥ 0.3) and procedures (up to 5) from `CognitiveMemoryClient`
2. Queries `KnowledgeClient` for domain knowledge relevant to the objective
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

**Meeting mode:** When the session request has `mode == OperatingMode::Meeting`,
the adapter bypasses the PTY path entirely. Instead, it invokes the `copilot`
binary directly via `std::process::Command` with `--no-custom-instructions
--silent --allow-all-tools --session-id UUID`. This prevents the amplihack
custom instructions (dev-orchestrator, auto-intent-router) from treating
meeting prompts as engineering tasks. See
[Copilot meeting mode](./copilot-meeting-mode.md) for details.

## Turn Context Enrichment

The `base_type_turn` module provides shared turn preparation for adapters that need memory and knowledge enrichment:

```
Objective → prepare_turn_context() → TurnContext → format_turn_input() → enriched prompt
                                                                             ↓
Raw LLM output ← terminal PTY ← enriched prompt
     ↓
parse_turn_output() → TurnOutput { actions, explanation, confidence }
```

### Normalized enrichment entry point (issue #1665)

Before #1665, only `CopilotSdkAdapter` called `prepare_turn_context`, so every
other shipped adapter ran with empty memory/knowledge context even when clients
were configured. Enrichment is now centralized on **one shared call site** so it
cannot silently diverge again:

- `EnrichmentClients` — a bundle of the optional `memory` (`CognitiveMemoryOps`)
  and `knowledge` (`KnowledgeClient`) clients. Each session that supports
  enrichment stores one and exposes it through `BaseTypeSession::enrichment()` /
  `enrichment_mut()`.
- `enrich_turn_input(input, memory, knowledge)` — recalls memory facts/
  procedures and domain knowledge for the input's `objective` via
  `prepare_turn_context`, renders them with `render_enrichment_block`, and
  returns a new `BaseTypeTurnInput` with that block injected into
  `prompt_preamble` (the `objective` and `identity_context` are preserved
  unchanged, so stateful adapters keep a clean conversation history).
- `BaseTypeSession::enrich_input(&self, input)` — the provided trait method
  every adapter inherits. It delegates to the session's configured
  `EnrichmentClients` (or to a no-op enrichment when none are configured).

| Adapter | `enrich_input` exposed | Applied in `run_turn` |
|---------|------------------------|-----------------------|
| `copilot-sdk` | Yes | Yes (PTY and meeting paths) |
| `rusty-clawd` | Yes | Yes (injected into the system prompt; production clients wired by #2383) |
| `terminal-shell` (harness) | Yes | No — runs literal shell commands |
| `claude-agent-sdk` / `ms-agent-framework` | Yes | No — `run_turn` is unimplemented |

**Honest degradation:** If a configured client call fails during enrichment, the
error propagates rather than silently degrading (PHILOSOPHY.md). A `None` client
is not a failure — it simply yields an unenriched, objective-only prompt.

### Production wiring (#1664, #2383)

Exposing `enrich_input` (above) is necessary but **not sufficient** for
production enrichment: a session only recalls memory + knowledge when its
`EnrichmentClients` are actually populated. Adapters that support production
enrichment provide a `with_enrichment(state_root)` builder that, on
`open_session`, launches the native cognitive-memory + knowledge clients (with
graceful degradation) via the shared `EnrichmentSource` policy in
`base_type_turn`:

- `EnrichmentSource::Disabled` (default) — empty clients, no filesystem side
  effects. This keeps lightweight callers and unit tests cheap.
- `EnrichmentSource::Native { state_root }` — launches real clients through the
  single shared `launch_enrichment_clients` helper (one launcher, no per-adapter
  duplication). A launch failure logs and degrades that client to `None`.

`SessionBuilder` opts both production adapters in by reading the default state
root (shared with the OODA daemon when running):

| Adapter | Production builder | Wired in `SessionBuilder` |
|---------|--------------------|---------------------------|
| `copilot-sdk` | `CopilotSdkAdapter::with_enrichment` | Yes — `with_enrichment(default_state_root())` (#1664) |
| `rusty-clawd` | `RustyClawdAdapter::with_enrichment` | Yes — `with_enrichment(default_state_root())` (#2383) |

Before #2383, RustyClawd had no `with_enrichment` builder and its
`SessionBuilder` arm injected no clients, so every production RustyClawd turn
recalled nothing despite the #1665 entry point being wired through `run_turn`.

## Bootstrap Wiring

Bootstrap registers all adapters via `register_builtin_base_type` in `bootstrap.rs`:

- `local-harness` → `TestAdapter` (lightweight canned-result adapter for tests)
- `terminal-shell` → `RealLocalHarnessAdapter` (PTY-backed shell execution)
- `rusty-clawd` → `RustyClawdAdapter` (rustyclawd-core SDK with process delegation)
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

Unsupported combinations fail with typed errors — never silent degradation.

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
