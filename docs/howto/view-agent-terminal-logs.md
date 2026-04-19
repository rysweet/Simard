# How to view live agent logs in the dashboard terminal widget

The operator dashboard ships with a **Terminal** tab that streams the live
stdout/stderr of any running subordinate agent over a WebSocket, rendered
through [xterm.js](https://xtermjs.org/). It behaves like `tail -f` on the
agent's log file and supports a 200-line backfill on connect so you do not
miss recent output.

## Prerequisites

- A running Simard operator dashboard (see
  [Simard CLI reference](../reference/simard-cli.md)).
- An authenticated browser session (cookie `simard_session`). The terminal
  WebSocket inherits the existing dashboard auth; no extra credentials are
  needed.
- At least one agent has been spawned via the supervisor so a log file exists
  under `<state_root>/agent_logs/`.

## Step 1 — Open the Terminal tab

1. Navigate to the dashboard root (`/`).
2. Click the **Terminal** tab in the top tab bar.
3. The Terminal pane shows:
   - An **Agent name** text input (pre-filled with the currently selected
     agent, if any).
   - **Connect** / **Disconnect** buttons.
   - A status line ("idle", "connecting…", "streaming", "closed", "error").
   - The xterm.js host area where output is rendered.

## Step 2 — Connect to an agent

1. Type the agent name (e.g. `engineer-7`) and click **Connect**.
2. The dashboard opens a WebSocket to
   `GET /ws/agent_log/{agent_name}` (using `wss://` when the dashboard is
   served over HTTPS).
3. On successful upgrade, the last **200 lines** of the agent log are
   replayed into the terminal, then new lines stream live (~200 ms poll
   cadence).

If the log file does not yet exist, the server waits up to **30 seconds**
for it to appear. If it never appears, you receive a single human-readable
notice frame and the connection closes cleanly. (The exact wording of
notice frames is not part of the API contract — see the
[WebSocket reference](../reference/agent-log-websocket.md#frame-format).)

## Step 3 — Disconnect

Click **Disconnect** (or close the tab). The client closes the WebSocket
and disposes the xterm instance. The server-side tail loop exits on the
next tick.

## Behavior details

For the full contract (status codes, state machine, security model), see
the [agent log WebSocket reference](../reference/agent-log-websocket.md).

| Behavior            | Value                                                         |
| ------------------- | ------------------------------------------------------------- |
| Backfill            | Last 200 lines via `read_tail` (fewer if the file is shorter) |
| Poll interval       | 200 ms                                                        |
| Per-tick read cap   | 1 MiB (remainder is delivered on subsequent ticks)            |
| Truncation handling | If file shrinks (rotation), position resets to 0 with notice  |
| Inbound frames      | Ignored (server → client only); only `Close` is honored       |
| Auth                | Inherited dashboard cookie middleware (`require_auth`)        |
| Frame format        | Plain UTF-8 text, one log line per frame                      |
| Concurrent viewers  | Safe — multiple tabs/clients may tail the same agent at once  |

## Troubleshooting

- **400 Bad Request on connect** — the agent name failed validation. Names
  must match `^[A-Za-z0-9_-]{1,64}$`. Slashes, dots, and unicode are
  rejected.
- **"log not found" style notice then close** — the supervisor never
  created `<state_root>/agent_logs/<name>.log` within 30 s. Confirm the
  agent was actually spawned and that `SIMARD_STATE_ROOT` (or
  `$HOME/.simard`) matches the supervisor's view.
- **Terminal renders nothing after connect** — check the browser console
  for WebSocket errors and verify the dashboard cookie is still valid.
- **Colors and cursor jumps in the output** — this is expected. xterm.js
  fully interprets ANSI escape sequences emitted by the agent (colors,
  bold, cursor movement, screen clears), so output looks the same as it
  would in a real terminal.
- **Genuinely garbled bytes** — if the agent emits non-UTF-8 sequences
  that are *not* valid ANSI, those bytes are passed through to xterm
  unchanged and may render as replacement characters.

## Related

- [Run dashboard e2e tests](run-dashboard-e2e-tests.md) — includes the
  `@structural` Playwright spec `agent-terminal.spec.ts` that mocks the
  WebSocket.
- [Spawn engineers from OODA daemon](spawn-engineers-from-ooda-daemon.md)
  — once spawned, every subordinate writes to a tail-able log.
