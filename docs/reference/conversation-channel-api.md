---
title: Conversation channel API reference
description: Rust API reference for the ConversationChannel trait, its message types (OperatorRef, Inbound, Outbound, OutKind), the shared apply_record dispatcher, the run_conversation driver, and MockConversationChannel.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#2527"]
related:
  - ../index.md
  - ../architecture/conversation-channel.md
  - ./meeting-backend-api.md
  - ./signal-conversation.md
  - ../howto/start-a-meeting.md
---

# Conversation channel API reference

The `conversation_channel` module (`src/conversation_channel/`) defines the one
operator↔Simard conversation abstraction and the shared machinery every meeting
frontend uses. It is `pub`-exported from `src/lib.rs` as
`pub mod conversation_channel;`.

Nothing in this module is named `bridge`/`Bridge`. `ConversationChannel` is a
first-class chat abstraction and is unrelated to the pre-existing cognitive-memory
`BridgeTransport`.

## Message types

`src/conversation_channel/mod.rs`

### `OperatorRef`

Identifies the sender of an inbound line and records whether it has cleared the
channel's authorization check (allowlist / identity / dashboard auth).

```rust
/// Who sent an inbound message. Used for the allowlist + identity binding.
#[derive(Clone, Debug)]
pub struct OperatorRef {
    /// Channel-native id: terminal user, dashboard session id, or Signal E.164.
    pub id: String,
    /// True once this ref has cleared the channel's allowlist/identity check.
    pub authorized: bool,
}
```

For the local CLI and dashboard frontends the operator is already authorized (the
CLI keeps its own loop; the dashboard has auth middleware). For a driver-based
channel, `authorized` is set by the channel after its own check — for
`SignalConversation` it reflects the E.164 allowlist result.

### `Inbound`

One operator-originated line, already trimmed of surrounding whitespace.

```rust
/// One operator-originated message line.
#[derive(Clone, Debug)]
pub struct Inbound {
    pub from: OperatorRef,
    pub text: String,
}
```

### `Outbound` and `OutKind`

A Simard-originated message, render-agnostic. The channel's `send` implementation
maps `OutKind` to its own presentation (ANSI color, JSON role, or a Signal send).

```rust
/// A Simard-originated message to deliver on the channel.
#[derive(Clone, Debug)]
pub struct Outbound {
    pub kind: OutKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutKind {
    /// Simard's conversational LLM reply.
    Assistant,
    /// A structured-capture acknowledgement ("Decision recorded: …", etc.).
    Recorded,
    /// Read-only session output (/status, /state, /recap, /preview, /help).
    Status,
    /// A notification-out: PR merge-ready, stall/problem, high-risk sign-off request.
    Notice,
    /// An error message.
    Error,
}
```

## The trait

```rust
/// A bidirectional operator↔Simard conversation channel. The meeting IS the
/// conversation; each implementation is one channel over the shared
/// `MeetingBackend`. Native async via return-position `impl Future` (no
/// `async-trait` dependency). Used with static dispatch — never as `dyn`.
pub trait ConversationChannel {
    /// Stable id for logs/metrics: `"cli"`, `"dashboard"`, or `"signal"`.
    fn name(&self) -> &'static str;

    /// Await the next authorized operator line.
    ///
    /// `Ok(None)` ends the session (EOF, socket closed, or operator quit).
    /// Implementations MUST NOT yield an inbound whose `from.authorized` is
    /// `false`; unauthorized input is dropped inside the channel before it
    /// reaches the driver.
    fn recv(&mut self)
        -> impl std::future::Future<Output = SimardResult<Option<Inbound>>> + Send;

    /// Deliver one Simard message OUT on this channel.
    fn send(&mut self, out: Outbound)
        -> impl std::future::Future<Output = SimardResult<()>> + Send;

    /// Per-channel hook fired after a record command is applied. The default is
    /// a no-op; `CliConversation` overrides it to emit `checkpoint_wip` and the
    /// live capture tally. Dashboard and Signal keep the default so behavior is
    /// preserved exactly.
    fn on_recorded(&mut self, _backend: &MeetingBackend)
        -> impl std::future::Future<Output = SimardResult<()>> + Send
    { async { Ok(()) } }
}
```

### Contract notes

- **Authorization is the channel's job.** The driver assumes every `Inbound`
  returned by `recv` is authorized. A channel that gates senders (Signal) filters
  before returning.
- **`send` is total.** A channel renders every `OutKind`; it may style them
  differently but must not drop any.
- **`on_recorded` fires only after a capture.** The driver calls it only for an
  `OutKind::Recorded` outbound, never for a read-only status view. A channel with
  post-record effects overrides it; the mock counts its calls. (The CLI REPL keeps
  its `checkpoint_wip` + capture tally inline in its own loop, so unifying the record
  applier does not move the CLI tally onto other channels.)

## Shared record dispatcher — `apply_record`

`src/conversation_channel/dispatch.rs`

Pure, synchronous. This is the duplicated per-channel record logic extracted once —
**both** the CLI REPL and the dashboard chat loop call it for the eight capture
commands. It performs the backend mutation and returns the canonical message; it
does **not** render and does **not** perform any I/O or LLM turn. Commands that call
`send_message`/`close`, write files, or fire an LLM turn (`Conversation`, `Close`,
`Export`, and the context-injection turn of `Template`) are **driver-handled** and
are *not* pure `apply_record` arms.

```rust
/// The canonical result of a record/status command: message text + kind.
/// Rendering is left to the channel; the text is identical across channels.
pub struct Recorded {
    pub kind: OutKind,
    pub text: String,
}

/// Apply a record/status command's backend mutation and return its canonical
/// message. Returns `None` for commands the driver handles directly
/// (`Conversation`, `Close`).
pub fn apply_record(backend: &mut MeetingBackend, cmd: &MeetingCommand)
    -> Option<Recorded>;
```

Mapping (every [`MeetingCommand`](./meeting-backend-api.md) variant is accounted
for — pure arms return `Some(Recorded)`; I/O-bearing commands return `None` and are
driver-handled):

| `MeetingCommand` | Effect | `Recorded` |
|------------------|--------|------------|
| `Decision { text, rationale }` | `backend.push_explicit_decision(text, rationale)` | `Recorded`, `"Decision recorded: …"` |
| `Action(t)` | `backend.push_explicit_action_item(t)` | `Recorded`, `"Action recorded: …"` |
| `Question(t)` | `backend.push_explicit_question(t)` | `Recorded`, `"Question recorded: …"` |
| `Risk(t)` | `backend.push_explicit_risk(t)` | `Recorded`, `"Risk recorded: …"` |
| `Disagree(t)` | `backend.push_explicit_disagreement(t)` | `Recorded`, `"Disagreement recorded: …"` |
| `Theme(t)` | `backend.push_theme(t)` | `Recorded`, `"Theme recorded: …"` |
| `Owner(n)` | `backend.push_next_owner(n)` | `Recorded`, `"Next owner recorded: …"` |
| `Goal(t)` | `backend.set_goal(t)` | `Recorded`, `"Goal recorded: …"` |
| `Status` / `State` / `Recap` / `Preview` / `Help` | read backend state | `Status`, a canonical rendering used by the driver-based channels (the CLI and dashboard keep their own richer rendering of these views) |
| `Unknown { .. }` | none | `Status`, the existing suggestion text |
| `Conversation(_)` / `Close` | — | `None` (driver-handled — `send_message`/`close`) |
| `Export` | writes a markdown export (`persist::write_markdown_export`) | `None` (driver-handled — file I/O) |
| `Template(n)` | `find_template(n)` → `apply_template(name, agenda)` **then** an LLM context-injection turn | `None` (driver-handled — pure lookup+apply, then `send_message`; empty/unknown name lists templates or errors) |

The exact acknowledgement strings for the eight capture commands are the ones the
CLI and dashboard already produced; they are not changed by the extraction.
`Export` and `Template` are driver-handled because they perform file I/O or an LLM
turn and so cannot be part of the pure, synchronous `apply_record`.

## The driver — `run_conversation`

`src/conversation_channel/driver.rs`

One loop over the trait, realized by the [`SignalConversation`](./signal-conversation.md)
channel and by `MockConversationChannel` (and reusable by any future channel). The
CLI REPL and dashboard keep their own presentation loops but share the record path
via `apply_record`.

```rust
/// Drive one operator↔Simard conversation to completion over `channel`, using
/// `backend` as the (synchronous) meeting engine. Returns when the operator
/// closes the meeting or the channel reaches end-of-stream.
pub async fn run_conversation<C: ConversationChannel>(
    channel: &mut C,
    backend: &mut MeetingBackend,
) -> SimardResult<()>;
```

Behavior:

```text
while let Some(inbound) = channel.recv().await? {
    match parse_command(&inbound.text) {
        Close          => { backend.close(); send(summary); break }
        Conversation t => { let r = backend.send_message(t);
                            send(Outbound{ Assistant, r.content }) }
        Export         => { let p = write_markdown_export(..);
                            send(Outbound{ Status, "Exported to …" }) }
        Template n     => { apply_template(backend, n);          // sync
                            let r = backend.send_message(ctx);   // LLM turn
                            send(Outbound{ Assistant, r.content }) }
        other          => if let Some(rec) = apply_record(backend, &other) {
                            let is_record = rec.kind == OutKind::Recorded;
                            send(Outbound{ rec.kind, rec.text });
                            if is_record { channel.on_recorded(backend).await?; }
                          }
    }
}
```

`Export` and `Template` are handled by the driver (not `apply_record`) because they
perform file I/O and/or an LLM turn. The engine (`MeetingBackend`) is synchronous;
each call completes *before* the next `.await`, so no `&mut MeetingBackend` is held
across an await point and the driver needs no `spawn_blocking`.

## Test double — `MockConversationChannel`

`src/conversation_channel/mock.rs`

```rust
/// A scripted ConversationChannel for driver + integration tests.
pub struct MockConversationChannel { /* … */ }

impl MockConversationChannel {
    /// Build from a script of inbound lines; each `recv()` yields the next,
    /// then `Ok(None)`.
    pub fn with_script(lines: Vec<&str>) -> Self;

    /// All Outbound messages captured by `send()`, in order.
    pub fn sent(&self) -> &[Outbound];

    /// Count of `on_recorded` invocations (asserts the hook fires per record).
    pub fn recorded_hook_calls(&self) -> usize;
}

impl ConversationChannel for MockConversationChannel { /* … */ }
```

Typical use:

```rust
let mut ch = MockConversationChannel::with_script(vec![
    "/goal ship the signal channel",
    "/decision use signal-cli JSON-RPC --rationale no embedded protocol",
    "let's wrap up",
    "/close",
]);
let mut backend = MeetingBackend::new_for_test("signal channel");
run_conversation(&mut ch, &mut backend).await.unwrap();

assert!(ch.sent().iter().any(|o| o.kind == OutKind::Recorded
    && o.text.contains("Goal recorded")));
assert_eq!(ch.recorded_hook_calls(), 2); // /goal + /decision
```

## Usage — implementing a channel

A channel provides transport-specific `recv`/`send` (and, if it has post-record
effects, `on_recorded`); all meeting logic comes from the driver + engine. The
worked example is [`SignalConversation`](./signal-conversation.md); its shape:

```rust
struct MyChannel<T> { transport: T /* … */ }

impl<T: Transport + Send> ConversationChannel for MyChannel<T> {
    fn name(&self) -> &'static str { "my-channel" }

    fn recv(&mut self)
        -> impl std::future::Future<Output = SimardResult<Option<Inbound>>> + Send
    { async move {
        // read the next authorized operator line from the transport;
        // end-of-stream → Ok(None); else Ok(Some(Inbound {
        //   from: OperatorRef { id: <sender>, authorized: true }, text }))
    }}

    fn send(&mut self, out: Outbound)
        -> impl std::future::Future<Output = SimardResult<()>> + Send
    { async move {
        // render out.kind + out.text onto the transport
    }}

    // on_recorded defaults to a no-op; override only for post-record effects.
}
```

`SignalConversation` follows this shape over a signal-cli JSON-RPC transport; its
`recv` additionally applies the allowlist and gates lightweight operator commands
before yielding a meeting turn. `on_recorded` stays the default no-op for it. The
CLI REPL and dashboard chat are **not** separate trait impls — they keep their own
loops and share only the record path via `apply_record`.

## Related reading

- [Conversation channels (architecture)](../architecture/conversation-channel.md)
- [Signal channel reference](./signal-conversation.md)
- [Meeting backend API reference](./meeting-backend-api.md) — the sync engine the
  channels drive, including the `push_explicit_*` / `set_goal` setters used by
  `apply_record`.
- [How to start a meeting with Simard](../howto/start-a-meeting.md)
