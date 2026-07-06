---
title: Signal continuous conversation — per-operator durable sessions
description: Reference for the continuous, multi-turn Signal conversation. One long-lived meeting session per operator identity (keyed by Signal number) is reused across successive inbound messages, appended turn-by-turn, persisted to a durable session store that survives daemon restarts, and controlled by the operator with /new (reset), the existing /help, and the existing /close. Reuses the dashboard-chat session-file envelope and crash-durable persistence pipeline (adding a new per-operator index); preserves the allowlist, identity binding, high-risk gating, and Note-to-Self loop prevention.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#2577", "#2586", "#2578", "#2575", "#2527"]
related:
  - ../index.md
  - ./signal-conversation.md
  - ./dashboard-chat.md
  - ./conversation-channel-api.md
  - ./meeting-backend-api.md
  - ./state-root-resolution.md
  - ../architecture/conversation-channel.md
  - ../howto/set-up-the-signal-channel.md
---

# Signal continuous conversation

The Signal channel holds a **single, long-lived conversation per operator**. Each
inbound message from an allowlisted operator is appended as the **next turn** in
that operator's existing session, and Simard answers with the full prior-turn
context — the conversation is **continuous**, not one prompt per session.

Sessions are **durable**: the complete turn history is persisted to disk, so the
conversation survives a process or daemon restart. When `simard-signal.service`
restarts, the operator's next message **resumes** the existing conversation
(the persisted history is replayed into the meeting backend) instead of starting
over.

The operator controls the conversation lifecycle over Signal: `/new` (alias
`/reset`) starts a fresh conversation, `/help` shows the command banner, and
`/close` ends the meeting and writes the handoff exactly as on every channel.

> **What changed.** Previously `signal_conversation::run` built **one** unkeyed,
> in-memory `MeetingBackend::new_session("Signal", …)` for the whole process and
> never persisted it. Every process restart minted a fresh session
> (`Meeting session created topic="Signal"` in the logs) and the agent lost all
> prior context — each prompt looked like a brand-new conversation. The
> continuous conversation replaces that single global backend with a
> **per-operator registry** backed by a **durable session store**.

> **Also new (issue #2527).** Each per-operator backend is now wired into the
> **same OODA-loop context and graph cognitive memory as the CLI meeting**. On
> first touch the operator's system prompt is enriched with live OODA state
> (recent meetings, decisions, active goals, operator identity, known projects)
> via the shared `build_enriched_meeting_system_prompt`, and the backend carries
> its **own** cognitive-memory store, so a `/close` **consolidates** the
> conversation back into graph memory (episodes, summary facts) — not just the
> flat handoff bundle. Recall is reached only through `operator_commands_meeting`;
> the channel never calls `build_live_meeting_context` directly.

> **Naming.** These features add no `bridge`/`Bridge` symbol of their own. The
> #2527 OODA wiring does call the pre-existing external helper
> `memory_ipc::launch_writer_bridge`, but binds the returned cognitive-memory
> handle locally as `memory`. Continuity is expressed in one-Brain / reasoner
> terms: one meeting **session** per operator, driven by the shared meeting
> backend.

This reference documents the concepts, the on-disk store, the lifecycle
commands, the public API, the tracing contract, configuration, and worked
examples. For channel setup, guardrails, and the wire protocol, see the
[Signal channel reference](./signal-conversation.md) and
[How to set up the Signal channel](../howto/set-up-the-signal-channel.md).

---

## Concepts

| Term | Meaning |
|------|---------|
| **Operator identity** | The allowlisted Signal address the message came from — the raw signal-cli `parsed.sender` E.164 (e.g. `+12062591306`), surfaced by the channel abstraction one layer up as `inbound.from.id` (`OperatorRef.id`). On a single-number linked-device setup this is the account's own number (Note to Self). The continuity key. |
| **Signal session** | One durable, resumable conversation for a single operator. Holds the complete ordered turn history (`user` / `assistant` / `system` `ConversationMessage`s). Identified by a stable UUIDv7 `session_id`. |
| **Turn** | One `ConversationMessage` (`role`, `content`, `timestamp`). Each inbound conversation message contributes a `user` turn and Simard's reply a paired `assistant` turn. |
| **Operator index** | The single `operators.json` file mapping each operator identity to its **active** `session_id`. This is how an E.164 (which is not a valid filename) points at a session file. |
| **Rehydration / resume** | On the first message after a restart, the operator's persisted history is replayed into a fresh `MeetingBackend` (via `restore`) so the agent regains full prior context — not just the transcript. |
| **Registry** | The in-process `HashMap<operator, MeetingBackend>` the run loop keeps so back-to-back messages reuse the same live backend without touching disk on every turn. |
| **OODA context** | The live OODA-loop state — recent meetings, decisions, active goals, operator identity, known projects, research topics, improvements — injected into each operator's system prompt on first touch via the shared `build_enriched_meeting_system_prompt` (issue #2527). Makes Simard start a Signal chat already knowing her own state, identical to the CLI meeting. |
| **Cognitive-memory store** | The per-operator graph-memory handle (`memory_ipc::launch_writer_bridge`, bound as `memory`) each backend carries. It supplies the OODA recall **in** and, on `/close`, consolidates the conversation back **into** graph memory (episodes, summary facts) — bidirectional, not read-only. |

Every Signal turn is routed through the **same** `MeetingBackend`
(`OperatingMode::Meeting`) used by the CLI meeting REPL and the dashboard chat —
continuity is a real agent conversation, not an echo. See
[Unified meeting backend](../architecture/unified-meeting-backend.md) and the
[Conversation channel API](./conversation-channel-api.md).

---

## Storage layout

Signal sessions are persisted under the resolved durable state root (see
[State-root resolution](./state-root-resolution.md)) in a dedicated
`signal_sessions/` subdirectory. Each session file is the Signal-side analogue of
the dashboard-chat store's session file (`chat_sessions/`, see
[Dashboard Chat](./dashboard-chat.md)), written through the **same** crash-durable
persistence pipeline — no second transcript format. The Signal envelope is a small
flat record (it carries the owning `operator` and `session_id` inline rather than a
`meta`/`title` block), and the per-operator index (`operators.json`) is a **new**
shape, distinct from the dashboard's session index (`index.json` /
`ChatSessionIndex`):

```text
<state_root>/signal_sessions/
├── operators.json                 # operator identity → active session_id (the index)
├── 018f3c9a-…-….json              # full, uncapped turn history for one session
└── …
```

- `<state_root>` resolves via [`crate::state_root::simard_state_root`] —
  `$SIMARD_STATE_ROOT` when set, else `~/.simard`. Tests thread an explicit
  temp-dir root through the `_at(state_root)` cores; nothing is written to a real
  `~/.simard` under test.
- Each `<session_id>.json` holds schema-versioned metadata plus the **complete,
  uncapped** history (independent of the in-memory `MeetingBackend`
  `MAX_HISTORY = 500` working-set cap), so a restart restores the whole
  conversation.
- Every write goes through [`crate::persistence::persist_json`] — the same
  crash-durable atomic pipeline the chat store uses, which chmods each file to
  `0o600`; the `signal_sessions/` directory is chmod'd to `0o700` on first use.
- Every `session_id` that reaches a filesystem path is validated against
  `^[A-Za-z0-9_-]{1,64}$` **before any path join** by `validate_session_id`,
  promoted into the shared crate-internal `crate::session_id` module so the Signal
  store and the dashboard chat store call the **same** guard (see
  [Public API](#public-api)). A UUIDv7 always satisfies it.

### `operators.json`

The index is the one file keyed by operator identity: a single object mapping each
operator's Signal address to its active `session_id`. The E.164 lives here as a
JSON **key**, never as a filename:

```json
{
  "schema_version": 1,
  "operators": {
    "+12062591306": "018f3c9a-7c2a-7e31-9b4d-2f1a6b8e0c44"
  }
}
```

The key is the raw allowlisted Signal address; the value is the UUIDv7 of that
operator's currently-active session. `set_active_session` upserts the mapping (on
first contact and on `/new`); the map overwrites rather than appends, so an
operator always has exactly one active session id.

> **Design decision (locked).** The session is keyed by a **UUIDv7 filename plus
> the `operators.json` index**, not by a deterministic per-sender key (an earlier
> design sketch normalized `+15551234` → `signal-15551234` and used it directly as
> the filename). The index is what lets `/new` mint a fresh `session_id` while
> **retaining** the operator's previous session file on disk — a fixed,
> sender-derived filename could only overwrite or collide. The E.164 is a lookup
> key in the index, never a path component.

### `<session_id>.json`

A flat record: schema version, the owning operator + session id, first/last-turn
timestamps, and the complete uncapped turn history.

```json
{
  "schema_version": 1,
  "operator": "+12062591306",
  "session_id": "018f3c9a-7c2a-7e31-9b4d-2f1a6b8e0c44",
  "created_at": "2026-07-05T15:39:58.001Z",
  "updated_at": "2026-07-05T15:41:02.184Z",
  "history": [
    { "role": "user",      "content": "Walk me through the deploy checklist.", "timestamp": "2026-07-05T15:39:58.001Z" },
    { "role": "assistant", "content": "Sure — first, …",                        "timestamp": "2026-07-05T15:40:01.522Z" }
  ]
}
```

`created_at` is set once on the first turn; `updated_at` advances on every appended
turn. The `operator` field is stamped from the index when the session is first
created (best-effort; empty only when a session file is written directly in a store
unit test without an index entry).

---

## Session lifecycle

The run loop keeps an in-process registry and resolves each accepted inbound to
exactly one operator session:

```text
inbound (allowlisted operator +E.164)
        │
        ▼
registry.get(operator) ── hit ─▶ reuse the live MeetingBackend (continuity)
        │ miss
        ▼
operators.json → active session_id?
        │ yes                          │ no
        ▼                              ▼
load_session_at(session_id)       mint session_id = new_session_id()
        │                          set_active_session(operator, id)
        ▼                              │
restore(history) into a fresh          ▼
MeetingBackend  ── resume ─▶      fresh MeetingBackend ── create
        │                              │
        └───────────────┬──────────────┘
                        ▼
               append user turn  ─▶ backend.send_message(text) ─▶ append assistant turn
                        │                    (append_turn_at persists each)
                        ▼
                     reply to operator over Signal
```

1. **Continuity (default).** Back-to-back messages from the same operator hit the
   registry and reuse the same live backend, so the second turn sees the first
   turn's content. Nothing about a new message starts a new session.
2. **Isolation.** A different operator address resolves to its own entry in the
   registry and its own `session_id` — histories never cross.
3. **Persistence.** Each accepted conversation turn appends the `user` message
   and then Simard's `assistant` reply to `<session_id>.json` via
   `append_turn_at`. The durable file is the source of truth; the in-memory
   working set stays `MAX_HISTORY`-capped.
4. **Resume across restart.** On the first message after a restart the registry is
   empty, so the loop reads `operators.json`, loads the active session, and
   `restore`s its history into a fresh backend before running the new turn.

### Lifecycle commands

Only **`/new`** (alias `/reset`) is new in this feature. The continuous run loop
(`run_continuous`) is now the **sole** Signal meeting driver — it replaces the
single-session `conversation_channel::run_conversation` on this channel — so all
three lifecycle words are matched by a small **run-loop pre-check** that runs
*before* the turn reaches the reasoner. The existing Signal classifier
`parse_inbound` (`src/signal_conversation/gating.rs`) is **unchanged** — it still
resolves only `status` / `pause` / `approve` / `deploy` / `merge #NNNN` /
conversation and knows nothing of the lifecycle words; `/new`, `/reset`, `/help`,
and `/close` are recognized by the run-loop pre-check on the accepted inbound text:

| Text (from an allowlisted operator) | New? | Action |
|-------------------------------------|------|--------|
| `/new` (alias `/reset`) | **new** | Start a **fresh** conversation for this operator: mint a new `session_id`, point `operators.json` at it, and drop the operator's in-memory backend. The prior session file is left on disk (history is detached, not deleted). The next message begins a brand-new session. Handled entirely in the run loop. |
| `/help` | pre-existing word | The pre-check intercepts `/help` and replies with a **Signal-specific banner** that also advertises `/new`. Because the interception happens in the run loop, this deliberately **supersedes** the generic meeting `/help` banner **on the Signal channel** — a conscious, documented replacement, not a silent shadow. Never persisted as a turn; never resets. |
| `/close` | pre-existing word | The pre-check closes the operator's live backend via `MeetingBackend::close` — writing the handoff bundle, **consolidating the conversation into graph cognitive memory** (episodes, summary facts), and carrying decisions onto the goal board exactly as on every channel — replies with the closing summary, and rotates the operator onto a fresh `session_id` so the **next** message begins a new conversation. |

So the run-loop pre-check owns exactly three words — `/new` (`/reset`), `/help`, and
`/close`; everything else (the lightweight commands and ordinary turns) continues
through the existing classifier untouched. An **idle-timeout** that rolls to a fresh
conversation after a long gap is an explicit **non-goal** (see
[Non-goals](#non-goals)); the default for back-to-back messages is always
continuous. Context is never silently dropped.

---

## Commands over Signal (full vocabulary)

The lifecycle commands sit alongside the existing lightweight command vocabulary.
Everything not listed is an ordinary meeting turn, answered conversationally with
full session context.

| Text | Action | Class |
|------|--------|-------|
| `status` | Daemon health + pause state | low-risk |
| `pause` | Pause autonomous dispatch | low-risk |
| `approve` | Record sign-off and run the pending high-risk request | low-risk |
| `/help` | Show the command banner | read-only |
| `/new` · `/reset` | Start a fresh conversation (reset context) | read-only lifecycle |
| `/close` | Close the meeting, write the handoff | lifecycle |
| `deploy` | Request a deploy → pending sign-off | **high-risk** |
| `merge #NNNN` | Merge PR #NNNN via the gated authority → pending sign-off | **high-risk** |
| *anything else* | Ordinary meeting turn, answered with full prior-turn context | conversation |

The `/help` banner Simard replies with:

```text
[simard] Commands:
  /help          — show this help
  /new  (/reset) — start a fresh conversation (clears prior context)
  /close         — end this conversation and write the handoff
  status | pause | approve | deploy | merge #NNNN — operator commands
Anything else is a message in our ongoing conversation.
```

---

## Public API

### Session store — `src/signal_conversation/session_store.rs`

The store follows the dashboard-chat store's `_at(state_root)` cores (the
`goals.rs` convention): callers thread a trusted-internal state root through
rather than resolving `SIMARD_STATE_ROOT` ambiently. It reuses the shared
`ConversationMessage` type, the promoted `crate::session_id` id guard and UUIDv7
generator (see [Shared id helpers](#shared-id-helpers-promotion) below), and
`crate::persistence::persist_json` — no format, guard, or durability logic is
duplicated.

```rust
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::meeting_backend::ConversationMessage;
use crate::error::SimardResult;

/// One durable Signal session: metadata plus the complete, uncapped turn history.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignalSession {
    pub schema_version: u32,
    pub operator: String,     // owning operator (E.164), best-effort from the index
    pub session_id: String,   // stable UUIDv7, validated against the traversal guard
    pub created_at: String,   // RFC3339, set once
    pub updated_at: String,   // RFC3339, advances every turn
    pub history: Vec<ConversationMessage>,
}

/// Metadata for one stored session, surfaced by `list_sessions_at`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignalSessionMeta {
    pub session_id: String,
    pub operator: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The operator's active session id from `operators.json`, or `None` when the
/// operator has no session yet.
pub fn active_session_for(state_root: &Path, operator: &str) -> SimardResult<Option<String>>;

/// Point `operators.json` at `session_id` for `operator` (upsert). Serialized by
/// a process-global index lock, exactly like the chat store's index upsert.
/// Validates `session_id` against the traversal guard before recording it.
pub fn set_active_session(state_root: &Path, operator: &str, session_id: &str) -> SimardResult<()>;

/// The full, uncapped persisted session for a `session_id`, or `None` if absent.
/// Errors (before any path join) when the id fails the traversal guard.
pub fn load_session_at(state_root: &Path, session_id: &str) -> SimardResult<Option<SignalSession>>;

/// Append one turn to `<session_id>.json` (creating the file lazily on the first
/// turn). Validates the id first, then persists the full uncapped history.
pub fn append_turn_at(state_root: &Path, session_id: &str, message: &ConversationMessage) -> SimardResult<()>;

/// Every stored session's metadata, newest-first. `Ok(vec![])` for an empty or
/// missing store (never an error for "nothing persisted yet").
pub fn list_sessions_at(state_root: &Path) -> SimardResult<Vec<SignalSessionMeta>>;
```

#### Shared id helpers (promotion)

`validate_session_id(id)` (the `^[A-Za-z0-9_-]{1,64}$` path-traversal guard) and
`new_session_id()` (a UUIDv7 generator) exist today as `pub fn`s **inside the
private `operator_commands_dashboard::chat_store` module** — both enclosing
modules (`mod operator_commands_dashboard`, `mod chat_store`) are private, so the
functions are not reachable from `signal_conversation`. This feature **promotes**
the two helpers into a shared crate-internal module, `crate::session_id`, and
re-points the dashboard chat store at it. That gives the security-critical
traversal guard exactly **one** implementation; the Signal session store imports
it rather than re-declaring the regex or the generator. The store itself stays
signal-local — only these two small, pure, channel-agnostic helpers are promoted,
which is why the section header above names a signal-local `session_store.rs`.

### Per-operator registry — `src/signal_conversation/channel.rs`

`signal_conversation::run` no longer builds a single global backend. It calls
`run_continuous`, which keeps a per-operator registry (plus a parallel
operator → active-`session_id` map) and resolves each accepted turn through the
store. `run_continuous` takes the state root and a `make_backend(operator)` factory
explicitly, so tests inject a fake backend and a temp-dir root:

```rust
// Illustrative shape of the continuous run loop (see channel.rs::run_continuous).
let mut backends: HashMap<String /* operator */, MeetingBackend> = HashMap::new();
let mut sessions: HashMap<String /* operator */, String /* session_id */> = HashMap::new();

while let Some(inbound) = channel.recv().await? {
    // `operator` is the channel-abstraction identity `OperatorRef.id`, which the
    // Signal channel populates from the raw signal-cli `parsed.sender` (E.164) —
    // the same operator, one layer up from the wire envelope.
    let operator = inbound.from.id.clone();

    // Lifecycle commands are handled before the turn reaches the reasoner.
    match lifecycle_command(&inbound.text) {
        Some(Lifecycle::Reset) => {
            let new_sid = session_id::new_session_id();
            session_store::set_active_session(state_root, &operator, &new_sid)?;
            backends.remove(&operator);
            sessions.remove(&operator);
            tracing::info!(target: "signal", operator = %redact_operator(&operator), session_id = %new_sid, "session.reset");
            channel.send(Outbound { kind: OutKind::Status, text: "[simard] Started a new conversation.".into() }).await?;
            continue;
        }
        Some(Lifecycle::Help)  => { channel.send(Outbound { kind: OutKind::Status, text: help_banner() }).await?; continue; }
        Some(Lifecycle::Close) => { /* MeetingBackend::close, reply summary, rotate to a fresh session */ continue; }
        None => {}
    }

    // Ensure a live backend for this operator, resuming persisted history on the
    // first touch this process (continuity across restart).
    if !backends.contains_key(&operator) {
        let sid = match session_store::active_session_for(state_root, &operator)? {
            Some(existing) => existing,                       // resume
            None => { let s = session_id::new_session_id();   // first contact
                      session_store::set_active_session(state_root, &operator, &s)?; s }
        };
        let mut backend = make_backend(&operator);
        if let Some(sess) = session_store::load_session_at(state_root, &sid)? {
            backend.restore(sess.history);                    // replay full prior context
        }
        backends.insert(operator.clone(), backend);
        sessions.insert(operator.clone(), sid);
    }

    // One turn: run it, then persist the newly-appended user+assistant pair from
    // the backend's history tail (cap-independent; on failure nothing is persisted).
    let sid = sessions[&operator].clone();
    let backend = backends.get_mut(&operator).unwrap();
    match backend.send_message(&inbound.text) {
        Ok(resp) => {
            let hist = backend.history();
            for msg in &hist[hist.len().saturating_sub(2)..] {
                session_store::append_turn_at(state_root, &sid, msg)?;
            }
            channel.send(Outbound { kind: OutKind::Assistant, text: resp.content }).await?;
        }
        Err(e) => channel.send(Outbound { kind: OutKind::Error, text: format!("[error: {e}]") }).await?,
    }
}
```

On a registry miss the loop loads `active_session_for(operator)` → `load_session_at`
→ `MeetingBackend::restore` to **resume**, or mints a session id
(`new_session_id()`) plus a fresh backend to **create**. `MeetingBackend::restore`
seeds the most-recent `MAX_HISTORY` turns into the live working set; the durable
store keeps the full history. Each successful turn appends the pair
`MeetingBackend::send_message` just recorded (the history tail) to
`<session_id>.json`, so the eviction-at-the-front working-set cap never drops a
newly-persisted turn.

### Injected seams (tests)

Continuity is testable with **no network**. `run_continuous` takes injectable seams
so the tests drive it deterministically:

- **Transport** — a fake `SignalTransport` (`MockTransport`) feeds canned inbound
  signal-cli JSON-RPC lines and records outbound replies.
- **Backend factory** — `make_backend(operator)` returns a fake meeting backend
  (`RecordingAgent`) that records the accumulated prompt preamble it receives per
  turn, so a test can assert the second turn saw the first turn's content.
- **State root** — a per-test temp directory, so persistence and restart-resume are
  exercised without touching `~/.simard`.

Timestamps come from the backend (`Utc::now`) as each `ConversationMessage` is
recorded; the continuity assertions check message **content** (accumulated history),
not byte-stable timestamps, so no clock injection is required.

---

## Tracing

Session lifecycle is observable through **structured tracing** on the
`target: "signal"` — never `println!`/`eprintln!` beyond the existing
operator-facing `[simard] …` reply convention.

| Event (`target: "signal"`) | Level | Fields | When |
|----------------------------|-------|--------|------|
| `session.create` | INFO | `operator`, `session_id` | A first-ever session is minted for an operator. |
| `session.resume` | INFO | `operator`, `session_id`, `turns` | A persisted session is loaded and replayed into the backend after a cold registry (e.g. after a restart). |
| `session.append` | DEBUG | `session_id`, `role`, `turns` | A turn is persisted. |
| `session.reset` | INFO | `operator`, `session_id` (the **new** id) | `/new` / `/reset` rolls the operator to a fresh session. |
| `session.close` | INFO | `operator` | `/close` closes the operator's backend and rotates to a fresh session. |

Message **bodies are never logged** (consistent with the channel's existing
"content is delivered, never logged" rule), and the operator address is emitted
through a redaction helper rather than as a raw E.164, matching the channel's
"non-allowlisted sender" logging that omits the number.

---

## Configuration

Continuity adds **no new required configuration**. The `[signal]` table is
unchanged (see the [Signal channel reference](./signal-conversation.md#configuration)):
`endpoint`, `account`, `allowlist`, `read_only_unknown`, and the optional
`own_device_id`.

- **State root.** The session store lives under `signal_sessions/` at the resolved
  durable state root — `$SIMARD_STATE_ROOT` when set, else `~/.simard`. No Signal
  session data is written anywhere else.
- **Working-set cap.** The live backend keeps the most-recent `MAX_HISTORY = 500`
  turns in memory; the durable `<session_id>.json` keeps the full, uncapped
  history. There is no operator-tunable cap.
- **Idle-timeout.** Not enabled by default and not configurable; the default is
  continuous (see [Non-goals](#non-goals)).

### Activating the change after merge

The live daemon is **not** touched by this change and no redeploy happens during
merge. After the PR merges, the operator redeploys the supervised unit to pick up
the new binary:

```bash
# On the operator host, after merge — activates continuous conversation:
sudo systemctl restart simard-signal.service
```

On restart, the **first** message from an operator with an existing
`operators.json` entry **resumes** that conversation; it does not start over.

---

## Guardrails preserved

Continuity changes only how an **already-accepted** inbound is routed to a
session. Every inbound guardrail from the [Signal channel reference](./signal-conversation.md#guardrails)
is unchanged and still runs first:

- **Allowlist (fail-closed).** A non-allowlisted sender is dropped before any
  session is created or loaded — an unknown number can never open or resume a
  session.
- **Identity binding.** The session is keyed by the authorized sender's address;
  replies and persisted turns are bound to that identity.
- **High-risk gating.** `deploy` / `merge #NNNN` still create a pending sign-off
  and never auto-execute — they are not conversation turns and never enter the
  session history as executed actions.
- **Note-to-Self loop prevention (single-number linked device).** The sync-sent
  acceptance predicate in `recv()` remains the **sole** inbound filter: a
  sync-sent message from signal-cli's own device id, a third-party destination, or
  an echo of a recent Simard outbound is ignored **before** continuity runs. So
  Simard's own outbound reply is never re-consumed as a new turn — continuity does
  not reintroduce a self-reply loop. See
  [Note to Self (sync-sent) and loop prevention](./signal-conversation.md#note-to-self-sync-sent-and-loop-prevention).

---

## Testing

All tests run under the default `cargo test` (the `signal` feature is on by
default) against a fake transport, a fake backend factory, and a temp-dir state
root — **no live signal-cli or network**:

- **Continuity (same operator).** Two sequential messages from the same operator
  address share **one** session: the fake backend proves the second turn receives
  the accumulated history (the first turn's content) and the same `session_id`;
  the store shows both turns' user+assistant messages appended to a single
  `<session_id>.json`.
- **Isolation (different operators).** A message from a different operator address
  gets its **own** `session_id` and history; the two operators' `<session_id>.json`
  files and `operators.json` entries never share turns.
- **`/new` (reset).** `/new` (and its `/reset` alias) mints a fresh `session_id`
  for that operator, re-points `operators.json`, and the next turn starts with an
  empty history — the prior session file remains on disk.
- **Persist + resume across restart.** After turns are appended, a **simulated
  restart** (a fresh run loop over the same state root, empty registry) resumes the
  operator's conversation: the persisted history is replayed into the backend
  (`session.resume`) and the next turn sees the full prior context. It does **not**
  start over.
- **Loop prevention holds under continuity.** A sync-sent echo of a recent Simard
  outbound (and a sync-sent message from the own device id / a third-party
  destination) is ignored and never appended as a new turn — the linked-device
  guard still closes the loop.
- **Path-traversal guard.** A hostile `session_id` is rejected by
  `validate_session_id` before any path join (shared guard with the chat store).

---

## Non-goals

- **Idle-timeout rollover.** Rolling to a fresh conversation after a long idle gap
  is intentionally out of scope; the default for back-to-back messages is
  continuous. No clock is injected to expire sessions.
- **Cross-operator or cross-channel session sharing.** Sessions are strictly
  per-operator and per-channel; a Signal session is not shared with the dashboard
  chat or CLI meeting.
- **Group-chat keying.** Continuity keys on a single operator's direct address /
  Note-to-Self; group conversations are not a target.
- **A parallel datastore.** The store reuses the dashboard-chat session-file
  envelope and the shared persistence pipeline; it does not introduce a second,
  divergent transcript format. The per-operator `operators.json` index is a thin
  new lookup file (operator → active id), not a parallel conversation store.

---

## Related reading

- [Signal channel reference](./signal-conversation.md) — the channel, guardrails,
  wire protocol, and lightweight command vocabulary this feature extends.
- [Dashboard Chat reference](./dashboard-chat.md) — the durable, resumable session
  store this feature mirrors.
- [Meeting Backend API](./meeting-backend-api.md) — `new_session`, `send_message`,
  `restore`, `history`, and `MAX_HISTORY`.
- [Conversation channel API](./conversation-channel-api.md) and
  [Conversation channels (architecture)](../architecture/conversation-channel.md) —
  the one channel abstraction and the shared meeting driver.
- [State-root resolution](./state-root-resolution.md) — where `signal_sessions/`
  lives and how `SIMARD_STATE_ROOT` is honored.
- [How to set up the Signal channel](../howto/set-up-the-signal-channel.md) — link
  a device, configure `[signal]`, and verify the round trip.
