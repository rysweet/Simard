---
title: Dashboard Chat — persistence, sessions, and streaming protocol
description: API reference for the Dashboard Chat feature — durable chat sessions, the session-list REST API, the WebSocket wire protocol (streaming with fallback), and the on-disk store layout.
last_updated: 2026-07-04
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./lightweight-chat-session.md
  - ./dashboard-chat-multiline-input.md
  - ./meeting-backend-api.md
  - ./conversation-channel-api.md
  - ./agent-log-websocket.md
  - ./state-root-resolution.md
  - ../dashboard.md
  - ../architecture/unified-meeting-backend.md
---

# Dashboard Chat

The **Chat** tab of the operator dashboard is a full conversational surface
over Simard's meeting backend. Unlike the earlier ephemeral chat widget, every
conversation is a **durable, resumable session**: the full turn history is
persisted to disk, survives page reloads *and* process restarts, and can be
reopened from a session list. Assistant responses render **incrementally**
(streamed) where the backend supports it, with automatic fallback to a single
complete response.

This reference documents the storage layout, the REST session API, and the
WebSocket wire protocol. For the operator-facing tour of the tab, see
[the dashboard guide](../dashboard.md#chat-tab-durable-resumable-sessions).

**Backend location:** `src/operator_commands_dashboard/chat.rs` (WebSocket
handler + turn loop), `src/operator_commands_dashboard/chat_store.rs` (durable
store), `src/operator_commands_dashboard/routes.rs` (REST + WS route
registration).

**Frontend location:** `src/operator_commands_dashboard/index_html/part_00.rs`
(layout + CSS), `src/operator_commands_dashboard/index_html/part_04.rs`
(session list + WebSocket client).

---

## Concepts

| Term | Meaning |
|------|---------|
| **Chat session** | One durable conversation, identified by a stable `session_id`. Holds the complete ordered turn history (`user` / `assistant` / `system` messages). |
| **Turn** | A single `ConversationMessage` (`role`, `content`, `timestamp`). |
| **Session index** | A single `index.json` file listing every session's metadata (id, title, timestamps) for the sidebar. |
| **Rehydration** | On reopening a session, the persisted history is replayed into a fresh `MeetingBackend` so the agent regains full prior context — not just the UI. |
| **Streaming** | The server emits an assistant reply as a sequence of `chunk` frames terminated by a `done` frame, so text appears incrementally. |
| **Fallback** | When streaming is unavailable, the server emits one complete `assistant` frame in a single update. |

Every chat turn is routed through the existing `SessionBuilder` /
`MeetingBackend` (`OperatingMode::Meeting`) path — the same backend used by the
CLI meeting REPL. Chat is a real agent conversation, not an echo or stub. See
[Unified meeting backend](../architecture/unified-meeting-backend.md).

---

## Storage layout

Chat sessions are persisted under the resolved durable state root (see
[State-root resolution](./state-root-resolution.md)) in a dedicated
`chat_sessions/` subdirectory, keyed strictly by `session_id`:

```
<state_root>/chat_sessions/
├── index.json                # session list (metadata only)
├── 018f3c…-…-….json          # full turn history for one session
├── 018f3d…-…-….json
└── …
```

- `<state_root>` is resolved via `SIMARD_STATE_ROOT` → `$HOME/.simard`
  (default) through the dashboard-local `resolve_state_root()` helper in
  `routes.rs` — the same resolver the rest of the dashboard (goals, hosts,
  memory) already uses, so chat sessions land beside the other dashboard state.
- The store joins the fixed static subdirectory `chat_sessions` onto that root
  (`resolve_state_root().join("chat_sessions")`).
- All writes go through `crate::persistence::persist_json` — a crash-durable
  atomic pipeline (write-temp → `fsync` → `rename` → parent-dir `fsync`) that
  chmods every file to `0o600` (owner-only). `persist_json` creates missing
  parent directories with `create_dir_all` (subject to the process umask), so
  the chat store additionally chmods the `chat_sessions/` directory itself to
  `0o700` on first use — the tree is owner-only end to end.
- History is **uncapped on disk.** The in-memory `MeetingBackend` working set
  retains its `MAX_HISTORY = 500` most-recent cap for live inference, but the
  durable store records every turn before any in-memory truncation, so
  "complete conversation history" always survives a restart.

> **Isolation from meeting autosave.** The legacy meeting autosave keys files
> by topic (`_autosave_<topic>.json`), which would collide across chat
> sessions that all share the topic "Dashboard Chat". The chat store keys
> strictly by `session_id`, so sessions never clobber one another.

> **Resolution note.** Chat uses the dashboard-local `resolve_state_root()`
> rather than the crate-level `state_root::resolve_subdir`. The two agree on the
> common `SIMARD_STATE_ROOT` / `$HOME/.simard` cases, but the crate-level ladder
> additionally honors narrow per-subsystem env vars and validates malformed
> roots. Chat deliberately stays on the dashboard resolver for consistency with
> the other dashboard surfaces (the `goals.rs` pattern the REST handlers follow);
> do not mix the two resolvers for the same path.

### `index.json`

```jsonc
{
  "schema_version": 1,
  "sessions": [
    {
      "id": "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88",
      "title": "How do I unblock a stuck OODA goal?",
      "created_at": "2026-07-04T15:20:11Z",
      "updated_at": "2026-07-04T15:41:02Z"
    }
  ]
}
```

The list is upserted after every turn (`updated_at` advances). The REST list
endpoint returns it sorted by `updated_at` **descending** (most recent first).

### `<session_id>.json`

```jsonc
{
  "schema_version": 1,
  "meta": {
    "id": "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88",
    "title": "How do I unblock a stuck OODA goal?",
    "created_at": "2026-07-04T15:20:11Z",
    "updated_at": "2026-07-04T15:41:02Z"
  },
  "history": [
    { "role": "user",      "content": "How do I unblock a stuck OODA goal?", "timestamp": "2026-07-04T15:20:11Z" },
    { "role": "assistant", "content": "Start by inspecting the goal board…",  "timestamp": "2026-07-04T15:20:19Z" }
  ]
}
```

`history` entries are `ConversationMessage` values (see the
[meeting backend API](./meeting-backend-api.md#conversationmessage)):
`role` is one of `user` / `assistant` / `system`; `timestamp` is RFC3339.

### Session lifecycle

- A session record is created **lazily, on the first user message.** Empty or
  abandoned WebSocket connections leave no session behind, so the sidebar never
  fills with blank sessions.
- The **title** is derived from the first user message, truncated (on a UTF-8
  char boundary) to ~60 chars. If the first message is empty, the title falls
  back to the creation timestamp (RFC3339).
- `created_at` is set once; `updated_at` advances on every persisted turn.

### Session IDs

`session_id` is a generated UUID (v7, time-ordered). Every id that reaches a
filesystem path — whether from the REST `{id}` path parameter or the WebSocket
`session_id` query parameter — is validated against:

```
^[A-Za-z0-9_-]{1,64}$
```

before any path join. Values that fail validation are rejected (**HTTP 400**
on REST, connection rejected on the WebSocket) before touching the store. This
is the same path-traversal guard used by the
[agent log WebSocket](./agent-log-websocket.md#path-parameters).

---

## REST API

Both endpoints are registered **inside** the dashboard's `require_auth`
middleware scope (`build_router` in `routes.rs`). The browser sends the
`simard_session` cookie automatically; unauthenticated requests are rejected by
`require_auth` (**HTTP 401**). Handlers follow the established thin-wrapper +
`_at(state_root)` core pattern used across the dashboard (see `goals.rs`), and
return `(StatusCode, Json<Value>)`.

### `GET /api/chat/sessions`

List all saved chat sessions, newest first.

**Response `200 OK`**

```json
{
  "sessions": [
    {
      "id": "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88",
      "title": "How do I unblock a stuck OODA goal?",
      "created_at": "2026-07-04T15:20:11Z",
      "updated_at": "2026-07-04T15:41:02Z"
    },
    {
      "id": "018f3b12-4a9c-7d02-8f31-11ab77e0c210",
      "title": "Summarize last cycle's actions",
      "created_at": "2026-07-04T14:02:55Z",
      "updated_at": "2026-07-04T14:09:40Z"
    }
  ]
}
```

- Sorted by `updated_at` descending.
- Returns `{"sessions": []}` when no sessions exist (never a 404, never a 500
  for an empty or missing store).

**Example**

```bash
curl -s --cookie "simard_session=$CODE" \
  http://localhost:8080/api/chat/sessions | jq
```

### `GET /api/chat/sessions/{id}`

Fetch the complete turn history for one session (used to render the panel when
a session is clicked in the sidebar).

**Path parameters**

| Param | Type   | Validation                          |
| ----- | ------ | ----------------------------------- |
| `id`  | string | Must match `^[A-Za-z0-9_-]{1,64}$` |

**Response `200 OK`**

```json
{
  "id": "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88",
  "title": "How do I unblock a stuck OODA goal?",
  "created_at": "2026-07-04T15:20:11Z",
  "updated_at": "2026-07-04T15:41:02Z",
  "history": [
    { "role": "user",      "content": "How do I unblock a stuck OODA goal?", "timestamp": "2026-07-04T15:20:11Z" },
    { "role": "assistant", "content": "Start by inspecting the goal board…",  "timestamp": "2026-07-04T15:20:19Z" }
  ]
}
```

**Status codes**

| Status | Condition |
| ------ | --------- |
| `200 OK` | Session found; full history returned. |
| `400 Bad Request` | `id` fails the `^[A-Za-z0-9_-]{1,64}$` validation. |
| `401 Unauthorized` | Request is missing/failing `require_auth`. |
| `404 Not Found` | No session with that (valid) `id` exists. |
| `500 Internal Server Error` | The session file exists but is corrupt/unreadable. The client sees a generic error; the specific filesystem detail is logged server-side only. |

**Example**

```bash
curl -s --cookie "simard_session=$CODE" \
  http://localhost:8080/api/chat/sessions/018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88 | jq
```

---

## WebSocket protocol

```
GET /ws/chat
GET /ws/chat?session_id=<id>
```

Mounted inside the `require_auth` scope. The `simard_session` cookie
authenticates the upgrade (no token-in-URL). The optional `session_id` query
parameter selects an existing session to resume; when omitted, a new session is
created lazily on the first user message.

**Upgrade guards.** The upgrade is gated by `require_auth`: the `simard_session`
cookie authenticates it (no token-in-URL). Cross-site WebSocket hijacking
(CSWSH) is prevented by that cookie being `SameSite=Strict` and `HttpOnly`, so a
cross-site page can neither read it nor send it on an upgrade in the first place.
A rejected `session_id` (failing the id validation below) also aborts the
upgrade with **HTTP 400** before any store access — mirroring the sibling
[agent-log WebSocket](./agent-log-websocket.md#path-parameters) guard.

**Message limits.** Inbound frames are bounded: the socket caps the maximum
WebSocket message/frame size (`max_message_size` / `max_frame_size`) and refuses
any single user message that exceeds the per-message length limit (the channel
is text chat, not file transfer). Oversized frames are refused, never persisted.

The connection carries JSON text frames in both directions. The server
announces its capabilities in a typed handshake so the client can adapt to
streaming or fallback behavior. All frames are single-line JSON objects.

### Handshake — server → client

Immediately after upgrade the server sends a `ready` frame:

```json
{
  "type": "ready",
  "session_id": "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88",
  "streaming": true,
  "protocol_version": 1
}
```

| Field | Meaning |
|-------|---------|
| `session_id` | The id this connection is bound to. For a fresh connection this is the newly minted id (also used once the first turn is persisted). |
| `streaming` | `true` when the server will emit `chunk`/`done` frames for assistant replies; `false` when it will emit single `assistant` frames. |
| `protocol_version` | Wire-protocol version. Currently `1`. |

`streaming` advertises a **server capability**, not a per-provider outcome.
Because replies are chunked server-side (see the streaming implementation note
below), the current implementation reports `true` for every connection; the
field exists so a future true token-stream — or a deployment that disables
chunking — can flip it without a protocol change. Clients must therefore branch
on the **frame shape they actually receive** (`type: "chunk"`/`"done"` vs. a
single `role: "assistant"` frame), not assume a path from the flag alone.

### Restore — server → client (resume only)

When the connection opens with a `session_id` that already exists, the server
replays the persisted history **before** accepting new input, so the panel
shows the full prior conversation and the agent context is rehydrated:

```json
{
  "type": "restore",
  "messages": [
    { "role": "user",      "content": "How do I unblock a stuck OODA goal?", "timestamp": "2026-07-04T15:20:11Z" },
    { "role": "assistant", "content": "Start by inspecting the goal board…",  "timestamp": "2026-07-04T15:20:19Z" }
  ]
}
```

Rehydration seeds the in-memory backend via `MeetingBackend::restore(history)`
(see [below](#meetingbackendrestore)).

### Sending a message — client → server

The client sends a plain user message as a JSON frame:

```json
{ "content": "What changed in the last cycle?" }
```

Slash-commands (`/help`, `/status`, `/close`, `/goal`, `/template`, …) are
sent the same way and are interpreted by the meeting backend command parser
(`parse_command`) exactly as in the CLI REPL — so the singular `/goal <text>`
records a goal, matching the parser's token set. For backward compatibility the
server also accepts a bare text frame (the raw message string) from older
clients.

### Streaming an assistant reply — server → client

When `streaming: true`, an assistant reply is delivered as an ordered run of
`chunk` frames terminated by a single `done` frame:

```json
{ "type": "chunk", "content": "Start by inspecting " }
{ "type": "chunk", "content": "the goal board with " }
{ "type": "chunk", "content": "`simard status`." }
{ "type": "done" }
```

The client appends each `chunk.content` to the in-progress assistant bubble and
finalizes it on `done`. The full assistant text is persisted as one
`ConversationMessage` regardless of how many chunks were emitted.

> **Streaming implementation note.** The meeting backend's `run_turn` is
> synchronous and returns the complete reply. Streaming is implemented as
> **server-side chunking** of that completed text over the existing WebSocket:
> incremental *appearance* is achieved without a token-level model API. The
> `streaming` capability flag and the `chunk`/`done` frames keep the wire
> protocol forward-compatible with a future true token-stream — the client code
> path does not change when real token streaming lands.

### Non-streaming (fallback) reply — server → client

When `streaming: false`, the same reply arrives as a single legacy frame:

```json
{ "role": "assistant", "content": "Start by inspecting the goal board with `simard status`." }
```

The client renders it in one update. Neither the streaming nor the fallback
path errors; a client that understands both simply keys off frame shape
(`type` vs. `role`).

### System / error frames — server → client

Informational and error lines use the legacy `role`-tagged shape and are
rendered verbatim as system bubbles:

```json
{ "role": "system", "content": "Connected to Simard. Speak naturally — /help for commands, /close to end." }
{ "role": "error",  "content": "No agent backend available. Check SIMARD_LLM_PROVIDER and auth config." }
```

### Frame reference

| Direction | Frame | Purpose |
|-----------|-------|---------|
| S → C | `{"type":"ready", …}` | Handshake: session id + capabilities. |
| S → C | `{"type":"restore","messages":[…]}` | Replay persisted history on resume. |
| C → S | `{"content":"…"}` | User message or slash-command. |
| S → C | `{"type":"chunk","content":"…"}` | Incremental assistant text (streaming). |
| S → C | `{"type":"done"}` | End of a streamed assistant reply. |
| S → C | `{"role":"assistant","content":"…"}` | Complete assistant reply (fallback). |
| S → C | `{"role":"system","content":"…"}` | System notice. |
| S → C | `{"role":"error","content":"…"}` | Error notice. |

### Persistence timing

- On the **first** user message, the session record and index entry are created
  (lazy creation), then both the user turn and the assistant reply are appended.
- On **every** subsequent turn, the user message and the assistant reply are
  appended to `<session_id>.json` and the index `updated_at` is refreshed.
- The `<session_id>.json` file is written **before** the `index.json` upsert, so
  a crash between the two leaves a recoverable session file rather than a
  dangling index entry. Concurrent index upserts are serialized.
- Only **conversational turns** are persisted: the `user` message and the
  `assistant` reply. Ephemeral wire notices — the connection banner,
  slash-command acknowledgements (`/status`, `/theme`, help, …), and `error`
  frames — are **not** `ConversationMessage`s and are **not** written to
  history or replayed on `restore`. Reopening a session replays the
  conversation, not the command side effects. (`error` is a wire-only frame
  tag; the persisted `Role` enum is only `user` / `assistant` / `system`.)
- **Structured-capture commands** (`/decision`, `/action`, `/goal`, `/theme`,
  `/risk`, `/disagree`, …) mutate the *live* `MeetingBackend` and flow into the
  meeting-close / handoff bundle produced by `/close` — a **separate**
  persistence surface from the chat transcript. Their `system` acknowledgements
  are not written to `chat_sessions/`, and the captures they record are scoped
  to the live connection: a resume replays only the conversation (which
  re-establishes agent context), not the prior connection's capture state. An
  operator who needs a capture to outlive a resume re-issues the command, or
  closes the meeting to snapshot the bundle. This keeps the chat transcript a
  clean, replayable conversation and avoids double-persisting state the handoff
  bundle already owns.

---

## `MeetingBackend::restore`

```rust
impl MeetingBackend {
    /// Seed the in-memory conversation history from a persisted session so a
    /// reopened chat regains full prior context. Generalizes the test-only
    /// `push_test_message` seeding path into a public rehydration hook.
    pub fn restore(&mut self, history: Vec<ConversationMessage>);
}
```

On WebSocket open with an existing `session_id`, the handler loads the history
from the chat store and calls `restore()` before accepting new input. This
guarantees the agent — not just the UI — sees the entire prior conversation on
resume, so replies stay contextually coherent across reloads and restarts.

If a persisted history exceeds `MAX_HISTORY` (500) turns, `restore` seeds the
working set with the **most-recent** `MAX_HISTORY` turns — the same cap live
turns obey (`src/meeting_backend/mod.rs`). The full transcript is unaffected:
it is replayed into the panel from the untruncated `restore` frame (or REST
history) and remains complete on disk; only the agent's in-memory inference
window is bounded.

---

## Frontend behavior

### Full-height responsive layout

The Chat tab fills the available viewport. The current fixed-size card — an
`#chat-messages { height: 400px }` transcript inside a `<div class="card"
style="max-width:720px">` wrapper — is replaced with a full-height flex column:

- `#tab-chat.active` is a `display:flex; flex-direction:column` container sized
  to the available height (`height: calc(100vh - …)`).
- A **sessions sidebar** lists saved sessions on the left; the conversation
  panel occupies the remaining width.
- `#chat-messages` is `flex: 1; overflow-y: auto` so the transcript scrolls
  while the input row stays anchored at the bottom.
- The layout is responsive: no fixed small height or width; the panel grows
  with the window.

### Session sidebar

On tab activation the frontend calls `GET /api/chat/sessions` and renders the
list (title + relative time). Clicking an item:

1. Fetches `GET /api/chat/sessions/{id}` and renders the full history into the
   panel.
2. Opens `GET /ws/chat?session_id={id}` to continue the conversation; the
   server's `restore` frame reconciles UI and backend state.

A **New chat** control opens `GET /ws/chat` with no `session_id`.

### Input & keyboard (multi-line composer)

The composer is a **multi-line `<textarea>`** (`#chat-input`), not a single-line
`<input>`. It starts at a single-line height, **auto-grows** as you add lines up
to a capped maximum (then scrolls internally), and **resets to one line after a
message is sent**. `Enter` sends; **`Shift+Enter` inserts a newline** for
deliberate multi-line composition and pasted snippets. The whitespace-only
guard, the streaming busy-disable, and the safe DOM sinks are all preserved.

Full behavior, the CSS/JS sizing contract, and test coverage are documented in
[Dashboard Chat — multi-line message input](./dashboard-chat-multiline-input.md).

### XSS safety

All message content — streamed `chunk`s, replayed `restore` history, and
sidebar titles — is rendered with `textContent` / `document.createTextNode`
(or the shared `esc` helper), never `innerHTML`. Stored conversation text and
model output are treated as untrusted and can never inject markup. This holds
for **multi-line and pasted** input too: a message such as
`<img src=x onerror=alert(1)>` renders as literal text.

---

## Configuration

Chat inherits the dashboard's existing configuration; there are no new required
settings.

| Setting | Source | Effect |
|---------|--------|--------|
| `SIMARD_STATE_ROOT` | env | Relocates `<state_root>`, and therefore `chat_sessions/`. Defaults to `$HOME/.simard`. See [state-root resolution](./state-root-resolution.md). |
| `SIMARD_LLM_PROVIDER` | env / `~/.simard/config.toml` | Selects the LLM provider backing chat turns (copilot, rustyclawd, …), exactly as for the CLI meeting REPL. |
| Dashboard login code | `~/.simard/.dashkey` | Gates the `require_auth` scope that both the REST and WS chat endpoints live in. |
| WS message-size cap | compile-time constant (`chat.rs`) | Upper bound on a single inbound WebSocket frame / user message; oversized frames are refused, not persisted. |

No at-rest encryption is applied to session files (accepted for a
single-operator surface); files are `0o600` under a `0o700` directory. Message
bodies and session titles are **never** written to logs.

---

## Examples

### List sessions and open the most recent

```bash
CODE=$(cat ~/.simard/.dashkey)
LATEST=$(curl -s --cookie "simard_session=$CODE" \
  http://localhost:8080/api/chat/sessions | jq -r '.sessions[0].id')

curl -s --cookie "simard_session=$CODE" \
  "http://localhost:8080/api/chat/sessions/$LATEST" | jq '.history | length'
```

### Minimal WebSocket client (Node/`ws`)

```js
import WebSocket from "ws";

// Resume an existing session; omit the query string to start a new one.
const id = "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88";
const ws = new WebSocket(`ws://localhost:8080/ws/chat?session_id=${id}`, {
  headers: { Cookie: `simard_session=${process.env.CODE}` },
});

let assistant = "";
ws.on("message", (raw) => {
  const f = JSON.parse(raw.toString());
  switch (f.type) {
    case "ready":   console.log("streaming:", f.streaming); break;
    case "restore": console.log("restored", f.messages.length, "messages"); break;
    case "chunk":   assistant += f.content; break;                 // streaming
    case "done":    console.log("assistant:", assistant); assistant = ""; break;
    default:
      if (f.role === "assistant") console.log("assistant:", f.content); // fallback
      else console.log(`[${f.role}]`, f.content);
  }
});

ws.on("open", () => ws.send(JSON.stringify({ content: "What changed in the last cycle?" })));
```

---

## Related reading

- [Dashboard guide](../dashboard.md#chat-tab-durable-resumable-sessions) — operator-facing tour of the Chat tab.
- [LightweightChatSession reference](./lightweight-chat-session.md) — the `SessionBuilder`-backed session that services each turn.
- [Meeting backend API reference](./meeting-backend-api.md) — `MeetingBackend`, `ConversationMessage`, and the turn lifecycle.
- [Conversation channel API reference](./conversation-channel-api.md) — the shared abstraction behind the CLI REPL, dashboard chat, and Signal.
- [Unified meeting backend](../architecture/unified-meeting-backend.md) — one conversational engine behind CLI and dashboard.
- [Agent log WebSocket API](./agent-log-websocket.md) — the sibling dashboard WS endpoint whose id-validation guard chat reuses.
- [State-root resolution reference](./state-root-resolution.md) — how `<state_root>` (and `chat_sessions/`) is resolved.
