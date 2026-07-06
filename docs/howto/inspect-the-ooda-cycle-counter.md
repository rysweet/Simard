---
title: How to inspect and verify the OODA cycle counter
description: Read the brain-relative OODA cycle number from the durable goal_board.json store, from cycle=N logs, from simard status, and from the dashboard — and prove it continues across a daemon restart instead of resetting to Cycle #1.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/durable-ooda-cycle-counter.md
  - ../concepts/brain-relative-ooda-cycle-counter.md
  - ../reference/dashboard-activity-cycle-reports.md
  - ./run-ooda-daemon.md
  - ./simard-status.md
---

# How to inspect and verify the OODA cycle counter

Use this guide to read Simard's **brain-relative** OODA cycle number and to prove
it is durable — that it **continues** across a daemon restart or deploy instead
of resetting to "Cycle #1". For the design, see the
[durable cycle counter API reference](../reference/durable-ooda-cycle-counter.md);
for the rationale, see
[Brain-relative OODA cycle counter](../concepts/brain-relative-ooda-cycle-counter.md).

## Prerequisites

- [ ] You are in the repository root and `simard` builds/runs locally.
- [ ] You know which **state root** the brain uses (`SIMARD_STATE_ROOT`, or the
      positional `[state-root]` argument; default `/tmp/simard-ooda`). The durable
      counter lives at `<state_root>/state/goal_board.json`.

## 1. Read the durable counter from the store

The authoritative value is the `cycle_count` field of `goal_board.json`:

```bash
STATE_ROOT="${SIMARD_STATE_ROOT:-/tmp/simard-ooda}"
jq '.cycle_count' "$STATE_ROOT/state/goal_board.json"
```

This prints the last **committed** cycle number — the brain's total lived
cognition. It is written under the store's `flock` on every cycle, so reading it
while the daemon runs is safe (you see the last completed cycle).

A brand-new brain (a fresh state root the daemon has not yet cycled) has no file
yet, or a file with `cycle_count` absent/`0`; the first cycle makes it `1`.

## 2. Read it from the live surfaces

All of these show the **same** durable number:

```bash
# The tracing span field, climbing across restarts:
journalctl -u simard --no-pager | grep -oE 'cycle=[0-9]+' | tail -5

# The heartbeat (pre-cycle "running" and post-cycle "healthy" both carry it).
# NOTE: daemon_health.json lives under the XDG data dir, NOT the state root:
jq '.cycle_number' "${XDG_DATA_HOME:-$HOME/.local/share}/simard/daemon_health.json" 2>/dev/null

# The unified status report:
simard status --json | jq '.daemon'      # daemon section reports the live cycle

# The highest persisted cycle report on disk:
ls "$STATE_ROOT"/cycle_reports "$STATE_ROOT"/state/cycle_reports 2>/dev/null \
  | grep -oE 'cycle_[0-9]+\.json' | grep -oE '[0-9]+' | sort -n | tail -1
```

On the **dashboard**, the "Cycle #N" on Overview, Whiteboard, and System Status,
the **Activity → Cycle Reports** card, and the **Thinking → Cycle History** table
all render this durable number (they reconcile through
`cycle_source::authoritative_cycle_number` — see
[Activity Cycle Reports](../reference/dashboard-activity-cycle-reports.md#relationship-to-the-authoritative-cycle-counter)).

## 3. Prove it survives a restart (the key check)

This is the behaviour the feature guarantees: the number must **continue**, not
reset to `1`.

```bash
export SIMARD_STATE_ROOT="$PWD/target/cycle-durability-check"
rm -rf "$SIMARD_STATE_ROOT"

# Run a bounded batch of cycles, then let the daemon exit.
simard ooda run --cycles=5 "$SIMARD_STATE_ROOT"
BEFORE=$(jq '.cycle_count' "$SIMARD_STATE_ROOT/state/goal_board.json")
echo "cycle_count after first run: $BEFORE"      # e.g. 5

# Restart the daemon against the SAME state root.
simard ooda run --cycles=5 "$SIMARD_STATE_ROOT"
AFTER=$(jq '.cycle_count' "$SIMARD_STATE_ROOT/state/goal_board.json")
echo "cycle_count after restart:  $AFTER"        # e.g. 10, NOT 5-again

test "$AFTER" -gt "$BEFORE" \
  && echo "PASS: counter continued across restart" \
  || echo "FAIL: counter did not advance"
```

Watch the logs of the second run: the first `cycle=` line reads about
`cycle=$BEFORE`, **not** `cycle=1` — the brain picks up where it left off. (The
`cycle=` tracing span field is captured at cycle *entry*, before the per-cycle
`+= 1`, so it trails by one; the persisted `cycle_count`, the `daemon_health.json`
`cycle_number`, and the dashboard "Cycle #N" for that same cycle read
`$((BEFORE + 1))`.)

> A fresh state root (as above, after `rm -rf`) begins that brain's own count:
> its first cycle is **reported** as `#1` (the `cycle=` span field, captured at
> entry, reads `cycle=0`) because that brain has no prior cognition. Continuity is
> a property of a brain's memory, so a genuinely new memory legitimately starts
> its own count.

## 4. Understand the two counters

If a number looks smaller than you expect, check **which** counter you are
reading:

| You are reading | Counter | Resets on restart? |
| --- | --- | --- |
| `goal_board.json` `cycle_count`, `cycle=` logs, `daemon_health.json` `cycle_number`, dashboard "Cycle #N" | **`cycle_count`** (brain-relative) | **No** — durable |
| The `--cycles=N` stop budget for *this run* | `cycles_run` (session) | Yes — per process, by design |

The `--cycles=N` budget always counts **this process's** cycles from `0` (so
`--cycles=5` runs five cycles this launch); the displayed "Cycle #N" is the
durable total. They are different numbers on purpose.

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Dashboard still shows "Cycle #1" after a deploy | The daemon is running an **old build** that predates the durable counter, or you are pointed at a **fresh/empty state root** | Confirm the running binary, and confirm `SIMARD_STATE_ROOT` points at the brain's real state root (not `/tmp/simard-ooda` if the brain lives elsewhere) |
| `cycle_count` is `0` but many `cycle_*.json` reports exist | First load after upgrading from a build that never persisted the field | Expected once: at **daemon startup** the daemon **backfills** `cycle_count` from the highest persisted report index, then persists it on the first `commit_cycle`. The number recovers on the first cycle — it does not stay at `#0`/`#1`. See [backfill](../reference/durable-ooda-cycle-counter.md#one-time-report-backfill) |
| The number never seems to increase | You are reading a **different state root** than the daemon writes | Match `SIMARD_STATE_ROOT` between your read command and the running daemon |
| `jq: No such file` on `goal_board.json` | The brain has not completed its first cycle yet | Wait one cycle; the store is written by the first `commit_cycle` |

## Related

- [Durable OODA cycle counter API reference](../reference/durable-ooda-cycle-counter.md)
- [Brain-relative OODA cycle counter](../concepts/brain-relative-ooda-cycle-counter.md)
- [How to run the OODA daemon](./run-ooda-daemon.md)
- [Read Simard's status with `simard status`](./simard-status.md)
- [Activity tab — Cycle Reports](../reference/dashboard-activity-cycle-reports.md)
