---
title: Authoritative goal-board store
description: Why the OODA goal board moved from a versioned cognitive-memory snapshot to a single durable, flock-guarded goal_board.json — closing the clobber/steerability, restart-reset, and re-seeding failures the partial #2534 fix left open.
last_updated: 2026-07-04
owner: simard
doc_type: concept
related:
  - ./steerable-ooda-daemon.md
  - ./goal-board-persistence.md
  - ./deploy-aware-done-gate.md
  - ./file-backed-goal-store-simplification.md
  - ../reference/goal-board-api.md
  - ../reference/state-root-resolution.md
---

# Authoritative goal-board store

Issue [#1](https://github.com/rysweet/Simard/issues/1) makes a single durable
file — `<state_root>/state/goal_board.json` — the **authoritative** home of the
OODA goal board (active goals + scored backlog), replacing the
searchable-semantic-fact snapshot that the daemon re-saved (and clobbered) every
cycle. It is a *redesign* that supersedes the partial fix in
[#2534](https://github.com/rysweet/Simard/pull/2534): the pieces #2534 added
(the completion-evidence done-gate and the `NoProgressTracker` breaker) were
sound but were never able to *fire* in production, because the board itself was
never a reliable, single, durable source of truth.

This is distinct from the [file-backed `GoalStore`](./file-backed-goal-store-simplification.md)
(`goal_store.json`, a flat `Vec<GoalRecord>` for the meeting backend / engineer
loop / bootstrap). This page is about the OODA `GoalBoard`.

## The three production failures

Verified in production 4.5h after #2534 deployed:

1. **Un-steerable / clobbered.** The board lived only as `goal-board:snapshot`
   cognitive-memory facts, read back with `search_facts(...).max_by(node_id)`
   and rewritten each cycle by a *union-by-id* `merge_boards`. An operator
   `goal remove` wrote a new snapshot, but the daemon's next merge unioned its
   still-in-memory copy back on top — resurrecting the just-removed goal even
   though the CLI exited `0`. There was no read-your-writes guarantee.

2. **Breaker never fired (0 log lines in 4.5h).** The `NoProgressTracker`'s
   per-goal counter lived only in `OodaState`. The daemon exec-reloads itself
   when a new binary is deployed (~hourly), which replaces the process image and
   rebuilt `OodaState` — resetting every counter to zero before it could reach
   the threshold of 3. The breaker code ran every cycle but never reached a
   terminal decision, so it logged nothing.

3. **Done goals re-litigated and re-seeded.** The evidence archive only
   *evaluated goals already claiming completion* (`status == Completed`). The
   four ladybug supply-chain goals sat at `not-started`/0%, so nothing ever
   checked them against the (already cross-repo-capable) evidence gate, and
   recalled memory kept re-introducing them.

## The redesign

### 1. One authoritative store (`goal_board_store`)

`goal_board.json` holds the whole `PersistentGoalState`: the `GoalBoard` plus
the persisted `NoProgressTracker`. Reads and writes take the **same
cross-process `flock`** the CLI and daemon already share
([`state_root::goal_board_lock_path`](../reference/state-root-resolution.md)),
and every write is an **atomic** temp-file-plus-`rename`. `load()` returns
exactly the last committed state — read-your-writes, always. The
`goal-board:snapshot` cognitive-memory fact is demoted to a **derived cache**
the daemon overwrites from the file each cycle (`overwrite_memory_cache`), so
the dashboard and memory recall stay consistent without ever being the source of
truth.

### 2. Single writer + steerability

The daemon is the single authority over the in-memory board. Each cycle it
**reloads** `goal_board.json` at the start (picking up operator edits and
meeting handoffs) and **commits** at the end through a *tombstone-aware*
`reconcile` that merges its in-flight board with the current file — it augments,
never clobbers. Operator mutations (`goal add / remove / complete /
reprioritize`) are surgical read-modify-writes on the file **under the flock**,
so they cannot be lost to (or lose) a concurrent daemon flush. An operator edit
is therefore reflected by the very next `goal list`, survives a full OODA cycle,
and survives a daemon restart. If the daemon is down, the CLI still writes the
authoritative file directly and the daemon loads it on next start.

### 3. Tombstones — no re-seeding

Removing or completing a goal writes a durable tombstone (the existing
`goal_tombstones.json`, now consulted at every board load, cycle commit, and
recall filter — not just meeting handoffs). Tombstoned ids are dropped from the
reconcile and from any board a recall / default-seed / handoff path tries to
introduce, so a completed objective can never resurrect a goal.

### 4. Done-gate every cycle, cross-repo aware

`sweep_done_goals` evaluates **every** active goal against the
completion-evidence gate each cycle — not only goals already claiming
completion. Because `GhCliEvidenceSource` resolves each goal's `repo` slug, a
merged PR or closed issue on **any** governed repo counts as evidence, so the
four ladybug goals auto-complete, leave the board, and are tombstoned. Every
decision is logged.

### 5. Restart-resilient no-progress breaker

The `NoProgressTracker` is persisted in `goal_board.json` and restored into
`OodaState` at daemon start and each cycle. A no-action livelock spanning the
~hourly exec-reload is now still bounded: after 3 cumulative no-progress cycles
the breaker forces one definitive resolution (verify once, then mark done / drop
/ escalate to a filed issue) and logs it — it never loops "I'll verify"
forever.

### 6. Curation merges, never clobbers

The cycle-commit `reconcile` unions the persisted board with the daemon's
in-flight board (in-flight wins field-for-field on collision, tombstones
dominate). Operator- and meeting-added goals are preserved rather than replaced
by a regenerated set.

## Verification

See the runbook in the issue-#1 pull request. In tests
(`src/goal_board_store/tests.rs`, `src/operator_cli/tests_goal.rs`):

- store read-your-writes round-trip;
- tombstone prevents re-seed from recalled memory;
- cross-repo done-evidence completes a goal;
- the no-progress breaker fires *after a simulated restart* using the persisted
  counter;
- the cycle reconcile preserves operator-added goals and drops operator-removed
  (tombstoned) ones even when the daemon's in-flight board still carries them;
- an operator `goal add` is reflected immediately and survives a restart; an
  operator `goal remove` survives a full daemon cycle and a restart.
