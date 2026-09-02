---
title: How to diagnose and recover lost creative-ideas goals
description: Operator walkthrough for verifying that ideas accepted by the creative-ideas thread become durable, visible goals tagged source:creative-ideas, reading the fail-closed route/review errors that now surface a dropped write, correlating memory-ipc Broken-pipe log lines, and recovering after the #2896 fix.
last_updated: 2026-07-07
owner: simard
doc_type: howto
status: howto
related:
  - ../reference/creative-ideas-goal-routing-fail-closed.md
  - ../reference/creative-ideas-api.md
  - ../reference/goal-labels.md
  - ../reference/cognitive-memory-client-helpers.md
  - ./label-and-filter-goals.md
  - ./configure-creative-ideas-thread.md
  - ./troubleshoot-goal-store.md
---

# How to diagnose and recover lost creative-ideas goals

When the creative-ideas thread accepts an idea for implementation it routes the
idea to a **goal** tagged `source:creative-ideas`. Before issue
[#2896](https://github.com/rysweet/Simard/issues/2896) those goals could be
**silently lost**: the thread logged `N → goal`, telemetry reported
`0 review error(s)`, yet `simard goal list --tag source:creative-ideas`
returned zero. The fix makes every persistence seam on that path **fail-closed**
— a dropped write now surfaces as a route/review error, and an in-process write
lands in the same store `goal list` reads. This guide shows how to verify the
fix is working, how to read the new fail-closed signals, and how to recover.

For the full contract see
[Creative-ideas goal routing — fail-closed persistence](../reference/creative-ideas-goal-routing-fail-closed.md).

## Prerequisites

- [ ] You can reach the `simard` CLI (it resolves the same state root as the
  daemon: `$SIMARD_STATE_ROOT`, else `$HOME/.simard/state`).
- [ ] The OODA daemon is running with the creative-ideas thread enabled
  (default-ON; opt-out via `SIMARD_CREATIVE_IDEAS_ENABLED`).
- [ ] For dashboard steps, the operator dashboard is up and you have the
  dashboard key (`~/.simard/.dashkey`).

---

## 1. Confirm creative-ideas goals are now visible

This is the headline check — the query that returned `0` before the fix:

```bash
simard goal list --tag source:creative-ideas
```

After a run that accepts at least one idea, this returns **one or more** goals:

```text
active goals: 3 / 20 (filtered by tag)
ID                              PRIORITY  STATUS       ASSIGNED  DESCRIPTION                       LABELS
idea-live-tag-filter-dashboard  p2        not-started  -         Ship the live tag filter …        source:creative-ideas
idea-goal-board-health-probe    p2        not-started  -         Probe goal-board health …         source:creative-ideas
…
```

Cross-check the same result over the dashboard API:

```bash
DASHKEY="$(cat ~/.simard/.dashkey)"
curl -s -u "operator:$DASHKEY" http://localhost:8080/api/goals \
  | jq '[.active[], .backlog[]] | map(select((.labels // []) | index("source:creative-ideas"))) | length'
```

A non-zero count confirms the write path persists and is visible.

---

## 2. Trigger a run and watch it land

Force a run instead of waiting for the schedule. There is no
`simard creative-ideas` CLI subcommand; a run is triggered from the dashboard or
its HTTP API (the schedule interval is `SIMARD_CREATIVE_IDEAS_INTERVAL_SECS`,
default 86400 — each daemon restart pushes the next scheduled tick out again):

- **Dashboard:** click **"Run now"** (Creative Ideas → "Run now", the
  `ci-run-btn` control).
- **Scriptable** — `POST /api/creative-ideas/run` (inherits the dashboard auth):

  ```bash
  DASHKEY="$(cat ~/.simard/.dashkey)"
  curl -s -u "operator:$DASHKEY" -X POST \
    http://localhost:8080/api/creative-ideas/run -d '{}' | jq
  #   → {"ok": true, "report": {"persisted": …, "reviewed": …, "routed_goal": …}}
  #   → {"running": true}  if a run is already in flight (re-entrancy guard)
  ```

Read the telemetry line for the run:

```text
creative_ideas: 10 generated, 10 reviewed (6 → goal, 3 → issue), 0 review error(s)
```

Then re-run the query from step 1. The count should rise by roughly the `→ goal`
number (dedup-by-slug may collapse duplicates). The key invariant after #2896:

> **`N → goal` with `0 review error(s)` now means N goals actually persisted.**
> If a write is dropped, it is counted as a **review error**, not a phantom
> `→ goal`.

---

## 3. Read the new fail-closed signals

If a memory-IPC transport fault occurs during routing, it now surfaces instead
of being swallowed.

### Telemetry: dropped writes show up as review errors

```text
creative_ideas: 10 generated, 10 reviewed (4 → goal, 3 → issue), 2 review error(s)
```

`2 review error(s)` means two routes failed loudly. The routed-goal count
(`4`) reflects only goals that actually persisted. Before #2896 this same
scenario reported `6 → goal, 0 review error(s)` and lost two of them.

### The daemon returns an explicit error

A live-daemon transport failure on the write now yields a typed error rather
than a silent fall-through, e.g.:

```text
cognitive-memory writer socket <state_root>/memory.sock is present and a daemon
is listening, but the connection failed (…); refusing to fall back to a
divergent direct-open handle that would silently drop the write (issue #2896).
```

Seeing this error is the **correct** behaviour — the write was refused rather
than dropped. Re-run once the transport recovers (step 5).

### Broken-pipe log lines no longer imply data loss

You may still see:

```text
[simard] memory-ipc: connection error: bridge 'memory-ipc' transport error: write-len: Broken pipe (os error 32)
```

These reflect the underlying transport condition and are **expected to appear
occasionally**. After #2896 they no longer correlate with lost goals: either the
client reconnects and the write persists, or the route is recorded as a review
error. To confirm there is no loss, correlate the log timestamps with step 1 —
the tagged-goal count should still track the `→ goal` totals across runs.

---

## 4. Distinguish "empty board" from "read error"

Before the fix, a transport error on the **read** path returned an empty list,
so a healthy board could look empty. After #2896 the read path is fail-closed:

- `simard goal list` returning an **empty** board means the store is genuinely
  empty.
- A read **transport error** now exits **non-zero** with an error message,
  instead of silently printing an empty board.

If `simard goal list` errors out, the daemon or state root is unreachable — see
[Troubleshoot the goal store](./troubleshoot-goal-store.md). An empty-but-exit-0
board is a real empty board, not a swallowed error.

---

## 5. Recover after a transport fault

If a run reported `review error(s)` or you saw the fail-closed writer error:

1. **Confirm the daemon is healthy.**

   ```bash
   simard status
   ```

   A running daemon owns the in-process writer; routed goals take the tier-0
   in-process path and never touch the socket.

2. **Check the socket and state-root permissions** (hardened by #2896 to
   owner-only `0700` dir / `0600` socket):

   ```bash
   ls -ld  "${SIMARD_STATE_ROOT:-$HOME/.simard/state}"
   ls -l   "${SIMARD_STATE_ROOT:-$HOME/.simard/state}/memory.sock" 2>/dev/null
   ```

   Expect `drwx------` on the directory and `srw-------` on the socket. Group- or
   world-access is unexpected.

3. **Re-run.** Because routing is now fail-closed, simply re-running recovers the
   dropped ideas — accepted ideas that failed to persist were **not** marked
   done, so the next run re-routes them. Trigger it from the dashboard
   ("Run now") or `POST /api/creative-ideas/run` (step 2) and re-check the board:

   ```bash
   simard goal list --tag source:creative-ideas
   ```

4. If writer errors persist across runs with a healthy daemon, the underlying
   memory-IPC transport needs attention — capture the daemon logs and see
   [Cognitive-memory client helpers](../reference/cognitive-memory-client-helpers.md).

---

## 6. Verify end-to-end

```bash
# 1. Baseline count (rows are the idea-* goal IDs).
before=$(simard goal list --tag source:creative-ideas | grep -c '^idea-' || true)

# 2. Force a run: dashboard "Run now", or the HTTP API:
#    curl -s -u "operator:$(cat ~/.simard/.dashkey)" -X POST \
#      http://localhost:8080/api/creative-ideas/run -d '{}'

# 3. Count again — accepted ideas produced visible, tagged goals.
simard goal list --tag source:creative-ideas
#    → count >= before; new goals carry source:creative-ideas

# 4. A run with N → goal and 0 review error(s) means N goals persisted.
```

---

## Troubleshooting

### `simard goal list --tag source:creative-ideas` is still empty after a run

Check the run telemetry (step 2). If it shows `0 → goal`, no idea was
*accepted for implementation* this run — the reviewers routed everything to
issues or rejected it. Try another run, or lower the acceptance bar per
[Configure the creative-ideas thread](./configure-creative-ideas-thread.md). If
it shows `N → goal` with `N > 0` and the count is still zero, that is a
regression of #2896 — capture the daemon logs and file a bug referencing #2896.

### A run reports `review error(s)` every time

The write path is fail-closed and refusing to drop data — good — but the
underlying transport keeps failing. Work through step 5; a persistently broken
memory-IPC socket is the root cause, not the goal store.

### `simard goal list` exits non-zero

That is the fail-closed read path surfacing a transport error (it no longer
prints an empty board on error). The daemon or state root is unreachable — see
[Troubleshoot the goal store](./troubleshoot-goal-store.md).

---

## Related reading

- [Creative-ideas goal routing — fail-closed persistence](../reference/creative-ideas-goal-routing-fail-closed.md)
  — the full contract, API, and tests.
- [Creative Ideas subsystem API](../reference/creative-ideas-api.md) — the
  routing pipeline and the `run_now` / `POST /api/creative-ideas/run` entrypoint.
- [How to label, categorize, and filter goals](./label-and-filter-goals.md) —
  the `--tag` filter and `source:*` provenance.
- [Configure and operate the creative-ideas thread](./configure-creative-ideas-thread.md).
- [Troubleshoot the goal store](./troubleshoot-goal-store.md).
