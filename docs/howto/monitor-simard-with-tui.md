---
title: Monitor Simard with the TUI dashboard
description: "How to launch simard-tui, navigate tabs, monitor engineers, read logs, run meetings, and check stats — all from a single terminal pane."
last_updated: 2026-06-04
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

This guide walks through launching `simard-tui` and using all six tabs
to monitor a running Simard daemon, inspect engineers, read logs,
conduct meetings, and review statistics.

## Prerequisites

- Simard daemon running via `simard ooda run` or the systemd service.
- `simard-tui` binary built (`cargo build --bin simard-tui`).
- **An interactive terminal** — stdout must be a TTY, or a controlling
  terminal (`/dev/tty`) must be available. If running over SSH, use
  `ssh -t` to allocate a pseudo-TTY. If running in a container, use
  `docker run -it`. The TUI detects non-TTY stdout and falls back to
  `/dev/tty` automatically; if neither is available, it exits with a
  clear error message.
- A terminal that supports 256-color output (most modern terminals).
- For the Activity tab: `journalctl` available (systemd host).
- For the Stats tab: `gh` CLI installed and authenticated (optional — metrics show `–` without it).
- For the Meeting tab: `simard` binary at `<state-root>/bin/simard`.

## 1. Launch the dashboard

```bash
simard-tui
```

The screen clears and the TUI opens in full-screen mode on the
**Overview** tab. The tab bar at the top shows:

```
[1:Overview] [2:Goals] [3:Engineers] [4:Activity] [5:Meeting] [6:Stats]
```

The active tab is highlighted. A footer bar at the bottom shows the
available keybindings:

```
Alt+1‥6: tabs | ←/→: cycle | q: quit
```

Press `q` at any time to quit and restore your terminal.

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
- **OODA** — cycle count. Shows `N/A` because no on-disk counter
  exists yet.
- **CPU** — percentage of one CPU core used by the daemon process,
  sampled over the last refresh interval. Shows `–` for the first
  2 seconds after launch (needs two samples).
- **Memory** — resident set size (RSS) of the daemon process.

## 3. Switch to the Goals tab

Press `Alt+2` (or `→` to cycle) to switch to the Goals tab. It displays a table of active
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

## 4. Monitor engineers

Press `Alt+3` to switch to the Engineers tab. It displays a table of
child processes spawned by the daemon, enriched with category
information from the subagent session registry:

```
┌ Engineers (3 processes) ─────────────────────────────────────────────────────┐
│ PID    Command                              Category    CPU%   Memory  Runtime│
│ ────────────────────────────────────────────────────────────────────────────│
│ 48312  simard engineer run --goal=goal-c…   engineer-…  12.4%  256 KiB 0h4m  │
│ 48345  simard engineer run --goal=goal-a…   engineer-…   3.1%  128 KiB 0h1m  │
│ 48398  simard engineer terminal --workt…    —            0.2%   64 KiB 0h0m  │
└──────────────────────────────────────────────────────────────────────────────┘
```

**What each column means:**

- **PID** — the child process ID.
- **Command** — the process command line, truncated to 64 characters.
- **Category** — the `agent_id` from the subagent session registry
  (`<state-root>/state/subagent_sessions.json`), if the PID matches
  a live registry entry. Shows `—` for processes not tracked in the
  registry (e.g., utility subprocesses or processes spawned without
  tmux tracking).
- **CPU%** — percentage of one CPU core, delta-sampled. Shows `–`
  until the second sample (2 seconds after the child first appears).
- **Memory** — resident set size (VmRSS) from `/proc/<PID>/status`.
- **Runtime** — time since the process started, derived from
  `/proc/<PID>/stat` starttime.

**Category data source.** The TUI reads
`<state-root>/state/subagent_sessions.json` on each engineer refresh
cycle (every 2 seconds). It filters out ended sessions
(`ended_at != null`) and entries with `pid == 0`, then builds a
PID → `agent_id` lookup map. Each discovered child process is matched
against this map. If the JSON file is missing or corrupt, all
categories default to `—` with no error displayed.

If the daemon is not running, you see:

> Daemon not running

If the daemon is running but has no children (between OODA cycles):

> No child processes

The tab refreshes every 2 seconds. Stale entries (processes that
exited) are removed on the next refresh.

## 5. Read activity logs

Press `Alt+4` to switch to the Activity tab. It shows the 50 most recent
log entries from the Simard systemd journal:

```
┌ Activity ─────────────────────────────────────────────────────────┐
│ 2026-06-03T04:01:12+0000 [INFO]  ooda: Cycle 47 — observe phase  │
│ 2026-06-03T04:01:13+0000 [INFO]  ooda: 3 signals collected       │
│ 2026-06-03T04:01:14+0000 [WARN]  ooda: No active engineer for …  │
│ 2026-06-03T04:01:15+0000 [INFO]  ooda: Decided: advance-goal(…)  │
│ 2026-06-03T04:01:16+0000 [ERROR] engineer: Worktree failed — …   │
└───────────────────────────────────────────────────────────────────┘
```

**Color coding:**

- **Red** — lines containing `ERROR`
- **Yellow** — lines containing `WARN`
- **Default** — everything else (INFO, DEBUG, etc.)

The log view is scrollable with the newest entries at the bottom. It
refreshes every 2 seconds by re-running `journalctl`.

If `journalctl` is unavailable or returns no entries, you see:

> No log entries

## 6. Run a meeting

Press `Alt+5` to switch to the Meeting tab. The TUI automatically spawns
a `simard meeting start` process:

```
┌ Meeting ──────────────────────────────────────────────────────────┐
│ simard> Welcome to Simard meeting mode.                           │
│ simard> What would you like to discuss?                           │
│                                                                   │
│ > Let's review the goal board priorities_                         │
└───────────────────────────────────────────────────────────────────┘
```

**How to interact:**

1. **Type** your message — all printable characters (including digits)
   appear at the prompt.
2. **Press Enter** to send the line to the meeting process.
3. **Press Backspace** to delete the last character.
4. **Press Escape** to kill the meeting process and return to idle.
5. **Switch tabs** with `Alt+1`–`Alt+6` or `←`/`→` arrow keys at any
   time — the meeting process continues running in the background.
   Return to Tab 5 to resume.

**Digits are typeable in meetings.** Tab switching uses `Alt+digit`
(not bare digits), so all printable characters including `0`–`9` go
directly to the meeting input buffer. This resolves a previous
limitation where digits 1–6 could not be typed in meeting mode.

**Meeting lifecycle:**

- The process spawns automatically on first visit to the Meeting tab.
- If the process exits on its own, the tab shows: `Meeting ended
  (exit code: N)`. Navigate away and back to start a new meeting.
- If `<state-root>/bin/simard` is missing, the tab shows:
  `Error: simard binary not found at <path>`.
- On TUI quit, the meeting process is killed automatically.

## 7. Check statistics

Press `Alt+6` to switch to the Stats tab:

```
┌ Stats ────────────────────────────────────────────────────────────┐
│ State files:     142                                              │
│ Session dirs:    7                                                │
│ Open issues:     23                                               │
│ Open PRs:        4                                                │
│ Active goals:    5                                                │
│ Daemon uptime:   2h 14m 33s                                      │
└───────────────────────────────────────────────────────────────────┘
```

**What each metric means:**

- **State files** — total files in `<state-root>/state/` (recursive
  count). Indicates how much state the daemon has accumulated.
- **Session dirs** — engineer session directories in
  `<state-root>/sessions/`. One per past or active session.
- **Open issues** — open GitHub issues in the repo. Requires `gh`
  CLI. Shows `–` if unavailable or still loading.
- **Open PRs** — open pull requests. Same requirements as issues.
- **Active goals** — count from the cached goal board (same data as
  Tab 2).
- **Daemon uptime** — same value as the Overview tab.

**How refresh works:**

Local metrics (state files, session dirs) update synchronously every
10 seconds. GitHub metrics (issues, PRs) are fetched in a background
thread so they never freeze the TUI. On first launch, issues/PRs show
`–` for 1–3 seconds until the background fetch completes, then
populate automatically. Goal counts update every 2 seconds (from the
shared goal board cache).

## 8. Custom state root

If your Simard instance uses a non-default state root:

```bash
SIMARD_STATE_ROOT=/srv/simard-state simard-tui
```

The TUI resolves the state root the same way the daemon does — see
[state-root resolution](../reference/state-root-resolution.md).

## 9. Custom service name

If you run Simard under a different systemd unit (e.g., a staging
instance):

```bash
SIMARD_TUI_SERVICE=simard-staging.service simard-tui
```

The service name must match `^[a-zA-Z0-9@._-]+\.service$`. Invalid
names are rejected at startup.

## 10. Running alongside other tools

`simard-tui` is safe to run alongside:

- The Simard daemon itself
- `simard goal-curation read` and other CLI commands
- The web dashboard
- Multiple `simard-tui` instances

The only resource the TUI locks is the meeting process stdin/stdout —
running two TUI instances with active Meeting tabs simultaneously is
not recommended (two meeting processes would compete for state).

## 11. Running from non-interactive environments

`simard-tui` automatically detects when stdout is not a terminal and
falls back to `/dev/tty` (the process's controlling terminal). This
means it works in most environments where a human is at a keyboard,
even if stdout has been redirected:

```bash
# These all work — /dev/tty fallback kicks in automatically:
simard-tui | tee tui.log          # stdout piped, /dev/tty used
simard-tui > /dev/null            # stdout redirected, /dev/tty used
ssh -t host simard-tui            # SSH with PTY allocation
docker run -it image simard-tui   # Container with TTY
tmux new-session simard-tui       # Inside tmux
```

**Environments without any terminal** (no stdout TTY and no
`/dev/tty`) are not supported — the TUI exits immediately with:

```
simard-tui requires a terminal. Run from an interactive shell or use: ssh -t host simard-tui
```

Examples of no-terminal environments:

- `ssh host simard-tui` (without `-t`)
- `nohup simard-tui &`
- Cron jobs
- CI/CD pipelines
- Agent sessions without PTY allocation

For headless monitoring in these environments, use the web dashboard
or `simard` CLI commands instead.

## Keyboard reference

| Key | Context | Action |
|---|---|---|
| `Alt+1`–`Alt+6` | Any tab | Switch to that tab |
| `←` (Left arrow) | Any tab | Cycle to previous tab (wraps around) |
| `→` (Right arrow) | Any tab | Cycle to next tab (wraps around) |
| `q` / `Q` | Any tab (no active meeting) | Quit |
| Printable chars | Meeting tab, process active | Type into input (all chars including digits) |
| `Enter` | Meeting tab, process active | Send input |
| `Backspace` | Meeting tab, process active | Delete last char |
| `Escape` | Meeting tab, process active | Kill meeting process |

> **Note on terminal compatibility:** Some terminals (especially over
> SSH or in screen/tmux) may not send `Alt+digit` as a modifier.
> `crossterm` handles the common encodings, but if Alt+digit does not
> work in your terminal, use `←`/`→` arrow keys as a fallback.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `simard-tui requires a terminal` | No TTY available (stdout is not a terminal and `/dev/tty` cannot be opened) | Run from an interactive shell, use `ssh -t host simard-tui`, or allocate a PTY with `docker run -it` |
| All fields show `–` or `unavailable` | Daemon not running or systemctl not available | Start the daemon: `simard ooda run` |
| Goals tab is empty | `cognitive_memory.ladybug` missing or no snapshot fact | Use `simard goal-curation read` to inspect goals via IPC |
| Terminal looks broken after exit | Rare: panic before `TerminalGuard` drop | Run `reset` to restore terminal |
| CPU stuck at `–` | First launch, only one sample taken | Wait 2 seconds for the second sample |
| Engineers tab: "Daemon not running" | No PID available | Start the daemon |
| Engineers tab: "No child processes" | Daemon idle between OODA cycles | Normal — wait for next cycle |
| Activity tab: "No log entries" | No journald or no entries for the unit | Check: `journalctl --user -u simard.service -n 5` (or use `SIMARD_TUI_SERVICE` if your unit has a different name) |
| Meeting tab: "simard binary not found" | Binary missing at state-root/bin/simard | Install Simard or check `SIMARD_STATE_ROOT` |
| Stats show `–` for issues/PRs | `gh` not installed or not authenticated, or still loading | Run `gh auth status` to diagnose; wait 3s for background fetch |

## See also

- [simard-tui reference](../reference/simard-tui.md) — full
  specification of tabs, data sources, refresh rates, and security.
- [Run the OODA daemon](./run-ooda-daemon.md) — start the daemon
  that the TUI monitors.
- [Daemon mode](../daemon-mode.md) — what the daemon does between
  cycles.
