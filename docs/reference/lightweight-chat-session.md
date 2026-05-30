---
title: LightweightChatSession — meeting backend API
description: API reference for LightweightChatSession, the SessionBuilder-backed BaseTypeSession used for meeting turns.
last_updated: 2026-05-25
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./meeting-backend-api.md
  - ./base-type-adapters.md
  - ../howto/start-a-meeting.md
---

# LightweightChatSession

`LightweightChatSession` is a `BaseTypeSession` implementation that delegates
each meeting conversation turn to the configured LLM provider via
`SessionBuilder`. It is used automatically when
`simard meeting` is invoked for non-copilot providers (RustyClawd, etc.).
When `SIMARD_LLM_PROVIDER=copilot`, the meeting session uses the
[copilot meeting-mode path](./copilot-meeting-mode.md) instead, which
invokes the `copilot` binary directly via `std::process::Command`.

**Location:** `src/meeting_backend/lightweight.rs`

## Motivation

Previously, `LightweightChatSession` spawned a hardcoded
`amplihack copilot --subprocess-safe` subprocess with piped stdin/stdout for each
turn. This caused hangs, truncation, and non-conversational output because the
Copilot CLI doesn't support non-interactive piped mode reliably.

The rewrite (fixes #2105, #2106) replaces the subprocess machinery with a
`SessionBuilder` + `LlmProvider::resolve()` call — the same pattern used by the
OODA brains and dashboard chat widget. Both CLI and web entry points now share
identical LLM backend behavior.

## Struct

```rust
pub struct LightweightChatSession {
    descriptor: BaseTypeDescriptor,
    inner: Option<Box<dyn BaseTypeSession>>,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
}
```

## Constructor

```rust
impl LightweightChatSession {
    pub fn new() -> SimardResult<Self>
}
```

Creates an unopened session. The inner `SessionBuilder` session is opened lazily
in `open()`.

## BaseTypeSession interface

`LightweightChatSession` implements the standard `BaseTypeSession` trait:

```rust
fn open(&mut self)  -> SimardResult<()>
fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome>
fn close(&mut self) -> SimardResult<()>
fn descriptor(&self) -> &BaseTypeDescriptor
```

### `open()`

Resolves the LLM provider via `LlmProvider::resolve()`, creates a session via
`SessionBuilder::new(OperatingMode::Meeting, provider)`, and opens it. The inner
session is stored for subsequent `run_turn()` calls.

### `run_turn(input)`

Forwards the `BaseTypeTurnInput` to the inner session's `run_turn()`. Returns
the `BaseTypeOutcome` from the inner session unchanged.

### `close()`

Closes the inner session and marks the `LightweightChatSession` as closed.
Idempotent guards prevent double-close.

## Capabilities

```rust
capability_set([
    BaseTypeCapability::PromptAssets,
    BaseTypeCapability::SessionLifecycle,
])
```

Supported topology: `RuntimeTopology::SingleProcess` only.

## Error variants

All errors are `SimardError::AdapterInvocationFailed` with `base_type =
"lightweight-chat"`. Possible reasons:

| Reason prefix | Cause |
|---------------|-------|
| `"SessionBuilder::open failed: ..."` | Provider resolution or adapter creation failed |
| `"inner session not initialized ..."` | `run_turn()` called before `open()` |

## Cost tracking

Each turn calls `crate::cost_tracking::record_cost("lightweight-chat",
"session-builder", plan_len, response_len, label)`. Failures are silently
ignored at `DEBUG` level.

## When it is used

`open_meeting_agent_session()` in `src/operator_commands_meeting/meeting_session.rs`
uses `SessionBuilder` directly for all providers. The `LightweightChatSession`
wrapper remains available for any caller that wants the simple
`new()` → `open()` → `run_turn()` → `close()` lifecycle without managing
provider resolution itself.

The dashboard chat widget (`src/operator_commands_dashboard/chat.rs`) also uses
`SessionBuilder` directly, giving both CLI and web identical backend behavior.

## Related reading

- [Meeting backend API reference](./meeting-backend-api.md) — `MeetingBackend` struct and
  higher-level meeting types.
- [Copilot meeting mode](./copilot-meeting-mode.md) — The `CopilotSdkAdapter` meeting-mode
  path that invokes `copilot` directly with `--no-custom-instructions`. This is the
  primary meeting backend when `SIMARD_LLM_PROVIDER=copilot`.
- [Base type adapters](./base-type-adapters.md) — Full `BaseTypeSession` trait contract.
- [How to start a meeting](../howto/start-a-meeting.md) — Operator-facing meeting guide.
