---
title: "How-to: diagnose and clear leaked engineer claims"
description: >
  Confirm whether the engineer-claim admission ledger is leaking cap slots
  within a running daemon incarnation, understand why the earlier reclaim paths
  don't catch them, and verify the periodic claim-reaper reclaims them. Covers
  inspecting the engineer_claims rows vs. the on-disk worktrees, reading the
  reaper's fail-visible log lines, and tuning or toggling the sweep.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/stale-engineer-claim-reaper.md
  - ../reference/claim-reaper-api.md
  - ../operations/claim-reaper-kill-switch.md
  - ./inspect-and-clean-engineer-worktrees.md
  - ./diagnose-rejected-progress-claims.md
---

# Diagnose and clear leaked engineer claims

> **Status: implemented.** The periodic reaper described here ships in
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
> and runs on the Overseer tick. Background:
> [Stale-Engineer-Claim Reaper](../concepts/stale-engineer-claim-reaper.md).

## Symptom

New engineers stop spawning even though goals are ready, and the Act phase keeps
reporting rejections like:

> *"an engineer claim is already active for rysweet/Simard:&lt;goal&gt;"*

…for **more goals than there are live engineers**. The `engineer_claims` table
is capped at 24; once all 24 slots are held by **orphaned** claims (whose
engineer is long gone but whose row lingers within this daemon incarnation), no
new engineer can be admitted until either the reaper reclaims a slot or the
daemon restarts.

## Step 1 — Compare claims to live worktrees

Count the held claims and list the on-disk engineer worktrees. If claims greatly
outnumber recently-touched worktrees, you are leaking.

```bash
# $SIMARD_STATE_ROOT defaults to ~/.simard; the ledger DB is outcomes.sqlite3.
STATE_ROOT="${SIMARD_STATE_ROOT:-$HOME/.simard}"

# Held claims (24 = full cap).
sqlite3 "$STATE_ROOT/outcomes.sqlite3" \
  'SELECT claim_key FROM engineer_claims ORDER BY claim_key;'

# Worktrees and how recently each was touched (newest-file mtime is the reaper's
# liveness signal).
ls -1 "$STATE_ROOT/engineer-worktrees/"
```

See [inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md)
for a fuller worktree walkthrough. Tell-tale leaks:

- Claim rows with **no** matching `engineer-worktrees/<goal>-*` directory
  (e.g. `g1`, `g2`, `test-goal`) — pure orphans, `reason=no-worktree`.
- Worktree directories whose newest file is **hours or days old** — idle
  engineers that never cleaned up, `reason=heartbeat-stale`.

## Step 2 — Understand why the older paths miss these

These orphans are invisible to the two pre-existing reclaim paths, which is
exactly why the reaper exists:

- **Collision-reclaim** only triggers when a **new** spawn collides on the same
  `claim_key`. A completed/removed/test goal is never spawned again, so no
  collision ever occurs. And its liveness check reads the worktree sentinel PID,
  which is the **daemon** PID — always alive within one incarnation.
- **Per-goal heartbeat cleanup** only runs while that specific goal is polled in
  the coverage set. An orphan whose goal left the coverage set is never polled.

The reaper sweeps **all** claims independently of polling, so it catches both.

## Step 3 — Confirm the reaper reclaimed them

The reaper runs every Overseer tick. Watch its fail-visible lines (drop `--user`
for a system-level install):

```bash
journalctl --user -u simard-ooda -n 500 -f | grep 'claim-reaper:'
```

You should see one line per reclaimed claim, naming the key, the staleness age,
and the reason:

```
[simard] claim-reaper: reclaimed rysweet/Simard:g1 (reason=no-worktree, age=n/a)
[simard] claim-reaper: reclaimed rysweet/Simard:test-goal (reason=no-worktree, age=n/a)
[simard] claim-reaper: reclaimed rysweet/Simard:goal-improve-tests (reason=heartbeat-stale, age=5142s)
```

After the sweep, re-run the `sqlite3` query from Step 1: the reclaimed rows are
gone and the held-claim count has fallen below the cap, so new engineers admit
again on the next Act phase. Cross-repo claims
(`rysweet/agent-kgpacks-rs-audit:…`, `rysweet/amplihack-rs:…`) are handled by the
same single sweep.

## Step 4 — If a reclaim looks wrong (fail-closed check)

The reaper is **fail-closed**: it never reclaims a claim whose worktree is
**fresh** (newest-file mtime within the threshold), and any scan/IO uncertainty
is treated as *live* → skip. If you believe a **live** engineer was reclaimed:

1. Confirm the worktree's newest-file mtime really was older than the threshold
   at reclaim time. If it was, the engineer was idle, not busy.
2. If the workload legitimately goes quiet for long stretches, **raise the idle
   window** rather than disabling the sweep:

   ```bash
   SIMARD_CLAIM_REAP_STALE_SECS=5400 simard daemon   # 90 min
   ```

3. Only if you suspect a genuine defect, disable temporarily and file an issue:

   ```bash
   SIMARD_CLAIM_REAP_ENABLED=off simard daemon
   ```

Full option semantics are in the
[claim-reaper kill switch & tuning](../operations/claim-reaper-kill-switch.md)
page.

## What the reaper will NOT do

- It will **not** kill a working engineer by elapsed time — staleness is
  idle-since-newest-activity, not run duration.
- It will **not** hand-roll a `DELETE` or use `--admin`; reclaim flows through
  `release_engineer_claim` plus the guarded worktree removal, so the
  [worktree-reaping safety guards](../reference/engineer-worktree-sweep-safety.md)
  still apply.
- It will **not** touch the cap value, the admission gate, or the
  collision-reclaim logic — it is a purely additive sweep.

## Related

- [Stale-Engineer-Claim Reaper (concept)](../concepts/stale-engineer-claim-reaper.md)
- [Claim-Reaper API (reference)](../reference/claim-reaper-api.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md)
- [Diagnose rejected progress claims](./diagnose-rejected-progress-claims.md)
