---
title: LightweightChatSession — meeting backend API
description: API reference for LightweightChatSession, the direct-subprocess BaseTypeSession used for Copilot-provider meeting turns.
last_updated: 2026-05-07
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./meeting-backend-api.md
  - ./base-type-adapters.md
  - ../howto/start-a-meeting.md
---

# LightweightChatSession

`LightweightChatSession` is a `BaseTypeSession` implementation that executes each
meeting conversation turn by piping a prompt directly to the `amplihack copilot`
subprocess. It is used automatically when `SIMARD_LLM_PROVIDER=copilot` (or
equivalent Copilot provider resolution) and `simard meeting` is invoked.

**Location:** `src/meeting_backend/lightweight.rs`

## Motivation

The standard `SessionBuilder` / PTY path spawns a full terminal session around each
turn. For interactive meetings this adds two problems:

1. **~50 s overhead per turn** — PTY sessions go through the amplihack startup sequence
   (hook installation, config validation, MCP server negotiation) before the first token
   is generated.
2. **Premature SIGTERM** — Long Copilot computations produce no transcript output. The
   transcript-idle timer in `PtyTerminalSession::finish()` would fire before the LLM
   responds, killing the session mid-turn.

`LightweightChatSession` bypasses both by using `std::process::Command` with piped
stdin/stdout. There is no PTY, no script wrapper, and no transcript file. The process
tree check in `has_active_work_processes` never needs to fire because `finish()` is
never called.

## Struct

```rust
pub struct LightweightChatSession {
    descriptor: BaseTypeDescriptor,
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

Creates an unopened session. Call `open()` before `run_turn()`.

## BaseTypeSession interface

`LightweightChatSession` implements the standard `BaseTypeSession` trait:

```rust
fn open(&mut self)  -> SimardResult<()>
fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome>
fn close(&mut self) -> SimardResult<()>
fn descriptor(&self) -> &BaseTypeDescriptor
```

### `open()`

Marks the session as open. No subprocess is spawned yet — the subprocess is spawned
fresh for each turn.

### `run_turn(input)`

Builds a prompt from `input.prompt_preamble`, `input.identity_context`, and
`input.objective` (joined with double newlines), then calls `execute_piped_turn()`.

Returns a `BaseTypeOutcome` with:

| Field | Value |
|-------|-------|
| `plan` | `"Lightweight chat turn N via piped subprocess."` |
| `execution_summary` | Simard's response (cleaned) |
| `evidence` | `["lightweight-chat-turn=N", "elapsed-ms=..."]` |

### `close()`

Marks the session as closed. Idempotent guards prevent double-close.

## `execute_piped_turn(prompt: &str) -> SimardResult<String>`

Private method — the core of the session.

**Subprocess command:**

```bash
amplihack copilot --subprocess-safe
```

Environment:

| Variable | Value | Purpose |
|----------|-------|---------|
| `AMPLIHACK_NONINTERACTIVE` | `1` | Suppresses interactive prompts |
| `AMPLIHACK_MAX_DEPTH` | `0` | Prevents nested amplihack sessions |

**Prompt delivery:** Written to the subprocess stdin, then stdin is dropped (EOF).

**Timeout:** 900 seconds (`TURN_TIMEOUT_SECS`). Implemented via a background thread and
`mpsc::Receiver::recv_timeout`. On timeout, an `AdapterInvocationFailed` error is
returned — the subprocess is abandoned (the OS reaps it). This matches the gym bridge
timeout bound (known working ceiling for long LLM calls).

**stderr handling:** Captured separately. Never included in the response. Logged at
`DEBUG` level with a line count. Non-zero exit codes are logged at `WARN` but do not
cause a hard error — the stdout is still returned as the response.

**Noise stripping:** The raw stdout is passed through `strip_copilot_noise()` before
being returned. This removes:

- Empty leading lines
- Copilot startup noise (`Staged N hook files`, `XPIA`, `Script started on`, `Warning:`)
- Single-character or bullet-prefix progress indicator lines (`●`)
- Usage-stats footer lines (`Total usage est:`, `API time spent:`, `Total session time:`,
  `Changes`, `Requests`, `Tokens`)

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
| `"failed to spawn copilot subprocess: ..."` | `amplihack` binary not on `PATH` |
| `"copilot subprocess failed: ..."` | `wait_with_output()` I/O error |
| `"copilot subprocess timed out after 900s"` | Subprocess did not exit within 900 s |

## Cost tracking

Each turn calls `crate::cost_tracking::record_cost("lightweight-chat",
"copilot-lightweight", prompt_len, response_len, label)`. Failures are silently
ignored at `DEBUG` level.

## When it is used

`open_meeting_agent_session()` in `src/operator_commands_meeting/meeting_session.rs`
selects `LightweightChatSession` when `LlmProvider::resolve()` returns
`LlmProvider::Copilot`. Other providers (RustyClawd, etc.) continue to use
`SessionBuilder`.

```rust
match provider {
    LlmProvider::Copilot => LightweightChatSession::new()?.open()? ...
    _                    => SessionBuilder::new(OperatingMode::Meeting, provider)...
}
```

## Related reading

- [Meeting backend API reference](./meeting-backend-api.md) — `MeetingBackend` struct and
  higher-level meeting types.
- [Base type adapters](./base-type-adapters.md) — Full `BaseTypeSession` trait contract.
- [How to start a meeting](../howto/start-a-meeting.md) — Operator-facing meeting guide.
- [Terminal session idle detection](./terminal-session-idle-detection.md) — Why the PTY
  path was unsuitable for meeting turns.
