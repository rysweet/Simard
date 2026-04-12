---
title: Unified Meeting Backend
description: Architecture for the unified meeting backend — one conversational engine, two thin frontends (CLI REPL and dashboard WebSocket).
last_updated: 2026-04-12
owner: simard
doc_type: architecture-decision
issues: ["#462"]
related:
  - ../index.md
  - ./ooda-meeting-handoff-integration.md
  - ../howto/start-a-meeting.md
  - ../reference/meeting-backend-api.md
  - ../reference/simard-cli.md
---

# Unified Meeting Backend

## Problem

Before this change, Simard had two divergent meeting paths:

1. **CLI REPL** (`meeting_repl/`) — a terminal-based meeting with structured line parsing (`agenda:`, `decision:`, `risk:`, etc.), auto-capture heuristics, and its own persistence layer.
2. **Dashboard WebSocket chat** (`operator_commands_dashboard/routes.rs`) — a separate chat handler with independent message handling, no conversation history, and no memory integration.

These paths had different capabilities, different personalities, and different persistence behavior. An operator using the CLI got structured meeting capture; an operator using the dashboard got a stateless chat. Neither provided what was actually needed: a natural conversation with Simard that maintains context, remembers past meetings, and persists outcomes.

The structured line parsing (`agenda:`, `update:`, `decision:`, `risk:`, `next-step:`, `open-question:`) was a holdover from before Simard had a real LLM backend. With a capable language model, these rigid prefixes add friction without adding value — the LLM can extract decisions, risks, and action items from natural conversation.

## Solution

### One backend, two frontends

```
┌─────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  CLI REPL   │────▶│  MeetingBackend  │◀────│  Dashboard WS   │
│  (stdin/out)│     │                  │     │  (WebSocket)    │
└─────────────┘     │  - history       │     └─────────────────┘
                    │  - persistence   │
                    │  - memory bridge │
                    │  - system prompt │
                    └────────┬─────────┘
                             │
                    ┌────────▼─────────┐
                    │ BaseTypeSession   │
                    │ (LLM execution)  │
                    └──────────────────┘
```

`MeetingBackend` is the single source of truth for all meeting behavior. It owns:

- **Conversation history** — a `Vec<ConversationMessage>` maintained across all turns.
- **System prompt construction** — Simard's personality, current goals/mission, and relevant memories injected at session start.
- **LLM delegation** — calls `BaseTypeSession::run_turn()` with the full conversation context.
- **Persistence** — JSON transcripts to `~/.simard/meetings/`, MeetingHandoff artifacts for OODA integration, and cognitive memory storage via the bridge.

The CLI REPL becomes a ~80-line stdin/stdout loop. The dashboard WebSocket handler becomes a ~50-line async adapter. Neither contains meeting logic.

### Conversational, not structured

The meeting is a natural conversation. The only special inputs are slash commands:

| Command   | Effect |
|-----------|--------|
| `/help`   | Print available commands |
| `/status` | Show session info: topic, duration, message count |
| `/close`  | End the meeting, persist transcript, generate summary |

Everything else is natural language sent to the LLM with full conversation history. Simard extracts decisions, action items, and risks through conversation — not through line-prefix parsing.

### Conversation history management

Every `send_message()` call appends the user message and Simard's response to the history vector. On each LLM call, the backend formats the history into the `prompt_preamble` field:

- **Last 30 messages**: included verbatim.
- **Earlier messages**: summarized into a rolling context paragraph prepended before the recent messages.
- **Hard cap**: 500 messages per session to prevent memory exhaustion.

### System prompt composition

At session creation, `MeetingBackend` builds the system prompt from three sources:

1. **Base personality**: `prompt_assets/simard/meeting_system.md` — Simard's conversational style, role, operator context, and ecosystem awareness.
2. **Live context**: `build_live_meeting_context()` output — current top-5 goals, active projects, recent session outcomes, and research tracker updates.
3. **Relevant memories**: Loaded from `CognitiveMemoryBridge` — episodic memories from recent meetings, semantic knowledge relevant to the topic, and prospective plans.

These are concatenated and injected via the `identity_context` field of `BaseTypeTurnInput`.

### Persistence on close

When the operator sends `/close`, `MeetingBackend` performs four operations:

1. **LLM summarization call** — Sends a final LLM turn asking Simard to summarize the conversation. This internal prompt is not visible to the operator but consumes one additional LLM call (adds latency and token cost). The resulting summary text is used in the handoff artifact and transcript.
2. **JSON transcript** → `~/.simard/meetings/{timestamp}_{sanitized_topic}.json` with `0o600` permissions. Contains all messages, timestamps, topic, and duration.
3. **MeetingHandoff artifact** → `target/meeting_handoffs/meeting_handoff.json` for OODA integration. Uses empty `decisions` and `action_items` vectors (downstream consumers handle empty collections). The conversation summary is placed in the `transcript` field.
4. **Cognitive memory** → Via `CognitiveMemoryBridge`: episodic memory of the meeting event, semantic extraction of key decisions/learnings, and prospective memory for agreed next steps.

### Compatibility with existing systems

| System | Compatibility |
|--------|--------------|
| `MeetingHandoff` struct | Unchanged. `close()` produces a handoff with empty structured fields. |
| `MeetingSession` (meeting_facilitator) | Unchanged. Not modified or consumed by `MeetingBackend`. |
| `PersistedMeetingRecord` (meetings.rs) | Unchanged. Separate module, out of scope. |
| OODA handoff integration | Compatible. `check_meeting_handoffs()` reads the same handoff file. |
| `meeting read` CLI command | Reads the new JSON transcript format. |

## Module structure

```
src/meeting_backend/
├── mod.rs        — MeetingBackend struct, new_session(), send_message(), close(), status()
├── types.rs      — ConversationMessage, MeetingResponse, MeetingSummary, SessionStatus
├── command.rs    — MeetingCommand enum (6 variants), parse_meeting_command()
└── persist.rs    — sanitize_filename(), write_transcript(), write_handoff(), store_cognitive_memory()
```

### Files deleted

| File | Reason |
|------|--------|
| `src/meeting_repl/auto_capture.rs` | Heuristic keyword scanning replaced by conversational LLM |
| `src/meeting_repl/command.rs` | Replaced by `meeting_backend/command.rs` with 6 variants |
| `src/meeting_repl/persist.rs` | Replaced by `meeting_backend/persist.rs` |

### Files modified

| File | Change |
|------|--------|
| `src/meeting_repl/repl.rs` | Thin stdin/stdout loop delegating to `MeetingBackend` |
| `src/meeting_repl/mod.rs` | Re-exports from `meeting_backend`, removes deleted submodules |
| `src/meeting_repl/test_support.rs` | Updated for new `MeetingBackend` API |
| `src/operator_commands_dashboard/routes.rs` | Thin WS handler delegating via `spawn_blocking` |
| `src/lib.rs` | Adds `pub mod meeting_backend` |

## Sync API design

`MeetingBackend` methods are **synchronous**, matching `BaseTypeSession::run_turn()`. The dashboard WebSocket handler wraps calls in `tokio::task::spawn_blocking()`. This keeps the core free of async runtime dependencies.

```rust
impl MeetingBackend {
    pub fn new_session(topic: &str, agent: Box<dyn BaseTypeSession>,
                       bridge: Option<Arc<dyn BridgeTransport>>,
                       system_prompt: String) -> Self;
    pub fn send_message(&mut self, input: &str) -> SimardResult<MeetingResponse>;
    pub fn close(&mut self) -> SimardResult<MeetingSummary>;
    pub fn status(&self) -> SessionStatus;
}
```

## Security considerations

- Topic strings are sanitized before filesystem use: path separators, `..`, and null bytes are stripped; length is capped at 128 characters.
- All persisted JSON files are written with `0o600` permissions.
- Conversation history is capped at 500 messages.
- No `unwrap()` or `expect()` on user-supplied data.
- Transcript content is logged at `DEBUG` level only; lifecycle events at `INFO`.
- WebSocket handler reuses existing dashboard auth middleware.
- WebSocket `max_message_size` is set to 64KB.

## Testing

- `meeting_backend/command.rs`: inline unit tests for all 6 command variants and edge cases.
- `meeting_backend/persist.rs`: inline tests for filename sanitization, JSON serialization, and permission verification (using temp dirs).
- `meeting_backend/mod.rs`: integration tests with mock `BaseTypeSession` verifying conversation history accumulation, system prompt injection, and close behavior.
- `meeting_repl/test_support.rs`: updated integration helpers for end-to-end CLI meeting flows.
