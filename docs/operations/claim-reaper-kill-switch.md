---
title: "Operations: claim-reaper kill switch & tuning (SIMARD_CLAIM_REAP_*)"
description: >
  The two environment variables that control the periodic stale-engineer-claim
  reaper — SIMARD_CLAIM_REAP_ENABLED (on/off) and SIMARD_CLAIM_REAP_STALE_SECS
  (idle threshold, default 1800s / 30min). What they do, the fail-safe defaults,
  when (and when not) to change them, how to set them via systemd, how to verify
  the reaper is running and reclaiming, and how to revert.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/stale-engineer-claim-reaper.md
  - ../reference/claim-reaper-api.md
  - ../howto/diagnose-leaked-engineer-claims.md
  - engineer-admission-kill-switch.md
---

# Claim-reaper kill switch & tuning (`SIMARD_CLAIM_REAP_*`)

> **Status: implemented.** This page describes the shipped configuration in
> present tense. The reaper it toggles lives in
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs);
> the resolvers live in
> [`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs).
> See the [concept](../concepts/stale-engineer-claim-reaper.md) and the
> [API reference](../reference/claim-reaper-api.md).

The reaper is a periodic sweep on the Overseer tick that reclaims
`engineer_claims` rows whose engineer is provably dead — independently of
per-goal polling. It is **enabled by default at a 30-minute idle threshold**.
Secure default = reaping ON. Two environment variables control it.

---

## The variables

| Variable | Default | Effect |
|---|---|---|
| `SIMARD_CLAIM_REAP_ENABLED` | enabled | Master on/off. **Only** an explicit falsey value (`0`, `false`, `no`, `off`, case-insensitive) disables the sweep. Unset / empty / any other value keeps it **enabled**. |
| `SIMARD_CLAIM_REAP_STALE_SECS` | `1800` (30 min) | Idle-staleness threshold in seconds. A worktree whose **newest-file mtime** is older than this is judged `heartbeat-stale` and its claim is reclaimed. Unset / empty / unparseable / `0` all resolve to the **1800s default** (never a 0-second threshold that would mass-reclaim). |

> **Fail-safe defaults.** Unknown or garbage values never silently weaken the
> reaper: a mis-spelled `SIMARD_CLAIM_REAP_ENABLED=nope` stays **on**, and a
> bad `SIMARD_CLAIM_REAP_STALE_SECS=abc` (or `0`) falls back to **1800s**. The
> only disable is the exact falsey set above.

Both variables are read at daemon startup (the config-resolution path). Changing
them during a run has no effect — **restart the daemon** to pick up a new value.

> **No wall-clock kill.** `SIMARD_CLAIM_REAP_STALE_SECS` is an **idle** threshold
> against newest-file mtime, not a run-duration cap. A busy engineer whose files
> keep changing is never reaped, no matter how long it runs. Raising the value
> only makes the reaper *more* patient.

---

## When to disable (`SIMARD_CLAIM_REAP_ENABLED=off`)

The reaper is fail-closed and conservative, so the legitimate reasons to turn it
off are narrow:

1. **A suspected defect is reclaiming claims that back a live engineer.** If you
   see a `heartbeat-stale` reclaim for a goal you know is actively working (its
   worktree files *are* changing), disable the reaper to stop the bleeding while
   you investigate, then re-enable.
2. **Isolating the reaper during an incident.** To rule the sweep out as one
   variable on a **non-production** daemon, toggle it off for a side-by-side
   comparison, then re-enable.

## When NOT to disable

- **"Claims keep filling the cap."** That is the leak the reaper *fixes*.
  Disabling it re-opens the within-incarnation leak (orphaned `g1`/`test-goal`
  rows holding cap slots until restart). Leave it on.
- **"A goal took a while and got reaped."** If the worktree's newest-file mtime
  was genuinely older than 30 minutes, the engineer was idle, not busy. Prefer
  **raising `SIMARD_CLAIM_REAP_STALE_SECS`** over disabling — see below.

## When to raise the threshold instead of disabling

If a workload legitimately goes quiet for long stretches (e.g. a very long
single compile/test with no intermediate file writes), raise the idle window
rather than turning the reaper off:

```bash
# Be more patient: 90-minute idle window instead of 30.
SIMARD_CLAIM_REAP_STALE_SECS=5400 simard daemon
```

The `no-worktree` reclaim path is unaffected by the threshold — a claim with no
backing directory is always reclaimed immediately (there is nothing to protect).

---

## How to set it

### One-shot for an interactive run

```bash
# Disable entirely
SIMARD_CLAIM_REAP_ENABLED=off simard daemon

# Keep enabled, widen the idle window to 60 minutes
SIMARD_CLAIM_REAP_STALE_SECS=3600 simard daemon
```

### Persistent across restarts (systemd unit)

The daemon ships a reference unit at
[`scripts/simard-ooda.service`](https://github.com/rysweet/Simard/blob/main/scripts/simard-ooda.service),
typically installed as a **user-level** unit at
`~/.config/systemd/user/simard-ooda.service`. For a system-wide install, drop
`--user` and prefer `systemctl edit` so the override lands in a drop-in.

Add to the unit's `[Service]` section:

```ini
[Service]
Environment="SIMARD_CLAIM_REAP_STALE_SECS=3600"
# Environment="SIMARD_CLAIM_REAP_ENABLED=off"   # only if you must disable
```

Reload and restart:

```bash
systemctl --user daemon-reload
systemctl --user restart simard-ooda
```

To revert, delete the `Environment=` line, `daemon-reload`, and restart.

---

## Verifying the reaper is running

Every reclaim emits one fail-visible line. Grep the daemon log (drop `--user`
for a system-level install):

```bash
journalctl --user -u simard-ooda -n 500 | grep 'claim-reaper:'
```

Expected reclaim lines (the `claim-reaper:`, `reason=`, and `age=` substrings
are stable):

```
[simard] claim-reaper: reclaimed rysweet/Simard:g1 (reason=no-worktree, age=n/a)
[simard] claim-reaper: reclaimed rysweet/Simard:goal-improve-tests (reason=heartbeat-stale, age=5142s)
```

If the reaper is **disabled**, no `claim-reaper: reclaimed` lines are emitted
even while orphaned claims exist. A healthy, enabled daemon with no leaks simply
emits nothing (the sweep found nothing to reclaim) — absence of reclaim lines is
normal steady state.

To confirm the leak is clearing, watch the live claim count fall back below the
cap after the first tick following a restart. See
[diagnose leaked engineer claims](../howto/diagnose-leaked-engineer-claims.md).

---

## Reverting

Remove the environment variable(s), `daemon-reload`, and restart. The reaper
returns to its secure default: **enabled at a 1800s (30-minute) idle
threshold**. Confirm via the reclaim log lines above on the next tick that finds
an orphaned claim.

---

## Related

- [Stale-Engineer-Claim Reaper (concept)](../concepts/stale-engineer-claim-reaper.md)
- [Claim-Reaper API (reference)](../reference/claim-reaper-api.md)
- [Diagnose leaked engineer claims (how-to)](../howto/diagnose-leaked-engineer-claims.md)
- [Engineer-Admission Kill Switch](engineer-admission-kill-switch.md) — the
  sibling admission-gate lever.
