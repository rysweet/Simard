---
title: Conversation channels
description: One clearly-named operator↔Simard conversation abstraction — the ConversationChannel trait — with the CLI/TUI meeting REPL, the dashboard WebSocket chat, and Signal all implemented as thin channels over the same unified meeting engine.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: architecture
issues: ["#2527"]
related:
  - ../index.md
  - ./unified-meeting-backend.md
  - ../reference/conversation-channel-api.md
  - ../reference/signal-conversation.md
  - ../reference/meeting-backend-api.md
  - ../howto/start-a-meeting.md
  - ../howto/set-up-the-signal-channel.md
  - ../concepts/operational-autonomy-model.md
---

# Conversation channels

A **meeting is a conversation.** Every operator↔Simard meeting — whether it
happens in a terminal, in the dashboard chat pane, or over Signal — is the same
bidirectional conversational session: an operator sends an inbound line, Simard
delivers outbound replies and notices, the session opens and closes, and the
structured capture commands (`/goal`, `/decision`, `/action`, `/question`, …)
feed the meeting handoff and goal-carryover chain.

`ConversationChannel` is the single, clearly-named abstraction for that session.
Each frontend routes its structured-capture dispatch through the same shared
applier over the one meeting engine
([`MeetingBackend`](../reference/meeting-backend-api.md)):

| Channel | How it uses the abstraction | Transport |
|---------|-----------------------------|-----------|
| CLI / TUI meeting REPL | delegates record/capture dispatch to the shared `apply_record` | stdin / stdout (ANSI) |
| Dashboard chat | delegates record/capture dispatch to the shared `apply_record` | WebSocket JSON frames |
| Signal | full `SignalConversation` impl driven by `run_conversation` | signal-cli JSON-RPC (feature-gated) |
| Tests | full `MockConversationChannel` impl driven by `run_conversation` | scripted in-memory |

> **Naming.** This abstraction is deliberately **not** called a "bridge." It is a
> first-class chat/conversation abstraction. No new type, module, field, trait,
> or feature introduced here contains `bridge`/`Bridge`. `SignalConversation`
> does **not** implement the pre-existing memory `RpcTransport` trait — that
> trait is the cognitive-memory transport and is out of scope here. The
> pre-existing memory / knowledge / gym RPC-client modules are untouched.

## Problem

Before this abstraction, the two meeting frontends shared the engine but
duplicated the *dispatch loop* around it:

- **CLI/TUI REPL** (`src/meeting_repl/repl.rs`) — a blocking stdin/stdout loop
  with its own `match parse_command(...)` arms, ANSI-colorized rendering, and
  CLI-only post-record effects (`checkpoint_wip`, the live capture tally).
- **Dashboard chat** (`src/operator_commands_dashboard/chat.rs`) — an async axum
  WebSocket loop with a *second* `match parse_command(...)` that applied the same
  backend mutations and produced the same message text, but rendered them as
  `{"role":"system"}` JSON frames and wrapped the sync engine calls in
  `tokio::task::spawn_blocking`.

The backend mutation and the canonical message text were **identical** across the
two loops; only rendering and the CLI-only post-effects differed. Divergence
between the two copies was a standing risk, and there was no third channel path
at all — an operator away from the terminal or dashboard had no way to command
Simard or receive her notifications.

## Solution

### One trait, one shared applier, one driver

`ConversationChannel` captures exactly the conversational session the meeting REPL
already embodies:

- **receive** an inbound operator message (`recv`),
- **deliver** Simard's outbound messages/replies/notices (`send`),
- **session lifecycle** (open by constructing the channel + a `MeetingBackend`;
  close via the `/close` command through the shared driver),
- **structured meeting capture** — `/goal /decision /action /question` … routed
  through one shared applier into the existing handoff + goal-carryover chain.

```
                    ┌───────────────────────────┐
   apply_record ───▶│  the ONE shared, pure      │◀─── delegated to by BOTH the
   (record/capture) │  record/capture applier    │     CLI REPL and dashboard loops
                    └───────────────────────────┘

        recv() ──▶ ┌───────────────────────────┐
 operator          │  run_conversation<C>()    │      ┌──────────────────┐
        ◀── send() │  (the unified driver)     │─────▶│  MeetingBackend  │
                   │   parse_command           │ sync │  (unchanged)     │
                   │   apply_record (shared)   │◀─────│  send_message()  │
                   │   on_recorded (per-chan)  │      │  close()         │
                   └───────────────────────────┘      └──────────────────┘
                     realized by:  SignalConversation · MockConversationChannel
```

- **`ConversationChannel`** (`src/conversation_channel/mod.rs`) — the trait plus
  the render-agnostic message types `OperatorRef`, `Inbound`, `Outbound`,
  `OutKind`.
- **`apply_record`** (`.../dispatch.rs`) — the *pure, synchronous* applier that
  performs a record command's backend mutation and returns the **canonical**
  message text + `OutKind`. This is the provably-identical logic extracted from
  the two loops exactly once. **Both** the CLI REPL and the dashboard chat loop now
  call it for `/decision`, `/action`, `/goal`, `/question`, `/theme`, `/owner`,
  `/risk`, and `/disagree`, so those captures are no longer divergent copies.
- **`run_conversation<C: ConversationChannel>`** (`.../driver.rs`) — the single
  loop that drives a channel end-to-end: `recv` → `parse_command` → route
  conversation turns, records (via `apply_record`), the `on_recorded` hook, and
  `/close`. It is realized by the new [`SignalConversation`](../reference/signal-conversation.md)
  channel and by `MockConversationChannel`, and is available for any future
  channel. The CLI REPL and dashboard keep their existing presentation loops (ANSI
  color / spinners / capture tally, and WebSocket JSON frames respectively) — they
  share the *capture* logic through `apply_record` rather than the whole loop.

See the [Conversation channel API reference](../reference/conversation-channel-api.md)
for the exact trait shape and types.

### Behavior is preserved

The refactor shares **only** the logic that was already identical across the two
loops, and it changes **no observable behavior**. It is worth being precise about
what "preserved" means:

- **Record / capture commands** (`/decision`, `/action`, `/goal`, `/question`,
  `/theme`, `/owner`, `/risk`, `/disagree`) are a **byte-for-byte** extraction:
  their backend mutation and canonical acknowledgement text move verbatim into the
  shared, pure `apply_record`, and **both** the CLI REPL and the dashboard chat loop
  now call it instead of holding their own copy. The strings are identical, so the
  existing tests stay green.
- **Everything else stays in the frontend that owns it.** The CLI REPL and the
  dashboard chat keep their own loops and their own rendering of read-only views
  (`/status`, `/state`, `/recap`, `/preview`, `/help`) and of the I/O- or
  LLM-bearing commands (`Conversation`, `Close`, `Export`, `Template`), because
  those are presentation-specific (ANSI multi-line vs. WebSocket JSON) and are not
  shared. Only the new `SignalConversation` and the mock drive those commands
  through `run_conversation`.

Everything a user can observe stays where it was:

| Preserved behavior | Where it lives |
|--------------------|----------------|
| CLI ANSI colorization, spinners, drift-guard | `meeting_repl/repl.rs` (unchanged loop) |
| CLI live capture tally + `checkpoint_wip` | `meeting_repl/repl.rs`, after the shared `apply_record` call |
| Dashboard `{"role":"system"}` / `{"role":"assistant"}` JSON frames | `operator_commands_dashboard/chat.rs` (unchanged loop) |
| Record mutation + canonical acknowledgement text | **shared** `apply_record`, called by both loops |
| `MeetingCommand` grammar + `parse_command` | unchanged in `src/meeting_backend/command.rs` |
| Handoff write + goal carryover | unchanged `default_handoff_dir()` → `check_meeting_handoffs` |

The existing `meeting_repl` and `chat.rs` tests remain the **behavioral guard**:
they run unchanged and stay green, proving the record extraction changed no
observable output. `run_conversation` is covered by its own mock-driven tests and by
the `SignalConversation` tests.

### Async model — native `impl Future`, no new dependency

The engine ([`MeetingBackend`](../reference/meeting-backend-api.md)) stays
**synchronous and unchanged** (`send_message`, `close`). Channels need async I/O
(the Signal socket), so `ConversationChannel` is an **async trait expressed with
native return-position `impl Future`** (RPITIT) — for example
`fn recv(&mut self) -> impl Future<Output = SimardResult<Option<Inbound>>> + Send`.

This adds **no** dependency: the codebase has no `async-trait` crate and none is
introduced. `run_conversation` calls the synchronous engine methods
(`send_message` / `close` / `apply_template`) directly; each completes *before* the
next `.await`, so no `&mut MeetingBackend` is ever held across an await point and the
driver needs no `spawn_blocking`.

## Module map

```
src/conversation_channel/
  mod.rs        trait ConversationChannel + OperatorRef / Inbound / Outbound / OutKind
  dispatch.rs   apply_record() + Recorded         (pure, synchronous, unit-tested)
  driver.rs     run_conversation<C>()             (the unified driver)
  mock.rs       MockConversationChannel           (scripted recv, captured sends)

src/meeting_repl/repl.rs                    → CLI loop; delegates record dispatch to apply_record
src/operator_commands_dashboard/chat.rs     → dashboard loop; delegates record dispatch to apply_record
src/signal_conversation/                    → SignalConversation impls ConversationChannel (#[cfg(feature = "signal")])
src/lib.rs                                  → pub mod conversation_channel;
```

The [Signal channel](../reference/signal-conversation.md) is an additional,
optional (`signal` feature, default-off) implementation of the same trait; it is
described in its own reference and how-to.

## Session lifecycle

The lifecycle below describes a channel driven by `run_conversation` (the
`SignalConversation` and the mock). The CLI REPL and dashboard run their own loops
but share step 2's record path via `apply_record`, and the same close/carryover
chain.

1. **Open** — a caller constructs a channel (`SignalConversation::new(transport,
   handler, &config)` or `MockConversationChannel::with_script(...)`) and a
   `MeetingBackend`, then calls `run_conversation(&mut channel, &mut backend)`.
2. **Turn loop** — for each authorized inbound line:
   - `Conversation(text)` → `backend.send_message(text)` →
     `channel.send(Outbound { kind: Assistant, .. })`.
   - a record/status command → `apply_record(&mut backend, &cmd)` →
     `channel.send(Outbound { kind, text })`; for an actual capture (`OutKind::Recorded`)
     the driver then fires `channel.on_recorded(&backend)`.
   - `Export` / `Template` — driver-handled (file I/O and/or an LLM
     context-injection turn), not `apply_record`.
3. **Close** — `/close` → `backend.close()` → the summary is sent on the channel,
   the loop ends. `close()` writes the handoff bundle exactly as before.
4. **Carryover** — the handoff is written under `default_handoff_dir()` (which is
   `$SIMARD_HANDOFF_DIR` when set, else `<state_root>/meeting_handoffs`, where
   `<state_root>` is `$SIMARD_STATE_ROOT` or, by default, `~/.simard` — so the
   out-of-the-box path is `~/.simard/meeting_handoffs`) and consumed
   unchanged by `ooda_loop::curate::check_meeting_handoffs`, which promotes meeting
   decisions/actions onto the goal board. This chain is **not** modified.

## Security & autonomy boundary

For the CLI and dashboard channels the operator is already local/authenticated
(terminal ownership; the dashboard's existing auth middleware). The Signal channel
introduces a remote command surface, so it carries three additional guardrails —
a fail-closed sender **allowlist**, **operator-identity binding** (the authorized
sender's E.164 is carried on `OperatorRef` and bound to command replies + sign-off),
and **high-risk gating** that never auto-executes a mutating command from a text:
`deploy`/`merge` create a pending sign-off and run only after an explicit `approve`,
handed to the existing operational-autonomy authority through an injected handler.
Those guardrails live in the Signal channel and are documented in the
[Signal channel reference](../reference/signal-conversation.md). The abstraction
itself carries only the `OperatorRef.authorized` flag that a channel sets after
its own authorization check; the driver never dispatches an unauthorized inbound.

## Testing

- **`apply_record`** — one pure unit test per record command asserting the backend
  mutation is applied and the canonical message text/kind is returned.
- **`run_conversation`** — driven over `MockConversationChannel` with a scripted
  `recv` sequence and captured `send`s; verifies conversation-turn routing, record
  routing, the `on_recorded` hook firing, and clean close.
- **Existing meeting tests** — the `meeting_repl` and dashboard `chat.rs` tests run
  unchanged and stay green, proving the record delegation changed no output.
- **Signal** — allowlist enforcement, high-risk gating (never auto-executing
  `deploy`/`merge`; executing only after `approve`), config resolution, and the
  JSON-RPC wire helpers are tested against a mock transport (no live signal-cli). See
  the [Signal channel reference](../reference/signal-conversation.md#testing).

## Related reading

- [Conversation channel API reference](../reference/conversation-channel-api.md) —
  the trait, message types, `apply_record`, `run_conversation`, and the mock.
- [Signal channel reference](../reference/signal-conversation.md) — the
  feature-gated `SignalConversation`, config, commands, notifications, guardrails.
- [How to set up the Signal channel](../howto/set-up-the-signal-channel.md) —
  linked-device / dedicated-number setup and configuration.
- [Unified meeting backend](./unified-meeting-backend.md) — the shared engine every
  channel drives.
- [Meeting backend API reference](../reference/meeting-backend-api.md) — the sync
  `MeetingBackend` API the channels wrap.
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the
  HIGH-RISK boundary the Signal channel routes mutating commands through.
