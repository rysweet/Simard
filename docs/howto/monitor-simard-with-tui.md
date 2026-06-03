---
title: Monitor Simard with the TUI dashboard
description: "How to launch simard-tui, navigate tabs, read daemon health, and interpret goal status — all from a single terminal pane."
last_updated: 2026-06-03
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/simard-tui.md
  - ../reference/state-root-resolution.md
  - ../daemon-mode.md
  - ./run-ooda-daemon.md
---

# Monitor Simard with the TUI dashboard

This guide walks through launching `simard-tui` and using it to monitor
a running Simard daemon. You will see daemon health, goal status, and
placeholder views for engineers, logs, meetings, and statistics.

## Prerequisites

- Simard daemon running via `simard ooda run` or the systemd service.
- `simard-tui` binary built (`cargo build --bin simard-tui`).
- A terminal that supports 256-color output (most modern terminals).

## 1. Launch the dashboard

```bash
simard-tui
```

The screen clears and the TUI opens in full-screen mode on the
**Overview** tab. The tab bar at the top shows:

```
[1:Overview] [2:Goals] [3:Engineers] [4:Activity] [5:Meeting] [6:Stats]
```

The active tab is highlighted. Press `q` at any time to quit and
restore your terminal.

## 2. Read the Overview tab

The Overview tab shows a summary panel:

```
┌ Daemon Status ──────────────────────┐
│ Service:  simard.service            │
│ State:    active (running)          │
│ PID:      48291                     │
│ Uptime:   2h 14m 33s               │
│ OODA:     N/A                       │
│ CPU:      3.2%                      │
│ Memory:   184 MiB                   │
└─────────────────────────────────────┘
```

**What each field means:**

- **State** — `active` means the daemon is running. `inactive` means
  it stopped cleanly. `failed` means it crashed. `unavailable` means
  systemctl could not be reached (non-systemd host).
- **PID** — the daemon's main process ID. Used to read `/proc` stats.
- **Uptime** — time since the service entered the `active` state.
- **OODA** — cycle count. Shows `N/A` in the MVP because no on-disk
  counter exists yet.
- **CPU** — percentage of one CPU core used by the daemon process,
  sampled over the last refresh interval. Shows `–` for the first
  2 seconds after launch (needs two samples).
- **Memory** — resident set size (RSS) of the daemon process.

## 3. Switch to the Goals tab

Press `2` to switch to the Goals tab. It displays a table of active
goals loaded from cognitive memory (`<state-root>/cognitive_memory.ladybug`):

```
┌ Goals ──────────────────────────────────────────────────────────┐
│ Description                  Priority  Status       Assigned    │
│ ────────────────────────────────────────────────────────────────│
│ Implement caching layer      1         in-progress  eng-alpha   │
│ Fix auth token refresh       0         in-progress  eng-beta    │
│ Add retry logic to bridge    2         not-started              │
│ Clean up legacy probes       3         paused                   │
└─────────────────────────────────────────────────────────────────┘
```

**Interpreting the columns:**

- **Priority** — numeric; `0` is highest. Empty means unset.
- **Status** — a `GoalProgress` variant: `proposed`, `not-started`,
  `in-progress`, `blocked`, `paused`, `completed`.
- **Assigned** — the engineer name if one is assigned. Empty otherwise.

If the cognitive memory DB is missing or has no goal board snapshot,
you see:

> No goals found — use `simard goal-curation read` to inspect the goal board.

If the DB could not be opened read-only (daemon holding exclusive
lock), you see the last cached snapshot with `(stale)` in the header.

## 4. Browse placeholder tabs

Press `3`, `4`, `5`, or `6` to see the placeholder tabs:

| Tab | What it will show |
|---|---|
| 3: Engineers | Active engineer subprocesses with worktree paths and status |
| 4: Activity | Live journal log stream from the Simard service |
| 5: Meeting | Interactive REPL connected to the meeting PTY |
| 6: Stats | Aggregate counts of goals, engineers, cycles, and memory |

Each placeholder shows a brief message describing the planned feature.

## 5. Custom state root

If your Simard instance uses a non-default state root:

```bash
SIMARD_STATE_ROOT=/srv/simard-state simard-tui
```

The TUI resolves the state root the same way the daemon does — see
[state-root resolution](../reference/state-root-resolution.md).

## 6. Custom service name

If you run Simard under a different systemd unit (e.g., a staging
instance):

```bash
SIMARD_TUI_SERVICE=simard-staging.service simard-tui
```

The service name must match `^[a-zA-Z0-9@._-]+\.service$`. Invalid
names are rejected at startup.

## 7. Running alongside other tools

`simard-tui` is read-only and uses no file locks for writing. It is
safe to run alongside:

- The Simard daemon itself
- `simard goal-curation read` and other CLI commands
- The web dashboard
- Multiple `simard-tui` instances

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| All fields show `–` or `unavailable` | Daemon not running or systemctl not available | Start the daemon: `simard ooda run` |
| Goals tab is empty | `cognitive_memory.ladybug` missing or no `goal-board:snapshot` fact | Use `simard goal-curation read` to inspect goals via IPC |
| Terminal looks broken after exit | Rare: panic before `TerminalGuard` drop | Run `reset` to restore terminal |
| CPU stuck at `–` | First launch, only one sample taken | Wait 2 seconds for the second sample |

## See also

- [simard-tui reference](../reference/simard-tui.md) — full
  specification of tabs, data sources, refresh rates, and security.
- [Run the OODA daemon](./run-ooda-daemon.md) — start the daemon
  that the TUI monitors.
- [Daemon mode](../daemon-mode.md) — what the daemon does between
  cycles.
