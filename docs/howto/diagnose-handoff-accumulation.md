---
title: Diagnose and prevent meeting handoff accumulation
description: Operator guide for identifying, resolving, and preventing unbounded growth of meeting handoff JSON files in the handoff directory.
last_updated: 2026-06-11
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../operations/meeting-handoffs.md
  - ../reference/handoff-lifecycle-api.md
  - ../reference/meeting-handoff-schema.md
  - ../architecture/ooda-meeting-handoff-integration.md
  - ../concepts/automated-disk-health.md
  - ./configure-disk-health-check.md
---

# Diagnose and prevent meeting handoff accumulation

Prior to issue #2268, meeting handoff files could accumulate without
bound. Three root causes drove the accumulation: empty handoffs written
from meetings with no decisions or action items, single-handoff-per-cycle
ingestion creating a processing bottleneck, and no expiry for processed
handoff files. This issue shipped three coordinated fixes that
eliminate all three causes.

This guide shows how to detect accumulation, verify the fixes are
active, and manually clean up a backlog inherited from older builds.

## When to use this

Use this guide when:

- The `meeting_handoffs/` directory contains more than ~20 `.json` files
- The OODA daemon log shows repeated `ingested 0 goal/backlog item(s)`
  lines (empty handoffs being ingested one-per-cycle)
- Disk usage under `~/.simard/meeting_handoffs/` is growing and you
  want to understand why
- You upgraded from a pre-#2268 build and want to drain the backlog

## Detect accumulation

Count unprocessed and total handoff files:

```bash
# Total handoff files
ls ~/.simard/meeting_handoffs/handoff-*.json 2>/dev/null | wc -l

# Unprocessed handoffs (processed: false)
grep -l '"processed": false' ~/.simard/meeting_handoffs/handoff-*.json 2>/dev/null | wc -l

# Empty handoffs (0 decisions AND 0 action items)
for f in ~/.simard/meeting_handoffs/handoff-*.json; do
  d=$(jq '.decisions | length' "$f" 2>/dev/null)
  a=$(jq '.action_items | length' "$f" 2>/dev/null)
  if [ "$d" = "0" ] && [ "$a" = "0" ]; then echo "$f"; fi
done | wc -l
```

A healthy system has few unprocessed handoffs (0–2 between cycles) and
zero empty handoffs (the write guard prevents them). On a pre-#2268
build, you may see dozens or hundreds of accumulated files.

## Understand the three-layer fix (#2268)

### Layer 1: Write guard (source prevention)

`write_handoff_with_explicit` in `src/meeting_backend/persist/mod.rs`
now checks whether both `decisions` and `action_items` are empty before
writing the handoff file to `meeting_handoffs/`. If both are empty, the
function skips writing the handoff queue file entirely. The per-meeting
bundle (under `~/.simard/meetings/<id>/`) is still written regardless —
it serves as the durable meeting record.

This prevents empty handoffs from entering the processing queue at the
source. Meetings that produce no decisions and no action items — such as
casual chat sessions, interrupted meetings, or dashboard close events —
no longer leave files for the OODA daemon to process.

**What the log looks like:**

```
INFO Meeting handoff artifact written
```

vs. the new guard message:

```
INFO Skipping handoff queue write: 0 decisions and 0 action items (bundle still written)
```

### Layer 2: Batch processing with fast-mark (drain acceleration)

`check_meeting_handoffs` in `src/ooda_loop/curate.rs` now processes up
to 10 handoff files per OODA cycle instead of 1. For each handoff in
the batch:

1. Load the oldest unprocessed handoff (FIFO order).
2. If it has 0 decisions **and** 0 action items, mark it processed
   immediately and `continue` to the next — no goal/backlog creation,
   no `created` count increment. This is the "fast-mark" path.
3. Otherwise, convert decisions to goals and action items to backlog
   entries as before, then mark processed and increment `created`.
4. After 10 handoffs or when no unprocessed handoffs remain, exit the
   batch loop.

The batch cap of 10 prevents resource exhaustion if the directory
contains thousands of files. Under normal operation, the batch drains
the queue in a single cycle; a legacy backlog of 100 files clears in
10 cycles (~10 minutes at the default 60s cycle interval).

**What the log looks like:**

```
[simard] OODA start: ingested 3 goal/backlog item(s) from meeting handoff
```

The count reflects only content-bearing handoffs that produced goals or
backlog items. Fast-marked empty handoffs do not contribute to this
count.

### Layer 3: Reaping old processed files (disk cleanup)

`reap_old_handoffs` in `src/ooda_loop/curate.rs` deletes processed
handoff files whose filesystem mtime is older than 7 days. It runs
during the resource cleanup phase in `cycle.rs`, after `handle_cleanup`.

For each file in the handoff directory matching `handoff-*.json`:

1. Read filesystem mtime via `symlink_metadata` (not `metadata` — avoids
   following symlinks).
2. Skip if mtime is less than 7 days old.
3. Parse the JSON and verify `processed == true`. Unprocessed files are
   never deleted regardless of age.
4. Delete the file. If deletion fails (TOCTOU `NotFound`, permission
   error), log the error and continue to the next file.

**What the log looks like:**

```
[simard] OODA cycle: reaped 12 old processed handoff file(s)
```

Or when nothing to reap:

```
(no log line — silent when 0 files reaped)
```

## Verify the fixes are active

After upgrading to a build that includes #2268:

```bash
# 1. Check write guard: close a meeting with no decisions
simard meeting repl "test empty close"
# (immediately /close without making any decisions)
# Verify no new file appeared:
ls -lt ~/.simard/meeting_handoffs/ | head -3

# 2. Check batch processing: look for batch ingestion in log
journalctl -u simard-ooda --since '10 min ago' \
  | grep 'ingested.*meeting handoff'

# 3. Check reaping: look for reap log
journalctl -u simard-ooda --since '10 min ago' \
  | grep 'reaped.*handoff'
```

## Drain a legacy backlog

If you upgraded from a pre-#2268 build with accumulated handoff files,
the batch processor drains them automatically at 10 per cycle. For a
backlog of N files, expect ~⌈N/10⌉ cycles to clear.

To monitor drain progress:

```bash
watch -n 60 'echo "Unprocessed: $(grep -l "\"processed\": false" \
  ~/.simard/meeting_handoffs/handoff-*.json 2>/dev/null | wc -l)"'
```

To drain faster (manual), mark all empty handoffs as processed:

```bash
for f in ~/.simard/meeting_handoffs/handoff-*.json; do
  d=$(jq '.decisions | length' "$f" 2>/dev/null)
  a=$(jq '.action_items | length' "$f" 2>/dev/null)
  if [ "$d" = "0" ] && [ "$a" = "0" ]; then
    jq '.processed = true' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
    echo "Marked processed (empty): $f"
  fi
done
```

This is safe because empty handoffs would be fast-marked anyway; you
are just skipping the OODA cycle intermediary.

## Delete old processed files manually

If the automatic 7-day reaping has not yet run (e.g., the daemon was
recently upgraded), you can reap manually:

```bash
find ~/.simard/meeting_handoffs/ -name 'handoff-*.json' -mtime +7 \
  -exec sh -c '
    for f; do
      if jq -e ".processed == true" "$f" >/dev/null 2>&1; then
        rm -v "$f"
      fi
    done
  ' _ {} +
```

This mirrors the `reap_old_handoffs` logic: file must match
`handoff-*.json`, mtime > 7 days, and `processed == true` in the JSON.

## Configuration

The handoff lifecycle has no env-var configuration knobs. The constants
are compiled into the binary:

| Parameter | Value | Location |
|---|---|---|
| Batch size cap | 10 | `src/ooda_loop/curate.rs` (`MAX_HANDOFFS_PER_CYCLE`) |
| Reap age threshold | 7 days | `src/ooda_loop/curate.rs` (`REAP_AGE_DAYS`) |
| Write guard condition | decisions empty AND action_items empty | `src/meeting_backend/persist/mod.rs` |

If these need tuning, they require a code change and rebuild. The
values were chosen conservatively: 10-per-cycle handles realistic
backlogs without starving the rest of the cycle budget, and 7-day
retention provides a comfortable window for post-mortem investigation.

## Troubleshooting

### Handoff files still accumulating after upgrade

Verify you are running a build that includes the #2268 changes:

```bash
# Check that reap_old_handoffs exists in the binary
nm "$(which simard)" 2>/dev/null | grep reap_old_handoffs
# If the binary is stripped (release builds with strip = true), nm
# produces no output regardless. Use strings as a fallback:
strings "$(which simard)" 2>/dev/null | grep reap_old_handoffs
```

If neither command produces output on an unstripped binary, you are on
a pre-#2268 build. You can also check `simard --version` against the
release that includes #2268.

### Empty handoffs still appearing

The write guard is in `write_handoff_with_explicit`, not in
`write_meeting_handoff` (the low-level writer). If a code path calls
`write_meeting_handoff` directly, bypassing the guard, empty files
can still appear. The known callers that go through the guarded path
are:

- `write_handoff` (meeting close pipeline)
- `write_handoff_with_explicit` (direct callers with explicit params)

The unguarded low-level function `write_meeting_handoff` writes
whatever it is given. Check custom integrations if empty files appear.

### Reap not running

`reap_old_handoffs` is called from the resource cleanup block in
`cycle.rs`. If resource cleanup is disabled or erroring out before the
reap call, processed files will not be deleted. Check:

```bash
journalctl -u simard-ooda --since '10 min ago' \
  | grep 'resource cleanup'
```

You should see `running resource cleanup` each cycle. If you see
`resource cleanup had errors`, the cleanup block may be failing before
reaching the reap step.

## Related

- [Meeting REPL & Handoff Ingestion](../operations/meeting-handoffs.md) — full meeting handoff operations guide
- [Handoff lifecycle API](../reference/handoff-lifecycle-api.md) — API reference for batch processing and reaping
- [Meeting handoff schema](../reference/meeting-handoff-schema.md) — JSON schema reference
- [Configure disk health check](./configure-disk-health-check.md) — broader disk cleanup
- [Automated disk health (concept)](../concepts/automated-disk-health.md) — layered cleanup design
