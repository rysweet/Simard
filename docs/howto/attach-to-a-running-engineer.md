# How to attach to a running engineer

When the OODA daemon spawns a subordinate engineer, the supervisor wraps the
engineer process inside a uniquely-named `tmux` session. You can attach to
that session from any shell on the same host to watch the engineer work in
real time, drive its TTY, or detach and let it continue.

This how-to covers the three common attach paths:

1. From the **operator dashboard** Terminal tab.
2. From the **Recent Actions** feed (inline Attach links).
3. From a **plain shell** using the registry file directly.

## Prerequisites

- A Simard host with `tmux` installed and on `PATH`.
- The OODA daemon running and dispatching engineers.
- For dashboard paths: the operator dashboard is reachable and you have its
  auth token.

## 1. From the dashboard Terminal tab

1. Open the operator dashboard.
2. Click the **Terminal** tab.
3. Find the **Subagent Sessions** card. Each live row shows
   `<agent_id> · <goal_id> · live · [Attach →]`.
4. Click **Attach →**. The attach command is copied to your clipboard.
5. In any terminal on the host, paste and run it:
   ```bash
   tmux attach -t simard-engineer-<sanitized_agent_id>
   ```
6. Detach without killing the engineer with `Ctrl-b d`.

Recently ended engineers (within the last 24 hours) appear in the same card
with status `ended <Δ ago>`. Their Attach button copies the same command, but
running it will fail with `no sessions` — the engineer process has exited.

## 2. From the Recent Actions feed

The Recent Actions list now renders an inline **Attach →** button next to any
detail line of the form:

```
spawn_engineer dispatched: agent='engineer-2026-04-19-...', task='...', pid=412017
```

…provided that the matching session is still in the registry (live or
recently-ended within 24h).

1. Click **Attach →** on the row. The attach command is copied to your
   clipboard.
2. Run it in a terminal on the host.

If the regex matches an engineer id but the session has been garbage-collected
(>24h after end), the line renders as plain text without a button — there is
nothing left to attach to.

## 3. From a plain shell

The registry file is always usable directly:

```bash
# All currently-live engineer sessions:
jq -r '.sessions[] | select(.ended_at == null) | "\(.agent_id)\t\(.session_name)"' \
  ~/.simard/state/subagent_sessions.json

# Attach by session name:
tmux attach -t simard-engineer-<sanitized_agent_id>
```

If you set `SIMARD_STATE_ROOT`, substitute that path for `~/.simard`.

You can also list tmux sessions directly:

```bash
tmux ls 2>/dev/null | grep '^simard-engineer-'
```

## Remote hosts

This release **always** records `host = "local"` for every spawned engineer.
There is no environment variable, CLI flag, or config setting that produces
a non-`local` host value, so the `ssh` form below is currently unreachable
in practice and exists only for forward compatibility with a future remote-
host feature.

The dashboard renderer already understands the remote form: when an entry
has a non-`local` host, the copied command would become:

```bash
ssh <host> -t tmux attach -t simard-engineer-<id>
```

## Tips

- **Read-only attach**: use `tmux attach -t <name> -r` to attach in
  read-only mode (no keystrokes forwarded).
- **Multi-watcher**: multiple humans can attach to the same session
  simultaneously; tmux mirrors the TTY.
- **Force-kill an engineer**: `tmux kill-session -t simard-engineer-<id>`.
  The next OODA cycle will mark `ended_at` and GC the entry within 24h.
- **Logs still work**: the engineer's stdout/stderr is also tee'd to its
  log file, so `/ws/agent_log` viewers continue to function unchanged.

## See also

- [`docs/reference/subagent-tmux-tracking.md`](../reference/subagent-tmux-tracking.md)
- [`docs/howto/view-agent-terminal-logs.md`](view-agent-terminal-logs.md)
- [`docs/howto/spawn-engineers-from-ooda-daemon.md`](spawn-engineers-from-ooda-daemon.md)
