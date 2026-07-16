---
title: Overseer Signal JSON-RPC notification transport reference
description: >
  Reference for the Overseer's LIVE Signal operator-notification transport. Documents the
  dependency-free JSON-RPC-over-TCP sender (JsonRpcSignalSender) that posts merge, deploy,
  and escalation notifications to a running Signal service at 127.0.0.1:7583, the
  env-driven SignalRpcConfig it is selected by, the exact wire format, the bounded
  timeouts, and the fail-safe Sent / Queued / Failed mapping (configured => Sent on
  success, Failed on error; unconfigured => Queued, never dropped).
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#4178"]
related:
  - ../index.md
  - ./overseer-operator-notifications.md
  - ../howto/configure-overseer-signal-rpc-notifications.md
  - ./signal-conversation.md
  - ../design/overseer.md
  - ../concepts/operational-autonomy-model.md
---

# Overseer Signal JSON-RPC notification transport reference

The Overseer notifies the operator on **every** merge, deploy, whisper, and — most
importantly — every blocked **"needs human review"** goal escalation. Signal is the
**primary reliable path**: the notification must actually reach the operator's phone.

This reference documents the **live Signal transport** — [`JsonRpcSignalSender`] — that
`SignalNotifyChannel::from_env` selects when the environment configures a Signal account
and recipient. When configured, an autonomous-merge notification is **POSTED** to a
running Signal service over JSON-RPC and the delivery telemetry records
`channel="signal" … outcome=Sent`. When **not** configured, the channel keeps its
fail-safe `Queued` behavior (logged, never dropped).

The transport lives in `src/overseer/notify.rs` (see the
[Overseer design](../design/overseer.md)) alongside the email sender it deliberately
mirrors. For the full two-channel delivery contract (`all_sent` vs `dispatched`, the
anti-self-ingest marker), see the
[Overseer operator-notification reference](./overseer-operator-notifications.md).

> **Fail-safe by construction.** There is no code path that drops a notification.
> An **unconfigured** Signal channel returns `Queued` (logged); a **configured** channel
> that errors (connection refused, timeout, JSON-RPC error) returns `Failed` (logged);
> only a successful POST returns `Sent`.

---

## Why a JSON-RPC transport

A live Signal service already runs locally on the daemon host: a **JSON-RPC over TCP**
endpoint at `127.0.0.1:7583` exposing a `send` method — the same service the operator's
own tooling uses. The Overseer's merge path is **synchronous**, so the notification
transport must be:

- **Dependency-free** — plain `std::net` + the `serde_json` already in the crate. No
  `tokio`/async on the notify path, no `reqwest`, no JSON-RPC crate, and **no** dependency
  on the `signal` cargo feature (this is a plain TCP client, not the async
  `ConversationChannel`).
- **Bounded** — a hung Signal service must never stall a merge. Every connect, read, and
  write is timeout-bounded so the transport fails fast into a logged `Failed`.
- **Fail-safe** — every terminal state maps to `Sent` or `Failed`; only an *unconfigured*
  environment resolves to `Queued`.

`JsonRpcSignalSender` is the Signal analogue of the email channel's minimal
`TcpSmtpSender`: a small, conservative, hermetically testable wire client behind the
object-safe [`SignalSender`] trait.

---

## Environment variables (the complete set)

The live Signal transport is configured **entirely from the environment** — never from
source, never hardcoded. [`SignalRpcConfig::from_env`] reads:

| Variable | Meaning | Default |
|----------|---------|---------|
| `SIMARD_SIGNAL_RPC_ADDR` | `host:port` of the local Signal JSON-RPC service. | `127.0.0.1:7583` |
| `SIMARD_SIGNAL_RPC_ACCOUNT` | The Signal account/number to send **as** (the operator's registered Signal number). | *(none)* |
| `SIMARD_SIGNAL_RPC_RECIPIENT` | The operator's Signal number to send **to**. | *(none)* |

[`SignalRpcConfig::is_configured`] is true **iff both `SIMARD_SIGNAL_RPC_ACCOUNT` and
`SIMARD_SIGNAL_RPC_RECIPIENT` are set** (non-empty after trimming). `SIMARD_SIGNAL_RPC_ADDR`
is **never** part of the configured check — it always has a loopback default and selecting
a non-loopback address is an explicit operator override, not an implicit trigger. This
mirrors [`EmailConfig::is_configured`] exactly (which requires host + from + recipient).

```rust
/// Env-driven Signal JSON-RPC configuration.
pub struct SignalRpcConfig {
    pub addr: String,       // default "127.0.0.1:7583"
    pub account: Option<String>,
    pub recipient: Option<String>,
}

impl SignalRpcConfig {
    /// Reads the three SIMARD_SIGNAL_RPC_* variables from the process environment.
    pub fn from_env() -> Self;
    /// Injectable-env constructor (tests build a fixed lookup map, no global-env races).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self;
    /// True iff BOTH account AND recipient are set. `addr` is never part of this check.
    pub fn is_configured(&self) -> bool;
}
```

`from_env` is a thin wrapper over `from_lookup(|k| std::env::var(k).ok())`, exactly like
`EmailConfig`, so unit tests exercise selection against a fixed map without mutating global
process state.

---

## Transport selection

[`SignalNotifyChannel::from_env`] selects the sender from the configured environment,
mirroring how [`EmailNotifyChannel::from_env`] picks its SMTP transport:

| Condition | Sender wired | `deliver()` outcome |
|-----------|--------------|---------------------|
| `SIMARD_SIGNAL_RPC_ACCOUNT` **and** `SIMARD_SIGNAL_RPC_RECIPIENT` set | `Some(JsonRpcSignalSender)` — live POST | `Sent` on success, `Failed` on error |
| either unset | `None` | `Queued { reason }` naming the RPC env vars (never dropped) |

```rust
impl SignalNotifyChannel {
    /// Production channel. When the environment configures a Signal account AND
    /// recipient, a live JsonRpcSignalSender is wired; otherwise no sender is wired
    /// and the channel queues (logged), never drops.
    pub fn from_env() -> Self {
        let cfg = SignalRpcConfig::from_env();
        if cfg.is_configured() {
            Self::new(Some(Box::new(JsonRpcSignalSender::new(cfg))))
        } else {
            Self::new(None)
        }
    }
}
```

The wiring change is confined to `from_env`. The [`SignalSender`] trait, the
`SignalNotifyChannel::new(Option<Box<dyn SignalSender>>)` injection seam, the
[`SignalNotifyChannel::deliver`] control flow (`None → Queued`, `Some → send_text`), the
anti-self-ingest `signal_wire_body` wrap, and the [`DualChannelNotifier`] composition are
all **structurally unchanged** — the channel simply now has a live sender to delegate to
when configured.

The **one** additional edit inside `deliver` is the unconfigured `Queued` reason string.
Because configuration is now `SIMARD_SIGNAL_RPC_ACCOUNT` / `SIMARD_SIGNAL_RPC_RECIPIENT`
(not "the ConversationChannel transport"), the `None` branch reads:

```rust
ChannelDelivery::Queued {
    reason: "Signal RPC not configured \
             (set SIMARD_SIGNAL_RPC_ACCOUNT and SIMARD_SIGNAL_RPC_RECIPIENT)"
        .to_string(),
}
```

so the journal line an operator actually sees points at the two env vars they must set,
matching the [how-to troubleshooting table](../howto/configure-overseer-signal-rpc-notifications.md#understanding-the-result).

> **Historical note.** Before #4178, `from_env` hardcoded `Self::new(None)`, so **every**
> autonomous-merge notification was `Queued` and never reached the operator, even though a
> live Signal service was running locally. `from_env` now constructs the live sender when
> the account/recipient env is present.

---

## The JSON-RPC wire format

`JsonRpcSignalSender` speaks a single request/response exchange to the Signal service.
It opens a TCP connection to `cfg.addr`, writes **one** newline-terminated JSON line, reads
**one** response line, and closes.

**Request** (built with `serde_json::json!` so every field is correctly escaped — no
hand-rolled JSON, no `format!`):

```json
{"jsonrpc":"2.0","id":1,"method":"send","params":{"account":"<account>","recipient":["<recipient>"],"message":"<body>"}}
```

- `method` is `"send"`.
- `recipient` is a **single-element array** (the JSON-RPC `send` contract).
- `message` is the operator-notification body **after** it has been wrapped by
  `signal_wire_body` (the reserved anti-self-ingest marker path — see below). The transport
  does not bypass that wrap.
- The request is terminated by a single `\n`. That newline is the **only** framing byte;
  the sender never writes a second request or drains the socket in a loop.

**Response** — the sender reads exactly one line and maps it:

| Reply | Result |
|-------|--------|
| Valid JSON containing a top-level `"result"` | `Ok(())` → channel records **`Sent`** |
| Valid JSON containing a top-level `"error"` | `Err(String)` (carries the JSON-RPC error) → **`Failed`** |
| Empty / closed socket, unparseable line, connect refused, or **timeout** | `Err(String)` (carries context) → **`Failed`** |

There is no silent-drop path: every terminal state is `Ok(())` or `Err(String)`, which
`SignalNotifyChannel::deliver` maps to `Sent` or `Failed` respectively.

```rust
/// Minimal, dependency-free, timeout-bounded JSON-RPC-over-TCP Signal sender.
/// Uses only std::net + serde_json. Not gated on the `signal` cargo feature.
pub struct JsonRpcSignalSender { /* addr + account + recipient */ }

impl JsonRpcSignalSender {
    pub fn new(config: SignalRpcConfig) -> Self;
}

impl SignalSender for JsonRpcSignalSender {
    /// Posts one JSON-RPC `send` line to the Signal service and maps the reply:
    /// `result` => Ok(()), `error`/closed/parse-failure/timeout => Err(String).
    fn send_text(&self, text: &str) -> Result<(), String>;
}
```

---

## Bounded I/O (the merge path can never hang)

Every phase is timeout-bounded so a stalled Signal service fails fast into a logged
`Failed` rather than blocking the synchronous merge path:

- **Connect:** `TcpStream::connect_timeout(&addr, 5s)`. The `addr` string is resolved with
  `to_socket_addrs()` and the first resolved socket address is used (the loopback default
  resolves to a single address).
- **Read:** `set_read_timeout(Some(5s))` — a service that accepts the connection but never
  replies surfaces as a timeout `Err`, not a hang.
- **Write:** `set_write_timeout(Some(5s))` — a stalled write cannot block indefinitely.

Five seconds is deliberately shorter than the email STARTTLS sender's timeout: the merge
path is synchronous, so the transport prioritizes **failing fast** over waiting.

---

## The anti-self-ingest marker is preserved

`SignalNotifyChannel::deliver` wraps the notification body via `signal_wire_body(n)`
**before** handing it to `send_text`, exactly as before. Under the `signal` feature the
body is passed through `wrap_operator_notification`, which both **prefixes** the reserved
`OPERATOR_NOTIFY_MARKER` sentinel (`🔔 SIMARD▶OPERATOR:`) and **appends** the
`OPERATOR_NOTIFY_FOOTER` (`— Simard automated notice · do not reply`) — i.e.
`format!("{OPERATOR_NOTIFY_MARKER} {body}{OPERATOR_NOTIFY_FOOTER}")`. The leading marker is
what the inbound Signal processor matches (via `contains`) to deterministically drop
Simard's own notification when it is synced back to a linked device; the footer is
operator-facing text. With the feature off the body is the plain text. `JsonRpcSignalSender`
sends whatever `deliver` hands it — it does not bypass the wrap. See the
[operator-notification reference](./overseer-operator-notifications.md#part-a-signal-anti-self-ingest-marker-primary-safety-control)
for the marker and its inbound drop gate.

> **Feature-off caveat.** `JsonRpcSignalSender` is deliberately **not** `signal`-gated, so a
> `--no-default-features` build with the RPC env set still wires a live sender while
> `signal_wire_body` returns the plain (unmarked) body. This is safe because no inbound
> Signal processor exists when the feature is off, so there is nothing to self-ingest.

---

## Delivery semantics for the Signal channel

The live transport slots into the existing [`ChannelDelivery`] contract:

| Environment | Signal service state | `signal` outcome |
|-------------|----------------------|:----------------:|
| account + recipient set | service accepts, replies `result` | `Sent` |
| account + recipient set | service replies `error` | `Failed` |
| account + recipient set | connect refused / no reply / timeout | `Failed` |
| account **or** recipient unset | (transport not attempted) | `Queued` |

Because the Signal channel is the primary reliable path, a configured daemon reaches the
operator on **every** autonomous merge — the structured summary on target
`overseer::notify` reads `signal=Sent`, and `dispatched()`/`all_sent()` aggregate it with
email exactly as documented in the
[delivery-semantics section](./overseer-operator-notifications.md#part-c-delivery-semantics-all_sent-vs-dispatched).

### Observability

The existing telemetry is unchanged and now shows the live outcome:

```text
target: overseer::notify  dispatched=true all_sent=true  signal=Sent email=Queued kind=merge
```

Compare to the pre-#4178 journal, where the same merge produced (note the old reason
string, which named the internal transport rather than an operator-settable knob):

```text
target: overseer::notify  operator notification not delivered live — queued/failed (never dropped)
  channel="signal" kind="merge" outcome=Queued { reason: "Signal channel not wired (configure the ConversationChannel transport)" }
```

After #4178 a still-unconfigured environment likewise queues, but the reason now names the
two env vars the operator must set:

```text
  channel="signal" kind="merge" outcome=Queued { reason: "Signal RPC not configured (set SIMARD_SIGNAL_RPC_ACCOUNT and SIMARD_SIGNAL_RPC_RECIPIENT)" }
```

The one-line summary carries only the bare `ChannelDelivery` variant name via
`delivery_variant` (no `reason` string, no secret-adjacent text); the paired
`log_degraded` warning still carries the full `?outcome` for diagnosis.

---

## Hermetic testability

The transport is a plain TCP client, so it is unit-tested against an in-test
`std::net::TcpListener` — no live Signal service, no network egress:

- **Well-formed request.** A test listener accepts one connection, reads the request line,
  and asserts it is valid JSON-RPC: `method == "send"`, `params.account` equals the
  configured account, `params.recipient` is a one-element array holding the configured
  recipient, and `params.message` carries the notification body.
- **`result` → `Ok`.** The listener replies `{"jsonrpc":"2.0","id":1,"result":{}}`; the
  sender returns `Ok(())` and the channel records `Sent`.
- **`error` / closed → `Err`.** The listener replies `{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"boom"}}`,
  or closes the socket without replying; the sender returns `Err(String)` and the channel
  records `Failed`.
- **Selection.** With `SIMARD_SIGNAL_RPC_ACCOUNT` + `SIMARD_SIGNAL_RPC_RECIPIENT` present,
  `SignalNotifyChannel::from_env()` builds a `Some`-sender channel; with them unset it
  yields the `Queued` fallback. Env-reading selection tests are `#[serial]` and remove both
  RPC variables to avoid global-env races; format/mapping tests use the injectable
  `SignalRpcConfig::from_lookup` / `SignalNotifyChannel::new` seams so they never touch
  process env.
- **End-to-end.** A [`DualChannelNotifier`] composed with a configured Signal channel
  returns `ChannelDelivery::Sent` for a `kind="merge"` notification.

---

## API surface

All symbols live in `src/overseer/notify.rs`.

| Symbol | Kind | Purpose |
|--------|------|---------|
| `SignalRpcConfig` | struct | Env-driven Signal JSON-RPC config (`addr` default `127.0.0.1:7583`, `account`, `recipient`); `from_env`, `from_lookup`, `is_configured`. |
| `JsonRpcSignalSender` | struct | Minimal, dependency-free, timeout-bounded JSON-RPC-over-TCP `SignalSender` (`std::net` + `serde_json`); `new`, `send_text`. |
| `SignalSender` | trait | *(unchanged)* `send_text(&str) -> Result<(), String>`. |
| `SignalNotifyChannel` | struct | *(unchanged trait/`new`/`deliver`)* — `from_env` now wires `JsonRpcSignalSender` when configured. |
| `ConversationSignalSender` | struct | *(unchanged)* Async `ConversationChannel` adapter (feature `signal`); not used by the JSON-RPC path. |

Unchanged and explicitly **out of scope** for this transport: every merge-gate
(objective gate, `MergeJudge`, #4147 engineer-scoping, the author/recursion guard), the
`SignalSender` trait shape, `SignalNotifyChannel::deliver`, `signal_wire_body`, and the
`DualChannelNotifier` composition. This change touches **only** the Signal notification
transport selection in `from_env`.

## Dependencies

**None added.** `JsonRpcSignalSender` uses only `std::net` (TCP + timeouts) and the
`serde_json` already in the crate. It is **not** gated on the `signal` cargo feature and
adds no `tokio`, `reqwest`, or JSON-RPC crate. `--no-default-features` continues to build.

## Related reading

- [Configure Overseer Signal notifications](../howto/configure-overseer-signal-rpc-notifications.md)
  — the worked systemd drop-in that sets the account/recipient at deploy time.
- [Overseer operator-notification reference](./overseer-operator-notifications.md) —
  the two-channel delivery contract, the anti-self-ingest marker, `all_sent` vs
  `dispatched`, and the email transport this one mirrors.
- [Signal channel reference](./signal-conversation.md) — the inbound command surface the
  marker gate protects.
- [Overseer design](../design/overseer.md) — where the notifier sits in the merge/deploy
  path.
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the HIGH-RISK
  boundary this transport never weakens.
