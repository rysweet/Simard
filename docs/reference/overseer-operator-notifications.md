---
title: Overseer operator-notification reliability reference
description: >
  Reference for the Overseer's reliable, safe operator-notification path across BOTH
  channels — Signal (primary, always-reliable) and email (authenticated SMTP relay). Covers
  the anti-self-ingest Signal marker (OPERATOR_NOTIFY_MARKER) and its deterministic inbound
  drop gate, the real STARTTLS + AUTH LOGIN email sender, the exact environment the operator
  must set, the ChannelDelivery / NotifyReport semantics (all_sent vs dispatched), the
  structured delivery telemetry, and the SMTP header-injection hardening.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#2631"]
related:
  - ../index.md
  - ../design/overseer.md
  - ./signal-conversation.md
  - ./conversation-channel-api.md
  - ../howto/configure-overseer-email-notifications.md
  - ../howto/set-up-the-signal-channel.md
  - ../concepts/operational-autonomy-model.md
  - ./cross-repo-merge-authority.md
---

# Overseer operator-notification reliability reference

The Overseer notifies the operator on **every** merge, deploy, whisper, and — most
importantly — every **blocked "needs human review" goal escalation**. That notification
MUST actually reach a human. This reference documents the two-channel delivery contract
that makes it **reliable** (the operator is always reached) and **safe** (Simard never
re-ingests her own Signal notification as an operator command).

The notifier lives in `src/overseer/notify.rs` (see the
[Overseer design](../design/overseer.md)); the Signal anti-self-ingest marker and its
inbound gate live in `src/signal_conversation/gating.rs` and
`src/signal_conversation/channel.rs`.

> **Two channels, always both.** [`DualChannelNotifier`] fires **every** channel on
> **every** notification and records each outcome. There is no code path that drops a
> notification on the floor: an unconfigured channel returns `Queued` (logged), a
> transport error returns `Failed` (logged).

## Channels at a glance

| Channel | Role | Configured by | Unconfigured behavior |
|---------|------|---------------|-----------------------|
| **Signal** | **Primary reliable path.** When the `[signal]` transport is wired, the operator is always reached on their phone. | [Signal channel setup](../howto/set-up-the-signal-channel.md) | `Queued { reason: "Signal channel not wired …" }` |
| **Email** | Secondary durable path via an **authenticated SMTP relay** (office365 or an internal relay). | Env vars (below) + the operator's systemd unit | `Queued { reason: "SMTP not configured …" }` |

The Signal channel is the **primary** path precisely because it is already wired in
production; email delivers in addition, once the operator adds relay credentials.

---

## Part A — Signal anti-self-ingest marker (primary safety control)

### The problem

Simard runs an **inbound** Signal processor: allowlisted operator messages become
commands (`status`, `approve`, `merge #NNNN`) or meeting turns. On a single-number
linked-device setup, Signal **syncs Simard's own outbound messages back** to the linked
device as *sync-sent* transcripts. Without a deterministic guard, Simard could read her
own operator notification as a new inbound command.

The pre-existing echo suppression (`matches_recent_outbound`) is **exact-body match
within a 5-minute window** — it is fragile: a late, quoted, or altered synced echo (e.g.
prefixed with "You sent:") slips through. The marker removes that fragility.

### The marker

Every outbound Overseer→operator Signal notification is wrapped with a distinct,
reserved sentinel so the inbound processor can **deterministically** recognize and skip
it — independent of the echo window.

```rust
// src/signal_conversation/gating.rs — single source of truth
pub const OPERATOR_NOTIFY_MARKER: &str = "🔔 SIMARD▶OPERATOR:";
```

- **Reserved.** The sentinel `🔔 SIMARD▶OPERATOR:` (bell emoji + `SIMARD▶OPERATOR:`) is
  reserved. Operators MUST NOT send any message *containing* it — detection is a
  **substring** test, not a prefix test. Its high-entropy glyph combination (`🔔` + `▶`
  + the literal `OPERATOR:`) never occurs in a real operator command, so normal messages
  are unaffected.
- **Human-readable.** On the operator's phone the message reads as a pleasant, labelled
  notice, followed by a plain footer.

A human footer is appended for readability. It is **display only** and is NOT used for
detection:

```text
— Simard automated notice · do not reply
```

### Formatting API

```rust
/// Wrap an operator-notification body in the reserved marker (+ footer).
pub fn wrap_operator_notification(body: &str) -> String;

/// True iff `text` carries the reserved marker anywhere in the message.
pub fn is_operator_notification(text: &str) -> bool;
```

- `wrap_operator_notification(body)` returns
  `format!("{OPERATOR_NOTIFY_MARKER} {body}{FOOTER}")` — the result **starts with** the
  marker and **ends with** the footer.
- `is_operator_notification(text)` is a **substring** test
  (`text.contains(OPERATOR_NOTIFY_MARKER)`), NOT `starts_with`. Substring matching
  survives a synced-echo prefix such as `"You sent: …"` or a quoted reply.

A rendered notification the operator sees:

```text
🔔 SIMARD▶OPERATOR: The Overseer autonomously performed a goal-blocked in rysweet/Simard.

Problem solved:
  Goal `g-4821` is blocked and needs human review.
  Reason: completion evidence rejected twice


— Simard automated notice · do not reply
```

(`plain_text()` already ends in a newline and the footer begins with `\n\n`, so the
rendered notice carries two blank lines before the footer.)

### Where wrapping happens

Only the Overseer **notification** path wraps its body. The wrap is applied in
`SignalNotifyChannel::deliver` (in `notify.rs`) immediately before `send_text`:

```rust
fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
    match &self.sender {
        None => ChannelDelivery::Queued { reason: "Signal channel not wired …".into() },
        Some(sender) => {
            // The marker helpers live in the `signal`-gated module, so the wrap
            // call site is feature-gated too (see the build note below).
            #[cfg(feature = "signal")]
            let body = wrap_operator_notification(&n.plain_text());
            #[cfg(not(feature = "signal"))]
            let body = n.plain_text();
            match sender.send_text(&body) { /* Sent / Failed */ }
        }
    }
}
```

> **Feature-gating (keeps `--no-default-features` building).** `wrap_operator_notification`
> lives in `src/signal_conversation/gating.rs`, which is compiled only behind
> `#[cfg(feature = "signal")]`, whereas `src/overseer/notify.rs` is **always** compiled.
> The wrap call site is therefore itself `#[cfg(feature = "signal")]`-gated. With the
> `signal` feature off there is no inbound Signal processor that could self-ingest, and
> `SignalNotifyChannel::from_env()` returns `sender = None` (so the channel always
> `Queued`s and the wrapped branch is unreachable) — the marker import is compiled out
> and the minimal build stays green. `gating.rs` remains the single source of truth for
> the marker.

Interactive replies (`status`, high-risk sign-off prompts, meeting turns) are **not**
wrapped — they are short-lived, in-thread, and already covered by the existing
device-1 / echo-suppression guards. The **email** body is never wrapped: the marker is a
Signal-only anti-self-ingest device.

### The inbound drop gate

The inbound processor drops any marked message **before** it reaches allowlist
authorization or command parsing. In `SignalConversation::recv`
(`src/signal_conversation/channel.rs`), the check sits immediately after
`parse_incoming` yields the parsed envelope and **before** the sync-sent / echo /
allowlist logic:

```rust
let Some(parsed) = super::transport::parse_incoming(&line) else { continue; };

// Fail-closed anti-self-ingest: a message bearing the reserved operator-notification
// marker is one of Simard's OWN notifications synced back — never an operator command.
if is_operator_notification(&parsed.body) {
    tracing::debug!(target: "signal", "dropping self-notification echo (operator marker)");
    continue;
}

// … existing sync-sent gate, echo suppression, allowlist, command parsing …
```

Properties:

- **Deterministic & independent of the echo window.** A marked message is dropped even
  if the synced echo arrives late, expired, altered, or quoted — the marker alone
  suffices. It does **not** rely on `matches_recent_outbound`.
- **Covers both paths.** The gate runs for both direct `dataMessage`s and sync-sent
  Note-to-Self transcripts, because it precedes the `is_sync_sent` branch.
- **Grants no privilege.** The marker only causes an inbound message to be *ignored*. It
  never authorizes anything; the allowlist and high-risk sign-off gate are unchanged.
- **Normal operator messages are unaffected.** A message without the marker flows
  through exactly as before.

---

## Part B — Email delivery via an authenticated SMTP relay

Delivering to a `microsoft.com` recipient requires an **authenticated SMTP relay**
(office365 or an internal relay). The email channel performs a **real** SMTP send when
host/port/user/pass are configured.

### Environment variables (the complete set)

Email is configured **entirely from the environment** — never from source, and never
with hardcoded secrets. [`EmailConfig::from_env`] reads:

| Variable | Meaning | Example |
|----------|---------|---------|
| `SIMARD_OVERSEER_EMAIL_TO` | Recipient(s), comma-separated. | `rysweet@microsoft.com` |
| `SIMARD_OVERSEER_EMAIL_FROM` | Envelope + header `From` (an authorized sender on the relay). | `simard-overseer@contoso.onmicrosoft.com` |
| `SMTP_HOST` | Relay host. | `smtp.office365.com` |
| `SMTP_PORT` | Relay port (STARTTLS submission). Defaults to `25` if unset. | `587` |
| `SMTP_USER` | Relay auth username (a mailbox). **Secret-adjacent.** | `simard-overseer@contoso.onmicrosoft.com` |
| `SMTP_PASS` | Relay auth password / app credential. **Secret.** | *(set in the systemd unit)* |

[`EmailConfig::is_configured`] is true iff a **host**, a **from**, and at least one
**recipient** are known. When it is false, the channel returns
`Queued { reason: "SMTP not configured …" }` — logged, never dropped.

> **Never hardcode secrets.** `SMTP_USER` / `SMTP_PASS` are supplied only via the
> environment (the operator sets them in the systemd unit). No credential literal appears
> in source, tests, docs, or logs. See
> [How to configure Overseer email notifications](../howto/configure-overseer-email-notifications.md)
> for the worked office365 setup.

### Transport selection

[`EmailNotifyChannel::from_env`] selects the SMTP sender from the configured
environment:

| Condition | Sender | Security (example port) |
|-----------|--------|-------------------------|
| `SMTP_USER` **and** `SMTP_PASS` set | [`StartTlsSmtpSender`] — **STARTTLS + AUTH LOGIN** | encrypted + authenticated (e.g. `smtp.office365.com:587`) |
| neither set | [`TcpSmtpSender`] — minimal plaintext | no TLS/AUTH (e.g. a local relay on `:25`) |

Selection depends **only** on whether `SMTP_USER` **and** `SMTP_PASS` are both set; the
actual port is whatever `SMTP_PORT` resolves to (default `25`). The ports above are the
worked-example values, not a property the sender enforces.

Both senders implement the object-safe [`EmailSender`] trait
(`fn send(&self, msg: &EmailMessage) -> Result<(), String>`), so the channel logic is
identical; only the wire transport differs.

### The STARTTLS + AUTH LOGIN sender

[`StartTlsSmtpSender`] speaks authenticated submission using **blocking `rustls`** (no
async runtime) and `base64` for the AUTH LOGIN payload. The conversation:

```text
S: 220 …                         (greeting)
C: EHLO simard-overseer
S: 250-… 250-STARTTLS …          (STARTTLS advertised)
C: STARTTLS
S: 220 …
        ── TLS handshake (rustls, default verifier, hostname = SMTP_HOST) ──
C: EHLO simard-overseer          (re-issued inside TLS)
S: 250-… 250-AUTH LOGIN …
C: AUTH LOGIN
S: 334 VXNlcm5hbWU6              (server's base64 prompt, e.g. "Username:")
C: <base64(SMTP_USER)>           (client answers positionally, not by matching the prompt)
S: 334 UGFzc3dvcmQ6              (server's base64 prompt, e.g. "Password:")
C: <base64(SMTP_PASS)>
S: 235 …                          (authenticated)
C: MAIL FROM:<…> / RCPT TO:<…> / DATA / …
C: QUIT
```

Security guarantees:

- **Mandatory STARTTLS when authenticating (fail-closed).** If credentials are set but
  the server does not advertise/complete STARTTLS, the sender returns
  `Failed { reason }` — it NEVER falls back to sending `AUTH LOGIN` in the clear.
  base64 is an encoding, not encryption; this blocks a STARTTLS-stripping MITM.
- **Standard certificate verification.** Uses the stock `rustls` verifier with
  `rustls-native-certs` (falling back to `webpki-roots`); the TLS peer name is validated
  against `SMTP_HOST`. No `dangerous_configuration`, no cert bypass.
- **Timeouts.** Read/write timeouts bound every reply read on the TLS stream (mirroring
  the plaintext sender), so a stalled relay cannot hang the notifier.
- **No credential leakage.** `SMTP_PASS`, the base64 AUTH payload, and the raw `AUTH`
  line are never logged and never placed in an error string; only server reply text may
  surface.

### Hermetic testability

The protocol is split from the TLS seam so it is unit-testable without a network:

- `smtp_converse<S: Read + Write>(stream, msg, auth)` — a **pure** state machine driven
  over any duplex stream. It is composed of two shared helpers,
  `smtp_starttls_prelude` (greeting → EHLO → **require** STARTTLS → STARTTLS → `220`) and
  `smtp_submit_authenticated` (EHLO → AUTH LOGIN → MAIL/RCPT/DATA → QUIT). Scripted
  in-memory duplex tests assert that STARTTLS is requested, that AUTH LOGIN emits the
  correct positional base64, that an injected subject/body is neutralized in the DATA
  section, and that a credentialed send against a server that omits STARTTLS fails
  **without** emitting any plaintext AUTH bytes.
- `StartTlsSmtpSender` reuses those **same two helpers** with a real `rustls` handshake
  spliced between them (prelude over the plain socket, submission over the TLS stream),
  so only the TLS handshake itself is an untested seam — the wire protocol is exactly the
  tested one.
- A `FakeSmtp` at the [`EmailSender`] level exercises channel `Sent` / `Queued`
  semantics.

### SMTP header-injection hardening

Notification content is partly GitHub-sourced (PR titles, headlines) and board-derived
(goal ids in the Subject). The shared `sanitize_header_value()` and `dot_stuff_body()`
(introduced in the plaintext sender) are applied by **both** senders when constructing
the message, at the single transport choke point:

- `sanitize_header_value()` — every control character (including CR/LF) in the `From`,
  each recipient, and the Subject is replaced with a space and the value is byte-bounded
  (≤ 512 octets, on a UTF-8 char boundary), so an injected `\r\n` cannot terminate the
  line and smuggle an extra header (e.g. `Bcc:`) or SMTP verb.
- `dot_stuff_body()` — the body is CRLF-normalized and any line beginning with `.` is
  escaped (`.` → `..`), so a board-derived lone `.` line cannot prematurely close the
  DATA section (RFC 5321 §4.5.2 transparency).

This blocks SMTP header/command injection (CWE-93) from an attacker-influenced
`pr_title` / `headline` / goal `reason` on both the plaintext and authenticated paths.

---

## Part C — Delivery semantics: `all_sent` vs `dispatched`

Each channel returns a [`ChannelDelivery`]; the notifier aggregates them into a
[`NotifyReport`].

```rust
pub enum ChannelDelivery {
    Sent,                       // delivered to the transport
    Queued { reason: String },  // not configured / degraded — logged, never dropped
    Failed { reason: String },  // transport attempted but errored — logged
}
```

```rust
impl NotifyReport {
    /// True iff there is at least one channel AND every channel delivered
    /// (no `Queued`, no `Failed`). An empty report is `false`.
    pub fn all_sent(&self) -> bool;
    /// True iff at least one channel recorded an outcome (`per_channel` is
    /// non-empty). It does NOT mean anything was successfully sent — a report
    /// holding only `Queued`/`Failed` is still `dispatched`.
    pub fn dispatched(&self) -> bool;
}
```

The distinction is the crux of "reliable but honest":

| Situation | `signal` | `email` | `dispatched()` | `all_sent()` |
|-----------|----------|---------|:--------------:|:------------:|
| Both configured, both deliver | `Sent` | `Sent` | ✅ | ✅ |
| Signal wired, email relay not yet set | `Sent` | `Queued` | ✅ | ❌ |
| Signal wired, relay auth wrong | `Sent` | `Failed` | ✅ | ❌ |
| Signal not wired, email delivers | `Queued` | `Sent` | ✅ | ❌ |

Because Signal is the wired primary path, **the operator is always reached** and the
escalation is **dispatched** even when email SMTP is not yet configured. An escalation is
therefore **never considered "lost"** merely because email is unconfigured; `all_sent()`
simply reports the honest fact that not every channel delivered.

### Observability

Whenever a channel does not deliver, `log_degraded` already emits a `tracing::warn!` on
target `overseer::notify` naming the **channel** and the **outcome** (the
`Queued`/`Failed` variant, so unconfigured-vs-transport-error is distinguishable). In
addition, [`DualChannelNotifier::notify`] emits one structured `tracing::info!` summary
per notification:

```text
target: overseer::notify
  dispatched = true
  all_sent   = false
  kind       = goal-blocked
  channels   = signal=Sent email=Queued
```

The `channels` field carries a space-separated `name=Variant` list built from
`delivery_variant` — the `ChannelDelivery` **variant name only** (`Sent` / `Queued` /
`Failed`), not the `Debug` form — so no `reason` string ever lands in the one-line
summary; the paired `log_degraded` warning still carries the full `?outcome` for
diagnosis. This makes "Signal Sent + email Queued" plainly visible as
**dispatched-but-not-all_sent**. No secret (password, base64 AUTH payload) is ever
included in any log field.

---

## API surface

All types live in `src/overseer/notify.rs` unless noted.

| Symbol | Kind | Purpose |
|--------|------|---------|
| `OperatorNotification` | struct | Channel-agnostic notification (`kind`, `headline`, `problem`, `link`, `repo`, `autonomous`). Builders: `deploy`, `whisper`, `goal_blocked`; `MergeNotification::to_operator`. |
| `ChannelDelivery` | enum | `Sent` \| `Queued { reason }` \| `Failed { reason }`. |
| `NotifyReport` | struct | `per_channel: Vec<(String, ChannelDelivery)>`; `all_sent()`, `dispatched()`. |
| `NotifyChannel` | trait | `name()`, `deliver(&OperatorNotification) -> ChannelDelivery`. |
| `DualChannelNotifier` | struct | Fires every channel; `new`, `from_env`, `notify`. |
| `OperatorNotifier` | trait | Object-safe seam (`notify`) so tests inject a fake. |
| `EmailConfig` | struct | Env-driven SMTP config; `from_env`, `from_lookup`, `is_configured`, `use_authenticated`. |
| `EmailMessage` | struct | `from`, `to`, `subject`, `body`. |
| `EmailSender` | trait | `send(&EmailMessage) -> Result<(), String>`. |
| `EmailNotifyChannel` | struct | Email channel; `from_env` selects the sender by config. |
| `TcpSmtpSender` | struct | Minimal plaintext SMTP (`:25`, no TLS/AUTH). |
| `StartTlsSmtpSender` | struct | **STARTTLS + AUTH LOGIN** authenticated relay sender. |
| `smtp_converse` | fn | Pure SMTP state machine (`S: Read + Write`) for hermetic tests. |
| `SmtpAuth` | struct | `user` / `pass` for AUTH LOGIN (no `Debug`; env-sourced, never hardcoded). |
| `sanitize_header_value` / `dot_stuff_body` | fn | Shared CWE-93 header/body injection hardening (applied by both senders). |
| `delivery_variant` | fn | Bare `ChannelDelivery` variant name for the structured summary (no `reason`). |
| `SignalSender` | trait | `send_text(&str) -> Result<(), String>`. |
| `ConversationSignalSender` | struct | Adapts any `ConversationChannel` into a `SignalSender`. |
| `SignalNotifyChannel` | struct | Signal channel; wraps the body with the operator marker before sending. |
| `OPERATOR_NOTIFY_MARKER` | const | Reserved sentinel (`gating.rs`). |
| `wrap_operator_notification` | fn | Wrap a body in the marker + footer (`gating.rs`). |
| `is_operator_notification` | fn | Substring marker detection (`gating.rs`). |

## Dependencies

The STARTTLS sender promotes these to direct dependencies, pinned to `Cargo.lock`, with
`--no-default-features` still building:

- `rustls` `0.23.38` (with the `ring` crypto provider enabled explicitly)
- `rustls-native-certs` `0.8.3` (OS trust store)
- `webpki-roots` `1.0.7` (compiled root fallback when the OS store is empty)
- `base64` `0.22.1` (AUTH LOGIN payload encoding, inside TLS only)

## Related reading

- [Configure Overseer email notifications](../howto/configure-overseer-email-notifications.md)
  — the office365 worked example and systemd unit.
- [Set up the Signal channel](../howto/set-up-the-signal-channel.md) — wire the primary
  reliable path.
- [Signal channel reference](./signal-conversation.md) — the inbound command surface the
  marker gate protects.
- [Overseer design](../design/overseer.md) — where the notifier sits in the merge/deploy
  path.
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the
  HIGH-RISK boundary the marker gate never weakens.
