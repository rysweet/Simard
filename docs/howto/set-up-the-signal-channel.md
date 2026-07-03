---
title: How to set up the Signal channel
description: Connect Simard to Signal so an allowlisted operator can command her and receive notifications, using a locally-run signal-cli JSON-RPC daemon. Covers the signal feature build, linking a device (or using a dedicated number), configuration, the allowlist, and verification.
last_updated: 2026-07-03
review_schedule: as-needed
owner: simard
doc_type: howto
issues: ["#2527"]
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
guide installs signal-cli, links or registers an account, builds Simard with the
`signal` feature, configures the `[signal]` table, and verifies the round trip.

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

> The Signal channel is **optional and off by default.** If you skip this guide,
> Simard's daemon runs exactly as before with no Signal code compiled in.

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
The `[signal]` table is only parsed and applied in a `--features signal` build
(step 5) — a default build ignores it.

```toml
[signal]
endpoint = "127.0.0.1:7583"     # the signal-cli daemon from step 3
account  = "+15551234567"        # the linked/dedicated account from step 2
allowlist = ["+15559876543"]     # YOUR operator number(s) — who may command Simard
read_only_unknown = false        # keep unknown senders fully ignored (default)
```

- **`allowlist` is the security boundary.** Only the E.164 numbers listed here may
  command Simard. It is **fail-closed**: an empty or missing allowlist means
  *nobody* may command her. Put the operator's own phone number here — the number
  you message Simard *from*, which is different from `account` (the number Simard
  receives *on*).
- Set `read_only_unknown = true` only if you want non-allowlisted senders to be
  able to read `status`; they can never trigger a mutation.

## 5. Build and run Simard with the `signal` feature

The Signal channel compiles only with the `signal` feature:

```bash
cargo build --features signal
```

Then run the daemon (or your normal Simard entrypoint) from that build. On startup
Simard reads `[signal]`, connects to the signal-cli endpoint, and begins receiving
inbound messages from allowlisted senders and sending notifications out.

## 6. Verify the round trip

From your **operator** phone (an allowlisted number), message Simard's Signal
account:

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
   — enable debug logging to see the drop. `allowlist` is *your* number, not
   `account`.
2. **Is the daemon reachable?** Confirm `signal-cli -a … daemon --tcp 127.0.0.1:7583`
   is running and `endpoint` matches host:port exactly.
3. **Built with the feature?** A default build has no Signal code. Rebuild with
   `cargo build --features signal`.

### Simard receives but never replies

Check that `account` is the number signal-cli is registered/linked as, and that
the daemon can send (test with `signal-cli -a … send -m "hi" <your-number>`).

### A high-risk command "did nothing"

That is the guardrail working. `deploy` and `merge #NNNN` never auto-execute from a
text — Simard creates a pending sign-off and asks for `approve`. Reply `approve` to
authorize; the action then runs through its existing gated authority.

### The daemon runs but I never wanted Signal

Nothing to do — the Signal channel is off unless you build with `--features signal`
*and* provide a `[signal]` table. A plain `cargo build` has no Signal code and needs
no signal-cli.

## Related reading

- [Signal channel reference](../reference/signal-conversation.md) — config keys,
  commands, notifications, and guardrails in full.
- [Conversation channels](../architecture/conversation-channel.md) — the shared
  abstraction the Signal channel implements.
- [Operational autonomy model](../concepts/operational-autonomy-model.md) — the
  HIGH-RISK boundary the Signal gating reuses.
- [How to start a meeting with Simard](./start-a-meeting.md) — the same meeting
  experience over CLI and dashboard.
