---
title: Meeting handoff lifecycle
description: Write gate, batch ingestion, and automatic reaper for meeting handoff files — the full lifecycle from creation to deletion.
last_updated: 2026-06-11
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../operations/meeting-handoffs.md
  - ./meeting-handoff-schema.md
  - ./meeting-close-lifecycle.md
  - ../architecture/ooda-meeting-handoff-integration.md
  - ../howto/clean-stale-meeting-handoffs.md
issues: ["#2269"]
---

# Meeting handoff lifecycle

This page documents the full lifecycle of `handoff-*.json` files in
`~/.simard/meeting_handoffs/`: how they are created, consumed, and
cleaned up. Issue #2269 introduced the write gate, batch ingestion,
and automatic reaper to prevent handoff file accumulation.

---

## Background

Before issue #2269, the meeting close pipeline unconditionally wrote a
`handoff-{timestamp}.json` file for every meeting — including empty or
automated meetings with no decisions and no action items. The OODA
daemon processed only one handoff per cycle (FIFO), so consumption
could never keep up with bulk creation. Over time, this caused
thousands of inert files to accumulate.

---

## Lifecycle phases

```text
Meeting close           OODA cycle (start)        OODA cycle (end)
     │                       │                          │
     ▼                       ▼                          ▼
┌─────────────┐     ┌────────────────┐         ┌───────────────┐
│ Write gate  │     │ Batch ingest   │         │ Reaper        │
│ (persist/)  │     │ (curate.rs)    │         │ (cycle.rs)    │
│             │     │                │         │               │
│ decisions=0 │─no──│ Up to 10 files │         │ processed=T   │
│ actions=0?  │     │ per cycle      │         │ mtime > 7d?   │
│             │     │ oldest first   │         │               │
│ yes → skip  │     │ empty → mark   │         │ yes → delete  │
│ no → write  │     │ full → ingest  │         │ no → keep     │
└─────────────┘     └────────────────┘         └───────────────┘
```

### Phase 1: Write gate

**Location**: `write_handoff_with_explicit` in
`src/meeting_backend/persist/mod.rs`

After the LLM enrichment pipeline has extracted decisions and action
items from the meeting conversation, the close pipeline checks:

```rust
if facilitator_decisions.is_empty() && facilitator_actions.is_empty() {
    info!("Skipping empty meeting handoff (0 decisions, 0 action items)");
    return Ok(());
}
```

- **Empty meetings** (0 decisions AND 0 action items): No
  `handoff-*.json` is written to `meeting_handoffs/`. The per-meeting
  archival bundle in `~/.simard/meetings/<id>/` is still written for
  record-keeping.
- **Non-empty meetings**: A `handoff-{timestamp}.json` is written as
  before, with `processed: false`.

The check is applied **after** enrichment, not before — so the LLM
has already attempted to extract structured output. Only genuinely
empty meetings are filtered.

### Phase 2: Batch ingestion

**Location**: `check_meeting_handoffs` in `src/ooda_loop/curate.rs`

At the start of each OODA cycle, the daemon calls
`find_unprocessed_handoffs(handoff_dir, 10)` to retrieve up to 10
unprocessed handoff files, oldest-first. For each file in the batch:

1. Parse the JSON. On failure: `warn!`, skip to next.
2. Check `decisions.is_empty() && action_items.is_empty()`:
   - **Yes**: Mark processed immediately, skip ingestion. This handles
     legacy empty handoffs from before the write gate.
   - **No**: Convert decisions to goals and action items to backlog
     entries. Mark processed.
3. Accumulate the `created` count.

The function returns the total count across all handoffs in the batch.

**Error isolation**: A parse or I/O error on one file does not abort
the batch. The remaining files are still processed.

**FIFO ordering**: `find_unprocessed_handoffs` sorts by filename
ascending, which gives deterministic oldest-first ordering since
filenames include ISO timestamps.

### Phase 3: Automatic reaper

**Location**: `reap_processed_handoffs` in
`src/meeting_facilitator/handoff/persistence.rs`, called from the
resource cleanup block in `src/ooda_loop/cycle.rs`

At the end of each OODA cycle, the cleanup phase calls:

```rust
reap_processed_handoffs(&handoff_dir, Duration::from_secs(7 * 86400))
```

The reaper iterates `handoff-*.json` files and deletes each file only
when **both** guards pass:

| Guard | Check | Rationale |
|-------|-------|-----------|
| Content | `processed == true` in parsed JSON | Never delete an unconsumed handoff |
| Age | `std::fs::metadata().modified()` older than `max_age` | Give operators time to inspect recent handoffs |

The well-known `meeting_handoff.json` (legacy singleton filename) is
**excluded** from reaping — it is consumed by multiple paths (engineer
loop, `act-on-decisions` CLI) and must not be auto-deleted.

Each deletion is logged at `info!` level with the filename and age.

---

## API reference

### `find_unprocessed_handoffs`

```rust
pub fn find_unprocessed_handoffs(
    dir: &Path,
    limit: usize,
) -> Result<Vec<PathBuf>>
```

Lists `handoff-*.json` files in `dir` that have `processed == false`
(or are missing the `processed` field — treated as unprocessed).
Returns up to `limit` paths sorted by filename ascending (oldest
first). Also includes the legacy `meeting_handoff.json` if present and
unprocessed. Returns an empty `Vec` if the directory does not exist.

**Errors**: Returns `Err` only on directory-read failures. Individual
file parse errors are logged at `warn!` and the file is skipped (not
included in results).

### `reap_processed_handoffs`

```rust
pub fn reap_processed_handoffs(
    dir: &Path,
    max_age: Duration,
) -> Result<usize>
```

Deletes `handoff-*.json` files in `dir` where `processed == true` and
file mtime is older than `max_age`. Returns the count of files
deleted. The `MEETING_HANDOFF_FILENAME` (`meeting_handoff.json`) is
always skipped.

**Errors**: Returns `Err` only on directory-read failures. Individual
file errors (parse, mtime, delete) are logged at `warn!` and the file
is skipped. The function always attempts all eligible files.

### Re-exports

Both functions are re-exported through:

- `meeting_facilitator::handoff::find_unprocessed_handoffs`
- `meeting_facilitator::handoff::reap_processed_handoffs`
- `meeting_facilitator::find_unprocessed_handoffs`
- `meeting_facilitator::reap_processed_handoffs`

---

## Configuration

| Setting | Default | Override |
|---------|---------|---------|
| Handoff directory | `<state-root>/meeting_handoffs` | `SIMARD_HANDOFF_DIR` or `SIMARD_STATE_ROOT` |
| Batch size (ingestion) | 10 per cycle | Compile-time constant in `curate.rs` |
| Reaper max age | 7 days | Compile-time constant in `cycle.rs` |
| Reaper exclusion | `meeting_handoff.json` always excluded | Not configurable |

The batch size and reaper max age are intentionally not runtime-
configurable to avoid operator error. If you need to adjust them,
change the constants and rebuild.

---

## Observability

### Tracing events

| Level | Message | When |
|-------|---------|------|
| `INFO` | `Skipping empty meeting handoff (0 decisions, 0 action items)` | Write gate filters an empty meeting |
| `INFO` | `OODA start: ingested N goal/backlog item(s) from M meeting handoff(s)` | Batch ingestion completes |
| `WARN` | `Failed to process handoff <path>: <error>; continuing` | Individual handoff fails in batch |
| `INFO` | `Reaped processed handoff: <filename> (age: Nd)` | Reaper deletes a file |
| `WARN` | `Reaper skipping <filename>: <error>` | Reaper can't read/parse/stat a file |
| `INFO` | `resource cleanup: reaped N stale meeting handoff(s)` | Cleanup block summary |

### Diagnostic commands

Count unprocessed handoffs:

```bash
find ~/.simard/meeting_handoffs/ -name 'handoff-*.json' \
  -exec grep -l '"processed": false' {} \; | wc -l
```

Count files eligible for reaping (processed + older than 7 days):

```bash
find ~/.simard/meeting_handoffs/ -name 'handoff-*.json' \
  -mtime +7 -exec grep -l '"processed": true' {} \; | wc -l
```

---

## Migration from pre-#2269

If you have an existing handoff directory with thousands of
accumulated files:

1. The batch ingestion (10 per cycle) will gradually drain the
   unprocessed queue. At one cycle per ~60s, 3,000 files take ~5 hours
   to fully process.
2. Once processed, the reaper will delete files older than 7 days at
   the end of each cycle.
3. For immediate cleanup, see
   [How to clean stale meeting handoffs](../howto/clean-stale-meeting-handoffs.md).

No manual migration is required — the new code handles legacy files
transparently.

---

## See also

- [Meeting REPL & Handoff Ingestion](../operations/meeting-handoffs.md)
  — operator-facing operations guide
- [Meeting handoff schema](./meeting-handoff-schema.md) — JSON schema
  and version history
- [Meeting close lifecycle](./meeting-close-lifecycle.md) — timeout
  budgets and atomic writes
- [OODA meeting handoff integration](../architecture/ooda-meeting-handoff-integration.md)
  — architecture decision record
- [How to clean stale meeting handoffs](../howto/clean-stale-meeting-handoffs.md)
  — manual cleanup guide
