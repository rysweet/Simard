---
title: How to troubleshoot the file-backed goal store
description: Operator playbook for diagnosing and fixing issues with the file-backed GoalStore (goal_store.json).
last_updated: 2026-06-02
owner: simard
doc_type: howto
related:
  - ../reference/file-backed-goal-store.md
  - ../concepts/file-backed-goal-store-simplification.md
  - ../reference/state-root-resolution.md
  - ./recover-goal-board.md
---

# How to troubleshoot the file-backed goal store

The file-backed `GoalStore` persists goal records to
`~/.simard/state/goal_store.json` (or `$SIMARD_STATE_ROOT/state/goal_store.json`).
This guide covers common issues and their resolution.

## Prerequisites

- SSH access to the host running Simard.
- Knowledge of `SIMARD_STATE_ROOT` (default: `~/.simard`).

---

## 1. Verify the goal store path

```bash
STATE_ROOT="${SIMARD_STATE_ROOT:-$HOME/.simard}"
ls -la "$STATE_ROOT/state/goal_store.json"
```

If the file does not exist, no goals have been written yet. The file is
created on the first `put()` call (e.g., the first meeting close that
writes goal records, or the first goal-curation run).

If you see the file at a different path (e.g.,
`$STATE_ROOT/goal_records.json`), this is the legacy location. Rename
it:

```bash
mkdir -p "$STATE_ROOT/state"
mv "$STATE_ROOT/goal_records.json" "$STATE_ROOT/state/goal_store.json"
```

---

## 2. Check the lock file

The lock file lives at `goal_store.json.lock` (sibling of the data
file):

```bash
ls -la "$STATE_ROOT/state/goal_store.json.lock"
```

This zero-byte file is normal and should not be deleted while any
Simard process is running. If you suspect a stale lock:

```bash
# Check if any simard process is holding the lock
fuser "$STATE_ROOT/state/goal_store.json.lock"
```

If no process holds it, the lock is stale. The next `flock()` call will
acquire it cleanly — no manual intervention needed, because `flock(2)`
advisory locks are released when the file descriptor is closed (including
on process crash).

---

## 3. Inspect the goal store contents

```bash
cat "$STATE_ROOT/state/goal_store.json" | python3 -m json.tool
```

Expected: a JSON array of `GoalRecord` objects:

```json
[
  {
    "id": "adopt-daily-backup-verification",
    "title": "Adopt a daily backup-verification job",
    "status": "Active",
    "priority": 1,
    "created_at": "2026-06-01T18:00:00Z",
    "updated_at": "2026-06-01T18:00:00Z"
  }
]
```

If the file contains invalid JSON, Simard will return
`GoalStoreParseFailed` on the next read. To fix:

```bash
# Back up the corrupt file
cp "$STATE_ROOT/state/goal_store.json" "$STATE_ROOT/state/goal_store.json.corrupt"
# Reset to empty
echo '[]' > "$STATE_ROOT/state/goal_store.json"
```

---

## 4. Meeting close did not write goals

Check that the meeting produced decisions. The meeting close pipeline
only writes goal records when `structured_decisions` is non-empty:

```bash
cat "$STATE_ROOT/meeting_handoffs/meeting_handoff.json" | python3 -c "
import json, sys
h = json.load(sys.stdin)
print(f'decisions: {len(h.get(\"decisions\", []))}')
print(f'action_items: {len(h.get(\"action_items\", []))}')
"
```

If `decisions: 0`, the meeting LLM did not extract any decisions. This
is not a goal store issue — it's a meeting content issue.

If decisions exist but the goal store was not updated, check for errors
in the meeting close output:

```bash
journalctl -u simard --since "10 min ago" | grep -E 'goal_store|GoalStore|goal.write'
```

---

## 5. Goal store and GoalBoard are out of sync

The file-backed `GoalStore` and the cognitive-memory `GoalBoard` are
two complementary stores that may diverge briefly. A goal record
written by meeting close appears in `goal_store.json` immediately but
is not reflected in the `GoalBoard` until the next OODA curate phase
processes the meeting handoff.

This is expected behavior, not a bug. To verify the GoalBoard:

```bash
simard goals inspect --json 2>/dev/null | python3 -c "
import json, sys
board = json.load(sys.stdin)
print(f'active goals: {len(board.get(\"active\", []))}')
print(f'backlog items: {len(board.get(\"backlog\", []))}')
"
```

---

## 6. Multiple processes writing simultaneously

The flock protocol serializes concurrent writes. If you see unexpected
data loss or duplication:

1. **Check that all writers use `FileBackedGoalStore`**, not raw file
   writes. Any process that writes directly to `goal_store.json`
   bypasses the locking protocol.

2. **Check for competing state roots.** Two processes with different
   `SIMARD_STATE_ROOT` values write to different files:
   ```bash
   # Daemon environment
   cat /proc/$(pgrep -f 'simard ooda')/environ | tr '\0' '\n' | grep SIMARD_STATE_ROOT
   # Meeting environment (if separate)
   echo $SIMARD_STATE_ROOT
   ```

---

## 7. Permissions errors

The goal store directory and file are created with restricted
permissions:

| Artifact | Expected mode |
|----------|---------------|
| `$STATE_ROOT/state/` | `0o700` |
| `goal_store.json` | `0o600` |
| `goal_store.json.lock` | `0o600` |

If another user or a misconfigured umask changed these:

```bash
chmod 700 "$STATE_ROOT/state"
chmod 600 "$STATE_ROOT/state/goal_store.json"
chmod 600 "$STATE_ROOT/state/goal_store.json.lock"
```

---

## See also

- [File-backed goal store reference](../reference/file-backed-goal-store.md)
  — API, locking protocol, and wiring.
- [State-root resolution](../reference/state-root-resolution.md) — path
  resolution ladder.
- [How to recover a corrupted goal board](./recover-goal-board.md) —
  for cognitive-memory GoalBoard issues (separate from the file store).
