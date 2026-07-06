---
title: Signal channel reference
description: Reference for SignalConversation — the optional, feature-gated ConversationChannel that connects Simard to a locally-run signal-cli JSON-RPC daemon, with a sender allowlist, operator-identity binding, high-risk gating, Note-to-Self (sync-sent) command handling with loop prevention, inbound commands, and outbound notifications.
last_updated: 2026-07-04
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#2527", "#2575"]
related:
  - ../index.md
  - ./signal-continuous-conversation.md
  - ../architecture/conversation-channel.md
  - ./conversation-channel-api.md
  - ../howto/set-up-the-signal-channel.md
  - ../concepts/operational-autonomy-model.md
  - ./cross-repo-merge-authority.md
  - ./stewardship-api.md
---

# Signal channel reference

`SignalConversation` (`src/signal_conversation/`, `#[cfg(feature = "signal")]`) is
the Signal implementation of [`ConversationChannel`](./conversation-channel-api.md).
It lets an allowlisted operator command Simard and receive her notifications over
Signal, using the **same** meeting engine and handoff/goal-carryover chain as the
CLI and dashboard channels.

Simard does **not** embed the Signal protocol. She talks to a locally-run
[`signal-cli`](https://github.com/AsamK/signal-cli) daemon over JSON-RPC; signal-cli
owns the Signal account, encryption, and delivery. See
[How to set up the Signal channel](../howto/set-up-the-signal-channel.md) for
installation and linking.

> **Naming.** `SignalConversation` is a first-class conversation channel. It does
> **not** implement, extend, or route through the pre-existing cognitive-memory
> `RpcTransport`. No symbol added by this feature contains `bridge`/`Bridge`.

## Feature gate

The Signal channel lives behind the `signal` Cargo feature, which is **on by
default** (issue #2576): a stock `cargo build` compiles the channel in. Only a
deliberately minimal `--no-default-features` build omits the Signal code and needs
no signal-cli installed.

```toml
# Cargo.toml
[features]
default = ["signal", "dashboard-audit"]  # signal is built in by default
signal = []                              # adds no new dependency
```

```bash
# Default build — Signal channel included:
cargo build
cargo test

# Minimal build — no Signal code:
cargo build --no-default-features
cargo test  --no-default-features
```

The transport uses tokio `net` (already a dependency), so the `signal` feature pulls in
no new crate — there is no HTTP client and no `async-trait`.

## Launching

Start the channel with the operator subcommand from any default build:

```bash
simard signal run
```

`simard signal run` (`src/operator_cli/signal.rs`) loads the `[signal]` config,
builds a tokio runtime, and calls `signal_conversation::run`
(`src/signal_conversation/channel.rs`), which connects to the signal-cli endpoint
and drives the operator conversation to completion. It exits when the signal-cli
socket closes; supervise it (systemd/tmux) to reconnect. A minimal
`--no-default-features` build still recognizes `simard signal run` but returns a
clear error telling the operator to rebuild with the feature.

## Transport

The operator runs signal-cli in JSON-RPC daemon mode over TCP:

```bash
signal-cli -a +15551234567 daemon --tcp 127.0.0.1:7583
```

`src/signal_conversation/transport.rs` opens a tokio `TcpStream` to that endpoint
and speaks **newline-delimited JSON-RPC 2.0**:

- **inbound** — signal-cli `receive` notifications are parsed by the pure
  `parse_incoming` helper into a `ParsedInbound` (see below). The channel dispatches a
  recognized command (`status`, `pause`, `approve`, `merge #NNNN`, …) internally; only
  an ordinary conversation turn surfaces to the driver as
  `Inbound { from: OperatorRef { id: <E.164>, authorized }, text }`.
- **outbound** — an `Outbound` is mapped to a JSON-RPC `send` request addressed to
  the operator number.

`signal-cli-rest-api` and any HTTP-based transport are out of scope for this
version; the JSON-RPC-over-TCP daemon is the supported transport.

### Inbound parsing (`ParsedInbound`)

`parse_incoming(line: &str) -> Option<ParsedInbound>` is pure, I/O-free, and total —
every unrecognized shape resolves to `None` (dropped), never to a coerced default. It
recognizes two envelope shapes and ignores everything else (JSON-RPC responses to our
own `send` calls, receipts, typing indicators, unparseable lines):

```rust
pub struct ParsedInbound {
    pub sender: String,               // E.164 the command is attributed to
    pub body: String,                 // the message text
    pub source_device: Option<u32>,   // envelope source device id (None if absent)
    pub is_sync_sent: bool,           // true for a syncMessage.sentMessage
    pub sync_destination: Option<String>, // sentMessage destination (sync only)
}
```

| Envelope shape | `is_sync_sent` | `sender` | `body` | `source_device` | `sync_destination` |
|----------------|----------------|----------|--------|-----------------|--------------------|
| `dataMessage` (normal inbound) | `false` | `sourceNumber`, else `source` | `dataMessage.message` | envelope `sourceDevice` (if present) | `None` |
| `syncMessage.sentMessage` (Note to Self / a message the account sent) | `true` | the **account**'s own number | `sentMessage.message` | envelope `sourceDevice` | `sentMessage.destinationNumber`, else `destination` |

`dataMessage` is checked **first**, so the normal dedicated-number path is byte-for-byte
unchanged. A missing or non-numeric `sourceDevice` becomes `None` — it is **never**
coerced to `1`, so a malformed sync envelope fails the primary-phone gate and is
rejected (fail-closed).

> **Implementation assumption — verify against a captured envelope before building.**
> This design commits to the JSON-RPC field name **`sourceDevice`** and to two semantics
> that the issue #2575 production logs do **not** prove (they only show the inbound-phone
> case, `sourceDevice: 1`):
>
> 1. a `syncMessage.sentMessage` envelope **carries `sourceDevice`**, and it is the id
>    of the device that *originated* the message — so Simard's own replies, emitted from
>    signal-cli's linked device, stamp `>= 2`; and
> 2. a genuine Note to Self exposes the account's own E.164 via
>    `sentMessage.destinationNumber` (or `destination`), so `sync_destination == account`
>    can be evaluated.
>
> **Confirm both against a real signal-cli line** (capture the raw `jsonRpc` output for a
> Note-to-Self message *and* for one of Simard's own replies) before implementing. If the
> field is actually named differently (`sourceDeviceId`, …) or is absent, or if
> `destination` is only a UUID with no E.164, `parse_incoming` yields `None` / the
> acceptance predicate fails and **every Note-to-Self command is silently rejected — the
> exact failure mode #2575 fixes.** The parser is fail-closed by construction, but these
> field names must be validated for the feature to *work*, not merely to be safe.

## Configuration

Signal settings live in the `[signal]` table of the runtime config file at
`<state_root>/config.toml` — the same file used for the LLM provider, where
`<state_root>` is `$SIMARD_STATE_ROOT` or, by default, `~/.simard` (so
`~/.simard/config.toml` out of the box). Resolution follows the existing
runtime-config rule: **environment wins, then the config file, then a clear error —
never a silent default.** When the `signal` feature is off, the `[signal]` table is
ignored.

> **Loaded by a dedicated typed loader.** `RuntimeConfig` (`src/runtime_config.rs`)
> is a minimal single-key hand parser for `llm_provider`, so the structured
> `[signal]` **table** is read by its own loader, `SignalConfig::load` /
> `load_from` (`src/signal_conversation/config.rs`), which deserializes real TOML
> via the already-present `serde` + `toml` crates — no new dependency. It keeps the
> same env-wins → file → error resolution and no-silent-default guarantee. Each
> field also has an environment override: `SIMARD_SIGNAL_ENDPOINT`,
> `SIMARD_SIGNAL_ACCOUNT`, `SIMARD_SIGNAL_ALLOWLIST` (comma-separated),
> `SIMARD_SIGNAL_READ_ONLY_UNKNOWN`, and `SIMARD_SIGNAL_OWN_DEVICE_ID`. `endpoint`
> and `account` are required; `allowlist` defaults to empty (fail-closed),
> `read_only_unknown` to false, and `own_device_id` to `None` (absent). A present
> `own_device_id` that is unparseable **or `< 2`** is a hard error — never a silent
> default. (Device 1 is always the operator's primary phone, so `own_device_id = 1`
> would disable every Note-to-Self command; the loader rejects it at startup rather than
> letting it fail closed at runtime.)

```toml
[signal]
# signal-cli JSON-RPC daemon endpoint (host:port).
endpoint = "127.0.0.1:7583"

# The Signal account signal-cli owns — a linked device or a dedicated number.
account = "+15551234567"

# E.164 operator numbers permitted to COMMAND Simard. Everyone else is ignored
# (or read-only, if read_only_unknown = true). Fail-closed: empty ⇒ nobody may command.
# On a single-number linked device this is the `account` number itself (Note to Self).
allowlist = ["+15557654321"]

# Opt-in: allow non-allowlisted senders to receive READ-ONLY results (e.g. status).
# They can never trigger a mutation. Default false (unknown senders fully ignored).
read_only_unknown = false

# Optional (single-number linked-device setups): signal-cli's OWN linked device id,
# an integer >= 2 from `signal-cli … listDevices`. A present value < 2 is rejected at
# load (device 1 is your phone). Defence-in-depth loop prevention; the device-1 gate
# already closes the loop without it. Omit for a dedicated number.
# own_device_id = 2
```

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `endpoint` | string `host:port` | — (required) | signal-cli JSON-RPC daemon TCP address |
| `account` | E.164 string | — (required) | the Signal account signal-cli operates |
| `allowlist` | array of E.164 | `[]` | numbers permitted to issue commands |
| `read_only_unknown` | bool | `false` | if true, unknown senders may read status only |
| `own_device_id` | integer `>= 2` (validated) | `None` (absent) | signal-cli's own linked device id; defence-in-depth Note-to-Self loop prevention. A present value `< 2` is a hard config error. |

## Guardrails

The Signal channel is a **remote command surface**, so it adds three guardrails on
top of the shared abstraction. They are layered — a message must clear all three to
cause any mutation. On a single-number linked-device setup, a Note-to-Self (sync-sent)
message must **additionally** pass the loop-prevention predicate in
[Note to Self (sync-sent) and loop prevention](#note-to-self-sync-sent-and-loop-prevention)
before it is treated as a command.

### (a) Sender allowlist — fail-closed

`src/signal_conversation/allowlist.rs` checks each inbound E.164 against
`[signal].allowlist`:

- **Allowlisted sender** → `OperatorRef.authorized = true`; the inbound proceeds.
- **Unknown sender** → **dropped and logged at `debug`** (fail-closed). No engine
  dispatch, no reply, unless `read_only_unknown = true`, in which case the sender
  may receive **read-only** results (e.g. `status`) but can **never** mutate.

Because the driver never receives an unauthorized `Inbound`
(see the [trait contract](./conversation-channel-api.md#contract-notes)), an
unknown sender cannot reach any command handler.

### (b) Operator-identity binding

An authorized inbound is bound to the sender's E.164 on
[`OperatorRef`](./conversation-channel-api.md#operatorref) (`from.id`), and every
command reply and the high-risk sign-off record are addressed to and attributed to
that same sender — so a command runs under, and is answered to, the operator's own
number, never an ambient or anonymous one. Only an allowlisted sender ever reaches
this point (guardrail (a)).

### (c) High-risk gating — never auto-execute a mutation from a text

`src/signal_conversation/gating.rs` classifies every inbound command; the channel
then routes it by its `gate` decision. **Nothing high-risk is ever auto-executed
from a text message.**

| Class | Commands | Handling |
|-------|----------|----------|
| **Low-risk (auto)** | `status`, `pause`, `approve`, and ordinary conversation | Executed immediately for an allowlisted sender. `approve` only **records** the operator's sign-off and then runs the previously-gated action; it never itself classifies a fresh mutation as safe. |
| **High-risk (gated)** | `deploy`, `merge #NNNN` | `classify` → `HighRisk` → `gate` → `PendingSignOff`. The channel records the pending command and replies asking for explicit sign-off. The mutation runs **only** after the operator replies `approve`. |

When an operator approves, the channel calls the injected
[`SignalCommandHandler::execute_approved`](#the-command-handler-seam) — the single
code path that performs a mutating action, and only ever after an explicit
`approve`. A deployment wires that handler to route `deploy` / `merge #NNNN` through
the existing operational-autonomy authority:

- `git_guardrails::check_git_safety` hard-blocks destructive git patterns and any
  write under `SIMARD_GIT_PROTECTED_REPOS`.
- `merge #NNNN` flows through
  `stewardship::merge_authority::merge_pr_if_merge_ready_with_allowlist`
  (objective gates + merge-judge verdict), the same gated authority used by
  [`simard merge-pr`](./cross-repo-merge-authority.md).

The default [`RuntimeCommandHandler`](#the-command-handler-seam) is conservative: it
**records** the signed-off action for that gated authority rather than performing
the mutation itself, so the gating guardrail is fully enforced out of the box and
the concrete execution is an explicit, injectable seam.

### The command-handler seam

`SignalConversation` is generic over a `SignalCommandHandler`
(`src/signal_conversation/channel.rs`):

```rust
pub trait SignalCommandHandler: Send {
    fn status(&self) -> String;                  // `status`
    fn pause(&mut self) -> String;               // `pause`
    fn execute_approved(&mut self, cmd: &InboundCommand) -> SimardResult<String>;
}
```

Keeping the effects behind this trait lets the security-critical allowlist + gating
routing be unit-tested in isolation (a spy handler proves `execute_approved` is
never called before an `approve`), and lets a deployment wire concrete
status/pause/execution to its real subsystems.

## Note to Self (sync-sent) and loop prevention

When signal-cli is a **linked device on the operator's own account** (a single-number
setup), the operator and Simard share one E.164. The operator commands Simard from
Signal's **Note to Self** conversation, which the account "sends" to itself; signal-cli
delivers it to Simard as a **sync-sent** message (`syncMessage.sentMessage`), not a
`dataMessage`. The channel treats a qualifying Note-to-Self message as an operator
command whose sender is the account itself.

Because a linked device also receives sync-sent transcripts of the messages **Simard
herself** sends (her replies are sent from the account and sync back), the channel
must not process its own output as new commands. It applies a **conjunctive** acceptance
predicate — a sync-sent message is accepted **only if every condition holds**:

```text
is_sync_sent
  && sync_destination == account          // a TRUE Note to Self, not a message to a third party
  && source_device == Some(1)             // typed on the operator's PRIMARY PHONE (device 1)
  && source_device != own_device_id       // defence-in-depth: not signal-cli's own linked device
  && !matches_recent_outbound(body)       // defence-in-depth: not an echo of something Simard just sent
  && allowlist.authorize(account) == Authorized  // the account is allowlisted (fail-closed, unchanged)
```

The pure decision lives in `should_accept_sync_sent(source_device, own_device_id,
destination, account, primary_device_id = 1)` and `matches_recent_outbound(body,
&recent, now)` in `transport.rs`; the stateful window and the allowlist call live in
`channel.rs`. The three loop guards are layered, not alternatives:

1. **Primary-phone gate (`source_device == Some(1)`).** Signal guarantees the
   account owner's **phone is always device 1**; every linked device (signal-cli,
   Desktop, iPad) is `>= 2`. Simard's own replies originate from signal-cli's linked
   device, so they are rejected. **This gate alone closes the loop**, even with
   `own_device_id == None` and an empty recent-outbound window — the loop-free
   guarantee does not depend on configuration.
2. **Own-device rejection (`source_device != own_device_id`).** If `own_device_id`
   is configured, a sync-sent message from signal-cli's own device id is explicitly
   rejected. Redundant with (1) by design; it satisfies the literal "reject signal-cli's
   own device" requirement as defence-in-depth.
3. **Recent-outbound echo suppression (`!matches_recent_outbound(body)`).** The
   channel records the body of each message it sends in a bounded
   `VecDeque<(String, Instant)>` (cap **64**, TTL **300 s**, pruned on insert) and
   rejects a sync-sent message whose body **exactly** matches a recent outbound. This
   catches any echo the first two guards miss. The window is in-memory only — never
   persisted, never logged. Because `reply()` records **every** outbound — command
   replies *and* notifications — an operator Note-to-Self whose body is byte-identical
   to a message Simard sent within the TTL is transiently suppressed. That is a
   deliberate fail-safe bias: guard (1) is the primary loop guard, so this rare
   false-negative is preferred over risking an unbroken echo.

Two further properties keep the change **monotonic** — the sync path only ever *adds*
gates, it never widens acceptance:

- **True Note to Self only.** A linked device also receives sync-sent transcripts of
  messages the operator sends to **other people**. `sync_destination == account`
  admits only genuine Note-to-Self messages; syncs destined for a third party are
  ignored, so Simard never reacts to the operator's unrelated conversations.
- **Fail-closed everywhere.** The account still runs through `Allowlist::authorize`,
  so an account that is not allowlisted is dropped. Any missing/malformed field
  (absent `sourceDevice`, absent destination) resolves to a rejection. Only the
  account's own **device-1** Note-to-Self is newly accepted; genuinely unknown senders
  are dropped exactly as before.

The **dedicated-number** path is untouched: a normal `dataMessage` from a separate
operator number carries `is_sync_sent = false`, skips this predicate entirely, and is
authorized and gated exactly as in the original channel.

## Commands in

`src/signal_conversation/gating.rs` parses a small lightweight command vocabulary;
the channel (`channel.rs`) routes each via the allowlist + `gate`. They are thin
wrappers, not a new command engine.

| Text (from an allowlisted operator) | Action | Class |
|-------------------------------------|--------|-------|
| `status` | Reports daemon health + pause state (via the handler; extend it for goals/engineers) | low-risk |
| `pause` | Pause autonomous dispatch | low-risk |
| `approve` | Record operator sign-off and run the pending high-risk request | low-risk |
| `deploy` | Request a deploy → pending sign-off | **high-risk** |
| `merge #NNNN` | Merge PR #NNNN via the gated merge authority → pending sign-off | **high-risk** |

Any other text is treated as an ordinary meeting turn (`Conversation`) and answered
conversationally by Simard, exactly as on the CLI/dashboard channels — so the full
meeting experience (including `/goal`, `/decision`, `/action`, … capture and
`/close`) is available over Signal too.

Successive turns from the same operator form **one continuous, durable
conversation** — the session is keyed by operator identity, persisted across daemon
restarts, and controlled with `/new` (reset), `/help`, and `/close`. See
[Signal continuous conversation](./signal-continuous-conversation.md).

## Notifications out

`SignalConversation::notify` (`src/signal_conversation/channel.rs`) sends a message
to every configured operator for:

| Notification | Trigger |
|--------------|---------|
| **PR merge-ready** | A governed PR has passed the objective gates + merge-judge and is ready to merge |
| **Stall / problem detected** | The OODA loop or an engineer detects a stall, repeated failure, or blocker |
| **High-risk sign-off request** | A gated `deploy`/`merge` needs explicit operator approval (guardrail (c)) |

A sign-off request is answered by replying `approve` (records sign-off) — the
gated action then proceeds through its authority; a text alone never executes it.

## Data flow

**Inbound command:**

```text
signal-cli ─▶ transport.recv_line ─▶ parse_incoming ─▶ ParsedInbound
   ├─ dataMessage (is_sync_sent = false) ─────────────────────────────────┐
   └─ syncMessage.sentMessage (is_sync_sent = true) ─▶ sync predicate ─────┤
        (dest == account && source_device == 1 && != own_device_id         │
         && !recent-outbound echo)  ── fail ▶ ignore (loop prevention)     │
                                                                            ▼
                                                      allowlist gate ─▶ parse_inbound
           ─▶ gate ┬─ AutoExecute    ─▶ handler (status/pause/approve) ─▶ reply
                   └─ PendingSignOff ─▶ record pending ─▶ reply "sign off?"
                                        (execute only after `approve`)
```

A `reply` also records the outbound body in the bounded recent-outbound window used by
the echo-suppression guard (guard 3 above).

**Notification out:**

```text
OODA / merge authority / gate ─▶ SignalConversation::notify ─▶ transport.send_line ─▶ signal-cli
```

**Meeting turn / carryover:** identical to every channel — a `Conversation` inbound
flows through `run_conversation` + `MeetingBackend`, with `/close` writing the
handoff under `default_handoff_dir()` (`$SIMARD_HANDOFF_DIR`, else
`<state_root>/meeting_handoffs`, default `~/.simard/meeting_handoffs`) and
`check_meeting_handoffs` carrying decisions onto the goal board. See
[Conversation channels](../architecture/conversation-channel.md).

## Testing

All Signal tests run under the default `cargo test` (the `signal` feature is on by
default; explicit `--features signal` is redundant) against a **mock JSON-RPC
transport** and a spy command handler — no live signal-cli or network is required:

- **Allowlist enforcement** — an unknown E.164 is dropped (no dispatch, no reply);
  with `read_only_unknown = true` it receives only read-only `status`; an
  allowlisted number is authorized and its conversation turn reaches the driver.
- **High-risk gating** — `deploy` and `merge #NNNN` create a pending sign-off and
  never auto-execute; the spy handler proves `execute_approved` runs only after an
  explicit `approve`, and exactly once; `status`/`pause`/`approve` run immediately.
- **Identity binding** — replies and the delivered conversation `Inbound` are bound
  to the sender's E.164.
- **Wire helpers** — `parse_incoming` maps a signal-cli `receive` notification to a
  `ParsedInbound`. It parses a normal `dataMessage` to `(sender, body)` with
  `is_sync_sent = false` (byte-identical to the original behavior), parses a
  `syncMessage.sentMessage` to a sync-sent `ParsedInbound` carrying `source_device`
  and `sync_destination`, and ignores responses/receipts/typing/unparseable lines;
  `build_send_request` produces valid JSON-RPC.
- **Note-to-Self acceptance + loop prevention** — driven by canned JSON-RPC envelope
  lines (no network), the tests assert the acceptance predicate exactly:
    - a sync-sent envelope from **device 1** (phone) with body `status`, destined for
      the account → parsed as an operator command from the account number;
    - a sync-sent envelope from signal-cli's **own device id** (`>= 2`) → **ignored**
      (loop prevention);
    - a sync-sent envelope whose body matches a **recent Simard outbound** → **ignored**
      (echo suppression);
    - a sync-sent envelope whose destination is a **third party** (not the account) →
      **ignored**;
    - a normal `dataMessage` from a separate allowlisted number → still parsed
      (regression — the dedicated-number path is unchanged);
    - a receipt / typing / unparseable line → still ignored.
- **Config** — env-first resolution, the `[signal]` table from `config.toml`, a clear
  error for a missing required key, and `own_device_id` resolving to `None` when absent
  and a hard error when present-but-unparseable.

## Related reading

- [Conversation channels (architecture)](../architecture/conversation-channel.md)
- [Conversation channel API reference](./conversation-channel-api.md)
- [How to set up the Signal channel](../howto/set-up-the-signal-channel.md)
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the
  HIGH-RISK boundary the gating reuses.
- [Signal continuous conversation](./signal-continuous-conversation.md) — how
  successive operator messages form one durable, resumable session.
- [Cross-repo merge authority](./cross-repo-merge-authority.md) — the gated
  authority `merge #NNNN` flows through.
