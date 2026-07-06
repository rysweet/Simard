# Agent Terminal — Agent Picker

The **Agent Terminal** card on the operator dashboard's **Workers → Terminal**
section includes an **agent picker**: a drop-down `<select>` that lists every
live, attachable agent session Simard is currently running. Choosing an agent
from the list attaches the terminal viewer to **that** agent's session, so the
operator can switch targets without typing a session name.

The picker is **populated from live data only** — it reads the same registry
that drives the Subagent Sessions card (`GET /api/subagent-sessions`, polled
every 5 s), so the list always reflects the agents that actually exist and are
attachable. There is no hard-coded or stale list.

The picker is **additive**: the existing **Agent name** text input, **Connect**,
and **Disconnect** controls are unchanged. The drop-down is a faster way to
reach the same attach mechanism.

---

## At a glance

| Capability            | Behaviour                                                                          |
| --------------------- | --------------------------------------------------------------------------------- |
| Location              | Workers tab → **Terminal** section → **Agent Terminal** card                       |
| Control               | `<select id="agent-terminal-select">` + label, in the existing control row        |
| Data source           | `subagentSessionsCache.live[]` from `GET /api/subagent-sessions` (client cache)    |
| Refresh               | Rides the existing **5 s** Subagent Sessions poll — no separate timer             |
| Options listed        | **Live** (attachable) sessions only; recently-ended sessions are excluded         |
| Option label          | `agent_id — goal_id — live` (the `goal_id` **label segment** is dropped when `goal_id` is `""`) |
| Option value          | `agent_id` (stable key, used to preserve selection across refreshes)              |
| Attach identifiers    | `data-host` + `data-session` on each option (never parsed from the label)         |
| Attach trigger        | `onchange` — a genuine user selection attaches immediately                         |
| Attach mechanism      | Existing `openTmuxAttach(host, session)` → `GET /ws/tmux_attach/{host}/{session}`  |
| Empty state           | A single disabled option **"no agents available"**; the `<select>` is disabled     |
| Single-agent case     | Drop-down pre-selects the one agent; attach still happens only on a user action    |

---

## Using the picker

1. Open the operator dashboard and click the **Workers** tab.
2. Scroll to the **Terminal** section and find the **Agent Terminal** card.
3. The card's control row shows, left to right:
   - An **Agent** drop-down (`#agent-terminal-select`) listing live agents.
   - The existing **Agent name** text input.
   - **Connect** / **Disconnect** buttons and a status line.
   - Below, the xterm.js terminal pane (`#xterm-host`).
4. Open the drop-down. Each entry is labelled with the agent's identity, goal,
   and status:

   ```
   engineer-2717-a1b2c3 — 2717 — live
   engineer-2698-9f8e7d — 2698 — live
   overseer-ooda-main — live
   ```

   (The middle `goal_id` segment is dropped for agents that carry no goal, as
   with `overseer-ooda-main` above. `goal_id` is always present in the JSON as
   an empty string `""` for such agents — the drop is a label-rendering choice,
   not a missing field.)

5. Select an agent. The terminal viewer:
   - tears down any previous attach WebSocket,
   - clears the xterm pane,
   - sets the status line to `attaching to <host>:<session>…`, then
     `attached: <host>:<session>` once the socket opens,
   - mirrors the target into the existing **Agent name** input as
     `<host>:<session>` (the picker and the text input share one attach path),
   - streams the agent's live tmux session into the pane.
6. To switch agents, pick a different entry — the previous WebSocket is closed
   automatically before the new one opens (the terminal is never multiplexed).

### Selection persistence

The drop-down is rebuilt on every 5 s refresh so it stays current. Rebuilding
**preserves the currently-selected `agent_id`** and does **not** re-trigger an
attach — only a genuine user selection attaches. If the agent you had selected
leaves the live set (its session ends), the drop-down resets to the disabled
empty option rather than attaching to a dead target; the active terminal
session is left as-is until you pick again.

### Empty state

When there are no live agents, the drop-down shows a single disabled option
reading **"no agents available"** and the `<select>` itself is disabled. This
is an explicit, non-interactive state — not a broken or empty control. The
text input and Connect/Disconnect controls remain usable.

### Single-agent case

When exactly one live agent exists, the drop-down pre-selects it. Consistent
with prior behaviour, the terminal does **not** auto-connect on page load —
attach happens only when the operator actively selects the entry (or uses the
existing Connect button).

---

## Data source

The picker is a pure reader of the client-side `subagentSessionsCache`, which
is filled by the existing Subagent Sessions poll:

```
GET /api/subagent-sessions   (every 5 s while the Workers tab is active)
        │
        ▼
subagentSessionsCache.live[]   ──▶  populateAgentSelect()  ──▶  <select> options
```

Only `live[]` entries (sessions whose tmux process is still running and
therefore attachable) are listed. `recently_ended[]` sessions are intentionally
excluded because their tmux session is gone and cannot be attached.

### `GET /api/subagent-sessions`

Returns the live and recently-ended subagent tmux sessions, sorted by
`created_at` descending. Always returns **200**.

**Response**

```json
{
  "live": [
    {
      "agent_id": "engineer-2717-a1b2c3",
      "session_name": "simard-engineer-2717-a1b2c3",
      "host": "vm-1",
      "pid": 48213,
      "created_at": 1751830000,
      "goal_id": "2717"
    }
  ],
  "recently_ended": [
    {
      "agent_id": "engineer-2698-9f8e7d",
      "session_name": "simard-engineer-2698-9f8e7d",
      "host": "vm-1",
      "pid": 47110,
      "created_at": 1751820000,
      "ended_at": 1751829000,
      "goal_id": "2698"
    }
  ]
}
```

**Field reference** (fields consumed by the picker)

| Field            | Type              | Used by picker for                                             |
| ---------------- | ----------------- | -------------------------------------------------------------- |
| `agent_id`       | `string`          | Option `value` + first label segment (selection key)          |
| `session_name`   | `string`          | Option `data-session` — the tmux target (`simard-engineer-…`) |
| `host`           | `string`          | Option `data-host` — the attach host                          |
| `goal_id`        | `string` (always present; `""` when the agent has no goal) | Middle label segment — dropped from the **label only** when `""`; the JSON field is never absent |
| `pid`            | `u32`             | Not shown in the option (shown in the Subagent Sessions card) |
| `created_at`     | `i64` (unix secs) | Registry sort order                                           |
| `ended_at`       | `i64 \| absent`   | Present only on `recently_ended[]`; such sessions are excluded |

### `GET /ws/tmux_attach/{host}/{session}` (WebSocket)

Selection routes through the **existing** attach WebSocket — no new endpoint,
schema, or transport is introduced. See the full contract in
[Azlin Tmux Sessions Panel → REST API](azlin-tmux-sessions.md#rest-api).

Both the client (`openTmuxAttach`) and the server validate identifiers against
`^[A-Za-z0-9_.-]{1,64}$`, and the server additionally whitelists `host` against
the configured hosts (`load_hosts()`):

| Layer  | Check                                          | Failure                          |
| ------ | ---------------------------------------------- | -------------------------------- |
| Client | `host` and `session` match the regex           | Status line: `invalid host or session name`; no socket opened |
| Server | `host`/`session` match `sanitize_tmux_ident`   | HTTP **400**                     |
| Server | `host` appears in `load_hosts()`               | HTTP **404**                     |

The picker passes each option's raw `data-host` / `data-session` values
straight to `openTmuxAttach`; it never derives the attach target from the
human-readable label.

---

## Configuration

The picker introduces **no new configuration**. It inherits everything from the
surrounding dashboard:

| Source                       | Used for                                             |
| ---------------------------- | ---------------------------------------------------- |
| `GET /api/subagent-sessions` | Live agent list (populated by the existing 5 s poll) |
| `~/.simard/hosts.json`       | Server-side host whitelist for the attach WebSocket  |
| Existing `azlin connect`     | Transport to the agent's host                        |
| Dashboard session cookie     | Auth — the attach WebSocket inherits it              |

The 5 s refresh interval and the attach transport are the existing ones; the
picker adds no timers, sockets, or credentials of its own.

---

## Examples

### Read the live agent list the picker sees

```bash
# :8080 is the dashboard's default URL (see ../dashboard.md); adjust if you run
# on a custom port via SIMARD_DASHBOARD_URL.
curl -s http://localhost:8080/api/subagent-sessions | jq '.live[] | {agent_id, goal_id, host, session_name}'
```

```json
{ "agent_id": "engineer-2717-a1b2c3", "goal_id": "2717", "host": "vm-1", "session_name": "simard-engineer-2717-a1b2c3" }
```

### Attach to the same session the picker would attach to

Selecting `engineer-2717-a1b2c3` (host `vm-1`, session
`simard-engineer-2717-a1b2c3`) is equivalent to:

```bash
wscat -b -c ws://localhost:8080/ws/tmux_attach/vm-1/simard-engineer-2717-a1b2c3
```

### Empty state

With no live agents, `GET /api/subagent-sessions` returns `{"live": [], …}`,
and the drop-down renders exactly one disabled option:

```html
<select id="agent-terminal-select" disabled>
  <option disabled selected>no agents available</option>
</select>
```

---

## Architecture notes

```
┌──────────────── Browser: Workers → Terminal ────────────────┐
│  Agent Terminal card                                        │
│  ┌───────────────────────────────────────────────────────┐ │
│  │ [ Agent ▾ engineer-2717 — 2717 — live ]  [name] Connect│ │
│  └───────────────────────────────────────────────────────┘ │
│        │ onchange (user selection only)                     │
│        ▼                                                     │
│  onAgentTerminalSelect()                                     │
│        │ reads option.dataset.host / .session               │
│        ▼                                                     │
│  openTmuxAttach(host, session)  ──WS──▶ /ws/tmux_attach/…    │
│                                                             │
│  fetchSubagentSessions() ─5s─▶ renderSubagentSessions()     │
│                                    │                        │
│                                    ▼                        │
│                            populateAgentSelect()            │
│                    (rebuilds options from live[],           │
│                     preserves selection, no attach)         │
└─────────────────────────────────────────────────────────────┘
```

**Why reuse `subagentSessionsCache.live[]`?**
It is the single shared live reader the Workers tab already maintains, and its
session objects already carry the `host` + `session_name` needed to attach. The
picker adds a projection of that data into `<option>` elements; it does not add
a second poll or a parallel source of truth, so the list can never drift from
the Subagent Sessions card.

**Why attach only on genuine `change`?**
The option list is rebuilt every 5 s. Attaching on a programmatic rebuild would
hijack the operator's terminal on every poll. The handler therefore fires only
on real user selections, and `populateAgentSelect()` restores the prior
selection without dispatching a `change` event.

**Why per-option `data-host` / `data-session` instead of a combined value?**
`openTmuxAttach` validates `host` and `session` **separately** against
`^[A-Za-z0-9_.-]{1,64}$`. Splitting them into two data attributes avoids any
delimiter parsing and keeps the attach target isolated from the display label
(defence against target injection / XSS via agent metadata).

---

## Testing

| Layer       | Test (in `src/operator_commands_dashboard/tests_attach.rs`)                                     |
| ----------- | ----------------------------------------------------------------------------------------------- |
| Unit (Rust) | `INDEX_HTML` contains `agent-terminal-select` (the drop-down is rendered)                        |
| Unit (Rust) | `INDEX_HTML` wires `onchange="onAgentTerminalSelect()"` (selection handler)                      |
| Unit (Rust) | `INDEX_HTML` defines `populateAgentSelect` reading `subagentSessionsCache` (live-data wiring)    |
| Unit (Rust) | `INDEX_HTML` references `data-session` and `openTmuxAttach` (attaches via existing mechanism)    |
| Unit (Rust) | `INDEX_HTML` contains the empty-state literal **`no agents available`**                          |

Run them with:

```bash
cargo test -p simard tests_attach
```

---

## Troubleshooting

| Symptom                                             | Likely cause / fix                                                        |
| --------------------------------------------------- | ------------------------------------------------------------------------- |
| Drop-down shows "no agents available"               | No live subagent sessions — check the Subagent Sessions card and that engineers are running |
| Selecting an agent does nothing                     | The chosen session may have just ended; the list refreshes every 5 s and resets to empty |
| Attach status shows `invalid host or session name`  | The option's `data-host`/`data-session` failed the client regex — usually a malformed registry entry |
| Attach WebSocket closes / 404                        | `host` is not in `~/.simard/hosts.json`; 400 means `host`/`session` failed the server regex |
| Selection jumps back to a previous entry            | The selected agent left `live[]`; the picker resets to the empty option by design |
| List does not update                                | You navigated away from the Workers tab — the 5 s poll is visibility-gated |

---

## Out of scope

The agent picker intentionally does **not**:

- Add, remove, or rename agent sessions — it only lists attachable ones.
- List `recently_ended[]` sessions (their tmux session is gone → not attachable).
- Multiplex multiple attach panes — selecting a new agent replaces the active
  attach.
- Introduce any new endpoint, WebSocket route, schema, or SSH transport.
- Auto-connect on page load, even when a single agent is present.

## See also

- [How to view live agent logs in the dashboard terminal widget](../howto/view-agent-terminal-logs.md)
- [Azlin Tmux Sessions Panel](azlin-tmux-sessions.md) — the sibling attach panel that shares `openTmuxAttach`
- [Subagent tmux tracking](../reference/subagent-tmux-tracking.md) — the registry behind `/api/subagent-sessions`
- [Dashboard overview](../dashboard.md) — the ten-tab dashboard, including **Workers → Terminal**
