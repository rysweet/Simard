---
title: Cognitive-memory WAL crash-consistency and single-owner checkpointing (#4687)
description: "Reference for the cognitive-memory write-ahead-log durability fix — single-owner checkpointing (the engine's own auto-checkpoint disabled on the read-write path), fsync-durable clean shutdown, fsync-before-advance checkpoint ordering, and the explicit error-level + monotonic-counter observability that replaces the previously silent good-prefix tail truncation. Additive and non-breaking; store format stays v42."
last_updated: 2026-09-03
review_schedule: as-needed
owner: simard
doc_type: reference
status: active
related:
  - ./cognitive-memory-open-serialization.md
  - ../memory.md
  - ../howto/recover-from-a-corrupt-cognitive-wal.md
---

# Cognitive-memory WAL crash-consistency (#4687)

Simard's cognitive store is backed by the upstream
[`amplihack-memory-lib`](https://github.com/rysweet/amplihack-memory-lib) crate
(the `amplihack-memory` crate, `persistent` feature, LadybugDB/`lbug` engine),
reached through the `LibraryCognitiveMemory` adapter. The live store is opened at
`state_root/cognitive` (`LIVE_STORE_SUBDIR`, see
`src/cognitive_memory/library_adapter.rs`), with its write-ahead log at the
sibling `state_root/cognitive.wal`. `state_root` defaults to `$HOME/.simard`, so
in the default deployment these resolve to `~/.simard/cognitive` and
`~/.simard/cognitive.wal`; the paths below use the default form for readability.

This page is the reference for the crash-consistency fix landed for issue
**#4687**. **The on-disk store format is unchanged (v42): stores written by the
previous pin (`c266e15d`) open and replay without migration, and the fix rolls
back cleanly.**

## What was broken

Before this fix the daemon logged, on **every** start, a two-fault pattern
against `~/.simard/cognitive`:

```
lbug_store: WAL replay failed on open; attempting recovery ...
    error=Storage exception: Checksum verification failed, the WAL file is corrupted
lbug_store: recovered from corrupt WAL (good prefix)
...
lbug_store: auto-checkpoint failed ...
    IO exception: Error renaming file ~/.simard/cognitive.wal
    to ~/.simard/cognitive.wal.checkpoint. No such file or directory
```

Two coupled defects with a single root cause:

1. **Two checkpoint owners raced the WAL rename.** `system_config()` enabled the
   `lbug` engine's *own* background `auto_checkpoint` on the read-write path, so
   the engine **and** the wrapper both drove checkpoints that rename
   `cognitive.wal → cognitive.wal.checkpoint`. One owner consumed the `.wal`; the
   loser hit `No such file or directory`, the checkpoint never advanced, and the
   concurrent WAL manipulation corrupted the log checksum.
2. **Recovery silently truncated committed writes.** The resulting checksum
   failure on the next open was "recovered" by replaying only the good prefix and
   dropping the WAL tail — with no error and no metric, so the most-recent
   cognitive-memory writes vanished unnoticed.

## The durability contract (post-#4687)

The fix is **wrapper-level** in
`amplihack-memory/src/graph/lbug_store/mod.rs`. No engine change, no format bump,
no new error variant, no change to any existing public signature.

### 1. Single-owner checkpointing

`system_config()` now passes `.auto_checkpoint(RW_ENGINE_AUTO_CHECKPOINT)` with
`RW_ENGINE_AUTO_CHECKPOINT = false` on the read-write path, matching the
read-only peek config which already disabled it. The wrapper (`LbugGraphStore`)
is the **sole** checkpoint owner. Every checkpoint flows through `do_checkpoint`,
driven from exactly one of:

- `note_write_and_maybe_checkpoint` — the write-cadence trigger (every
  `AUTO_CHECKPOINT_WRITES` = 128 writes; see [Configuration](#configuration)),
- `Drop` — the durable-shutdown path,
- an explicit `checkpoint()` call (the OODA consolidation checkpoint, or an
  operator/maintenance checkpoint).

Because the engine's background checkpointer is off, **no two owners can ever
race the WAL rename**. The rename itself is performed inside the engine's
`CHECKPOINT`; with a single owner it has no competitor, so the
`No such file or directory` fault cannot recur. This eliminates both the rename
error and the concurrent-writer checksum corruption at the source.

### 2. fsync-before-advance checkpoint ordering

`do_checkpoint` runs a crash-safe sequence — the write counter (the "advance")
**never** resets before the checkpoint is durable:

```
CHECKPOINT                       # engine folds the WAL into the main DB + renames
  → fsync(cognitive)             # folded main-DB file data durable
  → fsync(parent dir ~/.simard)  # the WAL rename's directory entry durable
  → reset the write counter      # only now is the checkpoint "advanced"
```

A durability-barrier failure is **surfaced** (returned as
`MemoryError::Storage`, recorded in `last_checkpoint_error`), not swallowed, and
leaves the counter un-reset so the next write retries the checkpoint.

### 3. Clean-shutdown durability

`Drop` now runs the **full** `do_checkpoint` (CHECKPOINT **+** the fsync barrier
above) instead of a bare `CHECKPOINT`. Without the barrier a clean close could
return before the engine's WAL fold + rename reached disk, so the next open
found a dangling / torn WAL and failed checksum verification — the exact
"corrupt WAL on every start" incident. With the barrier, a cleanly-closed store
leaves a fully-folded, fsync-durable main DB and **no WAL tail to replay**, so a
subsequent strict open replays with zero loss.

Consequently a clean shutdown never reaches the recovery ladder at all: the
fast-path strict open in `open_with_recovery` succeeds and reports
`WalRecoveryOutcome::Clean`. "A clean shutdown must be fully replayable" is thus
a **structural** property here, not a runtime check against a provenance marker.

### 4. Recovery contract — no more silent tail-drop

`open_with_recovery` still tolerates a genuinely corrupt WAL (crash provenance)
and the #2550 salvage-into-fresh-DB path is **unchanged** — a poisoned WAL can
never permanently brick startup, and a store that still holds records is never
reset to empty. What changed is that the previously **silent** good-prefix
truncation is now **loud and metered**:

| Open outcome | Before #4687 | After #4687 |
|--------------|--------------|-------------|
| `Clean` (clean shutdown) | reached only intermittently; often a checksum failure due to the race | reached reliably; no recovery, no metric |
| `RecoveredPrefix` (corrupt tail, good prefix replayed + checkpointed) | silent `warn!` | `error!`-level, `cognitive_memory_wal_recovery_total` counter increments |
| `CheckpointOnly` (whole uncheckpointed tail unrecoverable, opened from last checkpoint) | silent `warn!` | `error!`-level, counter increments |
| `RebuiltAfterCorruption` / `SuspectedDataLoss` | unchanged (#95 / #107) | unchanged |

Committed writes that were folded by a prior checkpoint live in the main DB and
are always preserved; only the un-checkpointed WAL tail left by a crash is ever
quarantined, and that quarantine is now an explicit, alertable signal instead of
a silent drop.

## API — additive surface

The fix is additive. No existing `MemoryError` variant, no existing
`WalRecoveryOutcome` variant, no public `lbug_store` signature, and no store
format changed. Two additions:

- **`graph::lbug_store::RW_ENGINE_AUTO_CHECKPOINT: bool`** (`false`) — the
  single-owner invariant, pinned by a regression test so it cannot silently flip
  back to `true`.
- **`graph::wal_recovery_event_count() -> u64`** — a process-global monotonic
  count of corrupt-WAL recovery opens (re-exported from
  `graph::lbug_store::wal_metrics`). Exposed so a consumer can surface WAL-loss
  as a health metric.

## Observability

The crate emits **structured `tracing` + OTel only** — no `print!`/`println!`.
The corrupt-WAL recovery paths (`RecoveredPrefix`, `CheckpointOnly`) emit an
`error!`-level event carrying a
`monotonic_counter.cognitive_memory_wal_recovery_total` field. `tracing-opentelemetry`'s
`MetricsLayer` scrapes `monotonic_counter.*` fields into an OTel counter, so an
operator dashboard can alert on
`increase(cognitive_memory_wal_recovery_total[1h]) > 0`.

The same event is also counted in-process and read via
`amplihack_memory::graph::wal_recovery_event_count()` for tests and embedders.

On the Simard side, the adapter's existing
`cognitive_memory::metrics::cognitive_memory_silent_drop_count(kind, site)`
counter (issue #1975) remains the downstream signal that a WAL/cognitive-memory
site dropped data; on the **clean** reopen path it stays **zero** — the
regression tests assert this, because a clean replay loses nothing.

A healthy daemon shows a **flat** `cognitive_memory_wal_recovery_total` across
restarts. A rising count is an integrity signal — a store had to recover a
corrupt WAL, which after this fix should only ever follow a real crash, never a
clean restart (see the
[how-to](../howto/recover-from-a-corrupt-cognitive-wal.md)).

## Configuration

The fix ships with safe defaults and requires **no** configuration to be
correct. The only tunable is the wrapper checkpoint cadence.

| Setting | Default | Effect |
|---------|---------|--------|
| Wrapper checkpoint cadence (`AUTO_CHECKPOINT_WRITES`) | every **128** writes + on `Drop` | How often `note_write_and_maybe_checkpoint` triggers `do_checkpoint`. Because the engine's own `auto_checkpoint` is now off, this cadence **alone** bounds WAL growth. Lower it for very write-heavy stores to keep the WAL small; raise it to reduce fsync frequency. |

Under heavy write bursts, validate that WAL size stays bounded at your chosen
cadence — this is the one property to watch now that the engine's
auto-checkpoint is disabled (see [Operational notes](#operational-notes)).

## Compatibility and rollback

- **Format:** unchanged **v42**. Stores written by pin `c266e15d` open and
  replay without migration.
- **Rollback:** reverting the Simard pin to `c266e15d` re-links the previous
  wrapper. The store and WAL remain readable — no sidecar files or format
  changes were introduced.
- **Engine lockstep:** the fix is wrapper-only, so the `lbug` engine rev
  (`rysweet/ladybug-rust`, the `lbug = { git = …, rev = … }` key in
  `[dependencies]`) is **unchanged**. Exactly one
  `lbug` version stays linked (`cargo tree -p lbug` shows one line). The engine
  rev bumps **only** if a defect is proven to live in engine code, in which case
  the memory-lib engine pin and Simard's direct `lbug` pin move together.

## Simard pin bump

Simard consumes the fix by bumping the `amplihack-memory` git rev in
`Cargo.toml` from `c266e15d…` to the WAL crash-consistency fix commit
(`0031505b…`, branch `fix/issue-4687-wal-only-on-c266e15`, durably anchored by
tag `issue-4687-wal-crash-consistency-c266e15`), then refreshing `Cargo.lock`:

```bash
# Cargo.toml `amplihack-memory` key — rev → 0031505b911151bf47409694a6c45f8b778d91b9
cargo update -p amplihack-memory      # refreshes Cargo.lock
cargo tree -p lbug                     # must show exactly one lbug version
```

> **Why a WAL-only commit and not upstream `main`.** The identical fix also
> landed on `amplihack-memory-lib` `main` (squash-merge `b3033f7…`, PR #144).
> Simard does **not** pin to that merge commit yet, because `main` additionally
> carries the #137 "multi-writer coordination layer", which introduces a new
> engine-level single-writer store lock (`<store>.writer.lock`). Adopting it is a
> separate, out-of-scope migration that would break Simard's pre-existing
> concurrent-open cognitive-memory tests. Pinning to the WAL-only commit
> (the same diff rebased onto Simard's current base `c266e15`) keeps this change
> additive/non-breaking and preserves store format v42. Follow-up: adopt the
> coordination layer and repin to `main` in its own change.

> **Lockstep caveat.** The fix is wrapper-only, so the merge commit keeps
> amplihack-memory's own `lbug` pin at the current fork rev (`5a2c1078…`). If
> `cargo tree -p lbug` ever reports **two** versions after the bump, Simard's
> direct `lbug` dep (the `lbug = …` key in `[dependencies]`) must be moved to the **same** rev in the
> same PR to restore the one-engine invariant and avoid the `std::format` ABI
> SIGSEGV.

The pin is verified by `tests/issue_4687_amplihack_pin_bump.rs`, a std-only,
grep-shaped test (same shape as `tests/issue_2626_amplihack_pin_bump.rs`) that
reads the raw `Cargo.toml`/`Cargo.lock` and asserts the rev advanced off the
buggy commit and the `lbug` lockstep invariant holds. It is **RED** on the
un-bumped tree and **GREEN** once the pin + lockfile are updated.

## Operational notes

- **Alert:** a rising `cognitive_memory_wal_recovery_total` is an integrity
  signal — a store had to recover a corrupt WAL. After this fix it should only
  follow a genuine crash, never a clean restart.
- **WAL size:** with engine auto-checkpoint off, the wrapper cadence alone bounds
  WAL growth. Monitor `~/.simard/cognitive.wal` size on write-heavy hosts.
- **Structured logging:** all WAL paths route through `tracing` + OTel rather
  than stray `print!/println!/eprintln!`.

## Regression tests

Landed upstream in `graph/lbug_store/tests.rs`:

1. **Single-owner invariant** (`rw_engine_auto_checkpoint_is_disabled_single_owner`)
   — the engine's auto-checkpoint stays disabled on the RW path.
2. **Clean-shutdown replay**
   (`clean_shutdown_reopen_preserves_all_records_including_wal_tail`) — clean
   close → strict reopen → every record intact, including the WAL-tail batch.
3. **Clean open is `Clean`**
   (`clean_shutdown_open_with_recovery_is_clean_and_meters_no_loss`) — a clean
   reopen reports `WalRecoveryOutcome::Clean` and never enters recovery.
4. **Explicit-checkpoint durability**
   (`explicit_checkpoint_is_fsync_durable_without_drop`) — an explicit checkpoint
   is durable on its own (proven with `mem::forget`, no drop-checkpoint help).
5. **Corrupt-WAL recovery is metered**
   (`corrupt_wal_recovery_increments_the_wal_loss_metric`) — a corrupt tail
   increments `wal_recovery_event_count()` and the pre-corruption prefix survives
   a strict reopen (never silently dropped, never reset to empty).

Plus the preserved data-loss gates: the #2550 non-destructive corrupt-WAL
recovery suite, the #107 empty-read gate, and the v40→v41 lossless-open gate all
stay green (format v42, no regression).

Downstream in Simard:

6. **Clean-shutdown durability** — `tests/wal_clean_shutdown_durability_4687.rs`
   (clean reopen preserves all records and the WAL tail; zero silent drops).
7. **Pin verification** — `tests/issue_4687_amplihack_pin_bump.rs`.

## See also

- [How to recover from a corrupt cognitive WAL](../howto/recover-from-a-corrupt-cognitive-wal.md)
- [Cognitive-memory open serialization (lock-contention safety net)](./cognitive-memory-open-serialization.md)
- [Memory architecture](../memory.md)
- Upstream: `amplihack-memory/src/graph/lbug_store/mod.rs`
