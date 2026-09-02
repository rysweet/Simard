---
title: How to configure Overseer Signal notifications
description: >
  Wire the Overseer's LIVE Signal channel so operator notifications (merges — including
  autonomous merges — deploys, and blocked "needs human review" goal escalations) are
  actually DELIVERED to the operator's phone, not just queued. Sets the three
  SIMARD_SIGNAL_RPC_* environment variables in the Overseer's systemd drop-in so
  SignalNotifyChannel::from_env constructs the JsonRpcSignalSender that POSTs to the local
  Signal JSON-RPC service at 127.0.0.1:7583. Covers the variables, the drop-in, and how to
  verify the journal shows outcome=Sent instead of Queued.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
issues: ["#4178"]
related:
  - ../index.md
  - ../reference/overseer-signal-jsonrpc-transport.md
  - ../reference/overseer-operator-notifications.md
  - ./configure-overseer-email-notifications.md
  - ./set-up-the-signal-channel.md
  - ../design/overseer.md
---

# How to configure Overseer Signal notifications

The Overseer notifies the operator on **every** merge, deploy, whisper, and — critically —
every blocked **"needs human review"** goal escalation. Signal is the **primary reliable
path**: the notification must actually reach the operator's phone.

This guide wires the **live Signal transport** so those notifications are **delivered**,
not merely queued. It sets three environment variables in the Overseer's systemd unit;
with them present, `SignalNotifyChannel::from_env` constructs a
[`JsonRpcSignalSender`](../reference/overseer-signal-jsonrpc-transport.md) that **POSTs**
each notification to the local Signal JSON-RPC service. Without them, the channel keeps its
fail-safe `Queued` behavior (logged, never dropped).

For the full transport contract (wire format, timeouts, `Sent`/`Queued`/`Failed`
mapping), see the
[Signal JSON-RPC transport reference](../reference/overseer-signal-jsonrpc-transport.md).

## Prerequisites

- [ ] A **Signal JSON-RPC service running locally** on the daemon host, exposing a `send`
      method over TCP (default `127.0.0.1:7583`) — the same service the operator's tooling
      uses. Confirm it is listening:

      ```bash
      ss -ltn 'sport = :7583'
      ```

- [ ] The operator's **registered Signal number** for both the sending account and the
      recipient (E.164 form, e.g. `+15551234567`).
- [ ] The Overseer runs under systemd (this guide edits its unit).

## 1. The environment variables

The live Signal transport is configured **entirely from the environment**
([`SignalRpcConfig::from_env`](../reference/overseer-signal-jsonrpc-transport.md#environment-variables-the-complete-set)):

| Variable | Value in this example | Required |
|----------|-----------------------|:--------:|
| `SIMARD_SIGNAL_RPC_ACCOUNT` | `+15551234567` (the Signal number to send **as**) | ✅ |
| `SIMARD_SIGNAL_RPC_RECIPIENT` | `+15551234567` (the operator's Signal number to send **to**) | ✅ |
| `SIMARD_SIGNAL_RPC_ADDR` | `127.0.0.1:7583` (default — omit unless the service moved) | — |

Rules:

- The channel is considered **configured** only when **both** `SIMARD_SIGNAL_RPC_ACCOUNT`
  **and** `SIMARD_SIGNAL_RPC_RECIPIENT` are set. Setting only one keeps the channel in the
  `Queued` fallback.
- `SIMARD_SIGNAL_RPC_ADDR` defaults to the loopback `127.0.0.1:7583`. Leave it unset unless
  the Signal service listens elsewhere. Point it at a **loopback** address; a non-loopback
  target is an explicit operator choice.
- For a single-number setup, the account and recipient are typically the **same** operator
  Signal number.

## 2. Add the variables to the systemd unit

Use a **drop-in** so you do not edit the shipped unit.

Create `/etc/simard/overseer-signal.env` (mode `0644`, owned by root — these values are
not secrets, but keep them with the unit):

```ini
SIMARD_SIGNAL_RPC_ACCOUNT=+15551234567
SIMARD_SIGNAL_RPC_RECIPIENT=+15551234567
# Optional — only if the Signal service is not on the default loopback port:
# SIMARD_SIGNAL_RPC_ADDR=127.0.0.1:7583
```

```bash
sudo install -m 0644 /dev/stdin /etc/simard/overseer-signal.env <<'EOF'
SIMARD_SIGNAL_RPC_ACCOUNT=+15551234567
SIMARD_SIGNAL_RPC_RECIPIENT=+15551234567
EOF
```

Reference it from a drop-in — `systemctl edit simard-overseer.service`:

```ini
[Service]
EnvironmentFile=/etc/simard/overseer-signal.env
```

If you also configured [email notifications](./configure-overseer-email-notifications.md),
add a **second** `EnvironmentFile=` line — each file is independent:

```ini
[Service]
EnvironmentFile=/etc/simard/overseer-signal.env
EnvironmentFile=/etc/simard/overseer-email.env
```

> **A missing `EnvironmentFile=` path is fatal.** Without a leading `-`, systemd refuses
> to start the unit if the file is absent. Prefix the path with `-` to make it optional.

Reload and restart:

```bash
sudo systemctl daemon-reload
sudo systemctl restart simard-overseer.service
```

## 3. Verify delivery

Trigger (or wait for) any operator notification — an autonomous merge or a blocked-goal
escalation both work. Then check the structured delivery log on target `overseer::notify`:

```bash
journalctl -u simard-overseer.service -o cat | grep 'overseer::notify' | tail
```

A **live Signal delivery** now shows `signal=Sent`:

```text
target: overseer::notify  dispatched=true all_sent=true  signal=Sent email=Queued kind=merge
```

Before this was wired, the same merge produced a **queued** line (the bug this guide
fixes) — the old reason string named an internal transport rather than a knob you can set:

```text
target: overseer::notify  operator notification not delivered live — queued/failed (never dropped)
  channel="signal" kind="merge" outcome=Queued { reason: "Signal channel not wired (configure the ConversationChannel transport)" }
```

If you leave the account/recipient unset today, you still get a queued line, but the reason
now names the two variables to set:

```text
  channel="signal" kind="merge" outcome=Queued { reason: "Signal RPC not configured (set SIMARD_SIGNAL_RPC_ACCOUNT and SIMARD_SIGNAL_RPC_RECIPIENT)" }
```

Then confirm the notice arrived on the operator's phone.

## Understanding the result

| You see | Meaning | Action |
|---------|---------|--------|
| `signal=Sent` | The Signal service accepted the POST. | Done — check the phone. |
| `signal=Queued` | Account and/or recipient not set. | Re-check both variables in step 1; confirm the drop-in is loaded (`systemctl show simard-overseer -p Environment`). |
| `signal=Failed` (connect refused) | The Signal service is not listening at the address. | Start the service; verify `SIMARD_SIGNAL_RPC_ADDR` and `ss -ltn 'sport = :7583'`. |
| `signal=Failed` (timeout) | The service accepted the connection but did not reply within 5s. | Check the Signal service health/logs. The merge path already continued — it never hangs. |
| `signal=Failed` (JSON-RPC error) | The service returned an `error` (e.g. unregistered account). | Verify the account is registered with the Signal service and the recipient number is valid. |

> **Why Signal never "silently" fails.** An unconfigured channel is `Queued` (logged); a
> transport or JSON-RPC error is `Failed` (logged). There is no path that drops the
> notification. See the
> [delivery-semantics table](../reference/overseer-operator-notifications.md#part-c-delivery-semantics-all_sent-vs-dispatched).

## Security notes

- **Loopback by default.** The transport targets `127.0.0.1:7583`; a non-loopback address
  is only ever an explicit operator override.
- **No new secrets, no async, no new deps.** The transport is a plain `std::net` +
  `serde_json` JSON-RPC client with 5-second connect/read/write timeouts, so a hung Signal
  service cannot stall a merge. It does not add `tokio`, `reqwest`, or require the `signal`
  cargo feature.
- **Notification text is JSON-escaped.** The request is built with `serde_json`, so a
  crafted PR title in a notification body cannot break the JSON-RPC frame. The body still
  passes through the anti-self-ingest marker path unchanged.

## Related reading

- [Signal JSON-RPC transport reference](../reference/overseer-signal-jsonrpc-transport.md)
  — the wire format, timeouts, and `Sent`/`Queued`/`Failed` mapping.
- [Overseer operator-notification reference](../reference/overseer-operator-notifications.md)
  — the two-channel contract, the Signal marker, and `all_sent` vs `dispatched`.
- [Configure Overseer email notifications](./configure-overseer-email-notifications.md) —
  the secondary durable channel.
- [Overseer design](../design/overseer.md) — where the notifier sits in the merge/deploy
  path.
