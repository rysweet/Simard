---
title: Copilot Meeting Mode
description: How CopilotSdkAdapter handles meeting sessions — direct subprocess invocation, --no-custom-instructions, session-id continuity, and the PTY bypass.
last_updated: 2026-05-30
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./base-type-adapters.md
  - ./meeting-backend-api.md
  - ./lightweight-chat-session.md
  - ../howto/start-a-meeting.md
  - ../architecture/unified-meeting-backend.md
---

# Copilot Meeting Mode

When `CopilotSdkSession` receives a session request with
`mode == OperatingMode::Meeting`, it switches from the default PTY-based
execution path to a direct subprocess invocation of the `copilot` binary.
This eliminates the OODA/dev-orchestrator custom instructions that cause
non-conversational responses in meeting contexts.

**Module:** `src/base_type_copilot/mod.rs`

## Problem

The default copilot execution path (`amplihack copilot --subprocess-safe`)
loads amplihack custom instructions — including the dev-orchestrator,
auto-intent-router, and workflow enforcement hooks. These instructions cause
the copilot to classify meeting prompts as engineering tasks and respond with
OODA action plans, workflow invocations, and structured analysis instead of
natural conversation.

Additionally, the PTY-based execution spawns a new `script`-wrapped process
per turn with `; exit` appended, adding ~50 seconds of startup overhead and
requiring transcript scraping to extract the response.

## Solution

Meeting mode bypasses both problems:

1. **Invokes `copilot` directly** — not `amplihack copilot` — so no custom
   instructions are loaded.
2. **Passes `--no-custom-instructions`** — defense-in-depth flag that
   explicitly disables any custom instruction loading.
3. **Uses `--silent`** — suppresses TUI chrome, giving clean text output on
   stdout.
4. **Uses `--session-id UUID`** — maintains copilot-side conversation state
   across turns without keeping a persistent process alive.
5. **Spawns via `std::process::Command`** — no PTY, no `script` wrapper, no
   transcript parsing.

## Architecture

```
Meeting turn flow (meeting mode):
┌──────────────┐     ┌───────────────────┐     ┌──────────────┐
│ MeetingBackend│────▶│ CopilotSdkSession │────▶│  copilot CLI │
│ .send_message │     │ .run_meeting_turn │     │  subprocess  │
└──────────────┘     └───────────────────┘     └──────────────┘
                           │                         │
                           │  std::process::Command  │
                           │  --no-custom-instructions│
                           │  --silent               │
                           │  --allow-all-tools      │
                           │  --session-id UUID      │
                           │  -p <prompt_content>    │
                           └─────────────────────────┘

Non-meeting turn flow (unchanged):
┌──────────────┐     ┌───────────────────┐     ┌──────────────┐
│  Caller      │────▶│ CopilotSdkSession │────▶│ PTY session  │
│              │     │ .run_pty_turn     │────▶│ script(1)    │
└──────────────┘     └───────────────────┘     └──────────────┘
```

The dispatch happens in `run_turn()`:

```rust
fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
    if self.is_meeting_mode() {
        self.run_meeting_turn(input)
    } else {
        self.run_pty_turn(input)
    }
}
```

## Session UUID lifecycle

A UUID v4 is generated once per session and reused across all turns:

| Event | UUID state |
|-------|-----------|
| `open_session()` | `session_uuid: None` |
| `open()` (meeting mode) | `session_uuid: Some(Uuid::new_v4().to_string())` |
| `open()` (non-meeting: Engineer, Curator, Improvement, Gym, Orchestrator) | `session_uuid: None` (unchanged) — these modes use the PTY path |
| `run_meeting_turn()` | Passes `--session-id {uuid}` to each invocation |
| `close()` | `session_uuid` set to `None` |

The UUID is a standard v4 random UUID from the `uuid` crate. Its alphabet
is restricted to `[a-f0-9-]`, eliminating any injection surface when passed
as a CLI argument.

## Command construction

Each meeting turn builds a `std::process::Command` with discrete `.arg()` calls
— no shell is involved:

```rust
Command::new("copilot")
    .arg("--no-custom-instructions")
    .arg("--allow-all-tools")
    .arg("--silent")
    .arg("-p").arg(&prompt_content)   // read from tempfile in Rust
    .arg("--session-id").arg(&uuid)
    .output()?
```

The enriched prompt is written to a `NamedTempFile` (reusing
`write_prompt_to_tempfile`) and then **read back into a `String` in Rust**
before being passed as a `Command::arg()`. The tempfile is an implementation
convenience — it reuses the existing helper and keeps the prompt available for
debugging — but the content travels to `copilot` via `execve` args, not via
shell expansion.

| Flag | Purpose |
|------|---------|
| `--no-custom-instructions` | Prevents amplihack custom instructions (dev-orchestrator, auto-intent-router) |
| `--allow-all-tools` | Required by copilot for non-interactive invocations |
| `--silent` | Suppresses TUI chrome; clean text output on stdout |
| `-p <content>` | Enriched prompt content, read from tempfile in Rust and passed directly via `Command::arg()` |
| `--session-id UUID` | Maintains conversation state server-side across turns |

> **Note:** Because the prompt travels through `execve` args, OS limits apply
> (~2 MB on Linux). Meeting prompts are typically 10–50 KB, well within this
> limit.

The binary invoked is hardcoded to `copilot` — the `config.command` field
(which defaults to `amplihack copilot`) is intentionally bypassed in meeting
mode to avoid the amplihack wrapper's custom instruction injection.

### Prompt content

The prompt file contains the same enriched content as the PTY path:

1. `prompt_preamble` — conversation history (last 30 messages verbatim,
   earlier messages summarized)
2. `identity_context` — Simard's personality, live goals, cognitive memories
3. `objective` — the current user message

These fields are concatenated, run through `prepare_turn_context()` for
memory/knowledge enrichment, and written to a `NamedTempFile`. The tempfile
is auto-deleted on drop after the subprocess completes.

## Response handling

Meeting mode reads the copilot response directly from stdout of the
subprocess, rather than parsing a PTY transcript:

1. `Command::output()` captures stdout and stderr.
2. stdout is decoded as UTF-8 (lossy) and trimmed.
3. Empty responses return `AdapterInvocationFailed` with a diagnostic message.
4. Non-zero exit codes return `AdapterInvocationFailed` with the exit code
   and stderr content.

No transcript scraping, no ANSI stripping, no footer-line filtering. The
`--silent` flag ensures stdout contains only the conversational response.

## Process management

Each turn spawns one `copilot` subprocess via `std::process::Command` and
waits for it to complete. There is no persistent background process:

- At most one `copilot` process is alive at any time (meeting turns are
  sequential via `MeetingBackend`'s `&mut self` API).
- Process cleanup is handled by the OS on normal exit or by Rust's `Child`
  drop implementation on error.
- The `--session-id` flag provides conversation continuity server-side
  without requiring a persistent local process.

## Security

| Concern | Mitigation |
|---------|-----------|
| Command injection | `Command::arg()` used exclusively — no shell interpretation. Binary name `copilot` is hardcoded, not from config. |
| Prompt injection | Prompt written to `NamedTempFile` (mode `0600`), read back in Rust, passed via `Command::arg()`. No shell involved at any stage. |
| Custom instruction re-injection | `--no-custom-instructions` flag + direct `copilot` invocation (not `amplihack copilot`). |
| Session ID injection | UUID v4 from `uuid` crate — alphabet `[a-f0-9-]` only. Passed via `Command::arg()`. |
| Tempfile cleanup | `NamedTempFile` auto-deletes on drop. Prompt handle kept alive for subprocess duration. |
| XPIA | Intentionally skipped — meeting prompts are system-generated from conversation history, not arbitrary user-supplied tool output. |

## Configuration

Meeting mode requires no additional configuration. The mode is detected
automatically from `BaseTypeSessionRequest::mode`:

```rust
fn is_meeting_mode(&self) -> bool {
    self.request.mode == OperatingMode::Meeting
}
```

### Environment requirements

| Requirement | Detail |
|-------------|--------|
| `copilot` on PATH | The `copilot` binary must be available. Not `amplihack copilot` — the bare `copilot` CLI. |
| `gh auth` configured | Copilot requires GitHub authentication via `gh auth login`. |
| No special env vars | Meeting mode does not read `AMPLIHACK_HOME`, `AMPLIHACK_AGENT_BINARY`, or other amplihack env vars. |

### Verifying the binary

```bash
which copilot        # must resolve
copilot --help       # must show --no-custom-instructions, --session-id, --silent flags
gh auth status       # must show authenticated
```

## Error handling

| Condition | Error |
|-----------|-------|
| `copilot` not on PATH | `AdapterInvocationFailed`: "failed to spawn copilot: No such file or directory" |
| Non-zero exit code | `AdapterInvocationFailed`: "copilot exited with status {code}: {stderr}" |
| Empty stdout | `AdapterInvocationFailed`: "copilot returned no conversational content" |
| Tempfile creation fails | `AdapterInvocationFailed`: "failed to create copilot prompt temp file" |
| Session already closed | `InvalidBaseTypeSessionState` (same as non-meeting path) |

## Cost tracking

Meeting-mode turns call `record_cost("copilot", prompt_chars,
completion_chars, label)` with the same parameters as PTY-mode turns.
The label includes `"copilot meeting turn {n}"` to distinguish meeting
turns in cost reports.

## Differences from PTY path

| Aspect | PTY path (non-meeting) | Direct path (meeting) |
|--------|----------------------|---------------------|
| Binary | `amplihack copilot` (from `config.command`) | `copilot` (hardcoded) |
| Custom instructions | Loaded by amplihack wrapper | Blocked by `--no-custom-instructions` |
| Process wrapper | `script(1)` PTY via `execute_terminal_turn` | `std::process::Command` |
| Response extraction | Transcript scraping + ANSI stripping | Direct stdout capture |
| Conversation state | None (full context in each prompt) | `--session-id UUID` (server-side) |
| Startup overhead | ~50 seconds (amplihack bootstrap) | ~2 seconds |
| Output format | Raw terminal transcript | Clean text via `--silent` |

## Testing

Tests are in `src/base_type_copilot/tests.rs`:

| Test | Validates |
|------|----------|
| `meeting_mode_detected` | `is_meeting_mode()` returns true for `OperatingMode::Meeting` |
| `non_meeting_mode_not_detected` | `is_meeting_mode()` returns false for `OperatingMode::Engineer` (and by extension Curator, Improvement, Gym, Orchestrator) |
| `session_uuid_generated_on_open` | `open()` in meeting mode populates `session_uuid` |
| `session_uuid_none_for_non_meeting` | `open()` in non-meeting mode leaves `session_uuid` as `None` |
| `session_uuid_cleared_on_close` | `close()` sets `session_uuid` to `None` |
| `run_turn_dispatches_to_meeting` | Meeting mode calls `run_meeting_turn` (verified via mock) |
| `run_turn_dispatches_to_pty` | Non-meeting mode calls `run_pty_turn` (verified via mock) |

## Related reading

- [Base type adapters](./base-type-adapters.md) — Full adapter catalog
  including the `copilot-sdk` PTY path.
- [Meeting backend API](./meeting-backend-api.md) — The `MeetingBackend`
  that calls `run_turn()` on the copilot session.
- [LightweightChatSession](./lightweight-chat-session.md) — Alternative
  meeting session using `SessionBuilder` directly.
- [How to start a meeting](../howto/start-a-meeting.md) — Operator-facing
  meeting guide.
