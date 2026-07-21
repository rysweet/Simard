---
title: How to configure and inspect the Overseer deploy throttle
description: >
  Operator runbook for the restart-durable, fail-closed self-deploy anti-thrash
  throttle (#4390) — tune the backoff base and cap, read the durable
  deploy-attempt ledger, interpret the stuck-state warning, and safely clear a
  known-bad commit's backoff.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/deploy-anti-thrash-throttle.md
  - ../reference/overseer-deploy-throttle-api.md
  - ../reference/self-deploy-api.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../reference/state-root-resolution.md
  - ../safe-self-update.md
---

# How to configure and inspect the Overseer deploy throttle

> **Status: implemented ([#4390](https://github.com/rysweet/Simard/issues/4390)).**
> The durable `DeployAttemptLedger` and its knobs exist today in
> [`src/overseer/deploy_throttle.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_throttle.rs)
> and [`src/overseer/deploy_trigger.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/deploy_trigger.rs).
> For the design see the
> [deploy anti-thrash throttle concept](../concepts/deploy-anti-thrash-throttle.md);
> for the full typed surface, the
> [API reference](../reference/overseer-deploy-throttle-api.md).

This guide is for an operator who wants to tune, inspect, or recover the
autonomous self-deploy throttle — the rail that stops a red-canary commit from
being re-deployed every overseer tick.

## When you need this

- The operator dashboard / logs show a repeating
  `deploy_throttle.stuck=true` warning for one commit.
- A merged self-change is **not** deploying and you want to know whether the
  throttle is (correctly) backing it off.
- You want to change how aggressively the daemon backs off a failing deploy.

## Tune the throttle

All knobs are environment variables read fail-safe (an unset/unparseable value
uses the default; out-of-range values are clamped). Set them in the daemon's
environment (e.g. the systemd unit) and restart the daemon.

| Env var | Default | Effect |
|---------|---------|--------|
| `SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS` | `900` (15 min) | Layer-1 min-interval **and** the backoff `base`. Floored at `60`. |
| `SIMARD_OVERSEER_AUTONOMOUS_DEPLOY` | enabled | Set `0`/`false`/`off`/`no` to **pin** the daemon (no autonomous deploy at all; the ledger is never consulted). |

The backoff **cap** is a fixed 6 h (`21600 s`) constant, not an env knob — tune
aggressiveness via the `base` (min-interval) instead.

Example — back off faster from a 10-minute base:

```bash
export SIMARD_OVERSEER_DEPLOY_MIN_INTERVAL_SECS=600   # 10-min base
```

The backoff grows exponentially per consecutive failure:
`min(base * 2^(failures-1), cap)`. With the defaults that is 15 min → 30 → 60 →
120 → 240 → 360 (capped).

## Inspect the durable ledger

The ledger is a small JSON file under the shared state root
([state-root resolution](../reference/state-root-resolution.md)):

```bash
cat ~/.simard/state/deploy-attempt-ledger.json | jq .
```

```json
{
  "version": 1,
  "entries": {
    "56b10bef5057…": {
      "failure_count": 5,
      "last_attempt_unix_secs": 1753118280,
      "backoff_until_unix_secs": 1753132680,
      "last_deploy_result": "failed"
    }
  }
}
```

Read it as:

- `failure_count` — how many consecutive canary/deploy failures for this commit.
- `backoff_until_unix_secs` — the commit is suppressed until this epoch-second.
  Convert with `date -d @1753132680`.
- `last_deploy_result` — `failed` means the canary was red; `succeeded` means the
  entry is cleared and the commit is eligible again.

## Interpret the stuck-state warning

On every suppressed tick the daemon logs one structured warning:

```text
WARN deploy_throttle.stuck=true target_sha=56b10bef5057…
     failure_count=5 backoff_until=1753132680 reason=backing_off
```

| `reason` | Meaning | What to do |
|----------|---------|------------|
| `backing_off` | The commit failed recently and is inside its backoff window. | Expected. It will retry after `backoff_until`. Fix the underlying red canary if it keeps failing. |
| `unreadable` | The ledger file is present but corrupt/torn. | Fail-closed for the candidate SHA. Inspect the file; see [recover](#recover-a-corrupt-ledger). (A *missing* file is not this — it loads empty and allows.) |
| `ambiguous` | A record exists but its result is unset. | Fail-closed. Usually transient; resolves once a terminal result is recorded. |

A single throttled warning per tick is the healthy signal — it replaces the old
stream of identical red-canary deploy attempts.

## Force a stuck commit to re-attempt now

The throttle is a backoff, not a permanent lock — a commit becomes eligible
again after `backoff_until`, or immediately once its canary goes green (a real
fix merges a new SHA, which is a fresh, un-throttled entry). Prefer fixing the
red canary over clearing the backoff.

If you must force an immediate re-attempt of the *same* SHA (e.g. after fixing an
environmental cause), stop the daemon, remove that commit's entry, and restart:

```bash
# Stop the daemon first so the write is not raced.
jq 'del(.entries["56b10bef5057…"])' \
  ~/.simard/state/deploy-attempt-ledger.json > /tmp/ledger.json \
  && mv /tmp/ledger.json ~/.simard/state/deploy-attempt-ledger.json
# Restart the daemon.
```

The next OBSERVE tick will treat the SHA as never-attempted and (subject to
layer-1 min-interval) re-attempt the deploy.

## Recover a corrupt ledger

A `reason=unreadable` warning means the file failed to deserialize; the throttle
is fail-closed for the drifting candidate SHA until it is readable again. The file
is written atomically (tmp+rename), so corruption is rare, but to recover:

```bash
# Stop the daemon, back up, and reset to an empty ledger.
mv ~/.simard/state/deploy-attempt-ledger.json \
   ~/.simard/state/deploy-attempt-ledger.json.corrupt.$(date +%s)
echo '{"version":1,"entries":{}}' > ~/.simard/state/deploy-attempt-ledger.json
chmod 600 ~/.simard/state/deploy-attempt-ledger.json
# Restart the daemon.
```

Resetting to empty is safe: a genuinely red canary will simply fail its next
attempt and re-populate the ledger with a fresh backoff.

## Pin the daemon entirely

To stop *all* autonomous self-deploy (throttle included) while you investigate:

```bash
export SIMARD_OVERSEER_AUTONOMOUS_DEPLOY=0
# Restart the daemon; deploy drift is now observed but never acted on.
```

See [verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
for the manual operator deploy path while pinned.

## See also

- [Deploy anti-thrash throttle (concept)](../concepts/deploy-anti-thrash-throttle.md)
- [Overseer durable deploy anti-thrash throttle API](../reference/overseer-deploy-throttle-api.md)
- [Verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md)
- [Self-Deploy API](../reference/self-deploy-api.md)
