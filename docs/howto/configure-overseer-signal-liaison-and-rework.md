---
title: How to configure the Overseer Signal liaison & PR rework loop
description: >
  Enable the two capabilities that let the Overseer run self-sufficiently, so the
  external human steward can retire: (1) the native Signal operator-liaison
  (receive/answer operator-group messages and launch fixes) via
  SIMARD_OVERSEER_SIGNAL_LIAISON, and (2) the autonomous PR rework loop (rework a
  fixable held PR, re-review with the same judge, merge or escalate) via
  SIMARD_OVERSEER_REWORK. Both are opt-in and default OFF. Covers the exact
  environment variables, a systemd drop-in, safe rollout, verification, and how
  to turn each capability back off.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/overseer-signal-liaison.md
  - ../reference/overseer-rework-loop.md
  - ../reference/merge-record-verdict-cli.md
  - ../reference/overseer-signal-jsonrpc-transport.md
  - ../reference/state-root-resolution.md
  - ./set-up-the-signal-channel.md
  - ./configure-overseer-email-notifications.md
  - ../design/overseer.md
  - ../concepts/agentic-recipes-first-principle.md
---

# How to configure the Overseer Signal liaison & PR rework loop

These two capabilities make the Overseer **self-sufficient** so the external
human-in-the-loop steward can retire. Both are **opt-in** and **default OFF**;
both are gated by the master `SIMARD_OVERSEER_ENABLED`. Turn them on
independently once the underlying Signal channel and merge authority are proven.

- **Signal operator-liaison** — the Overseer receives operator-group Signal
  messages, answers them in plain English on the same group, and launches a fix
  recipe when the message is a go-ahead. Replaces the hand-run python listener.
  Full contract: [Overseer Signal operator-liaison](../reference/overseer-signal-liaison.md).
- **PR rework loop** — on a *fixable* merge hold the Overseer reworks the PR,
  re-reviews it with the same merge-judge, and merges it (or escalates after a
  capped number of attempts). Full contract:
  [Overseer autonomous PR rework loop](../reference/overseer-rework-loop.md).

## Prerequisites

1. The **Signal channel** is wired and the signal-cli JSON-RPC daemon is running
   loopback-only on `127.0.0.1:7583` — see
   [Set up the Signal channel](./set-up-the-signal-channel.md).
2. The Overseer is enabled: `SIMARD_OVERSEER_ENABLED=1`.
3. The Overseer runs under a **distinct identity** (`SIMARD_OVERSEER_AUTHOR_LOGIN`
   set, and **not** the human operator's login) so the recursion guard admits
   real PRs while refusing the Overseer's own.
4. For the rework loop: the
   [gated merge authority](../reference/cross-repo-merge-authority.md) and
   [merge-verdict record tool](../reference/merge-record-verdict-cli.md) are in
   place (they are — the rework loop extends them).

## Enable the Signal operator-liaison

Set these environment variables (worked example — use your real operator number
and group id):

```bash
# master switch (already on for an active Overseer)
SIMARD_OVERSEER_ENABLED=1

# the liaison itself (default OFF — explicit truthy to enable)
SIMARD_OVERSEER_SIGNAL_LIAISON=1

# who may drive the Overseer over Signal, and where it listens/replies
SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER=+15551234567
SIMARD_OVERSEER_SIGNAL_GROUP_ID=cognitive-threads-group-id==

# distinct Overseer identity for the recursion guard
SIMARD_OVERSEER_AUTHOR_LOGIN=simard-overseer
```

- `SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER` is the authorized operator's **E.164**
  number. Authorization is fail-closed and exact-match; an empty/unset value
  authorizes nobody.
- `SIMARD_OVERSEER_SIGNAL_GROUP_ID` is the operator group the liaison receives on
  **and** replies to. Direct (non-group) messages are ignored.

### systemd drop-in

Add to the daemon unit's environment (never hardcode secrets in source):

```ini
# /etc/systemd/system/simard-ooda.service.d/overseer-liaison.conf
[Service]
Environment=SIMARD_OVERSEER_ENABLED=1
Environment=SIMARD_OVERSEER_SIGNAL_LIAISON=1
Environment=SIMARD_OVERSEER_SIGNAL_OPERATOR_NUMBER=+15551234567
Environment=SIMARD_OVERSEER_SIGNAL_GROUP_ID=cognitive-threads-group-id==
Environment=SIMARD_OVERSEER_AUTHOR_LOGIN=simard-overseer
```

```bash
sudo systemctl daemon-reload
sudo systemctl restart simard-ooda
```

### Verify

1. Post a message in the operator group from the authorized number, e.g.
   *"What's the current merge-queue depth?"*
2. Within a tick you should get a plain-English reply **in the same group**.
3. A go-ahead like *"go ahead and fix the flaky deploy canary"* should also
   launch a recipe. Confirm in the journal (bodies/numbers are redacted):

```bash
journalctl --user -u simard-ooda -f | grep -i '\[simard\].*liaison'
```

4. Confirm the durable decision record was written owner-only:

```bash
# The state root resolves to $SIMARD_STATE_ROOT (if set, absolute) or ~/.simard.
# See reference/state-root-resolution.md for the full resolution ladder.
STATE_ROOT="${SIMARD_STATE_ROOT:-$HOME/.simard}"
ls -l "$STATE_ROOT"/liaison_decisions/*/*.json   # mode 0600
```

Each operator message is handled **once** (durable high-water-mark); the
Overseer never answers its own posts (echo-suppression).

## Enable the PR rework loop

```bash
SIMARD_OVERSEER_ENABLED=1

# the rework loop (default OFF)
SIMARD_OVERSEER_REWORK=1

# per-PR attempt cap — default 3, clamped to 1..=10
SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS=3
```

systemd drop-in:

```ini
# /etc/systemd/system/simard-ooda.service.d/overseer-rework.conf
[Service]
Environment=SIMARD_OVERSEER_ENABLED=1
Environment=SIMARD_OVERSEER_REWORK=1
Environment=SIMARD_OVERSEER_REWORK_MAX_ATTEMPTS=3
```

```bash
sudo systemctl daemon-reload && sudo systemctl restart simard-ooda
```

### What happens with the flag on

When the `merge-readiness-judge` records a **fixable** hold (verdict `hold` +
`reworkable=true` + a `concern`), the Overseer:

1. reads the typed verdict fail-closed;
2. dispatches `default-workflow` on the PR's branch, passing the concern as a
   ContextFile (never argv);
3. re-runs the **same** judge on the next tick;
4. loops until the judge merges (squash-only) or the attempt cap is hit — then
   escalates to a human.

With the flag **off**, a fixable hold escalates to a human immediately (the prior
behavior).

### Verify

```bash
# State root resolves to $SIMARD_STATE_ROOT (if set, absolute) or ~/.simard;
# see reference/state-root-resolution.md.
STATE_ROOT="${SIMARD_STATE_ROOT:-$HOME/.simard}"

# the extended verdict record carries the new fields
cat "$STATE_ROOT"/merge_verdicts/*/<pr>.json | jq '{verdict, reworkable, concern}'

# the monotonic per-PR attempt counter
cat "$STATE_ROOT"/overseer/rework_attempts/*/<pr>.json

# rework dispatches and escalations in the journal
journalctl --user -u simard-ooda | grep -i 'rework_pr\|rework.*escalat'
```

## Safe rollout

- **Turn one capability on at a time.** Watch a few ticks before enabling the
  other.
- **Keep the attempt cap small.** The default `3` is deliberate; values outside
  `1..=10` are clamped, not honored.
- **Confirm the distinct identity.** If `SIMARD_OVERSEER_AUTHOR_LOGIN` is unset,
  the recursion guard fails **closed** and no reworks are dispatched — set it.
- **Merge policy is preserved.** Both capabilities keep squash-only merges,
  never `--admin`/`--no-verify`, all objective + pr-verify + security gates, and
  the email + Signal merge notification. The agent never gets to weaken these.

## Turn it back off

Remove or set the relevant flag to a falsey value and restart:

```bash
# disable the liaison
SIMARD_OVERSEER_SIGNAL_LIAISON=0
# disable the rework loop
SIMARD_OVERSEER_REWORK=0
```

```bash
sudo systemctl daemon-reload && sudo systemctl restart simard-ooda
```

With both off, the Overseer neither ingests operator-group messages nor reworks
held PRs — it behaves exactly as before these capabilities shipped.

## Related

- [Overseer Signal operator-liaison](../reference/overseer-signal-liaison.md)
- [Overseer autonomous PR rework loop](../reference/overseer-rework-loop.md)
- [Merge verdict record CLI & deterministic merge rail](../reference/merge-record-verdict-cli.md)
- [Set up the Signal channel](./set-up-the-signal-channel.md)
- [Configure Overseer email notifications](./configure-overseer-email-notifications.md)
- [Overseer — operator/observer co-process (design)](../design/overseer.md)
- [Agentic-recipes-first principle](../concepts/agentic-recipes-first-principle.md)
