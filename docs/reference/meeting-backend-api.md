---
title: Meeting backend API reference
description: Rust API reference for MeetingBackend — the unified meeting engine behind CLI REPL and dashboard WebSocket chat.
last_updated: 2026-04-12
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../index.md
  - ../architecture/unified-meeting-backend.md
  - ../howto/start-a-meeting.md
  - ./simard-cli.md
---

# Meeting backend API reference

`MeetingBackend` is the single meeting engine used by both the CLI REPL and dashboard WebSocket chat. It lives in `src/meeting_backend/`.

## Module layout

```
src/meeting_backend/
├── mod.rs        — MeetingBackend struct and public API
├── types.rs      — Data types for messages, responses, summaries
├── command.rs    — MeetingCommand enum and parser
└── persist.rs    — Transcript and handoff persistence
```

## Types

### `ConversationMessage`

```rust
pub struct ConversationMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

pub enum MessageRole {
    User,
    Assistant,
    System,
}
```

A single message in the conversation history. Every user input and Simard response is recorded as a `ConversationMessage`.

### `MeetingResponse`

```rust
pub struct MeetingResponse {
    pub content: String,
    pub message_count: usize,
}
```

Returned by `send_message()`. Contains Simard's response text and the running count of messages in the session.

### `MeetingSummary`

```rust
pub struct MeetingSummary {
    pub topic: String,
    pub summary: String,
    pub message_count: usize,
    pub duration: std::time::Duration,
    pub transcript_path: Option<PathBuf>,
    pub handoff_written: bool,
    pub memories_stored: usize,
}
```

Returned by `close()`. Contains the session outcome and paths to persisted artifacts.

### `SessionStatus`

```rust
pub struct SessionStatus {
    pub topic: String,
    pub message_count: usize,
    pub duration: std::time::Duration,
    pub is_open: bool,
}
```

Returned by `status()`. Lightweight snapshot of the current session state.

### `MeetingCommand`

```rust
pub enum MeetingCommand {
    Help,
    Close,
    Status,
    Template(Option<String>),
    Export,
    Conversation(String),
    Empty,
    Unknown(String),
}
```

The 8-variant command enum. `parse_command()` maps user input to these variants:

| Input | Variant | Behavior |
|-------|---------|----------|
| `/help` | `Help` | Display available commands |
| `/close` or `/done` | `Close` | End session, persist, summarize |
| `/status` | `Status` | Show session info |
| `/template` | `Template(None)` | List available templates |
| `/template standup` | `Template(Some("standup"))` | Apply the named template |
| `/export` | `Export` | Write markdown export to `~/.simard/meetings/` |
| `""` or whitespace | `Empty` | No-op, re-prompt |
| `/anything-else` | `Unknown(cmd)` | Print "unknown command" hint |
| anything else | `Conversation(text)` | Send to LLM via `send_message()` |

## MeetingBackend API

All methods are **synchronous**. Dashboard callers wrap them in `tokio::task::spawn_blocking()`.

### `new_session`

```rust
pub fn new_session(
    topic: &str,
    agent: Box<dyn BaseTypeSession>,
    bridge: Option<Arc<dyn BridgeTransport>>,
    system_prompt: String,
) -> Self
```

Creates a new meeting session.

**Parameters:**

- `topic` — Meeting topic, used for display and transcript filename.
- `agent` — The LLM execution backend. Constructed by the caller via `SessionBuilder`.
- `bridge` — Optional cognitive memory bridge for loading memories at start and storing them on close. When `None`, memory features are skipped gracefully.
- `system_prompt` — The base system prompt including Simard's personality and live context. Callers build this using `build_live_meeting_context()` and the base prompt from `prompt_assets/simard/meeting_system.md`. Does **not** need to include memories — `new_session` loads those from the bridge.

**Returns:** A `MeetingBackend` instance ready for conversation.

**Side effects:** Loads relevant memories from the bridge (if provided) and appends them to the system prompt context. Logs session start at `INFO` level.

### `send_message`

```rust
pub fn send_message(&mut self, input: &str) -> SimardResult<MeetingResponse>
```

Sends a user message and returns Simard's response.

**Parameters:**

- `input` — The user's message text. Must not be empty (returns an error for empty input after trimming).

**Returns:** `SimardResult<MeetingResponse>` containing Simard's response and the running message count.

**Behavior:**

1. Appends the user message to conversation history.
2. Formats the conversation history into `prompt_preamble`:
   - Last 30 messages included verbatim.
   - Earlier messages (if any) compressed into a rolling summary paragraph.
3. Calls `BaseTypeSession::run_turn()` with the formatted context.
4. Appends Simard's response to conversation history.
5. Returns the response.

**Errors:** Returns `SimardError` if the LLM call fails or the session is closed.

### `close`

```rust
pub fn close(&mut self) -> SimardResult<MeetingSummary>
```

Ends the meeting, persists all artifacts, and returns a summary.

**Returns:** `SimardResult<MeetingSummary>` with the topic, generated summary, message count, duration, transcript path, and memory storage count.

**Behavior:**

1. Sends a final LLM call asking Simard to summarize the conversation (the summary prompt is internal and not visible to the operator).
2. Writes the full transcript to `~/.simard/meetings/{timestamp}_{sanitized_topic}.json` with `0o600` permissions.
3. Writes a `MeetingHandoff` artifact to `target/meeting_handoffs/meeting_handoff.json` with:
   - `transcript`: contains the conversation summary
   - `decisions`: empty vec (decisions are captured in the summary narrative)
   - `action_items`: empty vec
   - `open_questions`: empty vec
   - `processed`: `false`
4. Stores cognitive memories via the bridge (if available):
   - 1 episodic memory (the meeting event)
   - Semantic memories extracted from the summary
   - Prospective memories for agreed next steps
5. Marks the session as closed. Further `send_message()` calls return an error.

**Errors:** Persistence failures are logged at `WARN` level but do not prevent the summary from being returned. The method succeeds even if disk writes or bridge calls fail — the summary is always returned to the operator.

### `status`

```rust
pub fn status(&self) -> SessionStatus
```

Returns a lightweight snapshot of the session state.

**Returns:** `SessionStatus` with the topic, message count, elapsed duration, and whether the session is still open.

### `history`

```rust
pub fn history(&self) -> &[ConversationMessage]
```

Returns a reference to the full conversation history. Used by `/export` to write the markdown file.

### `started_at`

```rust
pub fn started_at(&self) -> chrono::DateTime<chrono::Utc>
```

Returns the session start timestamp. Used for export frontmatter and duration calculations.

### `export_markdown`

```rust
pub fn export_markdown(&self) -> SimardResult<PathBuf>
```

Writes the current meeting state as a markdown file to `~/.simard/meetings/`.

**Returns:** `SimardResult<PathBuf>` — the path to the written file.

**Behavior:**

1. Delegates to `write_markdown_export()` in `persist.rs`.
2. The file includes YAML frontmatter (topic, date, duration, message count) and the full conversation rendered as markdown. The `themes` field is always an empty array in exports — theme extraction only happens on `/close`.
3. File permissions are set to `0o600`.
4. The directory is created if it doesn't exist.

**Errors:** Returns `SimardError` if the file cannot be written (permissions, disk full).

## Template system

### `get_template`

```rust
pub fn get_template(name: &str) -> Option<&'static str>
```

Returns the agenda text for a named template, or `None` if the name is unrecognized.

**Available templates:** `standup`, `1on1`, `retro`, `planning`.

### `template_names`

```rust
pub fn template_names() -> &'static [&'static str]
```

Returns the list of available template names: `["standup", "1on1", "retro", "planning"]`.

## Command parser

```rust
pub fn parse_command(input: &str) -> MeetingCommand
```

Parses raw user input into a `MeetingCommand` variant.

**Rules:**

1. Leading and trailing whitespace is trimmed.
2. Empty input after trimming → `Empty`.
3. Input starting with `/` is matched case-insensitively against known commands (`/help`, `/close`, `/done`, `/status`, `/template`, `/export`).
4. `/template` with no argument → `Template(None)`. `/template foo` → `Template(Some("foo"))`.
5. Unrecognized `/` commands → `Unknown(command_name)`.
6. Everything else → `Conversation(trimmed_input)`.

## Persistence format

### Transcript JSON

Written to `~/.simard/meetings/{timestamp}_{sanitized_topic}.json`:

```json
{
  "version": 1,
  "topic": "discuss the next Simard milestone",
  "started_at": "2026-04-12T14:30:00Z",
  "ended_at": "2026-04-12T15:15:00Z",
  "duration_seconds": 2700,
  "message_count": 24,
  "summary": "Discussed memory consolidation priorities...",
  "messages": [
    {
      "role": "user",
      "content": "Hey Simard, what's been on your mind?",
      "timestamp": "2026-04-12T14:30:12Z"
    },
    {
      "role": "assistant",
      "content": "I've been thinking about...",
      "timestamp": "2026-04-12T14:30:18Z"
    }
  ]
}
```

**Filename sanitization rules:**

- Path separators (`/`, `\`), `..`, and null bytes are stripped.
- Non-alphanumeric characters (except `-` and `_`) are replaced with `-`.
- Consecutive hyphens are collapsed.
- Maximum length: 128 characters (truncated with no trailing hyphen).

**File permissions:** `0o600` (owner read/write only).

### MeetingHandoff JSON

Written to `target/meeting_handoffs/meeting_handoff.json`:

```json
{
  "topic": "discuss the next Simard milestone",
  "started_at": "2026-04-12T14:30:00Z",
  "closed_at": "2026-04-12T15:15:00Z",
  "decisions": [],
  "action_items": [],
  "open_questions": [],
  "processed": false,
  "duration_secs": 2700,
  "transcript": ["Discussed memory consolidation priorities..."],
  "participants": ["operator"],
  "themes": ["memory consolidation", "gym benchmarks"]
}
```

This format is consumed by `check_meeting_handoffs()` in the OODA loop and by the `act-on-decisions` CLI command. Empty `decisions`, `action_items`, and `open_questions` vectors are valid — downstream consumers handle them without error. The `transcript` field carries the conversation summary. The `themes` field is optional (`#[serde(default)]`) and carries high-level topic tags extracted during the meeting; older handoffs without this field deserialize correctly with an empty vec.

### Markdown export

Written by `/export` to `~/.simard/meetings/{timestamp}_{sanitized_topic}.md`:

```markdown
---
topic: "discuss the next Simard milestone"
date: "2026-04-12T14:30:00Z"
duration_minutes: 45
message_count: 24
themes: []
---

# Meeting: discuss the next Simard milestone

**Date:** 2026-04-12 14:30 UTC
**Duration:** 45 minutes
**Messages:** 24

---

## Conversation

**You:** Hey Simard, what's been on your mind since our last meeting?

**Simard:** I've been thinking about the memory consolidation pipeline...

**You:** What about the gym scores?

**Simard:** The SecurityAudit scenarios are still below where I want them...
```

**File permissions:** `0o600` (owner read/write only).

The markdown file is a point-in-time snapshot — it captures the conversation as of the `/export` call. You can export multiple times during a meeting; each creates a new file with a different timestamp.

## Integration patterns

### CLI REPL integration

```rust
// Simplified — actual code is in src/meeting_repl/repl.rs
let backend = MeetingBackend::new_session(topic, agent, bridge, system_prompt);
loop {
    let input = read_line(stdin)?;
    match parse_command(&input) {
        MeetingCommand::Help => print_help(stdout),
        MeetingCommand::Status => print_status(stdout, backend.status()),
        MeetingCommand::Template(name) => {
            // List templates or apply one by name
        }
        MeetingCommand::Export => {
            let path = backend.export_markdown()?;
            writeln!(stdout, "Exported to {}", path.display())?;
        }
        MeetingCommand::Close => {
            let summary = backend.close()?;
            print_summary(stdout, &summary);
            break;
        }
        MeetingCommand::Conversation(text) => {
            let response = backend.send_message(&text)?;
            writeln!(stdout, "\nSimard: {}\n", response.content)?;
        }
        MeetingCommand::Empty => continue,
        MeetingCommand::Unknown(cmd) => {
            writeln!(stdout, "Unknown command: /{}. Type /help for commands.", cmd)?;
        }
    }
}
```

### Dashboard WebSocket integration

```rust
// Simplified — actual code is in src/operator_commands_dashboard/routes.rs
let backend = Arc::new(Mutex::new(
    MeetingBackend::new_session(topic, agent, bridge, system_prompt)
));

while let Some(msg) = ws_stream.next().await {
    let text = msg?.to_text()?;
    let backend = Arc::clone(&backend);
    let response = tokio::task::spawn_blocking(move || {
        let mut b = backend.lock().unwrap();
        match parse_command(text) {
            MeetingCommand::Close => {
                let summary = b.close()?;
                Ok(serde_json::to_string(&summary)?)
            }
            MeetingCommand::Conversation(text) => {
                let resp = b.send_message(&text)?;
                Ok(resp.content)
            }
            // ... other variants
        }
    }).await??;
    ws_stream.send(Message::Text(response)).await?;
}
```

## Limits and defaults

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Max conversation history | 500 messages | Prevent memory exhaustion |
| Verbatim history window | 30 messages | Balance context quality with token cost |
| Topic max length | 128 characters | Filesystem safety |
| Transcript file permissions | `0o600` | Privacy — meeting content is sensitive |
| WebSocket max message size | 64 KB | Prevent abuse |
