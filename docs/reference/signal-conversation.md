---
title: Signal channel reference
description: Reference for SignalConversation — the optional, feature-gated ConversationChannel that connects Simard to a locally-run signal-cli JSON-RPC daemon, with a sender allowlist, operator-identity binding, high-risk gating, inbound commands, and outbound notifications.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#2527"]
related:
  - ../index.md
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
> `BridgeTransport`. No symbol added by this feature contains `bridge`/`Bridge`.

## Feature gate

The Signal channel is compiled only when the `signal` Cargo feature is enabled; it
is **off by default**. The default build has no Signal code and needs no signal-cli
installed.

```toml
# Cargo.toml
[features]
signal = []  # default-OFF; adds no new dependency
```

```bash
# Default build — no Signal, no signal-cli needed:
cargo build
cargo test

# With the Signal channel:
cargo build --features signal
cargo test  --features signal
```

The transport uses tokio `net` (already a dependency), so enabling `signal` pulls in
no new crate — there is no HTTP client and no `async-trait`.

## Launching

Start the channel with the operator subcommand from a `--features signal` build:

```bash
simard signal run
```

`simard signal run` (`src/operator_cli/signal.rs`) loads the `[signal]` config,
builds a tokio runtime, and calls `signal_conversation::run`
(`src/signal_conversation/channel.rs`), which connects to the signal-cli endpoint
and drives the operator conversation to completion. It exits when the signal-cli
socket closes; supervise it (systemd/tmux) to reconnect. A default build (no
`signal` feature) still recognizes `simard signal run` but returns a clear error
telling the operator to rebuild with `--features signal`.

## Transport

The operator runs signal-cli in JSON-RPC daemon mode over TCP:

```bash
signal-cli -a +15551234567 daemon --tcp 127.0.0.1:7583
```

`src/signal_conversation/transport.rs` opens a tokio `TcpStream` to that endpoint
and speaks **newline-delimited JSON-RPC 2.0**:

- **inbound** — signal-cli `receive` notifications are mapped to
  `Inbound { from: OperatorRef { id: <E.164>, authorized }, text }`.
- **outbound** — an `Outbound` is mapped to a JSON-RPC `send` request addressed to
  the operator number.

`signal-cli-rest-api` and any HTTP-based transport are out of scope for this
version; the JSON-RPC-over-TCP daemon is the supported transport.

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
> `SIMARD_SIGNAL_ACCOUNT`, `SIMARD_SIGNAL_ALLOWLIST` (comma-separated), and
> `SIMARD_SIGNAL_READ_ONLY_UNKNOWN`. `endpoint` and `account` are required;
> `allowlist` defaults to empty (fail-closed) and `read_only_unknown` to false.

```toml
[signal]
# signal-cli JSON-RPC daemon endpoint (host:port).
endpoint = "127.0.0.1:7583"

# The Signal account signal-cli owns — a linked device or a dedicated number.
account = "+15551234567"

# E.164 operator numbers permitted to COMMAND Simard. Everyone else is ignored
# (or read-only, if read_only_unknown = true). Fail-closed: empty ⇒ nobody may command.
allowlist = ["+15557654321"]

# Opt-in: allow non-allowlisted senders to receive READ-ONLY results (e.g. status).
# They can never trigger a mutation. Default false (unknown senders fully ignored).
read_only_unknown = false
```

| Key | Type | Default | Meaning |
|-----|------|---------|---------|
| `endpoint` | string `host:port` | — (required) | signal-cli JSON-RPC daemon TCP address |
| `account` | E.164 string | — (required) | the Signal account signal-cli operates |
| `allowlist` | array of E.164 | `[]` | numbers permitted to issue commands |
| `read_only_unknown` | bool | `false` | if true, unknown senders may read status only |

## Guardrails

The Signal channel is a **remote command surface**, so it adds three guardrails on
top of the shared abstraction. They are layered — a message must clear all three to
cause any mutation.

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
signal-cli ─▶ transport.recv_line ─▶ parse_incoming ─▶ allowlist gate ─▶ parse_inbound
           ─▶ gate ┬─ AutoExecute    ─▶ handler (status/pause/approve) ─▶ reply
                   └─ PendingSignOff ─▶ record pending ─▶ reply "sign off?"
                                        (execute only after `approve`)
```

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

All Signal tests run under `cargo test --features signal` against a **mock JSON-RPC
transport** and a spy command handler — no live signal-cli or network is required:

- **Allowlist enforcement** — an unknown E.164 is dropped (no dispatch, no reply);
  with `read_only_unknown = true` it receives only read-only `status`; an
  allowlisted number is authorized and its conversation turn reaches the driver.
- **High-risk gating** — `deploy` and `merge #NNNN` create a pending sign-off and
  never auto-execute; the spy handler proves `execute_approved` runs only after an
  explicit `approve`, and exactly once; `status`/`pause`/`approve` run immediately.
- **Identity binding** — replies and the delivered conversation `Inbound` are bound
  to the sender's E.164.
- **Wire helpers** — `parse_incoming` maps a signal-cli `receive` notification to
  `(sender, text)` and ignores responses/receipts/unparseable lines;
  `build_send_request` produces valid JSON-RPC.
- **Config** — env-first resolution, the `[signal]` table from `config.toml`, and a
  clear error for a missing required key.

## Related reading

- [Conversation channels (architecture)](../architecture/conversation-channel.md)
- [Conversation channel API reference](./conversation-channel-api.md)
- [How to set up the Signal channel](../howto/set-up-the-signal-channel.md)
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the
  HIGH-RISK boundary the gating reuses.
- [Cross-repo merge authority](./cross-repo-merge-authority.md) — the gated
  authority `merge #NNNN` flows through.
