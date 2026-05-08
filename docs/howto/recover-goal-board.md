---
title: How to recover a corrupted or missing goal board
description: Steps to restore the Simard goal board when the cognitive-memory goal-board snapshot is missing, corrupted, or out of sync after a recovery operation.
last_updated: 2026-05-08
owner: simard
doc_type: howto
status: design — partially implemented
related:
  - ../concepts/goal-board-persistence.md
  - ../reference/goal-board-api.md
  - ../reference/cognitive-memory-bridge-helpers.md
  - ../howto/inspect-durable-goal-register.md
  - ../reference/simard-cli.md
---

# How to recover a corrupted or missing goal board

> **Status: design specification — partially implemented (issue [#1590](https://github.com/rysweet/Simard/issues/1590)).**
>
> This how-to relies on three CLI subcommands that are part of the issue
> #1590 deliverable but **do not exist yet**:
>
> - `simard goals inspect [--json]`
> - `simard goals restore --from <path>`
> - `simard goals clear-assignment --goal-id <id>`
>
> Until those subcommands land, the only steps that work today are:
>
> - Step 2 (read daemon logs)
> - Step 4 (force a board reseed via the `.reseed_goals` marker)
> - The first troubleshooting entry (verify `SIMARD_STATE_ROOT`)
>
> The rest of the document describes the recovery surface that issue
> #1590 will land. The doc is held out of mkdocs nav until those
> commands are wired.

After issue #1590 the goal board is stored exclusively in cognitive
memory under the `goal-board:snapshot` fact. The `goal_records.json` file
is migrated into cognitive memory on first startup by
`migrate_legacy_disk_file_if_present` and then deleted — see
[Goal board persistence — concept](../concepts/goal-board-persistence.md).
Recovery is therefore entirely a cognitive-memory operation.

In most cases the daemon recovers automatically — a corrupted snapshot is
logged by `load_goal_board` and the in-memory state is left untouched (the
OODA cycle only applies non-empty loaded boards), and the next cycle
persists a fresh snapshot. Use this guide when automatic recovery does
not produce the expected board state.

## Prerequisites

- [ ] You have SSH access to the host running the Simard daemon.
- [ ] You know the value of `SIMARD_STATE_ROOT` (default:
  `~/.simard/state`).
- [ ] The Simard daemon is stopped, or you are working on a state root it
  is not currently using.

---

## 1. Inspect the current snapshot

> Requires the `simard goals inspect` subcommand from issue #1590.

Use the `simard goals inspect` CLI to dump the live snapshot from cognitive
memory:

```bash
STATE_ROOT="${SIMARD_STATE_ROOT:-$HOME/.simard/state}"
SIMARD_STATE_ROOT="$STATE_ROOT" simard goals inspect --json | jq '.active | length'
```

Expected: a non-negative integer. If the command returns
`{"active": [], "backlog": []}`, no snapshot exists yet — go to **step 4** to
seed the default goals, or **step 3** to restore from a backup.

If the command's JSON is empty because the snapshot fact is corrupt, the
daemon logs will say so — go to **step 2**.

---

## 2. Check the daemon log for snapshot errors

`load_goal_board` does not raise an error on a corrupt snapshot — it logs
and returns an empty board. The daemon log records the problem:

```
[simard] load_goal_board: cognitive memory snapshot parse error (...) — returning empty board
[simard] load_goal_board: cognitive memory search_facts failed (...) — returning empty board
```

Check the daemon log:

```bash
journalctl -u simard --since "1 hour ago" | grep -E 'load_goal_board|goal-board'
```

If a parse error appears intermittently and the next cycle's
`persist_board` succeeded, no recovery is required — the snapshot has
already been overwritten with a clean revision. Re-run step 1.

If the corruption recurs every cycle (e.g., the integrity guard inside
`save_goal_board` keeps rejecting suspect boards because the OODA Decide
phase keeps producing them), you need to restore a known-good snapshot —
proceed to step 3 — or force a clean reseed in step 4.

---

## 3. Restore from a known-good backup

> Requires the `simard goals restore` subcommand from issue #1590.

The recommended backup format is the JSON dump produced by
`simard goals inspect --json`:

```bash
# Earlier — to take a backup
SIMARD_STATE_ROOT="$STATE_ROOT" simard goals inspect --json > /backups/goal-board-$(date +%F).json

# Now — to restore
SIMARD_STATE_ROOT="$STATE_ROOT" simard goals restore --from /backups/goal-board-2026-05-01.json
```

`simard goals restore` parses the file as a `GoalBoard`, calls
`launch_writer_bridge`, and invokes `save_goal_board`. The integrity guard
runs on the restored board — if your backup contains placeholder goals
the restore is rejected without modifying the snapshot. The restore must
be performed against a state root the daemon is **not** currently writing
to (stop the daemon first).

Validate the restore:

```bash
SIMARD_STATE_ROOT="$STATE_ROOT" simard goals inspect --json | jq '.active[] | {id, status}'
```

---

## 4. Force a board reseed

This step works today (it does not depend on the new CLI subcommands).

If no backup exists and you want to start fresh, reset the board to the
five default starter goals by creating a `.reseed_goals` marker:

```bash
touch "$STATE_ROOT/.reseed_goals"
```

On the next OODA cycle the daemon detects the marker, removes it, skips
`load_goal_board` entirely, and replaces the in-memory board with the
five default goals from `DEFAULT_SEED_GOALS`. The `Curate` step at the end
of that cycle persists the fresh snapshot to cognitive memory. All
previous active goals and backlog items are discarded.

> **Warning**: this permanently discards the current board. Use only as a
> last resort.

---

## 5. Clear a stale engineer assignment manually

> Requires the `simard goals clear-assignment` subcommand from issue
> #1590.

If a goal is stuck with an `assigned_to` value that points to a dead tmux
session, and the automatic sweep cannot clear it (e.g., because Simard is
not running inside tmux — see the "Assignment safety outside tmux" note
in the [persistence concept](../concepts/goal-board-persistence.md#guarantees-and-non-guarantees)),
clear it via the CLI:

```bash
SIMARD_STATE_ROOT="$STATE_ROOT" simard goals clear-assignment \
  --goal-id YOUR-GOAL-ID
```

This invokes `clear_goal_assignment` on the loaded board and persists the
result via `save_goal_board`. On the next OODA cycle the goal will be
re-dispatched automatically.

> **Why no `jq` recipe?** The board is no longer a single JSON file.
> Editing cognitive memory facts by hand is unsupported and risks
> corrupting the graph. Always go through the CLI or the dashboard.

---

## Troubleshooting

### Daemon keeps reloading a stale board

Check whether `SIMARD_STATE_ROOT` is set correctly in the daemon's
environment:

```bash
systemctl show simard -p Environment
# or
cat /proc/$(pgrep simard)/environ | tr '\0' '\n' | grep SIMARD_STATE_ROOT
```

If the daemon is reading from a different state root than the one your CLI
or dashboard is inspecting, the cognitive-memory store it loads will never
match what you edited.

### Dashboard write returns a "no writer available" error

This means `launch_writer_bridge` returned `Err` because neither the
daemon IPC socket nor the local writer lock could be obtained. Either:

- The daemon's IPC socket exists but the daemon is not responding —
  restart the daemon.
- A stale writer lock could not be reaped — stop everything that might be
  writing, remove the LadybugDB lock file
  (`$STATE_ROOT/cognitive_memory.ladybug.open.lock`), and try again.

See [Cognitive memory bridge helpers](../reference/cognitive-memory-bridge-helpers.md)
for the full ladder.

### The `.reseed_goals` marker was created but goals were not reset

The daemon only checks for the marker at the beginning of an OODA cycle, and
removes it immediately. If the daemon was already mid-cycle when you created
the marker, the reset takes effect on the *next* cycle start.

### "Where did `goal_records.json` go?"

After issue #1590, `load_goal_board` runs a one-shot bootstrap migration
(`migrate_legacy_disk_file_if_present`) on every startup. If a legacy
`$SIMARD_STATE_ROOT/goal_records.json` file exists, it is read, written
into cognitive memory, and then deleted. After a single successful daemon
startup the file is gone and cognitive memory holds the same content.

If you have a pre-migration backup of `goal_records.json` and want to
restore from it directly, you can either:

- Drop the file into `$SIMARD_STATE_ROOT` and start the daemon — the next
  `load_goal_board` call will pick it up via the bootstrap migration; or
- Use `simard goals restore --from /backups/goal_records.json` once that
  subcommand lands (it accepts the legacy schema and converts to the
  snapshot fact).

---

## Related reading

- [Goal board persistence — concept](../concepts/goal-board-persistence.md)
- [Goal board API reference](../reference/goal-board-api.md)
- [Cognitive memory bridge helpers](../reference/cognitive-memory-bridge-helpers.md)
- [How to inspect the durable goal register](./inspect-durable-goal-register.md)
