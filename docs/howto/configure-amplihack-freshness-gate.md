---
title: Configure the amplihack freshness gate
description: Operator guide to Simard's pre-spawn `amplihack update` gate — enable or disable it, tune the TTL, require strict freshness, verify the gate ran from the tracing lines, state file, and metric, diagnose a failed update, and understand serialize/dedup during a spawn burst.
last_updated: 2026-07-04
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/amplihack-freshness-gate.md
  - ../reference/amplihack-freshness-gate.md
  - ./check-for-updates.md
  - ../reference/state-root-resolution.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
---

# Configure the amplihack freshness gate

Simard runs `amplihack update` immediately before it spawns each engineer (and
once at daemon startup) so every engineer runs on a freshly-updated
`amplihack-rs`. This guide covers the operator tasks: turning the gate on or off,
tuning the dedup window, requiring strict freshness, verifying the gate actually
ran, and diagnosing a failed update.

For the design rationale see
[the concept page](../concepts/amplihack-freshness-gate.md); for the full
contract see
[the reference](../reference/amplihack-freshness-gate.md).

## 1. Enable or disable the gate

The gate is **on by default** — no configuration is required to get
fresh-before-every-spawn behaviour. To disable it (engineers then run on whatever
`amplihack` is already installed):

```bash
# Disable the gate for this daemon
export SIMARD_ENGINEER_AMPLIHACK_UPDATE=0
```

Re-enable it by unsetting the variable or setting any value other than `0`:

```bash
unset SIMARD_ENGINEER_AMPLIHACK_UPDATE   # back to default (on)
```

Set this in the daemon's environment (e.g. the systemd unit's `Environment=`),
because the gate runs inside the OODA daemon process that spawns engineers.

When the gate is disabled it emits a single `disabled` trace at startup, so you
can confirm the off state from the log rather than guessing:

```bash
journalctl --user -u simard-ooda | grep 'amplihack_update.*disabled' | tail
```

## 2. Tune the TTL (dedup window)

A successful `amplihack update` within the last `SIMARD_AMPLIHACK_UPDATE_TTL_SECS`
seconds is reused — the gate skips re-running for spawns inside that window. The
default is **300** seconds.

```bash
# Refresh at most once every 10 minutes (default is 300s)
export SIMARD_AMPLIHACK_UPDATE_TTL_SECS=600

# Aggressively fresh: refresh again after 30s
export SIMARD_AMPLIHACK_UPDATE_TTL_SECS=30
```

Lower values mean more frequent rebuilds (fresher, more cost); higher values mean
fewer rebuilds (cheaper, potentially staler). The lock still serializes runs
regardless of the TTL, so lowering it will not cause concurrent rebuilds — only
more sequential ones over time.

The gate never short-aborts an `amplihack update` that is still making progress.
Any bound on the update subprocess is an **idle/liveness** bound — it fires only
if the update genuinely stalls, not at a fixed runtime — so a slow but
progressing first build is not killed. A stalled update surfaces as an ordinary
`failed` outcome (step 5), never a silent kill. There is no operator knob for
this bound.

## 3. Require strict freshness for a run

By default a failed `amplihack update` is **surfaced but not fatal**: the
engineer still spawns on the last-known-good install (see step 5). For operators
who require that engineers never run on a possibly-stale install, make a failed
update **block** the spawn:

```bash
export SIMARD_REQUIRE_FRESH_AMPLIHACK=1
```

With this set, an update failure produces the `blocked` outcome and an explicit
error instead of a spawn. This is honest, surfaced degradation — the refusal is
loud — not a silent fallback. Leave it unset to keep the default
proceed-on-last-known-good behaviour.

## 4. Verify the gate ran

There are three independent signals. Resolve your state root first (the lockfile
and state file live directly under it — see
[state-root resolution](../reference/state-root-resolution.md)):

```bash
STATE_ROOT="${SIMARD_STATE_ROOT:-$HOME/.simard}"
echo "state root: $STATE_ROOT"
```

### a. The tracing lines

The gate traces every decision on the `simard::amplihack_update` target. In the
daemon's log, look for the outcome and durations:

```bash
journalctl --user -u simard-ooda | grep amplihack_update | tail -20
```

You will see one of `outcome=ran`, `outcome=skipped-fresh`, `outcome=failed`, or
`outcome=blocked`, each carrying `ttl_secs`, `gate_duration_ms`, and (for a real
update) `update_duration_ms`. The gate never uses `println!`/`eprintln!`, so
these are structured `tracing` events, filterable by target.

### b. The durable state file

The last **successful** update is recorded here and survives restarts:

```bash
cat "$STATE_ROOT/amplihack-update-state.json"
# {"last_success_epoch_secs":1751645000}

# Human-readable, and how long ago it was:
last=$(jq -r .last_success_epoch_secs "$STATE_ROOT/amplihack-update-state.json")
echo "last success: $(date -d @"$last")  ($(( $(date +%s) - last ))s ago)"
```

If the age is under your TTL, the next spawn will take the `skipped-fresh` path.

### c. The failure metric

Successful gates record no failure metric. A failure appends an
`amplihack_update_failure` row to `~/.simard/metrics/metrics.jsonl`:

```bash
grep amplihack_update_failure ~/.simard/metrics/metrics.jsonl | tail -5
```

No matching rows means the gate has not failed — updates have been succeeding or
skipping fresh.

## 5. Diagnose a failed `amplihack update`

When `amplihack update` fails (network, build, or install error), the gate does
**not** swallow it. Two signals fire together.

**The log** carries a warn/error `outcome=failed` (or `outcome=blocked` under
strict mode) with the `error` field:

```bash
journalctl --user -u simard-ooda | grep 'amplihack_update.*outcome=failed' | tail
```

**The metric** records the failure and the resulting decision. Read it with
`jq`:

```bash
# Pretty-print recent amplihack update failures with their context
jq -c 'select(.metric_name == "amplihack_update_failure")' \
  ~/.simard/metrics/metrics.jsonl | tail -10

# Count failures in the last hour
cutoff=$(date -u -d '1 hour ago' +%Y-%m-%dT%H:%M:%SZ)
jq -r --arg c "$cutoff" \
  'select(.metric_name=="amplihack_update_failure" and .timestamp > $c) | .context' \
  ~/.simard/metrics/metrics.jsonl | wc -l
```

The `context` string names the cause and whether the engineer proceeded on the
last-known-good install (`failed`) or the spawn was refused (`blocked`).

### Recover

1. **Reproduce the update by hand** — the gate runs the exact operator command:

   ```bash
   amplihack update
   ```

   Fix whatever it reports (network reachability, a broken build, disk space).

2. **Confirm the fresh timestamp advances.** After a manual success, the daemon's
   next gate will run its own update and write a new
   `last_success_epoch_secs`; you can also confirm your manual run succeeded by
   re-reading the state file (step 4b) after the next spawn.

3. **If you were blocking spawns** with `SIMARD_REQUIRE_FRESH_AMPLIHACK=1` and
   need cognition to continue while you fix the root cause, unset it to fall back
   to proceed-on-last-known-good — the staleness will still be surfaced in logs
   and the metric.

## 6. Understand serialize/dedup during a spawn burst

When an OODA round dispatches several engineers at once (see
[concurrent engineer dispatch](../reference/concurrent-engineer-dispatch.md)),
they do **not** each rebuild `amplihack`:

- The first spawner acquires the `flock` over
  `$STATE_ROOT/amplihack-update.lock` and runs the update.
- Every other spawner **blocks on that lock**, then — once the first releases it
  — re-reads the now-fresh timestamp, finds it within the TTL, and takes the
  `skipped-fresh` path.

So a burst of N spawns performs **one** update, not N. You can watch this: during
a burst you will see a single `outcome=ran` followed by several
`outcome=skipped-fresh` lines close together.

```bash
journalctl --user -u simard-ooda --since '2 min ago' \
  | grep -oE 'amplihack_update.*outcome=[a-z-]+'
# amplihack_update ... outcome=ran
# amplihack_update ... outcome=skipped-fresh
# amplihack_update ... outcome=skipped-fresh
```

## See also

- [The amplihack freshness gate](../concepts/amplihack-freshness-gate.md) — why
  it runs before every spawn.
- [amplihack freshness gate reference](../reference/amplihack-freshness-gate.md)
  — config, files, algorithm, metric, and tracing fields.
- [Check for updates](./check-for-updates.md) — the separate informational
  check for a newer *Simard* release.
- [How OODA spawns engineer agents](./spawn-engineers-from-ooda-daemon.md) — the
  spawn path this gate runs in front of.
