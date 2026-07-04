---
title: How to set up the Signal channel
description: Connect Simard to Signal so an allowlisted operator can command her and receive notifications, using a locally-run signal-cli JSON-RPC daemon. Covers linking a device (or using a dedicated number), the Note-to-Self flow for single-number linked-device setups, loop prevention, configuration, the allowlist, and verification. The signal feature is built by default, so a plain build is signal-capable.
last_updated: 2026-07-04
review_schedule: as-needed
owner: simard
doc_type: howto
issues: ["#2527", "#2575", "#2576"]
related:
  - ../index.md
  - ../reference/signal-conversation.md
  - ../architecture/conversation-channel.md
  - ../reference/conversation-channel-api.md
  - ../concepts/operational-autonomy-model.md
  - ./start-a-meeting.md
---

# How to set up the Signal channel

The Signal channel lets an allowlisted operator talk to Simard over Signal — issue
lightweight commands (`status`, `approve`, `merge #NNNN`, …), hold a full meeting,
and receive notifications (PR merge-ready, stalls, high-risk sign-off requests).

Simard does not implement Signal herself. She connects to a locally-run
[`signal-cli`](https://github.com/AsamK/signal-cli) daemon over JSON-RPC. This
guide installs signal-cli, links or registers an account, configures the
`[signal]` table, and verifies the round trip. The `signal` feature is **built by
default** (issue #2576), so a plain `cargo build` already ships the channel — no
`--features signal` needed.

For the full contract (config keys, commands, guardrails), see the
[Signal channel reference](../reference/signal-conversation.md).

## Prerequisites

- [ ] You are in the repository root.
- [ ] A Rust toolchain that builds Simard (`cargo build` already works).
- [ ] `SIMARD_LLM_PROVIDER` configured (Signal meetings use the same engine — see
      [How to start a meeting](./start-a-meeting.md)).
- [ ] Java 21+ runtime (signal-cli requires it) — `java -version`.
- [ ] A phone number for Simard's Signal identity: either an existing Signal
      account you will **link** as a device, or a **dedicated** number to register.

> The Signal channel is **built by default but dormant until configured.** If you
> skip this guide, the Signal code is still compiled in, but with no `[signal]`
> config Simard's daemon runs exactly as before — the channel never starts.

## 1. Install signal-cli

Install signal-cli from your package manager or the
[release page](https://github.com/AsamK/signal-cli/releases). Verify:

```bash
signal-cli --version
```

## 2. Give signal-cli an account

Choose **one** of the two setups.

### Option A — Link as a device (recommended)

Keep Simard as an extra **linked device** on your existing Signal account. Your
phone stays the primary device.

```bash
signal-cli link -n "simard-daemon"
```

signal-cli prints a `sgnl://linkdevice?...` URI (and a QR code). On your phone:
**Signal → Settings → Linked devices → Link new device**, then scan the QR (or use
the URI). When linking finishes, note the account number (your own E.164, e.g.
`+15551234567`) — that is the `account` value below.

> **Single number? You chat with Simard via _Note to Self._** When signal-cli is a
> linked device on your *own* account, the number you message Simard **from** and the
> `account` she receives **on** are the **same** number. There is no separate number
> to text, so the operator commands Simard by messaging **themselves** — Signal's
> **Note to Self** conversation. signal-cli delivers those to Simard as *sync-sent*
> messages, and she accepts a Note-to-Self message from your **primary phone** as an
> operator command. See [Chatting via Note to Self](#chatting-via-note-to-self-single-number-setups)
> below for the trust boundary and how this stays loop-free.

While you are linked, record signal-cli's own **device id** — you will use it for
defence-in-depth loop prevention (`own_device_id`, step 4):

```bash
signal-cli -a +15551234567 listDevices
```

Your phone is **always device 1**; the `simard-daemon` device you just linked has a
higher id (e.g. `2`). Note that number.

### Option B — Register a dedicated number

Register a separate number that Simard owns:

```bash
signal-cli -a +15551234567 register
# Enter the SMS/voice code you receive:
signal-cli -a +15551234567 verify 123456
```

Here `+15551234567` is a dedicated Signal number and becomes the `account` value.

## 3. Start the signal-cli JSON-RPC daemon

Run signal-cli in JSON-RPC daemon mode over TCP. Simard connects to this endpoint;
signal-cli owns the account, encryption, and delivery.

```bash
signal-cli -a +15551234567 daemon --tcp 127.0.0.1:7583
```

Leave it running (systemd unit, tmux, or a supervisor of your choice). The
`127.0.0.1:7583` host:port is the `endpoint` value below — bind to loopback so the
JSON-RPC surface is not exposed off-host.

## 4. Configure the `[signal]` table

Add a `[signal]` table to the runtime config file at `<state_root>/config.toml`
(the same file Simard uses for the LLM provider; `<state_root>` is
`$SIMARD_STATE_ROOT` or, by default, `~/.simard`, so `~/.simard/config.toml` out of
the box). Environment variables still win over the file; there is no silent default.
The `[signal]` table is parsed and applied by any default build (the `signal`
feature is on by default since issue #2576). Only a deliberately minimal
`--no-default-features` build ignores it.

```toml
[signal]
endpoint = "127.0.0.1:7583"      # the signal-cli daemon from step 3
account  = "+15551234567"        # the linked/dedicated account from step 2
allowlist = ["+15559876543"]     # YOUR operator number(s) — who may command Simard
read_only_unknown = false        # keep unknown senders fully ignored (default)
# own_device_id = 2              # single-number setups only: signal-cli's OWN linked
                                 # device id (>= 2) from `listDevices` — defence-in-depth
                                 # loop prevention. A value < 2 is rejected at load. Omit
                                 # it for a dedicated number.
```

- **`allowlist` is the security boundary.** Only the E.164 numbers listed here may
  command Simard. It is **fail-closed**: an empty or missing allowlist means
  *nobody* may command her. Put the operator's own phone number here — the number
  you message Simard *from*.
    - **Dedicated number (Option B):** the operator number is **different** from
      `account` (the number Simard receives *on*).
    - **Single-number linked device (Option A):** the operator number **is** the
      `account` number, so `account` and `allowlist` hold the **same** E.164. This is
      correct and required — a Note-to-Self command's sender *is* the account. It does
      **not** weaken fail-closed behavior: an unknown sender is still ignored, and a
      Note-to-Self message is accepted only when it comes from your **primary phone**
      (device 1) — see [Chatting via Note to Self](#chatting-via-note-to-self-single-number-setups).
- **`own_device_id` (optional, single-number setups).** signal-cli's own linked
  device id (from `signal-cli … listDevices`, an integer `>= 2`, e.g. `2`). It is
  defence-in-depth: even without it Simard already rejects her own echoes (only device 1
  may command), but setting it makes the own-device rejection explicit. Resolve it
  env-first with `SIMARD_SIGNAL_OWN_DEVICE_ID`. Omit it for a dedicated number. A present
  value that is unparseable **or `< 2`** is a hard config error (device 1 is your phone,
  so `own_device_id = 1` would disable Note to Self) — never a silent default.
- Set `read_only_unknown = true` only if you want non-allowlisted senders to be
  able to read `status`; they can never trigger a mutation.

## 5. Build and run Simard

The Signal channel is compiled into a stock build (the `signal` feature is a
default), so the plain build command already includes it:

```bash
cargo build
```

Then launch the Signal channel with the `signal run` subcommand:

```bash
simard signal run
```

On startup Simard reads the `[signal]` table, connects to the signal-cli endpoint,
and begins receiving inbound messages from allowlisted senders and sending
notifications out. Leave it running (systemd unit, tmux, or a supervisor) alongside
the signal-cli daemon from step 3. `simard signal run` exits when the signal-cli
socket closes; supervise it if you want it to reconnect. Only a deliberately
minimal `--no-default-features` build omits the Signal code; it still recognizes
`simard signal run` but tells you to rebuild with the feature.

## 6. Verify the round trip

Message Simard from your **operator** phone (an allowlisted number):

- **Dedicated number (Option B):** open the conversation with Simard's Signal
  `account` and send to it.
- **Single-number linked device (Option A):** open **Note to Self** (you are the
  account) and send there — that is the only way to reach Simard on a single number.

```text
status
```

Simard replies with daemon health and pause state. Then try a normal sentence — she
answers conversationally, exactly like a CLI/dashboard meeting:

```text
what are you working on right now?
```

To confirm the guardrail, message a high-risk command:

```text
merge #2531
```

Simard does **not** merge from the text. She creates a pending sign-off and replies
asking for explicit approval; replying `approve` records your sign-off and the
merge then proceeds through the gated
[merge authority](../reference/cross-repo-merge-authority.md). See the
[operational autonomy model](../concepts/operational-autonomy-model.md) for the full
HIGH-RISK boundary.

## Chatting via Note to Self (single-number setups)

When signal-cli is a **linked device on your own account** (Option A), you and Simard
share one number, so you command her from **Note to Self**. This section explains how
that works and — critically — how Simard avoids an infinite self-reply loop.

### Why Note to Self

A linked device receives everything the account does. When you type into **Note to
Self** on your phone, Signal syncs that to every linked device as a **sync-sent**
message (`syncMessage.sentMessage`) — a transcript of a message the account sent to
itself. signal-cli forwards it to Simard, who reads the body (`status`, a question,
`merge #NNNN`, …) and treats the **account itself** as the sender. Because the account
number is on the `allowlist`, the command is authorized and answered right back into
Note to Self.

> A dedicated-number setup (Option B) never uses this path: those are ordinary
> `dataMessage`s from a separate operator number and behave exactly as before.

### The loop problem — and the three guards

A linked device **also** receives sync-sent transcripts of the messages **Simard
herself** sends via signal-cli (her replies are sent *from* the account, so they sync
back too). Naïvely, Simard would read her own reply as a new command and answer it
forever. Three layered guards prevent this; a sync-sent message is accepted as a
command **only if all of them pass**:

1. **Primary-phone gate (device 1).** Signal guarantees the account owner's **phone is
   always device 1**; every linked device (signal-cli, Signal Desktop, an iPad) has a
   higher id. Simard accepts a Note-to-Self command **only** when the envelope's source
   device is **device 1** — i.e. it was typed on your phone. Her own replies originate
   from signal-cli's linked device (id ≥ 2) and are therefore rejected. This gate alone
   closes the loop, even with no extra configuration.
2. **Own-device rejection (defence-in-depth).** If you set `own_device_id` (step 4),
   Simard *also* explicitly rejects any sync-sent message whose source device equals
   signal-cli's own id. This is redundant with the device-1 gate by design.
3. **Recent-outbound echo suppression (defence-in-depth).** Simard remembers the bodies
   of the messages she just sent (a small, bounded, in-memory window — the last 64
   messages, expiring after 5 minutes) and ignores a sync-sent message whose body
   exactly matches one of them. This catches any echo the first two guards might miss.

Only **your primary phone's** Note-to-Self messages are newly accepted. Commands from
Signal Desktop or any other linked device are **not** honored — issue commands from
your phone. And a genuinely unknown sender is still dropped, fail-closed, exactly as
for a dedicated number.

> **Third-party syncs are ignored too.** When you text *someone else* from your phone,
> your linked device receives a sync-sent transcript of that message as well. Simard
> only treats a sync-sent message as a command when it is a **true Note to Self** — its
> destination is the account itself. Messages you send to other people never reach her
> command surface.

## What you can do over Signal

| You send | Simard does |
|----------|-------------|
| `status` | Reports daemon health + pause state (low-risk, immediate) |
| `pause` | Pauses autonomous dispatch (low-risk, immediate) |
| `approve` | Records your sign-off for the pending high-risk request |
| `deploy` | Requests a deploy → asks for sign-off (high-risk, gated) |
| `merge #NNNN` | Merges the PR via gated authority → asks for sign-off (high-risk) |
| anything else | Ordinary meeting turn — full conversation, `/goal` `/decision` `/action` capture, `/close` |

| Simard notifies you when |
|--------------------------|
| a governed PR is merge-ready |
| a stall or problem is detected |
| a high-risk action needs your sign-off |

## Troubleshooting

### Simard never receives my Signal messages

1. **Are you allowlisted?** Your sending number must be in `[signal].allowlist`
   (E.164, e.g. `+15559876543`). Unknown senders are dropped and logged at `debug`
   — enable debug logging to see the drop. On a **dedicated number** the allowlist is
   *your* number, not `account`; on a **single-number linked device** the allowlist is
   the `account` number itself (they are the same).
2. **Is the daemon reachable?** Confirm `signal-cli -a … daemon --tcp 127.0.0.1:7583`
   is running and `endpoint` matches host:port exactly.
3. **Built with the feature?** A default build already includes the Signal
   channel. You would only be missing it in a deliberately minimal
   `--no-default-features` build — rebuild with a plain `cargo build`.

### My Note-to-Self messages are ignored (single-number setup)

1. **Are you sending from your phone?** Simard accepts a Note-to-Self command **only**
   from **device 1** (your primary phone). Messages typed in **Note to Self** on Signal
   Desktop, a linked iPad, or any other linked device are rejected by the loop-prevention
   gate. Send from your phone.
2. **Is the account number allowlisted?** For a single-number linked device the sender
   *is* the `account`, so `account` must appear in `allowlist`.
3. **Is `own_device_id` set to the wrong id?** It must be signal-cli's **own** linked
   device id (≥ 2) from `listDevices`. A value `< 2` (e.g. `1`, your phone) is rejected
   at startup with a config error, because it would disable every Note-to-Self command.
   When in doubt, omit it — the device-1 gate already prevents loops.

### Simard receives but never replies

Check that `account` is the number signal-cli is registered/linked as, and that
the daemon can send (test with `signal-cli -a … send -m "hi" <your-number>`).

### A high-risk command "did nothing"

That is the guardrail working. `deploy` and `merge #NNNN` never auto-execute from a
text — Simard creates a pending sign-off and asks for `approve`. Reply `approve` to
authorize; the action then runs through its existing gated authority.

### The daemon runs but I never wanted Signal

The Signal channel is built by default, but it stays dormant until you provide a
`[signal]` table (with a fail-closed, empty-by-default allowlist) — without it,
`simard signal run` simply reports missing config and Simard never touches
signal-cli. If you want a binary with no Signal code at all, build with
`--no-default-features`.

## Related reading

- [Signal channel reference](../reference/signal-conversation.md) — config keys,
  commands, notifications, and guardrails in full.
- [Conversation channels](../architecture/conversation-channel.md) — the shared
  abstraction the Signal channel implements.
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the
  HIGH-RISK boundary the Signal gating reuses.
- [How to start a meeting with Simard](./start-a-meeting.md) — the same meeting
  experience over CLI and dashboard.
