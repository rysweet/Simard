---
title: Handoff lifecycle API
description: Reference for the meeting handoff write guard, batch processing, and reaping APIs introduced in issue #2268.
last_updated: 2026-06-11
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../operations/meeting-handoffs.md
  - ../howto/diagnose-handoff-accumulation.md
  - ./meeting-handoff-schema.md
  - ./disk-health-api.md
---

# Handoff lifecycle API

Issue #2268 introduced three coordinated changes to prevent unbounded
accumulation of meeting handoff JSON files. This reference documents the
public API surface for each change.

## Write guard

**Module:** `src/meeting_backend/persist/mod.rs`

**Function:** `write_handoff_with_explicit`

The write guard is a conditional check added to `write_handoff_with_explicit`
before the call to `write_meeting_handoff`. It prevents empty handoff files
from being written to the handoff queue directory.

### Behavior

After constructing the `MeetingHandoff` struct (with `facilitator_decisions`
and `facilitator_actions` populated from the meeting's extracted content —
these are the local variables that populate `handoff.decisions` /
`handoff.action_items` respectively):

1. If `facilitator_decisions.is_empty() && facilitator_actions.is_empty()`:
   - Log an info message indicating the skip.
   - Return `Ok(())` without writing to `default_handoff_dir()`.
   - The per-meeting bundle write (`write_handoff_bundle`) is **not**
     affected — it still writes to `~/.simard/meetings/<id>/`.
2. Otherwise, proceed with writing `meeting_handoff.json` to the handoff
   queue directory as before.

### Rationale

Empty handoff files (0 decisions, 0 action items) provide no value to the
OODA daemon or engineer loop — there are no goals or backlog items to
create. Writing them creates unnecessary work for the batch processor and
contributes to disk accumulation. The per-meeting bundle is still written
for record-keeping and transcript preservation.

### Impact on callers

No API change. The function signature and return type are unchanged.
Callers that previously relied on a handoff file always existing in the
queue directory after a meeting close must now handle the case where the
file is absent (which is the same as the "already processed" case they
should already handle).

---

## Batch processing

**Module:** `src/ooda_loop/curate.rs`

**Function:** `check_meeting_handoffs(board: &mut GoalBoard, handoff_dir: &Path, state_root: &Path) → SimardResult<u32>`

### Previous behavior (pre-#2268)

Processed exactly one handoff per OODA cycle. Called
`find_oldest_unprocessed_handoff` once, loaded the handoff, converted
decisions/action items to goals/backlog, marked processed, returned the
count.

### Current behavior

Processes up to `MAX_HANDOFFS_PER_CYCLE` (10) handoffs per call, in
FIFO order (oldest first). The outer loop structure:

```
for _ in 0..MAX_HANDOFFS_PER_CYCLE {
    path = find_oldest_unprocessed_handoff(handoff_dir)?;
    if path is None → break;

    handoff = load and parse path;

    if handoff.decisions.is_empty() && handoff.action_items.is_empty() {
        mark_handoff_processed(path);
        continue;  // fast-mark: no created increment
    }

    // Normal path: convert to goals/backlog, mark processed
    created += convert_and_mark(handoff, board, path);
}
return Ok(created);
```

### Constants

| Name | Value | Description |
|---|---|---|
| `MAX_HANDOFFS_PER_CYCLE` | `10` | Upper bound on handoffs processed per call |

### Fast-mark path

When a handoff has 0 decisions and 0 action items, it is marked as
processed immediately without creating any goals or backlog items. The
`created` return count is not incremented. This handles legacy empty
handoffs written before the write guard was added.

The fast-mark path:
- Sets `handoff.processed = true` and writes the updated JSON back to
  the same file (inline mutation, same pattern as `curate.rs` L225-235).
- Does not log an ingestion count for this handoff.
- Does not modify the `GoalBoard`.
- `continue`s to the next iteration of the batch loop.

### FIFO diagnostic log

The FIFO starvation diagnostic (logging when the oldest unprocessed file
differs from the newest file) runs on the first iteration of the batch
loop. Subsequent iterations do not repeat the diagnostic — the first
iteration's log is sufficient to surface any starvation pattern.

### Return value

Returns the total number of goals and backlog items created across all
handoffs in the batch. Fast-marked empty handoffs contribute 0 to this
count.

### Error handling

Errors from loading or parsing a single handoff log a warning and
**skip** the problematic file, continuing with the next handoff in the
batch. This prevents a single corrupt file from blocking all subsequent
handoffs indefinitely cycle after cycle. The return value reflects items
created from successfully processed handoffs only.

> **Note:** If a corrupt handoff file is logged repeatedly across cycles,
> remove or repair it manually. See
> [Diagnose handoff accumulation](../howto/diagnose-handoff-accumulation.md#troubleshooting)
> for the manual cleanup procedure.

---

## Reaping

**Module:** `src/ooda_loop/curate.rs`

**Function:** `reap_old_handoffs(handoff_dir: &Path) → SimardResult<u32>`

### Purpose

Deletes processed handoff JSON files older than 7 days from the handoff
directory. Prevents indefinite disk accumulation of files that have
already been ingested by the OODA daemon or engineer loop.

### Algorithm

```
count = 0
for entry in read_dir(handoff_dir) {
    filename = entry.file_name();
    if !filename.starts_with("handoff-") || !filename.ends_with(".json") {
        continue;
    }

    metadata = symlink_metadata(entry.path());  // no symlink following
    mtime = metadata.modified();
    if now - mtime < 7 days {
        continue;
    }

    content = read_to_string(entry.path());
    handoff = serde_json::from_str(content);
    if !handoff.processed {
        continue;  // never delete unprocessed handoffs
    }

    remove_file(entry.path());
    count += 1;
}
return Ok(count);
```

### Constants

| Name | Value | Description |
|---|---|---|
| `REAP_AGE_DAYS` | `7` | Minimum age (in days) before a processed file is eligible for deletion |

### Safety invariants

1. **Symlink safety:** Uses `symlink_metadata` instead of `metadata` to
   read the mtime. This prevents a symlink in the handoff directory from
   causing the function to stat (and potentially delete) a file outside
   the directory.

2. **Processed-only deletion:** A file is only deleted if `processed ==
   true` is confirmed by parsing the JSON content. Age alone is not
   sufficient — an unprocessed handoff that is 30 days old is preserved.

3. **Filename filter:** Only files matching `handoff-*.json` are
   considered. Other files in the directory (e.g., `meeting_handoff.json`
   for legacy single-file handoffs, `.tmp` files from atomic writes) are
   ignored.

4. **Per-file error tolerance:** If any individual file fails to read,
   parse, or delete, the error is logged at `warn` level and the function
   continues to the next file. A single corrupt or locked file does not
   prevent other files from being reaped. TOCTOU races (file deleted
   between `read_dir` and `remove_file`) produce `NotFound` errors that
   are handled the same way.

### Return value

Returns the count of files successfully deleted.

### Callsite

Called from `src/ooda_loop/cycle.rs` in the resource cleanup block
(lines ~152–159), after `handle_cleanup`:

```rust
// --- Resource cleanup: proactive disk/process management ---
{
    use crate::cmd_cleanup::handle_cleanup;
    eprintln!("[simard] OODA cycle: running resource cleanup");
    if let Err(e) = handle_cleanup() {
        eprintln!("[simard] OODA cycle: resource cleanup had errors: {e}");
    }

    // Reap old processed meeting handoff files (issue #2268).
    let handoff_dir = crate::meeting_facilitator::default_handoff_dir();
    match crate::ooda_loop::reap_old_handoffs(&handoff_dir) {
        Ok(n) if n > 0 => {
            eprintln!("[simard] OODA cycle: reaped {n} old processed handoff file(s)");
        }
        Err(e) => {
            eprintln!("[simard] OODA cycle: handoff reap failed: {e}");
        }
        _ => {}
    }
}
```

The `handoff_dir` is computed via `default_handoff_dir()` — a separate
call from the one at line ~105 — because the resource cleanup block is
in a different scope.

### Re-export

`reap_old_handoffs` is re-exported from `src/ooda_loop/mod.rs`:

```rust
pub use curate::{check_meeting_handoffs, promote_from_backlog, reap_old_handoffs, tombstone_goals};
```

---

## Test coverage

### Batch processing tests

| Test | Description |
|---|---|
| `batch_processes_multiple_handoffs` | Two content-bearing handoffs in the directory; both processed in one call. Verifies `created` count reflects both. |
| `fast_marks_empty_handoff` | One handoff with 0 decisions and 0 action items. Verifies it is marked processed without incrementing `created`. |
| `batch_cap_at_10` | 15 unprocessed handoffs in directory. Verifies exactly 10 are processed per call; 5 remain. |
| `check_meeting_handoffs_picks_oldest_unprocessed_first_fifo` (updated) | Two handoffs A (content-rich, older) and B (empty, newer). Both consumed in cycle 1: A creates goals, B is fast-marked. Cycle 2 finds nothing. Previously expected 1-per-cycle behavior. |

### Write guard tests

| Test | Description |
|---|---|
| `write_guard_skips_empty_handoff` | Calls `write_handoff_with_explicit` with empty decisions and empty action items. Verifies no file in `default_handoff_dir()`. Verifies per-meeting bundle still exists. |

### Reap tests

| Test | Description |
|---|---|
| `reap_deletes_old_processed_files` | Processed handoff file with mtime set to 8 days ago. Verifies it is deleted. Returns count 1. |
| `reap_preserves_recent_and_unprocessed` | Two files: one processed but 2 days old, one 10 days old but unprocessed. Verifies neither is deleted. Returns count 0. |
| `reap_handles_empty_directory` | Empty handoff directory. Verifies no error, returns count 0. |

---

## Integration with existing systems

### Disk health check

The `reap_old_handoffs` function complements the recipe-driven disk
health check (see [disk health API](./disk-health-api.md)). The disk
health recipe cleans large consumers (worktrees, cargo targets, backups);
`reap_old_handoffs` handles the smaller but unbounded handoff file
accumulation. Both run in the resource cleanup phase of each OODA cycle.

### Engineer-loop ingestion

The engineer loop (`src/engineer_loop/meeting_decisions.rs`) scans the
same handoff directory. The batch processor and write guard do not
change the engineer loop's behavior — it still processes one handoff at
a time on its own schedule. However, the write guard means the engineer
loop will also encounter fewer empty handoffs to skip.

### Meeting close pipeline

The write guard is transparent to the meeting close pipeline. The
`/close` command still calls `write_handoff` (which calls
`write_handoff_with_explicit`), and the per-meeting bundle is always
written. The only observable difference is the absence of a file in
`meeting_handoffs/` for meetings with no decisions or action items.

## Related

- [Meeting REPL & Handoff Ingestion](../operations/meeting-handoffs.md) — operations guide
- [Diagnose handoff accumulation](../howto/diagnose-handoff-accumulation.md) — troubleshooting guide
- [Meeting handoff schema](./meeting-handoff-schema.md) — JSON schema reference
- [Disk health API](./disk-health-api.md) — complementary disk cleanup
