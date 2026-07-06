---
title: How to configure Overseer email notifications
description: >
  Wire the Overseer's email channel to deliver operator notifications (merges, deploys,
  and blocked "needs human review" goal escalations) to a real inbox via an authenticated
  SMTP relay. Worked example uses smtp.office365.com:587 STARTTLS + AUTH LOGIN to reach
  rysweet@microsoft.com, with the credentials set in the systemd unit (never hardcoded).
  Covers the exact environment variables, the systemd drop-in, verification, and how the
  Signal channel already provides the primary reliable path.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
issues: ["#2631"]
related:
  - ../index.md
  - ../reference/overseer-operator-notifications.md
  - ./set-up-the-signal-channel.md
  - ../reference/signal-conversation.md
  - ../design/overseer.md
  - ../concepts/operational-autonomy-model.md
---

# How to configure Overseer email notifications

The Overseer notifies the operator on every merge, deploy, whisper, and — critically —
every blocked **"needs human review"** goal escalation, on **both** Signal and email.
Signal is
the **primary reliable path**: once the [Signal channel](./set-up-the-signal-channel.md)
is wired, the operator is always reached on their phone. This guide adds the **email**
channel so the same notice also lands in an inbox.

Delivering to a `microsoft.com` address requires an **authenticated SMTP relay**. This
guide uses **office365** (`smtp.office365.com:587`, STARTTLS + AUTH LOGIN) as the worked
example; an internal relay works the same way. All credentials come from the
**environment** (set in the systemd unit) — never from source, never hardcoded.

For the full delivery contract (channels, semantics, the Signal marker), see the
[Overseer operator-notification reference](../reference/overseer-operator-notifications.md).

## Prerequisites

- [ ] The Signal channel is already wired (the primary path) — see
      [Set up the Signal channel](./set-up-the-signal-channel.md). Email is additive.
- [ ] A mailbox on the relay that is **authorized to send** (office365: a licensed
      mailbox or one with SMTP AUTH enabled; e.g.
      `simard-overseer@contoso.onmicrosoft.com`).
- [ ] Its password or, preferably, an **app credential** for that mailbox.
- [ ] The Overseer runs under systemd (this guide edits its unit).

> **office365 note.** SMTP AUTH must be enabled for the sending mailbox (tenant-wide
> "Authenticated SMTP" is off by default). If your tenant enforces modern auth only, use
> an app credential or an internal relay that accepts the mailbox.

## 1. The environment variables

The email channel is configured **entirely from the environment**
([`EmailConfig::from_env`](../reference/overseer-operator-notifications.md#environment-variables-the-complete-set)):

| Variable | Value in this example |
|----------|-----------------------|
| `SIMARD_OVERSEER_EMAIL_TO` | `rysweet@microsoft.com` |
| `SIMARD_OVERSEER_EMAIL_FROM` | `simard-overseer@contoso.onmicrosoft.com` |
| `SMTP_HOST` | `smtp.office365.com` |
| `SMTP_PORT` | `587` |
| `SMTP_USER` | `simard-overseer@contoso.onmicrosoft.com` |
| `SMTP_PASS` | *(the mailbox password / app credential — secret)* |

Rules:

- `SIMARD_OVERSEER_EMAIL_TO` may be a **comma-separated** list for multiple recipients.
- When `SMTP_USER` **and** `SMTP_PASS` are both set, the channel uses the **STARTTLS +
  AUTH LOGIN** sender (`StartTlsSmtpSender`) — encrypted and authenticated. Without them,
  it falls back to a minimal plaintext sender for a local `:25` MTA.
- **Never put the password in source, a committed file, or a shell history.** Set it only
  in the systemd unit (below), ideally via a root-only credential file.

## 2. Add the variables to the systemd unit

Use a **drop-in** so you do not edit the shipped unit. Put the secret in a root-only
environment file.

Create `/etc/simard/overseer-email.env` (mode `0600`, owned by root):

```ini
SIMARD_OVERSEER_EMAIL_TO=rysweet@microsoft.com
SIMARD_OVERSEER_EMAIL_FROM=simard-overseer@contoso.onmicrosoft.com
SMTP_HOST=smtp.office365.com
SMTP_PORT=587
SMTP_USER=simard-overseer@contoso.onmicrosoft.com
SMTP_PASS=<your-office365-app-password>
```

```bash
sudo install -m 0600 /dev/stdin /etc/simard/overseer-email.env <<'EOF'
SIMARD_OVERSEER_EMAIL_TO=rysweet@microsoft.com
SIMARD_OVERSEER_EMAIL_FROM=simard-overseer@contoso.onmicrosoft.com
SMTP_HOST=smtp.office365.com
SMTP_PORT=587
SMTP_USER=simard-overseer@contoso.onmicrosoft.com
SMTP_PASS=<your-office365-app-password>
EOF
```

Reference it from a drop-in — `systemctl edit simard-overseer.service`:

```ini
[Service]
EnvironmentFile=/etc/simard/overseer-email.env
```

This single file holds every variable from step 1, so one `EnvironmentFile=` line is
enough. If you prefer to keep non-secret defaults in their own file, add a **second** line
with a leading `-`:

```ini
[Service]
EnvironmentFile=-/etc/simard/overseer-email.service.env
EnvironmentFile=/etc/simard/overseer-email.env
```

The leading `-` makes systemd treat that file as optional, so the unit still starts if you
never create it. **Without the `-`, a missing `EnvironmentFile=` path is fatal and the
Overseer service fails to start.**

Reload and restart:

```bash
sudo systemctl daemon-reload
sudo systemctl restart simard-overseer.service
```

## 3. Verify delivery

Trigger (or wait for) any operator notification — the easiest is a blocked-goal
escalation, but a merge notification works too. Then check the structured delivery log on
target `overseer::notify`:

```bash
journalctl -u simard-overseer.service -o cat | grep 'overseer::notify' | tail
```

A **successful both-channel** delivery shows:

```text
target: overseer::notify  dispatched=true all_sent=true  signal=Sent email=Sent kind=goal-blocked
```

If email is still queued or failing, you will instead see:

```text
target: overseer::notify  dispatched=true all_sent=false signal=Sent email=Queued kind=goal-blocked
# or, on a transport/auth error:
target: overseer::notify  dispatched=true all_sent=false signal=Sent email=Failed kind=goal-blocked
```

`dispatched=true` confirms the operator **was reached** (Signal delivered);
`all_sent=false` is the honest report that email did not deliver yet. Then confirm the
message arrived at `rysweet@microsoft.com`.

## Understanding the result

| You see | Meaning | Action |
|---------|---------|--------|
| `email=Sent` | The relay accepted the message. | Done — check the inbox. |
| `email=Queued` | SMTP is not fully configured (missing host/from/recipient). | Re-check the six variables in step 1. |
| `email=Failed` (auth) | The relay rejected `AUTH LOGIN`. | Verify `SMTP_USER`/`SMTP_PASS`; enable SMTP AUTH / use an app credential. |
| `email=Failed` (STARTTLS) | The relay did not offer STARTTLS but credentials are set. | The sender **refuses** to send auth in the clear. Use a relay that supports STARTTLS on `587`. |

> **Why email never "silently" fails.** An unconfigured channel is `Queued` (logged); a
> transport error is `Failed` (logged). There is no path that drops the notification. And
> because Signal is the wired primary path, the escalation is always **dispatched** even
> before email works — see the
> [semantics table](../reference/overseer-operator-notifications.md#part-c-delivery-semantics-all_sent-vs-dispatched).

## Security notes

- **Credentials are env-only.** `SMTP_USER` / `SMTP_PASS` live in the root-only
  `EnvironmentFile`, never in source, a repo, or logs. The Overseer never logs the
  password or the base64 AUTH payload.
- **Encryption is mandatory when authenticating.** With credentials set, the sender uses
  STARTTLS with standard certificate verification and **refuses** to fall back to
  plaintext AUTH (base64 is not encryption).
- **Header injection is blocked.** Subjects/headers derived from GitHub content (PR
  titles) are sanitized (CR/LF stripped), so a crafted PR title cannot inject extra SMTP
  headers.

## Related reading

- [Overseer operator-notification reference](../reference/overseer-operator-notifications.md)
  — channels, the Signal marker, `all_sent` vs `dispatched`, and the API surface.
- [Set up the Signal channel](./set-up-the-signal-channel.md) — the primary reliable
  path (do this first).
- [Overseer design](../design/overseer.md) — where the notifier sits in the merge/deploy
  path.
