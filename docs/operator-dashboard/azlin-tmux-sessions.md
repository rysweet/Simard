# Azlin Tmux Sessions Panel

The **Azlin Sessions** panel is a companion card on the operator dashboard's
**Terminal** tab that lists `tmux` sessions running on every reachable azlin
host in your cluster, and lets you attach to any session in one click.

It reuses the canonical cluster-membership source (`~/.simard/hosts.json` via
`load_hosts()`) and the same `azlin connect` exec channel that powers the
existing host-status checks — no new SSH transport is introduced.

---

## At a glance

| Capability         | Behaviour                                                                     |
| ------------------ | ----------------------------------------------------------------------------- |
| Host source        | `load_hosts()` → `~/.simard/hosts.json` (canonical)                           |
| Session discovery  | `azlin connect <host> --no-tmux -- tmux list-sessions -F '…'` (5 s/host)      |
| Parallelism        | All hosts queried concurrently (`futures::future::join_all`)                  |
| Auto-refresh       | Every **10 s** while the Terminal tab is active                               |
| Attach transport   | WebSocket → `azlin connect <host> --no-tmux -- tmux attach -t <session>`      |
| Terminal renderer  | The existing xterm.js instance in the Agent Log card (re-used)                |
| Unreachable hosts  | Greyed row + error text; never break the table                                |

---

## Using the panel

1. Open the operator dashboard.
2. Click the **Terminal** tab.
3. The **Azlin Sessions** card appears alongside the Agent Log card and lists
   every host with one table per host:

   | Session | Created             | Attached? | Windows | Action |
   | ------- | ------------------- | --------- | ------- | ------ |
   | main    | 2026-04-19 10:13:02 | ✓         | 3       | `Open` |
   | build   | 2026-04-19 11:42:58 | —         | 1       | `Open` |

4. The footer shows **Last refreshed: HH:MM:SS** (locale-formatted) and a
   manual **Refresh** button (`data-testid="tmux-refresh"`).
5. Click **Open** on any row. The Agent Log terminal switches to attach mode:
   - `#agent-log-name` → `<host>:<session>`
   - `#agent-log-status` → `attached`
   - The xterm pane shows the live tmux session.
6. To leave, switch tabs or click **Open** on a different session — the
   previous WebSocket is closed automatically (`kill_on_drop` ensures the
   server-side `azlin connect` subprocess is reaped).

### Reachability semantics

| Subprocess outcome                                  | UI state                                  |
| --------------------------------------------------- | ----------------------------------------- |
| Exit 0, parsed sessions                             | Reachable, table populated                |
| Exit 0, empty stdout                                | Reachable, "No sessions"                  |
| Exit ≠ 0 with `no server running` (or empty stdout) | Reachable, "No sessions"                  |
| Spawn error / 5 s timeout / non-zero with stderr    | Unreachable, greyed row, `error` text     |

`stderr` from the per-host probe is captured into `error` (truncated to 256
chars) only when the host is marked unreachable; otherwise it is discarded.

---

## REST API

### `GET /api/azlin/tmux-sessions`

Returns the current snapshot of tmux sessions across all hosts. The handler
always returns **200**; per-host failures are encoded in `error`.

**Response**

```json
{
  "hosts": [
    {
      "host": "vm-1",
      "reachable": true,
      "sessions": [
        {
          "name": "main",
          "created": 1700000000,
          "attached": false,
          "windows": 3
        }
      ],
      "error": null
    },
    {
      "host": "vm-2",
      "reachable": false,
      "sessions": [],
      "error": "timeout after 5s"
    }
  ],
  "refreshed_at": "2026-04-19T17:43:42Z"
}
```

**Field reference**

| Field                       | Type                | Notes                                       |
| --------------------------- | ------------------- | ------------------------------------------- |
| `hosts[].host`              | `string`            | Host name from `hosts.json`                 |
| `hosts[].reachable`         | `bool`              | See reachability table above                |
| `hosts[].sessions[].name`   | `string`            | tmux `#S`                                   |
| `hosts[].sessions[].created` | `i64` (unix secs)  | tmux `#{session_created}`                   |
| `hosts[].sessions[].attached` | `bool`            | tmux `#{session_attached}` (`1` → `true`)   |
| `hosts[].sessions[].windows` | `u32`              | tmux `#{session_windows}`                   |
| `hosts[].error`             | `string \| null`    | Non-null only when `reachable=false`        |
| `refreshed_at`              | ISO-8601 string     | UTC timestamp of the snapshot               |

### `GET /ws/tmux_attach/{host}/{session}` (WebSocket)

Bidirectional binary WebSocket bridging xterm.js to a server-side
`azlin connect <host> --no-tmux -- tmux attach -t <session>` subprocess.

| Direction        | Frame type      | Contents                              |
| ---------------- | --------------- | ------------------------------------- |
| Client → Server  | Binary or Text  | xterm key bytes → subprocess `stdin`  |
| Server → Client  | Binary (4 KiB)  | subprocess `stdout` → xterm           |
| Server close     | —               | Subprocess exited or hit error        |
| Client close     | —               | Server kills subprocess (kill_on_drop)|

**Validation** (server-side, before spawn):

- `host` must appear in `load_hosts()`.
- `session` must match `^[A-Za-z0-9_.-]{1,64}$`.

**Close codes**

| Code | Meaning                                                       |
| ---- | ------------------------------------------------------------- |
| 1000 | Normal closure (subprocess exited 0, or client disconnected)  |
| 1008 | Policy violation (host not in `hosts.json` or bad session name) |
| 1011 | Server error (subprocess exited non-zero or spawn failed)     |

---

## Configuration

The panel has **no new configuration**. It inherits:

| Source              | Used for                                |
| ------------------- | --------------------------------------- |
| `~/.simard/hosts.json` | Host enumeration via `load_hosts()`  |
| Existing `azlin connect` SSH config | Transport to each host  |

Per-host tmux query timeout is fixed at **5 seconds**. The auto-refresh
interval is fixed at **10 seconds** (matches the existing `tabRefreshTimers`
idiom on other dashboard tabs).

---

## Examples

### Curl the snapshot

```bash
curl -s http://localhost:8765/api/azlin/tmux-sessions | jq
```

### Attach via `wscat`

```bash
wscat -b -c ws://localhost:8765/ws/tmux_attach/vm-1/main
```

(Binary frames; press keys to drive tmux, Ctrl-C to detach the WebSocket —
the server reaps the subprocess automatically.)

### Programmatic poller

```python
import requests, time
while True:
    snap = requests.get("http://localhost:8765/api/azlin/tmux-sessions").json()
    for h in snap["hosts"]:
        if h["reachable"]:
            print(h["host"], [s["name"] for s in h["sessions"]])
        else:
            print(h["host"], "DOWN:", h["error"])
    time.sleep(10)
```

---

## Architecture notes

```
┌───────────── Browser ─────────────┐        ┌──────────── Dashboard ─────────────┐
│                                   │  REST  │                                    │
│  Terminal tab                     │◀──────▶│  GET /api/azlin/tmux-sessions      │
│  ├── Agent Log card (xterm)       │        │      │                             │
│  └── Azlin Sessions card          │        │      ▼                             │
│       ├── per-host <table>        │        │  load_hosts() ──▶ join_all(        │
│       └── Open buttons            │        │      azlin connect <h> --no-tmux   │
│            │                      │        │        -- tmux list-sessions -F …) │
│            ▼                      │   WS   │                                    │
│      ws://…/tmux_attach/…  ◀──────┼──────▶ │  GET /ws/tmux_attach/{host}/{sess} │
│                                   │ binary │      │                             │
└───────────────────────────────────┘        │      ▼                             │
                                             │  tokio::process::Command           │
                                             │   azlin connect <h> --no-tmux      │
                                             │     -- tmux attach -t <s>          │
                                             │  (kill_on_drop = true)             │
                                             └────────────────────────────────────┘
```

**Key design choice — why no new SSH path?**
Both the snapshot route and the attach WebSocket invoke `azlin connect <host> --no-tmux -- <cmd>`,
which is the same exec channel used by `distributed()` and host-status checks
in `routes.rs`. The attach WebSocket simply runs that channel in duplex mode
(piped stdin/stdout) instead of one-shot. tmux itself handles terminal
emulation end-to-end; the dashboard only needs binary passthrough.

---

## Testing

| Layer        | Test                                                                                  |
| ------------ | ------------------------------------------------------------------------------------- |
| Unit (Rust)  | `parse_tmux_sessions_basic` / `_empty` / `_no_server` / `_malformed`                  |
| Unit (Rust)  | `host_enumeration_reads_load_hosts` (tempdir + `HOME` override)                       |
| E2E (Playwright, `@structural`) | `azlin-tmux-sessions.spec.ts` — mocks `/api/hosts`, `/api/azlin/tmux-sessions`, and `/ws/tmux_attach/vm-1/main` via `routeWebSocket`; verifies table render, last-refreshed text, click-to-attach, and that a canned server frame appears in xterm. |

Run them with:

```bash
cargo test -p operator_commands_dashboard parse_tmux_sessions
cargo test -p operator_commands_dashboard host_enumeration_reads_load_hosts
npx playwright test tests/e2e-dashboard/specs/azlin-tmux-sessions.spec.ts
```

---

## Troubleshooting

| Symptom                                         | Likely cause / fix                                              |
| ----------------------------------------------- | --------------------------------------------------------------- |
| Host always shows "DOWN: timeout after 5s"      | `azlin connect <host>` is hanging — check SSH/network           |
| Empty session list but host is up               | tmux server not running on host (expected; row says "No sessions") |
| Attach WS closes with code `1008`               | Bad host (not in `hosts.json`) or session name failed regex     |
| Attach pane hangs on connect                    | tmux session name is correct? Try `tmux ls` on host directly    |
| Last-refreshed timestamp stops updating         | You navigated away from the Terminal tab (refresh is paused)    |

---

## Out of scope

The panel intentionally does **not** support:

- Creating, killing, or renaming tmux sessions.
- Multiplexing multiple attach panes simultaneously (Open replaces the active attach).
- A new SSH library or transport — only `azlin connect` is used.
- Window/pane-level navigation beyond what tmux itself renders inside the attach pane.
