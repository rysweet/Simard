---
title: "simard-tui: Terminal monitoring dashboard"
description: "Reference for the standalone TUI binary that monitors the Simard daemon, goals, engineers, and activity from a single terminal pane."
last_updated: 2026-06-03
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

`simard-tui` is a read-only terminal dashboard for monitoring a running
Simard daemon. It displays daemon health, goal status, engineer
subprocesses, journal logs, and aggregate statistics in a tabbed
full-screen layout using [ratatui](https://ratatui.rs) and
[crossterm](https://docs.rs/crossterm).

The binary is standalone — it does not link against `libsimard` or
import `use simard::*`. It reads the same files and system interfaces
that the daemon writes, using locally-defined serde DTOs that tolerate
schema drift via `#[serde(default)]`.

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
Press number keys `1`–`6` to switch tabs, `q` to quit.

---

## Tabs

### Tab 1: Overview

Displays daemon health at a glance:

| Field | Source | Notes |
|---|---|---|
| Service status | `systemctl show -p ActiveState` | `active`, `inactive`, `failed`, `unavailable` |
| PID | `systemctl show -p MainPID` | Shown as `–` when service is not active |
| Uptime | `systemctl show -p ActiveEnterTimestamp` | Human-friendly duration (e.g., `2h 14m 33s`) |
| OODA cycle | N/A in MVP | Shows `N/A` — no persistent counter file on disk yet |
| CPU % | `/proc/<PID>/stat` fields 14–15 | Sampled over the refresh interval; `–` when PID unavailable |
| Memory (RSS) | `/proc/<PID>/status` `VmRSS` line | Displayed in MiB; `–` when PID unavailable |

The overview refreshes daemon info from systemctl every 10 seconds and
process stats from `/proc` every 2 seconds. This avoids spawning
`systemctl` on every tick while keeping CPU/memory readings responsive.

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

### Tab 3: Engineers (placeholder)

Shows active engineer subprocesses. MVP displays a placeholder message:

> Engineer monitoring — coming soon.

Future implementation will enumerate `/proc` entries matching known
Simard engineer worktree patterns.

### Tab 4: Activity (placeholder)

Tails `journalctl` logs for the Simard service. MVP displays:

> Activity log — coming soon.

Future implementation will stream `journalctl -u simard.service -f`
output into a scrollable buffer.

### Tab 5: Meeting (placeholder)

REPL connecting to the meeting PTY. MVP displays:

> Meeting REPL — coming soon.

### Tab 6: Stats (placeholder)

Aggregate counts and summaries. MVP displays:

> Statistics — coming soon.

---

## Keyboard controls

| Key | Action |
|---|---|
| `1`–`6` | Switch to tab 1–6 |
| `q` | Quit and restore terminal |

All keys are processed on `KeyEvent` with `kind == Press` to avoid
double-firing on terminals that emit both press and release events.

---

## Refresh behavior

The TUI uses a dual-rate refresh strategy:

| Data source | Interval | Rationale |
|---|---|---|
| `/proc/<PID>/stat`, `/proc/<PID>/status` | 2 s | Cheap kernel reads; responsive CPU/memory |
| Cognitive memory (goal board) | 2 s | Read-only DB open; stale cache on contention |
| `systemctl show` | 10 s | Avoids spawning a subprocess every 2 s |

The event loop polls for keyboard input with a 200 ms timeout, then
checks whether the next refresh tick has elapsed. This keeps input
responsive without busy-waiting.

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

## Data files read

| File | Format | What the TUI extracts |
|---|---|---|
| `<state-root>/cognitive_memory.ladybug` | LadybugDB (SQLite-based) | `goal-board:snapshot` fact → `GoalBoard` JSON |

The TUI defines its own serde DTOs (`GoalBoard`, `ActiveGoal`,
`GoalProgress`) with `#[serde(default)]` on every field. Unknown fields
are silently ignored. This tolerates schema changes in the daemon
without requiring a synchronized TUI release.

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
      "description": "Add retry logic to bridge",
      "priority": 2
    }
  ]
}
```

`GoalProgress` variants serialise as: `"Proposed"`, `"NotStarted"`,
`{"InProgress": {"percent": 40}}`, `{"Blocked": "reason"}`,
`"Paused"`, `"Completed"`. The TUI DTOs accept both tagged enum form
and the single-string form via `#[serde(untagged)]` fallback.

All fields except `description` in `ActiveGoal` are optional and
default to empty/unknown when absent.

---

## Process information

The TUI reads kernel pseudo-files for the daemon's PID:

| Path | Fields used | Purpose |
|---|---|---|
| `/proc/<PID>/stat` | Field 14 (`utime`), field 15 (`stime`), field 22 (`starttime`) | CPU usage calculation and PID-reuse guard |
| `/proc/<PID>/status` | `VmRSS:` line | Resident memory in kB |

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
available.

### PID reuse guard

Field 22 (`starttime`) is compared between consecutive reads. If it
changes, the PID has been reused by a different process and the TUI
resets its CPU sampling state and re-queries systemctl for the new PID.

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
   which drops `TerminalGuard` through normal RAII cleanup.

---

## Security considerations

- **No shell invocation.** All subprocess calls use
  `Command::new("systemctl").arg(...)` — never
  `Command::new("sh").arg("-c")`. This prevents shell injection by
  construction.

- **Service name validation.** `SIMARD_TUI_SERVICE` is validated
  against `^[a-zA-Z0-9@._-]+\.service$` at startup. Invalid values
  cause an immediate exit with an error message.

- **Bounded reads.** Goal board payload reads are capped at 10 MB;
  `/proc` reads at 4 KB. This prevents OOM from corrupt or oversized
  data.

- **No unsafe code.** The TUI uses safe Rust throughout. File I/O
  uses `SQLITE_OPEN_READONLY` and bounded `read_to_string` calls.

- **No network access.** The TUI is strictly a local monitoring tool.

- **Read-only.** The TUI never writes to any Simard state file,
  process, or service. It is a passive observer.

---

## Limitations

- **OODA cycle count** is not available on disk — the daemon does not
  persist a counter file. The Overview tab shows `N/A`. This can be
  extracted from journal logs in a future iteration.

- **Engineer process discovery** is not yet implemented. Tab 3 shows
  a placeholder.

- **Journal log tailing** (Tab 4), **Meeting REPL** (Tab 5), and
  **Statistics** (Tab 6) are placeholders in the MVP.

- **Non-systemd hosts** (containers, macOS, WSL without systemd) show
  daemon status as `unavailable`. Process-level monitoring still works
  if a PID file is present or a future `--pid` flag is added.

- **Schema drift.** The standalone serde DTOs may diverge from the
  daemon's `GoalBoard`/`ActiveGoal`/`GoalProgress` types over time.
  Fixture-based tests with real JSON samples mitigate this, but
  operators should rebuild the TUI when upgrading the daemon.

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
(2 seconds).

---

## Building from source

```bash
# Debug build
cargo build --bin simard-tui

# Release build
cargo build --release --bin simard-tui

# Run tests (TUI-specific tests)
cargo test --test tui_types_test --test tui_goals_test --test tui_system_test
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
