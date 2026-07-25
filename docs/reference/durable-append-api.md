---
title: Durable line-append API
description: >
  The util::durable_append helper — a single, loss-free source of truth for
  O_APPEND JSONL writes shared by the cost ledger and the self-metrics stream.
  Guarantees that no concurrently-appended record is ever torn, glued, or
  dropped under massively-parallel host load.
last_updated: 2026-07-24
review_schedule: when a new append-only JSONL writer is added
owner: simard
doc_type: reference
related:
  - ./spawn-retry-api.md
  - ../testing/resource-isolated-test-suite.md
  - ./string-truncation-helpers.md
  - ../testing/hermetic-tests.md
---

# Durable line-append API

`src/util/durable_append.rs` exports one helper that every append-only JSONL
writer in Simard uses to record a line without ever tearing, gluing, or
dropping a concurrently-written record:

```rust
pub fn append_line(path: &Path, line: &str) -> std::io::Result<()>
```

It is the single source of truth for durable appends. The two production
writers that share it are the cost ledger
(`cost_tracking::append_line`) and the self-metrics stream
(`self_metrics::record_metric`). Both previously carried their own ad-hoc
append code; both now delegate here.

## Why this exists

Simard runs under massive host concurrency: 30+ engineer subprocesses plus the
self-deploy canary running the full lib test binary in parallel copies. Several
of those processes share a single `$HOME` and append to the **same** ledger and
metrics files at once. Two independent defects caused silent record loss under
that load (issue [#4577](https://github.com/rysweet/Simard/issues/4577)):

1. **Torn lines (the live drop bug).** Emitting the JSON body and its trailing
   `\n` as two separate `write()` syscalls (what `writeln!` does on an
   unbuffered file) lets two appenders splice bytes into one line (`{a}{b}\n\n`).
   A spliced line fails JSON parse, and the line-by-line readers silently
   `continue` past it — the record is gone. This is what dropped records in
   production: on the two-syscall `writeln!` path, parallel binary copies sharing
   one `$HOME` lost **630/1763** of 2000 metric writes, while a single-process
   control run dropped nothing (the loss requires cross-process contention, not
   in-process interleave). Collapsing each record to **one** `write_all` — a
   single sub-`PIPE_BUF` `O_APPEND` `write()`, atomic at EOF across processes —
   closes this window.
2. **Divergent, unaudited writers.** The cost ledger and `self_metrics` each
   carried their own append code, so a fix applied to one did not protect the
   other. This is a live divergence risk, not merely a style issue: the 630/1763
   loss was measured on the metrics writer's old two-syscall path; the ledger's
   fix (single `write_all` + a process-global mutex) never reached it. Even after
   the metrics writer was moved to a single `write_all` — which, being atomic
   under `PIPE_BUF`, already closes the cross-process drop window — it still
   lacks the mutex and `flush`, and any future writer would start from scratch.
   Consolidating both writers on one audited helper eliminates the divergence and
   is the durability guarantee going forward; for self-metrics specifically the
   consolidation is defense-in-depth hardening on top of the already-shipped
   single-`write_all` fix, not a patch for a currently-bleeding 630/1763 drop.

`append_line` closes both windows at one site so every writer inherits the fix.
Cross-process durability is provided by the single atomic `O_APPEND` write
(records stay under `PIPE_BUF`); the process-global mutex adds in-process
single-writer discipline as defense-in-depth (and covers the edge case of a
record large enough to force `write_all` into multiple syscalls). A per-call fd
is opened on every append — writers never share a handle — so `O_APPEND`
atomicity, not a shared file offset, is what orders concurrent writes.

## Contract

`append_line(path, line)`:

1. Creates `path`'s parent directory if it does not exist.
2. Builds the whole record — `line` plus exactly one trailing `\n` — in a single
   buffer. `line` must not already contain a trailing newline; the helper owns
   the record terminator.
3. Acquires a **process-global append mutex** so no two in-process threads write
   concurrently. The lock is poison-tolerant (`into_inner` on poison) so a panic
   in an unrelated writer cannot wedge the ledger.
4. Opens the file with `create(true).append(true)` (`O_CREAT | O_APPEND`) and
   writes the entire record with **one** `write_all`, then `flush`es. Records are
   well under `PIPE_BUF`, so a lone `O_APPEND` `write()` is atomic at EOF — this
   makes the write safe across *processes* too, not just threads.
5. Propagates every `io::Result`. No IO error is ever swallowed: a failed
   `create_dir_all`, `open`, `write_all`, or `flush` returns `Err` to the caller,
   which is responsible for surfacing it.

### Guarantees

- **No torn records.** Body and newline are one syscall; concurrent appenders
  cannot interleave within a record.
- **No dropped records.** In-process writers are serialized by the mutex;
  cross-process writers are serialized by `O_APPEND` atomicity. Every successful
  `append_line` call contributes exactly one parseable line.
- **No silent failure.** All IO errors propagate.

### Non-guarantees

- Ordering across processes is not specified — records land in `O_APPEND`
  arrival order, which is fine for JSONL streams read as an unordered set.
- The helper does not deduplicate; callers that must not double-write are
  responsible for their own idempotency.
- It is not a general logging framework — it appends one line per call.

## Usage

```rust
use std::path::Path;
use crate::util::durable_append::append_line;

let record = serde_json::to_string(&entry)?; // no trailing newline
append_line(Path::new(&ledger_path), &record)?;
```

Both shipped call sites follow this shape:

```rust
// cost_tracking.rs — the cost ledger
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    crate::util::durable_append::append_line(path, line)
}

// self_metrics/mod.rs — the metrics stream
let line = serde_json::to_string(&entry)?;
crate::util::durable_append::append_line(&path, &line)?;
```

## Testing

`durable_append` is proven with a hermetic concurrency test that owns a unique
`tempfile::TempDir`: N threads each append M lines to one file, then the file is
read back and must contain **exactly** N×M parseable lines with zero loss and
zero torn records. The suite-level regression tests
`cost_tracking::concurrent_appends_never_interleave_or_drop_entries` and the
self-metrics concurrency test exercise the same guarantee end-to-end. See
[Resource-isolated test suite](../testing/resource-isolated-test-suite.md) for
how these run under the parallel canary gate.
