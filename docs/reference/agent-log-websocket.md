# Agent log WebSocket API

The dashboard exposes a single WebSocket endpoint for streaming live agent
logs to in-browser terminal widgets (or any compatible WS client).

## Endpoint

```
GET /ws/agent_log/{agent_name}
```

Mounted inside the `require_auth` scope of the operator dashboard router
(`build_router` in `src/operator_commands_dashboard/routes.rs`).

## Path parameters

| Param        | Type   | Validation                       |
| ------------ | ------ | -------------------------------- |
| `agent_name` | string | Must match `^[A-Za-z0-9_-]{1,64}$` |

Names that fail validation cause the handler to return **HTTP 400 Bad
Request** before the WebSocket upgrade.

## Authentication

The endpoint is registered behind the dashboard's `require_auth` middleware.
Browsers transparently send the `simard_session` cookie on the WS upgrade
request. There is no token-in-URL or query-string auth — credentials must
travel via the cookie header. Unauthenticated upgrades are rejected by
`require_auth`; the exact response status (e.g. `401`, `403`, or a `302`
redirect) is determined by the dashboard's auth policy and is **not**
specified by this endpoint.

## Log file resolution

```
<state_root>/agent_logs/<agent_name>.log
```

Where `<state_root>` is resolved by a dashboard-local helper:

1. `$SIMARD_STATE_ROOT` if set.
2. Otherwise `$HOME/.simard`.

> **Operational note.** The dashboard maintains its **own** copy of this
> resolution logic; it does not import from the supervisor crate. Any
> change to state-root semantics must be applied to **both** the
> supervisor (`agent_supervisor`) and the dashboard
> (`operator_commands_dashboard`) to keep the producer and consumer
> agreed on the log file location. This duplication is intentional to
> avoid a cross-module dependency, but it is a known coupling risk.

## Lifecycle

The server-side tail loop is a small state machine:

| State           | Behavior                                                            |
| --------------- | ------------------------------------------------------------------- |
| **WaitForFile** | Poll for log existence at 200 ms; max ~30 s. On timeout, send a single notice text frame and close. |
| **Backfill**    | Send the last 200 lines via `read_tail`; record file position. If the file has fewer than 200 lines, all available lines are sent. |
| **Stream**      | Every 200 ms read appended bytes from current position. Emit each complete UTF-8 line as a text frame. Buffer trailing partial line until a newline arrives. Per-tick read cap: **1 MiB** (excess is delivered on subsequent ticks). |

### Truncation / rotation

If on a tick the file length is less than the recorded position, the loop
treats this as a rotation: position is reset to 0, **any buffered partial
line from the pre-rotation file is discarded**, a notice frame is emitted
(see Frame format), and streaming resumes from the start.

### Concurrent viewers

Multiple clients may tail the same agent simultaneously. Each connection
opens its own independent file handle and maintains its own position and
partial-line buffer; they do not interfere with one another.

### Client → server frames

Ignored. The handler is server-push-only. The only inbound frame that
affects the loop is `Close`, which terminates it.

## Frame format

- **Type**: `Text` (UTF-8).
- **Payload**: one log line, **without** a trailing newline.
- **Notice frames**: same `Text` type, used for out-of-band server
  messages (e.g. log-not-found, rotation). Notices never share a frame
  with data lines.

> **Notice payload wording is not contractual.** The exact human-readable
> strings used for notice frames (currently along the lines of
> `agent log not found` and `-- log rotated --`) may change without
> notice. Clients **must not** parse notice payloads to drive logic;
> they exist for human display only. If a client needs to react
> programmatically to log-not-found or rotation, that signal will be
> introduced as a separate, versioned mechanism.

## Status / error codes

| Condition                                  | Response                            |
| ------------------------------------------ | ----------------------------------- |
| Invalid `agent_name`                        | `400 Bad Request` (no upgrade)      |
| Missing/invalid auth cookie                | Per `require_auth` policy (status not fixed by this endpoint) |
| Upgrade succeeds, log absent ≥30 s         | One notice frame, then `1000` close |
| I/O error on read                          | Warn-logged server-side; `1011` close |
| Client disconnect                          | Loop exits cleanly                  |

## Producer side

The supervisor function `spawn_subordinate`
(`src/agent_supervisor/lifecycle.rs`) is responsible for ensuring the log
file exists:

1. Creates `<state_root>/agent_logs/` if missing.
2. Opens (append-mode) `<agent_name>.log`.
3. Sets `Stdio::from(file.try_clone()?)` for both child stdout and stderr.
4. **Fail-open**: if any of the above fail, the agent is still spawned
   with inherited stdio and a `warn!` is emitted. Log streaming will
   then time out for that agent.

## Example (raw client)

```bash
# Authenticated cookie required; replace SESSION with your value.
websocat \
  -H 'Cookie: simard_session=SESSION' \
  'wss://dashboard.example/ws/agent_log/engineer-7'
```

## Example (browser, condensed)

```html
<link rel="stylesheet"
      href="https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.css" />
<script src="https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.js"></script>

<div id="xterm-host" style="height: 480px;"></div>

<script>
  const term = new Terminal();
  term.open(document.getElementById('xterm-host'));

  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const name = encodeURIComponent('engineer-7');
  const ws = new WebSocket(`${proto}//${location.host}/ws/agent_log/${name}`);

  // Frames omit the trailing newline (see Frame format), so writeln is
  // correct here — it appends CRLF after each line. Using write() would
  // run lines together.
  ws.onmessage = (e) => term.writeln(e.data);
  ws.onclose   = ()  => term.writeln('\r\n[connection closed]');
  ws.onerror   = ()  => term.writeln('\r\n[connection error]');
</script>
```

## Security model

- **INV-7 (path traversal)**: the allow-list regex
  `^[A-Za-z0-9_-]{1,64}$` rejects `/`, `\`, `.`, NUL, and any unicode by
  construction. No path canonicalization is required.
- **DoS bounds**: 200-line backfill cap + 1 MiB per-tick read cap.
- **No XSS surface**: `term.write` treats input as terminal data, not
  HTML; status DOM updates use `textContent`.
- **AuthZ is flat**: any authenticated dashboard user may tail any agent
  log. This matches the existing dashboard authorization model.
- **Subresource Integrity (SRI) is intentionally deferred.** The browser
  example loads xterm pinned to the exact version `5.3.0` (no floating
  tag), but without an `integrity=` attribute. Adding SRI hashes is
  deferred to a future Content-Security-Policy hardening pass that will
  cover all dashboard-loaded third-party assets together.

## See also

- [How to view live agent logs](../howto/view-agent-terminal-logs.md)
- [Dashboard e2e tests](dashboard-e2e-tests.md)
