---
title: "How to clean stale meeting handoffs"
description: Manually clean accumulated handoff files and verify the automatic reaper is working.
last_updated: 2026-06-11
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../operations/meeting-handoffs.md
  - ../reference/meeting-handoff-lifecycle.md
  - ../reference/meeting-handoff-schema.md
  - ../reference/state-root-resolution.md
---

# How to clean stale meeting handoffs

Use this guide when the `~/.simard/meeting_handoffs/` directory has
accumulated more files than the automatic reaper can drain in a
reasonable time, or when you need to reclaim disk space immediately.

## Prerequisites

- [ ] You know your state root (default `~/.simard`, overridable via
      `SIMARD_STATE_ROOT`)
- [ ] No OODA daemon or engineer loop is actively ingesting handoffs
      (stop with `systemctl stop simard-ooda` or wait for a cycle
      boundary)

## 1. Assess the situation

Count total handoff files, unprocessed files, and processed files:

```bash
HANDOFF_DIR="${SIMARD_STATE_ROOT:-$HOME/.simard}/meeting_handoffs"

echo "Total handoff files:"
find "$HANDOFF_DIR" -name 'handoff-*.json' | wc -l

echo "Unprocessed (pending ingestion):"
find "$HANDOFF_DIR" -name 'handoff-*.json' \
  -exec grep -l '"processed": false' {} \; | wc -l

echo "Processed (safe to delete if old enough):"
find "$HANDOFF_DIR" -name 'handoff-*.json' \
  -exec grep -l '"processed": true' {} \; | wc -l

echo "Older than 7 days:"
find "$HANDOFF_DIR" -name 'handoff-*.json' -mtime +7 | wc -l
```

## 2. Delete processed files older than 7 days (safe)

This matches exactly what the automatic reaper does — processed files
with mtime > 7 days. Safe to run at any time:

```bash
find "$HANDOFF_DIR" -name 'handoff-*.json' -mtime +7 \
  -exec grep -l '"processed": true' {} \; \
  -exec rm -v {} \;
```

This does **not** touch:
- Unprocessed files (the OODA daemon still needs to ingest them)
- Files newer than 7 days
- The legacy `meeting_handoff.json` singleton

## 3. Delete empty processed files (safe)

If you have legacy empty handoffs (0 decisions, 0 action items) that
were written before the write-gate filter was added, you can safely
delete them even if they haven't been processed yet — they contain no
actionable content:

```bash
find "$HANDOFF_DIR" -name 'handoff-*.json' -exec sh -c '
  decisions=$(jq ".decisions | length" "$1" 2>/dev/null)
  actions=$(jq ".action_items | length" "$1" 2>/dev/null)
  if [ "$decisions" = "0" ] && [ "$actions" = "0" ]; then
    echo "Removing empty handoff: $1"
    rm "$1"
  fi
' _ {} \;
```

## 4. Aggressive cleanup (all processed, any age)

If you need to reclaim disk space immediately and accept that recently
processed handoffs will no longer be available for inspection:

```bash
find "$HANDOFF_DIR" -name 'handoff-*.json' \
  -exec grep -l '"processed": true' {} \; \
  -exec rm -v {} \;
```

> **Warning**: This removes all processed handoffs regardless of age.
> You lose the ability to re-inspect recently ingested handoffs. The
> per-meeting bundle in `~/.simard/meetings/` is unaffected and
> provides a full archival record.

## 5. Verify the automatic reaper is running

After the OODA daemon completes a cycle, check the journal for reaper
activity:

```bash
journalctl -u simard-ooda --since "10 min ago" \
  | grep -E "reaped|reaper"
```

Expected output when files were reaped:

```
INFO resource cleanup: reaped 42 stale meeting handoff(s)
```

Expected output when nothing to reap:

```
INFO resource cleanup: reaped 0 stale meeting handoff(s)
```

If you see no reaper log lines at all, the daemon may be running an
older version without the reaper. Rebuild and restart.

## 6. Verify the write gate is working

Run a meeting that produces no decisions and no action items, then
verify no handoff file was created:

```bash
# Count before
BEFORE=$(find "$HANDOFF_DIR" -name 'handoff-*.json' | wc -l)

# Run a trivial meeting with no decisions
simard meeting repl "test empty meeting"
# Type: "Hello" then /close

# Count after
AFTER=$(find "$HANDOFF_DIR" -name 'handoff-*.json' | wc -l)

echo "Before: $BEFORE, After: $AFTER"
# Should be equal — no new handoff file created
```

Check the daemon log for the skip message:

```bash
journalctl -u simard-ooda --since "2 min ago" \
  | grep "Skipping empty meeting handoff"
```

## Troubleshooting

### Handoffs accumulating despite the write gate

The write gate only filters meetings with zero decisions AND zero
action items. If the LLM extracts even one decision or action item
from the conversation, a handoff file is written. This is by design —
the filter catches only truly empty meetings.

### Reaper not deleting old files

The reaper requires **both** `processed == true` and mtime > 7 days.
If old files have `processed == false`, they are unprocessed and the
reaper correctly leaves them alone. The batch ingestion (10 per cycle)
must process them first, then the reaper will pick them up 7 days
later.

### `meeting_handoff.json` never deleted

The legacy singleton `meeting_handoff.json` is intentionally excluded
from reaping. It is used by the engineer loop and `act-on-decisions`
CLI. To reset it manually:

```bash
rm "$HANDOFF_DIR/meeting_handoff.json"
```

## See also

- [Meeting REPL & Handoff Ingestion](../operations/meeting-handoffs.md)
- [Meeting handoff lifecycle](../reference/meeting-handoff-lifecycle.md)
  — write gate, batch ingestion, and reaper API reference
- [Meeting handoff schema](../reference/meeting-handoff-schema.md)
