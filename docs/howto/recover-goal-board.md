---
title: How to recover a corrupted or missing goal board
description: Steps to restore the Simard goal board when goal_records.json is missing, corrupted, or out of sync with cognitive memory.
last_updated: 2026-05-08
owner: simard
doc_type: howto
related:
  - ../concepts/goal-board-persistence.md
  - ../reference/goal-board-api.md
  - ../howto/inspect-durable-goal-register.md
  - ../reference/simard-cli.md
---

# How to recover a corrupted or missing goal board

Simard's goal board is loaded at the start of every OODA cycle using a
three-tier fallback. In most cases recovery is automatic. Use this guide when
automatic recovery does not produce the expected board state.

## Prerequisites

- [ ] You have SSH access to the host running the Simard daemon.
- [ ] You know the value of `SIMARD_STATE_ROOT` (default: `~/.simard`).
- [ ] The Simard daemon is stopped, or you are working on a state root it is
  not currently using.

---

## 1. Verify the current disk file

```bash
STATE_ROOT="${SIMARD_STATE_ROOT:-$HOME/.simard}"
cat "$STATE_ROOT/goal_records.json" | jq '.active | length'
```

Expected: a non-negative integer. If you see a parse error, the file is
corrupted — go to **step 3**.

If the file is missing (`cat: No such file or directory`), the daemon has not
completed a full cycle yet or the state root has changed — go to **step 2**.

---

## 2. Inspect the cognitive memory fallback

The daemon logs a message when it falls back to cognitive memory:

```
[simard] load_goal_board: goal_records.json parse error (…) — falling back to cognitive memory
```

Check the daemon log:

```bash
journalctl -u simard --since "1 hour ago" | grep load_goal_board
```

If the daemon successfully loaded from cognitive memory and completed a cycle,
`goal_records.json` should now exist. Re-run step 1.

---

## 3. Restore from a known-good backup

If you have a backup of `goal_records.json`:

```bash
cp /path/to/backup/goal_records.json "$STATE_ROOT/goal_records.json"
```

Validate the restored file:

```bash
cat "$STATE_ROOT/goal_records.json" | jq '.active[] | {id, status: .status}'
```

---

## 4. Rebuild the file from cognitive memory manually

If no backup exists but the cognitive memory process is running, you can
trigger a fresh disk write by starting the daemon — it will load from
cognitive memory (tier 2) and write `goal_records.json` at the end of the
first successful OODA cycle.

The simplest approach is to start the daemon with the correct `SIMARD_STATE_ROOT`
and let it run one full OODA cycle. After the cycle completes, stop the daemon
and verify `goal_records.json` was written (step 1).

---

## 5. Force a board reseed

If both disk and cognitive memory are unavailable or corrupted beyond
recovery, you can reset the board to the five default starter goals by
creating a `.reseed_goals` marker:

```bash
touch "$STATE_ROOT/.reseed_goals"
```

On the next OODA cycle start the daemon detects the marker, removes it, and
replaces the board with the five default goals from `DEFAULT_SEED_GOALS`.
All previous active goals and backlog items are discarded.

> **Warning**: this permanently discards the current board. Use only as a
> last resort.

---

## 6. Clear a stale engineer assignment manually

If a goal is stuck with an `assigned_to` value that points to a dead tmux
session, and the automatic sweep has not cleared it (e.g., because Simard is
not running inside tmux), clear it manually:

```bash
# Edit goal_records.json in-place
jq '(.active[] | select(.id == "YOUR-GOAL-ID") | .assigned_to) = null |
    (.active[] | select(.id == "YOUR-GOAL-ID") | .status) = "NotStarted"' \
  "$STATE_ROOT/goal_records.json" > /tmp/board.json && \
  mv /tmp/board.json "$STATE_ROOT/goal_records.json"
```

Verify the change:

```bash
jq '.active[] | select(.id == "YOUR-GOAL-ID") | {id, assigned_to, status}' \
  "$STATE_ROOT/goal_records.json"
```

On the next OODA cycle the goal will be re-dispatched automatically.

---

## Troubleshooting

### Daemon keeps reloading a stale board

Check whether `SIMARD_STATE_ROOT` is set correctly in the daemon's environment:

```bash
systemctl show simard -p Environment
# or
cat /proc/$(pgrep simard)/environ | tr '\0' '\n' | grep SIMARD_STATE_ROOT
```

If the daemon is writing to a different state root than the one you are
inspecting, the disk file it loads will never match what you edited.

### `goal_records.json` is world-readable

The file is written with `fs::write` which inherits the process umask.
To restrict permissions:

```bash
chmod 600 "$STATE_ROOT/goal_records.json"
```

Consider setting `umask 077` in the daemon's startup environment for
new files.

### The `.reseed_goals` marker was created but goals were not reset

The daemon only checks for the marker at the beginning of an OODA cycle, and
removes it immediately. If the daemon was already mid-cycle when you created
the marker, the reset takes effect on the *next* cycle start.

---

## Related reading

- [Goal board persistence — concept](../concepts/goal-board-persistence.md)
- [Goal board API reference](../reference/goal-board-api.md)
- [How to inspect the durable goal register](./inspect-durable-goal-register.md)
