---
title: "simard-tui: Terminal monitoring dashboard"
description: "Reference for the standalone TUI binary that monitors the Simard daemon, goals, engineers, and activity from a single terminal pane."
last_updated: 2026-06-04
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./simard-cli.md
  - ./state-root-resolution.md
  - ../daemon-mode.md
  - ../howto/monitor-simard-with-tui.md
---

# simard-tui: Terminal monitoring dashboard

`simard-tui` is a terminal dashboard for monitoring and interacting
with a running Simard daemon. It displays daemon health, goal status,
engineer subprocesses, journal logs, an interactive meeting REPL, and
aggregate statistics in a tabbed full-screen layout using
[ratatui](https://ratatui.rs) and [crossterm](https://docs.rs/crossterm).

The binary is standalone — it does not link against `libsimard` or
import `use simard::*`. It reads the same files and system interfaces
that the daemon writes, using locally-defined serde DTOs that tolerate
schema drift via `#[serde(default)]`.

Five of the six tabs are read-only observers. The Meeting tab (Tab 5)
is the exception — it spawns and interacts with a `simard meeting
start` child process.

---

## Installation

`simard-tui` ships as a `[[bin]]` target in the Simard workspace:

```bash
# Build from source
cargo build --release --bin simard-tui

# Run directly
cargo run --bin simard-tui
```

The release binary is at `target/release/simard-tui`.

---

## Quick start

```bash
# Launch the dashboard (daemon must be running)
simard-tui

# Override state root location
SIMARD_STATE_ROOT=/srv/simard simard-tui

# Monitor a non-default systemd service
SIMARD_TUI_SERVICE=simard-staging.service simard-tui
```

The TUI opens in full-screen mode with the **Overview** tab selected.
Press `Alt+1`–`Alt+6` to switch tabs directly, `←`/`→` arrow keys to
cycle through tabs, or `q` to quit.

---

## Tabs

### Tab 1: Overview

Displays daemon health at a glance:

| Field | Source | Notes |
|---|---|---|
| Service status | `systemctl show -p ActiveState` | `active`, `inactive`, `failed`, `unavailable` |
| PID | `systemctl show -p MainPID` | Shown as `–` when service is not active |
| Uptime | `systemctl show -p ActiveEnterTimestamp` | Human-friendly duration (e.g., `2h 14m 33s`) |
| OODA cycle | N/A | Shows `N/A` — no persistent counter file on disk yet |
| CPU % | `/proc/<PID>/stat` fields 14–15 | Sampled over the refresh interval; `–` when PID unavailable |
| Memory (RSS) | `/proc/<PID>/status` `VmRSS` line | Displayed in MiB; `–` when PID unavailable |

The overview refreshes daemon info from systemctl every 2 seconds and
process stats from `/proc` every 2 seconds.

### Tab 2: Goals

Reads the active goal board from cognitive memory and renders a table:

| Column | Source field | Notes |
|---|---|---|
| Description | `description` | Truncated to terminal width |
| Priority | `priority` | Numeric (`0` = highest); blank if unset |
| Status | `status` | `GoalProgress` variant: `proposed`, `not-started`, `in-progress`, `blocked`, `paused`, `completed` |
| Assigned | `assigned_to` | Engineer name, if any |

**Data source:** The `goal-board:snapshot` fact in cognitive memory,
stored in `<state-root>/cognitive_memory.ladybug`. The TUI opens the
LadybugDB file read-only, queries for the latest `goal-board:snapshot`
fact, and deserialises it as a `GoalBoard` (containing `active` and
`backlog` vectors). Only the `active` goals are rendered in the table.

If the cognitive memory DB does not exist or contains no
`goal-board:snapshot` fact, the tab shows:

> No goals found — use `simard goal-curation read` to inspect the goal board.

The DB is opened with `SQLITE_OPEN_READONLY`. If the open fails
(e.g., the daemon holds an exclusive lock during a write transaction),
the previous cached snapshot is displayed with a `(stale)` indicator.

**Size cap:** The parsed JSON payload is bounded at 10 MB before
deserialisation. Payloads exceeding this are treated as corrupt and
the stale cache is shown.

### Tab 3: Engineers

Displays daemon child processes in a table. These are the engineer
subprocesses spawned by the OODA loop to advance goals:

| Column | Source | Notes |
|---|---|---|
| PID | `/proc` scan or `pgrep --parent <daemon_pid>` | Child process ID |
| Command | `/proc/<PID>/cmdline` | Executable and arguments, truncated to 64 chars |
| Category | `<state-root>/state/subagent_sessions.json` | `agent_id` from subagent registry for matching PID; `—` if PID not in registry or file missing |
| CPU % | `/proc/<PID>/stat` delta sampling | Percentage of one CPU core; `–` until second sample |
| Memory | `/proc/<PID>/status` `VmRSS` line | Resident memory in KiB |
| Runtime | `/proc/<PID>/stat` field 22 (`starttime`) | Time since process started, as `HhMmSs` |

**Process discovery.** The TUI first tries `pgrep --parent <pid>` to
enumerate child PIDs. If `pgrep` is unavailable, it falls back to
scanning `/proc/*/stat` files and filtering by the PPID field (field
index 3 after the comm field). Thread discovery uses
`/proc/<pid>/task/` to count threads belonging to the daemon.

**CPU delta sampling.** Each child PID is tracked in a
`HashMap<u32, CpuSample>`. On each refresh, the TUI reads
`/proc/<PID>/stat`, computes the CPU delta against the previous
sample, and removes stale entries for PIDs that no longer exist. The
first sample for any new child shows `–` until a second reading is
available.

**Fallback states:**

- If the daemon is not running (no PID), the tab shows:
  > Daemon not running
- If the daemon is running but has no child processes, the tab shows:
  > No child processes

**Refresh rate:** Every 2 seconds (same as `/proc` reads for Overview).

### Tab 4: Activity

Displays the most recent 50 log entries from the Simard systemd
journal in a scrollable list, newest at the bottom:

```
2026-06-03T04:01:12+0000 [INFO]  ooda: Cycle 47 — observe phase
2026-06-03T04:01:13+0000 [INFO]  ooda: 3 signals collected
2026-06-03T04:01:14+0000 [WARN]  ooda: Goal goal-cache-layer has no active engineer
2026-06-03T04:01:15+0000 [INFO]  ooda: Decided: advance-goal(goal-cache-layer)
2026-06-03T04:01:16+0000 [ERROR] engineer: Worktree creation failed — disk full
```

**Data source.** The TUI runs:

```
journalctl --user -u simard.service --no-pager -n 50 --output=short-iso
```

The unit name defaults to `simard.service` and can be overridden via
the `SIMARD_TUI_SERVICE` environment variable.

Output is parsed into individual log lines and stored in
`Vec<String>` on the app.

**Color coding by severity:**

| Level | Color |
|---|---|
| `ERROR` | Red |
| `WARN` | Yellow |
| `INFO` | Default (white/terminal foreground) |

Level detection is case-insensitive substring matching on each log
line — `[ERROR]`, `[WARN]`, `error:`, `warning:`, etc.

**Fallback states:**

- If `journalctl` is unavailable or returns no entries:
  > No log entries
- If `journalctl` fails (permission denied, no journal for the unit):
  the previous log buffer is retained and displayed.

**Refresh rate:** Every 2 seconds.

### Tab 5: Meeting

An interactive text REPL that connects to a `simard meeting start`
child process. This is the only tab that writes data (to the meeting
process's stdin).

**Layout:**

```
┌ Meeting ──────────────────────────────────────┐
│ [meeting output scrolls here]                 │
│                                               │
│ simard> Welcome to Simard meeting mode.       │
│ simard> What would you like to discuss?       │
│                                               │
│ > Let's review the goal board priorities_     │
└───────────────────────────────────────────────┘
```

The top region shows meeting process stdout (scrollable, capped at
1000 lines to prevent OOM). The bottom line shows the input prompt
`> ` with the current input buffer and a cursor.

**Meeting process lifecycle:**

1. **Auto-spawn.** When the user first navigates to the Meeting tab,
   the TUI spawns `<state-root>/bin/simard meeting start` as a child
   process with piped stdin/stdout. If the binary does not exist at
   that path, the tab shows: `Error: simard binary not found at
   <path>`.

2. **Non-blocking I/O.** The child's stdout pipe is set to
   non-blocking mode using `fcntl(fd, F_SETFL, O_NONBLOCK)`. On each
   refresh tick, the TUI drains available output lines without
   blocking. This keeps the event loop responsive.

3. **Input handling.** When on the Meeting tab with an active process:
   - Printable characters append to the input buffer (capped at 4096
     bytes)
   - `Enter` sends the buffer contents + newline to the process stdin
     and clears the buffer
   - `Backspace` deletes the last character from the buffer
   - `Escape` kills the meeting process and returns to idle state

4. **Process exit.** If the meeting process exits on its own, the tab
   shows: `Meeting ended (exit code: N)`. Navigating away and back
   spawns a new process.

5. **Cleanup.** On TUI quit (`q` key, panic, or any exit path), the
   meeting child process is killed via SIGKILL and waited on. This is
   enforced by a `Drop` implementation on the `App` struct.

**Key routing precedence:** Tab-switch keys (`Alt+1`–`Alt+6` and
`←`/`→` arrows) always switch tabs, even when the meeting process is
active. All printable characters — including digits 0–9 — go to the
meeting input buffer when the meeting process is running. This means
digits can be typed in meeting mode without conflict with tab
switching. When the meeting process is not active, `q` quits the TUI
normally.

### Tab 6: Stats

Displays aggregate metrics as a key-value list:

```
┌ Stats ────────────────────────────────────────┐
│ State files:     142                          │
│ Session dirs:    7                            │
│ Open issues:     23                           │
│ Open PRs:        4                            │
│ Active goals:    5                            │
│ Daemon uptime:   2h 14m 33s                   │
└───────────────────────────────────────────────┘
```

| Metric | Source | Notes |
|---|---|---|
| State files | Recursive file count in `<state-root>/state/` | Total memory/state files on disk. Counted synchronously (fast local I/O) |
| Session dirs | Directory count in `<state-root>/sessions/` | One per engineer session. Counted synchronously |
| Open issues | `gh issue list --state open --limit 1000 --json number` | Parsed with `serde_json`, count of items. Shows `–` if `gh` unavailable or result pending |
| Open PRs | `gh pr list --state open --limit 1000 --json number` | Same approach. Shows `–` if `gh` unavailable or result pending |
| Active goals | Goal board `active` vector length | From the same cached `GoalBoard` used by Tab 2 |
| Daemon uptime | `DaemonInfo.uptime_secs` | Same value as Overview tab; human-formatted |

**Two-tier refresh strategy.** The Stats tab splits data collection
into a synchronous path and an asynchronous path to avoid blocking
the TUI render loop:

1. **Synchronous (local filesystem).** State file count and session
   directory count are computed inline during `refresh_stats()`. These
   are fast local I/O operations (typically <1 ms) and pose no
   blocking risk.

2. **Asynchronous (GitHub CLI).** Open issue and PR counts are fetched
   via `gh` CLI in a background thread using `std::thread::spawn` and
   `std::sync::mpsc::channel`. The thread runs both `gh issue list`
   and `gh pr list` commands, then sends the results (as
   `(Option<usize>, Option<usize>)`) back through the channel. The
   main event loop drains results via `try_recv()` on every tick, so
   values appear as soon as the background thread completes without
   ever blocking the UI.

**Duplicate fetch guard.** A boolean `gh_in_flight` flag prevents
overlapping background threads. When a `gh` fetch thread is already
running, `refresh_stats()` skips spawning another one. The flag is
cleared when the receiver channel disconnects (thread completed) or
when a result is successfully received.

**GitHub CLI interaction.** The `gh` commands are run as subprocesses
in the background thread with `Command::new("gh").arg(...)`. Output
is parsed as `Vec<serde_json::Value>` and counted with `.len()`. No
`jq` dependency. If `gh` is not installed, not authenticated, or the
command fails, the thread sends `None` for that metric and the UI
shows `–`. The `gh` commands inherit the process environment (for
`GH_TOKEN`, `GITHUB_TOKEN`, etc.) but no credentials are handled
directly by the TUI.

**Refresh rate:** Local filesystem stats refresh every 10 seconds
(gated by `tick_count % 5 == 0` on the 2-second refresh cycle). The
background `gh` thread is spawned on the same 10-second cycle. Results
from the background thread are drained on every tick (every 2 seconds),
so `gh` values appear as soon as the commands complete — typically
1–3 seconds after the fetch is initiated.

**Graceful degradation.** When `gh` is unavailable:

- The background thread sends `(None, None)`.
- The channel disconnects normally when the thread exits.
- `gh_in_flight` is cleared on the next drain cycle.
- The UI shows `–` for Open issues and Open PRs.
- No error is displayed — this is the expected state on machines
  without `gh` installed or authenticated.

---

## Keyboard controls

| Key | Context | Action |
|---|---|---|
| `Alt+1`–`Alt+6` | Any tab | Switch to tab 1–6 |
| `←` (Left arrow) | Any tab | Cycle to previous tab (wraps: tab 1 → tab 6) |
| `→` (Right arrow) | Any tab | Cycle to next tab (wraps: tab 6 → tab 1) |
| `q` / `Q` | Any tab (meeting not active) | Quit and restore terminal |
| Printable chars | Meeting tab, process active | Append to meeting input buffer (all chars including digits) |
| `Enter` | Meeting tab, process active | Send input buffer to meeting process |
| `Backspace` | Meeting tab, process active | Delete last character from input buffer |
| `Escape` | Meeting tab, process active | Kill meeting process |

All keys are processed on `KeyEvent` with `kind == Press` to avoid
double-firing on terminals that emit both press and release events.

The `handle_key` method accepts a full `crossterm::event::KeyEvent`
(not just a `char`) to support modifier detection (Alt) and special
keys (arrows, Enter, Backspace, Escape) for the meeting REPL and tab
navigation.

**`Tab::from_key()` signature.** The `from_key` method takes a
`&KeyEvent` and matches `KeyCode::Char('1'..='6')` only when the
`ALT` modifier is present (`key.modifiers.contains(KeyModifiers::ALT)`).
Bare digit presses without Alt are ignored by the tab-switching logic
and fall through to meeting input or are discarded on non-meeting tabs.

**Arrow key wrapping.** Left arrow on the first tab (index 0) wraps
to the last tab (index 5). Right arrow on the last tab wraps to the
first. The wrapping uses modular arithmetic on `ALL_TABS.len()`.

**Terminal compatibility note.** Some terminals (especially over SSH,
in screen/tmux, or on macOS Terminal.app) may not send `Alt+digit`
as a `KeyModifiers::ALT` modifier — they may encode it as an escape
sequence that crossterm handles differently. The `←`/`→` arrow keys
provide a universal fallback for tab cycling in those environments.

### Layout

The TUI frame uses a 3-chunk vertical layout:

```
┌──────────────────────────────────────────────┐
│ [1:Overview] [2:Goals] ... [6:Stats]         │ ← Tab bar (3 rows)
├──────────────────────────────────────────────┤
│                                              │
│         Active tab content                   │ ← Content (flexible)
│                                              │
├──────────────────────────────────────────────┤
│ Alt+1‥6: tabs | ←/→: cycle | q: quit        │ ← Footer (1 row)
└──────────────────────────────────────────────┘
```

The footer is a `Paragraph` widget rendered in a 1-row bottom chunk,
showing a compact keybinding reference. The constraints are
`[Length(3), Min(0), Length(1)]`.

---

## Refresh behavior

The TUI uses a tiered refresh strategy:

| Data source | Interval | Rationale |
|---|---|---|
| `/proc/<PID>/stat`, `/proc/<PID>/status` (daemon + children) | 2 s | Cheap kernel reads; responsive CPU/memory |
| Cognitive memory (goal board) | 2 s | Read-only DB open; stale cache on contention |
| `systemctl show` | 2 s | Service status and PID |
| `journalctl` (activity logs) | 2 s | Keeps log view current |
| Child process scan (engineers) | 2 s | `/proc` walk or `pgrep` |
| Meeting process stdout drain | Every key event + every tick | Non-blocking read; latency-sensitive |
| Local fs stats (state files, session dirs) | 10 s | Directory scans; gated by tick counter |
| `gh` commands (issues, PRs) — background thread | 10 s spawn, 2 s drain | Spawned on slow cycle; results drained via `try_recv()` every tick |

The event loop polls for keyboard input with a 200 ms timeout, then
checks whether the next refresh tick has elapsed. A `tick_count: u32`
field on `App` increments each refresh. Stats-tab data sources gate
on `tick_count % 5 == 0`. The `gh` commands run in a background
thread via `std::thread::spawn` + `std::sync::mpsc::channel`, so
they never block the render loop. Results are picked up on the next
tick via `try_recv()`. A `gh_in_flight` guard prevents overlapping
thread spawns when the previous fetch has not yet completed.

---

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `SIMARD_STATE_ROOT` | `$HOME/.simard` | Root directory for all Simard state files. Resolution follows the same ladder as the daemon (see [state-root resolution](./state-root-resolution.md)). |
| `SIMARD_TUI_SERVICE` | `simard.service` | systemd unit name to query. Must match `^[a-zA-Z0-9@._-]+\.service$` — rejected with an error if it contains shell-unsafe characters. |

Both variables are read once at startup and do not change during the
session.

---

## State root resolution

`simard-tui` replicates the daemon's state-root resolution logic
locally (it cannot import `simard::state_root` because it is a
standalone binary):

1. If `$SIMARD_STATE_ROOT` is set, non-empty, absolute, and free of
   NUL bytes → use it.
2. Otherwise → `$HOME/.simard/`.

A relative or NUL-bearing `SIMARD_STATE_ROOT` is silently ignored,
matching the daemon's behavior documented in
[state-root resolution](./state-root-resolution.md).

---

## Data files and system interfaces

| Source | Format | What the TUI extracts |
|---|---|---|
| `<state-root>/cognitive_memory.ladybug` | LadybugDB (SQLite) | `goal-board:snapshot` fact → `GoalBoard` JSON |
| `<state-root>/state/` | Directory tree | Recursive file count for stats |
| `<state-root>/sessions/` | Directory listing | Session directory count for stats |
| `<state-root>/state/subagent_sessions.json` | JSON file | PID → `agent_id` mapping for engineer category enrichment |
| `<state-root>/bin/simard` | Executable | Meeting process binary (spawned, not read) |
| `/proc/<PID>/stat` | Kernel pseudo-file | CPU time fields (utime, stime, starttime) |
| `/proc/<PID>/status` | Kernel pseudo-file | `VmRSS:` line for memory |
| `/proc/<PID>/cmdline` | Kernel pseudo-file | Process command line (NUL-separated) |
| `/proc/<PID>/task/` | Kernel pseudo-dir | Thread listing for daemon |
| `systemctl show` | CLI output | Service state, PID, timestamps |
| `journalctl --user` | CLI output | Recent log entries |
| `pgrep --parent` | CLI output | Child PID enumeration |
| `gh issue list` / `gh pr list` | CLI + JSON output | Open issue/PR counts (run in background thread) |

The TUI defines its own serde DTOs (`GoalBoard`, `ActiveGoal`,
`GoalProgress`). Required fields (`id`, `description`, `priority`,
`status`) must be present; optional fields (`assigned_to`,
`current_activity`, `wip_refs`) use `#[serde(default)]` and default to
`None` or empty when absent. Unknown fields are silently ignored. This
tolerates schema additions in the daemon without requiring a
synchronized TUI release.

For the subagent session registry, the TUI defines lightweight local
structs matching the `Registry` / `SubagentSession` shapes from
`simard::subagent_sessions`. Only the fields needed for PID → category
lookup are deserialized (`agent_id`, `pid`, `ended_at`). No new crate
dependencies are required — `serde` and `serde_json` are already in the
workspace. Corrupt or missing JSON produces an empty map (no error).

### Goal board JSON schema (as consumed)

The `goal-board:snapshot` fact payload is a JSON object:

```json
{
  "active": [
    {
      "id": "goal-cache-layer",
      "description": "Implement caching layer",
      "priority": 1,
      "status": "InProgress",
      "assigned_to": "engineer-alpha",
      "current_activity": "Writing integration tests"
    }
  ],
  "backlog": [
    {
      "id": "b-retry",
      "description": "Add retry logic to bridge",
      "source": "review",
      "score": 0.72
    }
  ]
}
```

`GoalProgress` variants use serde's default externally-tagged
representation and serialise as: `"Proposed"`, `"NotStarted"`,
`{"InProgress": {"percent": 40}}`, `{"Blocked": "reason"}`,
`"Paused"`, `"Completed"`.

`ActiveGoal` has four required fields (`id`, `description`, `priority`,
`status`) and three optional fields (`assigned_to`, `current_activity`,
`wip_refs`) that default to `None` or empty when absent.

---

## Process information

The TUI reads kernel pseudo-files for the daemon PID and its children:

| Path | Fields used | Purpose |
|---|---|---|
| `/proc/<PID>/stat` | Field 14 (`utime`), field 15 (`stime`), field 4 (`ppid`), field 22 (`starttime`) | CPU usage, parent-child relationships, PID-reuse guard |
| `/proc/<PID>/status` | `VmRSS:` line | Resident memory in kB |
| `/proc/<PID>/cmdline` | NUL-separated bytes | Process command line for display |
| `/proc/<PID>/task/` | Directory entry count | Thread count |

Reads are capped at 4 KB — kernel proc files are small; a larger
response indicates something unexpected and is treated as a read
failure.

### CPU calculation

CPU percentage is derived from two consecutive `/proc/<PID>/stat`
samples:

```
cpu_pct = (delta_utime + delta_stime) / (delta_wall_clock * clock_ticks_per_sec) * 100
```

Where `clock_ticks_per_sec` is obtained from `sysconf(_SC_CLK_TCK)`.
The first sample after startup shows `–` until a second sample is
available. This applies both to the daemon PID (Overview tab) and to
each child PID (Engineers tab).

### PID reuse guard

Field 22 (`starttime`) is compared between consecutive reads. If it
changes, the PID has been reused by a different process and the TUI
resets its CPU sampling state and re-queries systemctl for the new PID.

### Child process discovery

Two strategies, tried in order:

1. **`pgrep --parent <daemon_pid>`** — returns one PID per line.
   Preferred because it is a single subprocess call.

2. **`/proc` scan fallback** — reads `/proc/*/stat` for all numeric
   directories, extracts the PPID field (field 4 after the `(comm)`
   field), and filters to those matching the daemon PID. Capped at
   500 PIDs to bound scan time.

For each discovered child PID, the TUI reads `/proc/<PID>/cmdline`
(NUL-delimited, decoded to space-separated, truncated to 64 chars)
and `/proc/<PID>/status` for `VmRSS`.

---

## Terminal safety

`simard-tui` uses a `TerminalGuard` pattern to guarantee terminal
restoration on any exit path:

1. **`TerminalGuard` struct** — enables raw mode and enters the
   alternate screen in its constructor; disables raw mode and leaves
   the alternate screen in its `Drop` implementation.

2. **Chained panic hook** — installs a panic hook that restores the
   terminal before delegating to the default handler. This prevents
   a broken terminal on unhandled panics.

3. **Graceful exit** — pressing `q` exits the event loop normally,
   which drops `TerminalGuard` through normal RAII cleanup. The app's
   `cleanup()` method kills any active meeting process before exit.

4. **RAII meeting cleanup** — The `App` struct implements `Drop` to
   kill and wait on the meeting child process. This handles panics,
   early returns, and any exit path that bypasses explicit cleanup.

---

## Security considerations

- **No shell invocation.** All subprocess calls use
  `Command::new("systemctl").arg(...)` — never
  `Command::new("sh").arg("-c")`. This prevents shell injection by
  construction. The meeting process, `gh`, `pgrep`, and `journalctl`
  calls all follow this pattern.

- **Service name validation.** `SIMARD_TUI_SERVICE` is validated
  against `^[a-zA-Z0-9@._-]+\.service$` at startup. Invalid values
  cause an immediate exit with an error message.

- **Bounded `gh` output.** `--limit 1000` caps issue/PR list queries
  to prevent unbounded memory from large repositories.

- **Bounded reads.** Goal board payload reads are capped at 10 MB;
  `/proc` reads at 4 KB. Meeting output is capped at 1000 lines.
  Meeting input is capped at 4096 bytes. Log lines are truncated to
  1024 chars. Process names are truncated to 64 chars. This prevents
  OOM from corrupt, malicious, or oversized data.

- **Child process scan cap.** The `/proc` fallback walk for child
  PIDs is capped at 500 entries to prevent resource exhaustion on
  systems with many processes.

- **No unsafe code.** The TUI uses safe Rust throughout, with one
  exception: `fcntl` for non-blocking I/O on the meeting stdout pipe
  uses `libc::fcntl` through the `std::os::unix::io::AsRawFd` trait.
  This is a single, well-understood syscall.

- **No persistent writes.** The TUI never writes to any Simard state
  file or system service. The only write target is the meeting
  process's stdin pipe.

- **No `/proc/*/environ` reads.** The TUI reads only `stat`,
  `status`, `cmdline`, and `task/` — never environment variables.

- **Sanitized external output.** `gh` stderr is not shown to the user
  — failures display a generic `–` placeholder. `journalctl` errors
  retain the previous log buffer. The background `gh` thread
  communicates only via `mpsc::channel` with typed
  `(Option<usize>, Option<usize>)` values — no raw strings cross
  the thread boundary.

- **Meeting output is memory-only.** Meeting session text is not
  persisted to disk by the TUI.

---

## Limitations

- **OODA cycle count** is not available on disk — the daemon does not
  persist a counter file. The Overview tab shows `N/A`. This can be
  extracted from journal logs in a future iteration.

- **Alt+digit may not work on all terminals.** Some terminals
  (especially over SSH, in screen/tmux, or macOS Terminal.app) encode
  Alt+digit as escape sequences that crossterm may not recognize as
  an ALT modifier. The `←`/`→` arrow keys provide a universal fallback
  for tab cycling in those environments.

- **Category column depends on tmux tracking.** The Engineers tab
  Category column requires the daemon to use tmux-wrapped engineer
  spawning (which writes `subagent_sessions.json`). If tmux is not
  installed or the daemon predates the tracking feature, all categories
  show `—`. This is cosmetic — process monitoring works normally.

- **Non-systemd hosts** (containers, macOS, WSL without systemd) show
  daemon status as `unavailable`. Process-level monitoring still works
  if a PID file is present or a future `--pid` flag is added.

- **Schema drift.** The standalone serde DTOs may diverge from the
  daemon's `GoalBoard`/`ActiveGoal`/`GoalProgress` types over time.
  Fixture-based tests with real JSON samples mitigate this, but
  operators should rebuild the TUI when upgrading the daemon.

- **`gh` CLI required for stats.** Open issue/PR counts require `gh`
  to be installed and authenticated. Without it, those fields show `–`.

- **`journalctl` required for activity.** The Activity tab reads from
  the systemd journal. On systems without journald, the tab shows
  "No log entries".

- **Meeting binary path.** The meeting process expects
  `<state-root>/bin/simard` to exist. If it is missing (e.g., the
  binary was not installed), the Meeting tab shows an error.

- **Single-threaded render loop.** The TUI render loop runs in a
  single thread. Local subprocess calls (`systemctl`, `journalctl`,
  `pgrep`) are synchronous but fast. The `gh` CLI calls are the
  exception — they run in a background thread via
  `std::thread::spawn` to avoid blocking the UI during network I/O
  (typically 1–3 seconds).

---

## Troubleshooting

### "Service unavailable" on Overview tab

The TUI could not reach `systemctl`. Possible causes:

- Not running on a systemd host (container, macOS).
- The service unit name is wrong — check `SIMARD_TUI_SERVICE`.
- The user does not have permission to query the service (try
  `systemctl --user` if running as a user service).

### "(stale)" indicator on Goals tab

The cognitive memory DB could not be opened read-only — the daemon
may be holding an exclusive lock during a write transaction. The
display shows the last successfully read snapshot. This resolves on
the next 2-second refresh.

### Blank Goals tab with "No goals found" message

The file `<state-root>/cognitive_memory.ladybug` does not exist, or
it contains no `goal-board:snapshot` fact. Use `simard goal-curation
read` to inspect the live goal board via the daemon's IPC channel.

### CPU shows "–" for the first few seconds

This is expected. CPU percentage requires two consecutive samples to
compute a delta. The value appears after the first refresh interval
(2 seconds). This applies to both the daemon CPU (Overview) and
child process CPU (Engineers).

### Engineers tab shows "Daemon not running"

The daemon PID is not available — either the service is not active or
systemctl is unreachable. Start the daemon with `simard ooda run`.

### Engineers tab shows "No child processes"

The daemon is running but has no active engineer subprocesses. This is
normal between OODA cycles — the daemon only spawns engineers when it
decides to advance a goal.

### Activity tab shows "No log entries"

Possible causes:

- `journalctl` is not installed (non-systemd host).
- The user journal has no entries for the configured service unit
  (default: `simard.service`, overridden by `SIMARD_TUI_SERVICE`).
- The user does not have permission to read the journal.

Try: `journalctl --user -u simard.service -n 5` to verify access
(substitute your `SIMARD_TUI_SERVICE` value if overridden).

### Meeting tab shows "simard binary not found"

The binary at `<state-root>/bin/simard` does not exist. Install Simard
or verify `SIMARD_STATE_ROOT` points to the correct location.

### Stats show "–" for issues/PRs

The `gh` CLI is either not installed, not authenticated, or the
repository is not accessible. It is also normal to see `–` briefly
(1–3 seconds) after launch while the background fetch thread is
running. If the values remain `–` after 10 seconds, run
`gh auth status` and `gh issue list` manually to diagnose.

---

## Building from source

```bash
# Debug build
cargo build --bin simard-tui

# Release build
cargo build --release --bin simard-tui

# Run tests (TUI-specific inline #[cfg(test)] modules)
cargo test --bin simard_tui
```

The TUI depends on `ratatui` and `crossterm`, which are added to the
workspace `[dependencies]` section. These are compile-time only — no
runtime dynamic linking.

---

## See also

- [How to monitor Simard with the TUI](../howto/monitor-simard-with-tui.md)
  — step-by-step usage guide.
- [`simard` CLI reference](./simard-cli.md) — the primary CLI surface.
- [State-root resolution](./state-root-resolution.md) — how the state
  root is determined.
- [Daemon mode](../daemon-mode.md) — what the daemon does between
  cycles.
- [Dashboard](../dashboard.md) — the web-based alternative.
