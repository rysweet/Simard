# Subagent Tmux Tracking

Reference documentation for the **subagent tmux tracking** subsystem that wraps
every spawned engineer in a uniquely-named `tmux` session, persists a registry
of live and recently-ended sessions, exposes that registry via the operator
dashboard API, and renders attach links inline in the **Recent Actions** feed.

## Overview

When the OODA daemon dispatches a subordinate engineer (the line you see in
logs as `spawn_engineer dispatched: agent='engineer-XYZ', task='...', pid=N`),
the supervisor now:

1. Sanitizes the `agent_id` and computes a tmux session name of the form
   `simard-engineer-<sanitized_id>`.
2. Wraps the engineer's argv inside `tmux new-session -d -s <name> sh -c
   '<cmd> 2>&1 | tee -a <logfile>'` so existing `/ws/agent_log` log viewers
   continue to work.
3. Records a `SubagentSession` entry in `~/.simard/state/subagent_sessions.json`
   using an atomic temp-file + rename.
4. On every OODA cycle (after the Act phase), polls each registry entry with
   `tmux has-session -t <name>`. Missing sessions are stamped with
   `ended_at = now`. Entries whose `ended_at` is older than 24 hours are
   garbage-collected from the registry.

The dashboard surfaces this registry as live data in two places:

- A new **Subagent Sessions** card on the Terminal tab.
- An inline **Attach →** button rendered next to matching `agent='engineer-…'`
  entries in the Recent Actions feed (both overview and workboard renderers).

If `tmux` is not installed on the host, spawning falls back to the previous
direct `exec` path; no registry entry is written and a warning is logged. The
feature is therefore strictly additive — no engineer spawn ever fails because
of tracking.

## Storage

### Path resolution

```
state_root = $SIMARD_STATE_ROOT  // if set
           | $HOME/.simard       // fallback
registry   = state_root + "/state/subagent_sessions.json"
```

The parent directory is created on first write if missing.

### File format

```json
{
  "sessions": [
    {
      "agent_id":     "engineer-2026-04-19-17-41-53-abcd1234",
      "session_name": "simard-engineer-engineer-2026-04-19-17-41-53-abcd1234",
      "host":         "local",
      "pid":          412017,
      "created_at":   1745083313,
      "ended_at":     null,
      "goal_id":      "goal-7c3f"
    }
  ]
}
```

| Field          | Type            | Notes                                              |
|----------------|-----------------|----------------------------------------------------|
| `agent_id`     | string          | As emitted by the spawn site.                      |
| `session_name` | string          | `simard-engineer-<sanitize(agent_id)>`. Note the doubled `engineer-` prefix in the example above is **not** a typo: the supervisor prepends `simard-engineer-` to the full sanitized `agent_id`, which itself already begins with `engineer-`. |
| `host`         | string          | Always `"local"` in this release.                  |
| `pid`          | u32             | Engineer pane PID via `tmux list-panes -F '#{pane_pid}'`, or `child.id()` fallback. |
| `created_at`   | i64 (unix sec)  | Time of registry write.                            |
| `ended_at`     | i64 \| null     | Set by `poll_and_gc` when `tmux has-session` fails.|
| `goal_id`      | string          | Owning OODA goal.                                  |

### Atomic write

```
1. Serialize the in-memory Registry to bytes.
2. Write to "subagent_sessions.json.tmp.<pid>" in the same directory.
3. fsync the temp file.
4. std::fs::rename(temp, final).
```

After a successful write, **no** `*.tmp.<pid>` siblings remain. Corrupt or
missing files at load time produce an empty `Registry { sessions: vec![] }`
and a warn-level log line; loading never panics.

## Identifier sanitization

```
sanitize_id(raw):
    out = raw.chars().map(|c| if matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-') { c } else { '-' })
    if out.is_empty() { "engineer" } else { out }
```

Invariants enforced by the sanitizer:

- The result matches `^[A-Za-z0-9_\-]+$`.
- The result is non-empty.
- The session name is therefore always a legal tmux identifier.

## Public Rust API

Module: `simard::subagent_sessions`

```rust
pub struct SubagentSession {
    pub agent_id: String,
    pub session_name: String,
    pub host: String,
    pub pid: u32,
    pub created_at: i64,
    pub ended_at: Option<i64>,
    pub goal_id: String,
}

pub struct Registry {
    pub sessions: Vec<SubagentSession>,
}

pub trait SessionProbe {
    fn alive(&self, session_name: &str) -> bool;
}

pub struct TmuxProbe; // production probe; runs `tmux has-session -t <name>`

pub fn registry_path() -> std::path::PathBuf;
pub fn load() -> Registry;
pub fn save_atomic(reg: &Registry) -> std::io::Result<()>;
pub fn record_spawn(s: SubagentSession) -> std::io::Result<()>;
pub fn poll_and_gc<P: SessionProbe>(probe: &P) -> std::io::Result<()>;
pub fn sanitize_id(raw: &str) -> String;
```

Module: `simard::agent_supervisor::tmux`

```rust
/// Build the argv used to spawn the engineer wrapped in a detached tmux
/// session whose stdout/stderr is also tee'd to `log_path`.
pub fn build_tmux_wrapped_command(
    session_name: &str,
    inner_argv: &[String],
    log_path: &std::path::Path,
) -> Vec<String>;
```

Returned argv shape:

```
["tmux", "new-session", "-d", "-s", "<session_name>",
 "sh", "-c", "<shell-quoted inner_argv> 2>&1 | tee -a <log_path>"]
```

## OODA integration

`run_ooda_cycle` invokes `subagent_sessions::poll_and_gc(&TmuxProbe)` once per
cycle, immediately after the **Act** phase. Polling semantics:

- For each session with `ended_at = None`, run `tmux has-session -t
  <session_name>`. Exit code `0` ⇒ alive; non-zero ⇒ set `ended_at = now()`.
- For each session with `ended_at = Some(t)` where `now() - t > 86400`,
  drop the entry from the registry.
- A single `save_atomic` call persists the updated registry at the end of
  the polling pass.

Polling errors (e.g. tmux not installed) are logged at warn level and do not
abort the OODA cycle.

## HTTP API

### `GET /api/subagent-sessions`

Auth: requires the same operator token used by all other dashboard endpoints
(`require_auth` middleware).

Response body:

```json
{
  "live": [
    {
      "agent_id":     "engineer-...",
      "session_name": "simard-engineer-engineer-...",
      "host":         "local",
      "pid":          412017,
      "created_at":   1745083313,
      "ended_at":     null,
      "goal_id":      "goal-7c3f"
    }
  ],
  "recently_ended": [
    {
      "agent_id":     "engineer-...",
      "session_name": "simard-engineer-engineer-...",
      "host":         "local",
      "pid":          411902,
      "created_at":   1745079700,
      "ended_at":     1745082010,
      "goal_id":      "goal-7c3e"
    }
  ]
}
```

Sorting and partitioning rules:

- `live`           = sessions with `ended_at == null`.
- `recently_ended` = sessions with `ended_at != null` (≤24h by GC invariant).
- Both arrays are sorted by `created_at` descending.
- `live ∩ recently_ended = ∅`.

The handler reads the on-disk registry on each call (no in-memory caching at
the **server** layer) so dashboard polling and OODA writes do not need
explicit synchronization. The dashboard JavaScript maintains a separate
**client-side** `subagentSessionsCache` keyed by `agent_id` that is refreshed
between the 5-second polls; this cache is purely a render-time lookup so
`renderActionDetail` can build attach buttons without re-fetching. The two
caches are independent: the server is stateless, the client cache is best-
effort and rebuilt on every poll.

## Dashboard UI

### Subagent Sessions card (Terminal tab)

A new card with `id="subagent-sessions"` is injected into the existing
`#tab-terminal` panel. The card is populated by the dashboard JavaScript:

- On Terminal tab activation, an immediate `fetch('/api/subagent-sessions')`.
- A `setInterval` of **5 seconds** while the tab is active.
- Cleared when the tab is hidden.

Each row renders as:

```
<agent_id> · <goal_id> · <status> · [Attach]
```

Where:

- `<status>` is `live` for the `live` array and `ended <Δ ago>` for
  `recently_ended`.
- `[Attach]` is a `<button class="attach-btn" data-cmd="...">Attach →</button>`.
  Clicking copies the attach command to the clipboard and flashes "Copied!".

The data-cmd value is:

| Host       | data-cmd                                              |
|------------|-------------------------------------------------------|
| `local`    | `tmux attach -t simard-engineer-<id>`                 |
| `<remote>` | `ssh <remote> -t tmux attach -t simard-engineer-<id>` |

### Recent Actions inline Attach links

The dashboard previously rendered Recent Actions `outcome.detail` strings as
plain text in two locations (overview list near line ~3855, workboard list
near line ~4740). Both call sites now route through a single shared helper:

```js
function renderActionDetail(detail) {
  const re = /agent='(engineer-[A-Za-z0-9_\-]+)'/;
  const m  = re.exec(detail);
  if (!m) return escapeHtml(detail);

  const agentId = m[1];
  const session = subagentSessionsCache.get(agentId); // populated by /api/subagent-sessions
  if (!session) return escapeHtml(detail);

  const cmd = session.host === 'local'
    ? `tmux attach -t ${session.session_name}`
    : `ssh ${session.host} -t tmux attach -t ${session.session_name}`;

  return `${escapeHtml(detail)} <button class="attach-btn" data-cmd="${escapeHtml(cmd)}">Attach →</button>`;
}
```

If the regex matches but no registry entry exists (e.g. the session ended >24h
ago and was garbage-collected), the detail renders as plain text without a
button.

## Configuration

| Variable             | Default              | Effect                                         |
|----------------------|----------------------|------------------------------------------------|
| `SIMARD_STATE_ROOT`  | `$HOME/.simard`      | Overrides registry directory root.             |
| (none)               | —                    | Polling interval and 24h retention are fixed.  |

There are no new CLI flags. Tracking is on whenever `tmux` is on `PATH`.

## Operational notes

### Manually attaching from another shell

```bash
tmux attach -t simard-engineer-<sanitized_agent_id>
# Detach without killing: Ctrl-b d
```

Listing all currently-tracked sessions:

```bash
jq -r '.sessions[] | select(.ended_at == null) | .session_name' \
  ~/.simard/state/subagent_sessions.json
```

Cross-checking against tmux:

```bash
tmux ls 2>/dev/null | grep '^simard-engineer-'
```

### Cleaning up

Killing a tmux session manually is sufficient — the next OODA cycle will
mark `ended_at` and the entry will be GC'd within 24 hours:

```bash
tmux kill-session -t simard-engineer-<id>
```

To wipe the registry entirely (forces all live sessions to be re-discovered
as "missing" on the next poll):

```bash
rm ~/.simard/state/subagent_sessions.json
```

## Failure modes

| Condition                                | Behavior                                                  |
|------------------------------------------|-----------------------------------------------------------|
| `tmux` not installed                     | Direct exec fallback; no registry entry; `warn!` logged.  |
| `pane_pid` query fails                   | One 100ms retry, then fall back to `child.id()`.          |
| Registry file is corrupt JSON            | `load()` returns empty `Registry`; `warn!` logged.        |
| Atomic rename fails (e.g. ENOSPC)        | `record_spawn` returns `Err`; spawn caller logs warn and proceeds. |
| `SIMARD_STATE_ROOT` is read-only         | `record_spawn` returns `Err` (open/rename fails); spawn caller logs warn and proceeds without a registry entry. |
| `poll_and_gc` errors                     | OODA cycle continues; warn logged.                        |
| Two writers race                         | Single-process OODA invariant; last-writer-wins.          |
| Session ends mid-spawn (poll race)       | `record_spawn` writes `ended_at=null`; the very next `poll_and_gc` stamps `ended_at`. Benign — entry simply appears in `recently_ended` instead of `live`. |

## Tests

| Test file                                                  | Coverage                                                                                                  |
|------------------------------------------------------------|-----------------------------------------------------------------------------------------------------------|
| `src/subagent_sessions/tests.rs`                           | Atomic write leaves no `.tmp.*` siblings; round-trip; GC drops `>86400s`-ended; stub probe sets `ended_at`. |
| `src/agent_supervisor/tests_tmux.rs`                       | `build_tmux_wrapped_command` argv prefix and `2>&1 \| tee -a <log>` tail; shell-quoting of inner argv.    |
| `src/operator_commands_dashboard/tests_attach.rs`          | `INDEX_HTML.contains("renderActionDetail")`, `"simard-engineer-"`, and regex source `"agent='(engineer-"`. |

Run only the new and adjacent tests:

```bash
CARGO_TARGET_DIR=/tmp/simard-ws-1015 \
  cargo test --lib -- subagent_sessions agent_supervisor operator_commands_dashboard
```

## Invariants

```
INV1: ∀ s ∈ registry. s.session_name = "simard-engineer-" ++ sanitize(s.agent_id)
INV2: ∀ s ∈ registry. s.ended_at.is_none() ⟹ probe.alive(s.session_name) at last poll
INV3: ∀ s ∈ registry. s.ended_at.is_some() ⟹ now() − s.ended_at ≤ 86400
INV4: save_atomic Ok ⟹ registry_path exists ∧ ∄ sibling "subagent_sessions.json.tmp.<pid>"
INV5: sanitize(x) matches [A-Za-z0-9_-]+ ∧ non-empty (empty input → "engineer")
INV6: API: live ∩ recently_ended = ∅
INV7: Spawn-site registry write failure is non-fatal (warn-only).
INV8: tmux unavailable ⟹ direct-exec fallback (pre-change behavior preserved).
```

## See also

- [`docs/howto/view-agent-terminal-logs.md`](../howto/view-agent-terminal-logs.md)
- [`docs/howto/spawn-engineers-from-ooda-daemon.md`](../howto/spawn-engineers-from-ooda-daemon.md)
- [`docs/reference/agent-log-websocket.md`](agent-log-websocket.md)
