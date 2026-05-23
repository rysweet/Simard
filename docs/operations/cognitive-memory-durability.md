# Cognitive Memory Durability

This page documents the three layers that together guarantee Simard's
cognitive memory survives graceful restarts, forced restarts, and
mid-write process death of the OODA daemon:

1. **Per-write fsync barrier** (issue #1973) — every mutating op is
   flushed to stable storage *before* it is observed as committed.
2. **SIGTERM-safe shutdown handler** (issue #1631) — graceful signals
   still drain the WAL.
3. **Periodic verified backups** (issue #1631) — bounded RPO under
   SIGKILL/OOM/power-loss.

> Quick reference for contributors: see the
> [Cognitive Memory Durability section in CONTRIBUTING.md](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#cognitive-memory-durability-per-write-barrier--sigterm--periodic-backups).

---

## Background

Simard's cognitive memory is stored at `~/.simard/cognitive_memory.ladybug`
using lbug `0.15.x`. As of issue #1973, this path is a **single file**.
Earlier prototype builds used a KuzuDB directory at the same path; on
first open with a legacy directory present, `NativeCognitiveMemory::open`
**renames the directory aside** to `cognitive_memory.ladybug.kuzu-backup`
and creates a fresh, empty lbug file. The legacy directory is preserved
on disk for manual inspection but its contents are **not** automatically
migrated into the new file format (the data models are incompatible). lbug
only flushes its WAL inside `Database::drop`. Process termination via
`SIGTERM` (the default signal `systemctl restart` sends) does not invoke
`Drop`, and even an in-process write that has returned `Ok(())` to the
caller is not necessarily on stable storage until the next checkpoint
plus `fsync(2)` have completed.

Two incidents motivated the current design:

- **2026-05-09** — active goal `improve-the-dashboard-via-playwright-driven-testing`
  was wiped during a routine `systemctl restart simard-ooda`. Root cause:
  `ctrlc` only caught `SIGINT`, so `SIGTERM` escalated to `SIGKILL` and
  the WAL was discarded. Fixed by the SIGTERM handler (#1631).
- **2026-05-18** — a write returned `Ok(())` from `CognitiveMemoryOps`
  but was absent after an OOM-kill milliseconds later. Root cause: the
  daemon relied on the periodic backup loop (≤ 5 min RPO) and on
  `Database::drop` for flushing, neither of which fires on SIGKILL.
  Fixed by the per-write fsync barrier (#1973).

The hardening described below ensures data loss of either class cannot
recur. Under SIGKILL / OOM / power loss, the **last committed write is
preserved**; the periodic backup remains a secondary, bounded-RPO
recovery point for catastrophic corruption.

---

## Architecture

```
+---------------------------+
|   OODA Daemon (PID N)     |
|                           |
|  +---------------------+  |
|  | OODA loop           |  |   shutdown: AtomicBool
|  | (checks flag /cycle)|<-+---+
|  +---------------------+  |   |
|                           |   |
|  +---------------------+  |   |
|  | ctrlc handler       |--+---+
|  | (SIGTERM/INT/HUP)   |  |
|  +---------------------+  |
|                           |
|  +---------------------+  |   per-cycle elapsed-time check
|  | backup tick         |  |   default interval = 300s
|  +---------------------+  |
|                           |
|  +---------------------+  |   #1973: every mutating op calls
|  | post_write_barrier  |  |     fsync(data file) →
|  | (per-write fsync)   |  |     fsync(parent dir) before return
|  +---------------------+  |
+---------------------------+
            |
            v
+---------------------------+
| ~/.simard/                |
|   cognitive_memory.ladybug              (lbug DB file as of #1973;
|                                          a legacy KuzuDB directory
|                                          at this path is renamed to
|                                          cognitive_memory.ladybug.kuzu-backup
|                                          and a fresh empty DB is opened)
|   cognitive_memory.ladybug.wal          (WAL sibling, may exist)
|   cognitive_memory.wal                  (alt WAL name, may exist)
|   cognitive_memory.ladybug.kuzu-backup  (legacy KuzuDB dir, if migrated;
|                                          contents NOT auto-imported)
|   backups/                |
|     cognitive_memory.ladybug.<ts>          (paired snapshot)
|     cognitive_memory.ladybug.wal.<ts>      (paired WAL backup)
|     cognitive_memory.wal.<ts>              (paired alt WAL backup)
+---------------------------+
```

---

## Per-Write fsync Barrier (issue #1973)

Every mutating operation on `NativeCognitiveMemory` is followed by a
**per-write fsync barrier** before control returns to the caller. The
barrier is the foundational durability guarantee on which the SIGTERM
handler and periodic backups are layered.

### Guarantee

For any successful return from a `CognitiveMemoryOps` mutating method
on a **file-backed** store, the mutation is durable across:

- `SIGKILL` of the daemon process,
- OOM-kill,
- power loss on a journaled filesystem (ext4 `data=ordered`, xfs, zfs,
  apfs, ntfs with FlushFileBuffers).

The barrier does **not** apply to the in-memory backend (see
[In-Memory Backend](#in-memory-backend) below).

### Mechanism

After a mutating Cypher write succeeds, the store calls a private
method `post_write_barrier(op: &'static str)` which performs two
steps in this exact order:

1. **`fsync(data file)`** — `File::open(&self.path)?.sync_all()` forces
   the kernel to flush the file's data and metadata to the underlying
   block device. lbug's internal WAL is co-resident in this same data
   file, so a single fsync of the file durably captures both committed
   pages and any uncheckpointed WAL frames; on reopen lbug replays
   those frames automatically.
2. **`fsync(parent dir)`** — `File::open(&self.parent_dir)?.sync_all()`
   forces the directory-entry update so the data file's new size /
   timestamp survive a crash on filesystems that journal metadata
   separately (notably ext4).

> **Why no `CHECKPOINT;`?** An earlier draft of this barrier issued
> `CognitiveMemoryOps::checkpoint()` as a first step. That was removed
> after CI exposed a lbug bug: issuing `CHECKPOINT;` between writes
> inside a multi-statement op (notably `consolidate_episodes`) caused
> subsequent reads of `e.content` on previously-written `Episode`
> rows to return raw page bytes instead of the stored string literal,
> breaking `bootstrap`, `goal_stewardship`, and `improvement_curation`
> integration tests. The crash-recovery integration tests
> (`tests/cognitive_memory_crash_durability.rs` and
> `tests/daemon_sigterm_durability.rs`) empirically confirm that the
> fsync pair above — **without** CHECKPOINT — preserves every
> acknowledged write across `SIGKILL`/restart cycles.
>
> `CHECKPOINT;` remains in use at backup time (see
> `NativeCognitiveMemory::backup`), which is the only context where
> the WAL must be folded into the data file before the file is copied.

The order is **non-negotiable** and called out by a `// SAFETY:`
comment in `post_write_barrier`. Reordering steps 1 and 2 — or
omitting either — re-introduces the lost-write window that motivated
issue #1973.

Both steps propagate failures as typed `SimardError` variants
(see [Error Mapping](#error-mapping) below); the barrier never
swallows an fsync result. A failure at either step short-circuits the
remaining step and returns `Err(...)` to the caller, surfacing the
durability failure rather than masking it.

### Methods that invoke the barrier

The barrier fires after each of the following mutating operations
implemented on `NativeCognitiveMemory`:

| Method | Barrier label |
|---|---|
| `store_fact` | `store_fact` |
| `store_episode` | `store_episode` |
| `update_episode_status` | `update_episode_status` |
| `link_episodes` | `link_episodes` |
| `delete_fact` | `delete_fact` |
| `tag_fact` | `tag_fact` |
| `record_observation` | `record_observation` |
| `record_goal_event` | `record_goal_event` |
| `consolidate_episodes` | `consolidate` |

`consolidate_episodes` calls the barrier **once after the entire
consolidation loop completes successfully**, not per-iteration. This
bounds write amplification to 1× per logical operation; per-iteration
barriers would impose a fsync per consolidated episode and were
explicitly rejected during design (see issue #1973, decision D5).

### Recovery-replay barrier

The same barrier semantics extend to the recovery path. When
`try_restore_from_backup` copies a verified backup into place, the
copy is routed through `atomic_copy_with_fsync` (see
[atomic_copy_with_fsync](#atomic_copy_with_fsync-readback-verification)
below), which performs the same `fsync(file) → fsync(parent dir)`
sequence and additionally re-reads the destination to verify the
copy.

`atomic_copy_with_fsync` emits its own action labels
(`fsync-data-file`, `fsync-parent-dir`, `verify-readback`) because it
is shared with the forward backup path. The recovery caller
(`try_restore_from_backup`) is responsible for **remapping** these
labels — when it propagates an `Err(SimardError::PersistentStoreIo)`
out, it rewrites `action` to `recovery-replay-fsync` (preserving the
underlying step name in the `reason` string) so log triage and the
runbook table below can distinguish forward-write failures from
recovery-path failures. The remap is a single match-and-rebuild in
the recovery function, not deep inside the copy helper.

### Latency tradeoff

Each barrier costs one `fsync` round-trip on the data file plus one
on the parent directory. Realistic measured latencies on common
storage stacks:

- **NVMe SSD + ext4 (`data=ordered`)** — typically **1–10 ms** per
  barrier. The lower bound is set by the device's flush latency;
  the upper bound includes jbd2 journal commit overhead on a busy
  filesystem.
- **SATA SSD** — typically **2–15 ms**.
- **Spinning HDD** — typically **5–25 ms**.

The OODA daemon's write rate (≤ a few hundred mutations per minute
under steady state) makes this overhead negligible. Workloads that
bulk-import episodes should batch through `consolidate_episodes`
(which fires a single barrier per call) rather than calling
`store_episode` in a tight loop.

This tradeoff is intentional and accepted. The previous behavior
(periodic backup ≤ 5 min RPO, plus `Database::drop` on graceful
shutdown) was insufficient under SIGKILL, which is the operational
reality the daemon must survive.

### Error Mapping

The barrier maps failures to typed `SimardError` variants. Each call
site uses a distinct `action` label so log scrapers and on-call
runbooks can identify the failing step at a glance. The `store`
field is the kebab-case identifier `"cognitive-memory"`, matching
the convention already established by `NativeCognitiveMemory::open`
(`src/cognitive_memory/mod.rs`):

| Step | Failure variant | `action` label |
|---|---|---|
| `fsync(data file)` open | `PersistentStoreIo { store: "cognitive-memory", action, path, reason }` | `fsync-data-file-open` |
| `fsync(data file)` | `PersistentStoreIo { store: "cognitive-memory", action, path, reason }` | `fsync-data-file` |
| `fsync(parent dir)` open | `PersistentStoreIo { store: "cognitive-memory", action, path, reason }` | `fsync-parent-dir-open` |
| `fsync(parent dir)` | `PersistentStoreIo { store: "cognitive-memory", action, path, reason }` | `fsync-parent-dir` |
| Recovery-replay copy | `PersistentStoreIo { store: "cognitive-memory", action, path, reason }` (remapped by `try_restore_from_backup`) | `recovery-replay-fsync` |
| Readback hash mismatch | `PersistentStoreIo { store: "cognitive-memory", action, path, reason }` | `verify-readback` |

The `op` label (e.g. `store_fact`, `consolidate`) is included in the
`reason` string of the `PersistentStoreIo` variant. The format is
pinned as:

```rust
reason: format!("op={op}: {io_err}")
```

(where `io_err` is the `Display` form of the underlying
`std::io::Error` or, for `verify-readback`, the truncated digest
mismatch summary). Callers and log scrapers may rely on the
`op=<name>:` prefix being present.

### `atomic_copy_with_fsync` readback verification

The backup/recovery helper `atomic_copy_with_fsync` was hardened in
issue #1973 to no longer trust the syscall return alone. After the
copy + fsync sequence, the destination is re-opened read-only and
its full contents are streamed through a SHA-256 digest, then
compared against the same digest computed from the source. A
mismatch returns:

```rust
SimardError::PersistentStoreIo {
    store: "cognitive-memory",
    action: "verify-readback",
    path: dst.clone(),
    reason: format!(
        "post-fsync hash mismatch: src={src_hex:.16} dst={dst_hex:.16}"
    ),
}
```

(Only the first 16 hex chars are surfaced in `reason`; the full
digests are emitted via `tracing::error!` for forensic capture.)

This defends against:

- **Page-cache aliasing** — the destination is read via fresh
  `File::open` + `BufReader`, bypassing the kernel page cache that
  could otherwise mask a torn write.
- **Silent disk-level corruption** of the just-written bytes (rare,
  but possible on misbehaving storage).
- **Logic bugs** in the copy path that produce a truncated or
  zero-length destination (which previously passed silently because
  `sync_all()` reports success on an empty file).

### In-Memory Backend

`NativeCognitiveMemory::in_memory()` constructs a store with
`durable_writes = false`. The first line of `post_write_barrier` is:

```rust
if !self.durable_writes { return Ok(()); }
```

so the in-memory backend skips the **fsync round-trip** entirely —
no `File::open`, no `sync_all` on the data file or the parent dir.

Note that the in-memory backend is not literally an in-RAM store:
`in_memory()` allocates a `tempfile::TempDir` and points lbug's
`Database::new` at a file inside it. lbug 0.15 still writes to that
tempdir on every mutation. The barrier opt-out therefore avoids the
fsync cost (which dominates per-write latency) but not the underlying
lbug write itself. The TempDir is reaped on drop via an
`Arc<TempDir>` held by the store, so unit tests do not leak
filesystem state.

A dedicated unit test (`in_memory_barrier_is_noop`) asserts that
`post_write_barrier` returns `Ok(())` without ever opening
`self.path` or `self.parent_dir` for fsync, even after a sequence of
writes.

### Operational signature

Successful barrier calls are not logged (the operation is on the
hot path). Barrier failures log at `error` level with the structured
fields `store="cognitive-memory"`, `action`, `op`, `path`, and the
underlying `io::Error`'s `kind()` and message. A representative log
line:

```
ERROR cognitive-memory: durability barrier failed
  store="cognitive-memory" action="fsync-parent-dir" op="store_fact"
  path="/home/simard/.simard/cognitive_memory.ladybug"
  io_kind=PermissionDenied
  reason="op=store_fact: Permission denied (os error 13)"
```

Treat any such line as a paging event: subsequent writes will
continue to fail until the underlying I/O fault is resolved, and the
caller of the mutating op already has the `Err(...)` return value.

---

## Shutdown Sequence (SIGTERM / SIGINT / SIGHUP)

The daemon registers a single
[`ctrlc::set_handler`](https://docs.rs/ctrlc) at startup, with the
`termination` feature so the same handler catches `SIGINT`, `SIGTERM`,
and `SIGHUP`. Before the `termination` feature was enabled, only
`SIGINT` was caught and `systemctl stop` (which sends `SIGTERM`)
escalated to `SIGKILL`, losing all WAL-resident writes.

The handler sets `shutdown.store(true, Ordering::SeqCst)` and returns.
At the top of the next OODA loop iteration, the daemon observes the
flag, breaks out of the loop, and invokes `shutdown_daemon(...)` (in
`src/operator_commands_ooda/daemon/mod.rs`) with `signal_driven=true`.

| Step | Operation | Failure mode (`signal_driven=true`) |
|---|---|---|
| 1 | `persist_board(&state.active_goals, &*bridges.memory)` — writes the active-goal board through the live cognitive-memory writer so it is captured by the next checkpoint. | Logged via `daemon_log`, next step still runs. |
| 2 | `shared_mem.checkpoint()` — collapses the WAL into the main DB file through the `CognitiveMemoryOps::checkpoint` trait method (delegates to `NativeCognitiveMemory::checkpoint`). | Logged, next step still runs. |
| 3 | `bridges.session.close()` — closes the LLM session if bound. | Logged, next step still runs. |
| 4 | `memory_ipc::clear_in_process_writer()` — drops the global `Weak`/`Arc` registration so the daemon-owned writer Arc becomes the sole strong reference. | Cannot fail. |
| 5 | (implicit) Bridges + the daemon's strong `Arc<NativeCognitiveMemory>` drop on function return. `Database::drop` runs `force_checkpoint_on_close` as a defense-in-depth backstop. | Inside `Drop` — failures are logged by lbug. |

The single function `shutdown_daemon` is shared by both the
signal-driven shutdown path and the normal-exit path. When called from
tests or normal exit (`signal_driven=false`), errors propagate as
`Result<(), Box<dyn Error>>` so assertions can fire; under signal
driven shutdown they are logged-and-continued because the process is
already dying.

> **Audit note**: shutdown logs go through `daemon_log` and contain the
> step name, not record contents — there is no `Debug`-printed memory
> data in the shutdown banner.

### What happens on SIGKILL

`SIGKILL` cannot be intercepted; the daemon dies immediately. As of
issue #1973:

- **Any write whose mutating call returned `Ok(())` before the kill
  is preserved** — the per-write fsync barrier already flushed it.
  This is the foundational guarantee; the integration test
  `cognitive_memory_crash_durability::sigkill_preserves_last_write`
  exercises it directly.
- A write that was **in flight** at the moment of the kill (the
  mutating call had not yet returned) may or may not be present.
  Callers must treat unacknowledged writes as undefined on crash.
- The most recent periodic backup (≤ `SIMARD_DB_BACKUP_INTERVAL_SECS`
  old, default ~5 minutes) is the secondary recovery point used only
  if the main data file is unreadable (corruption). Restore via the
  [Restoring from Backup](#restoring-from-backup) procedure.

---

## Periodic Backup Loop

Backups are not driven by their own task. At the **start of every OODA
cycle**, the daemon compares `Instant::elapsed()` since the last backup
against `SIMARD_DB_BACKUP_INTERVAL_SECS` (default `300`); if the
interval has passed, the in-line backup routine runs before any other
cycle work. The implementation lives at
`src/operator_commands_ooda/daemon/mod.rs` (search for
`periodic DB backup`).

### Per-Tick Operation

1. **Checkpoint** — `shared_mem.checkpoint()` flushes the WAL through
   the `CognitiveMemoryOps::checkpoint` trait method. A failure here is
   logged via `daemon_log` and the backup attempt still proceeds (the
   prior backup may already be missing recent writes).
2. **`NativeCognitiveMemory::create_verified_backup(&state_root)`** —
   copies `~/.simard/cognitive_memory.ladybug` (the lbug DB **file**;
   see [Background](#background)) to a sibling under
   `~/.simard/backups/` using `atomic_copy_with_fsync`:
   - `std::fs::copy(src, &dst.tmp)`
   - `File::open(&dst.tmp).sync_all()?` (errors **propagate** as of
     #1973 — previously swallowed via `let _ = ...`)
   - `std::fs::rename(dst.tmp, dst)`
   - `File::open(parent_dir).sync_all()?` (same — now propagated)
   - **SHA-256 readback verification** of `dst` against `src`
     (#1973); mismatch returns
     `PersistentStoreIo { action: "verify-readback", .. }`.
   - The destination filename is
     `cognitive_memory.ladybug.<unix_ts>` where `<unix_ts>` is the
     unix-second timestamp at the moment the backup tick began.
3. **Pair WAL siblings** — `wal_paths(&db_path)` returns the two
   possible WAL filenames lbug 0.15 may use
   (`cognitive_memory.ladybug.wal` and `cognitive_memory.wal`). Each
   that exists on disk is copied to `<wal_name>.<unix_ts>` with the
   **same** timestamp so a restore is unambiguous.
4. **Read-only verify** — the new backup is opened with
   `lbug::SystemConfig::default().read_only(true)` and run through
   `verify_db_health` before the function returns success.
5. **Prune** — `NativeCognitiveMemory::prune_old_backups(&state_root,
   db_backup_keep)` lists backups, sorts by timestamp descending, keeps
   the first `SIMARD_DB_BACKUP_KEEP` (default `24`), and removes the
   rest. For each pruned timestamp, the matching WAL backups (both
   `cognitive_memory.ladybug.wal.<ts>` and `cognitive_memory.wal.<ts>`)
   are removed alongside the directory.
6. **Consecutive-failure tracking** — an `AtomicU32` counter
   (`backup_consecutive_failures`) increments on every failure and
   resets on every success. The first two failures log at warn level;
   the third and subsequent log `ERROR: DB backup failed N consecutive
   times — last error at <backup-dir>: <e>`.

> **Note on the consecutive-failure counter**: the daemon does **not**
> halt backups after N failures; it continues to attempt the next tick
> (so a transient I/O error does not silently disable durability).
> Operators should treat the `ERROR: DB backup failed 3 consecutive
> times` log line as a paging event.

### Naming

```
cognitive_memory.ladybug.<unix_ts>           # main DB file backup
cognitive_memory.ladybug.wal.<unix_ts>       # WAL sibling, if present
cognitive_memory.wal.<unix_ts>               # alt WAL sibling, if present
```

`<unix_ts>` is the seconds-since-epoch at the moment the backup tick
began. All files in a pair share the same timestamp.

### Startup recovery

`NativeCognitiveMemory::open_or_recover` (in
`src/cognitive_memory/backup.rs`) attempts to open the main DB; if it
fails with corruption symptoms the routine falls back to the most
recent verified backup. Orphaned `.tmp` files left over from a crashed
backup are cleaned up by `atomic_copy_with_fsync` on the next run via
its best-effort `remove_file(&tmp)` at the start.

---

## Configuration

| Setting | Default | How to override | Notes |
|---|---|---|---|
| Backup interval (seconds) | `300` (5 min) | `SIMARD_DB_BACKUP_INTERVAL_SECS=N` env var | Read once at daemon start; per-cycle elapsed-time check |
| Retention count | `24` paired snapshots | `SIMARD_DB_BACKUP_KEEP=N` env var | `0` disables pruning |
| Backup directory | `<state_root>/backups/` | Derived from state root (compile-time) | Created on first backup |
| Dashboard port | `8080` | `SIMARD_DASHBOARD_PORT=N` env or `--dashboard-port=N` | Default in `src/operator_commands_ooda/daemon/config.rs` |

### Sample systemd unit excerpt

```ini
[Service]
Environment=SIMARD_DB_BACKUP_INTERVAL_SECS=300
Environment=SIMARD_DB_BACKUP_KEEP=24
KillSignal=SIGTERM
TimeoutStopSec=30
ExecStart=/usr/local/bin/simard ooda daemon
Restart=on-failure
```

`KillSignal=SIGTERM` (the systemd default) plus
`TimeoutStopSec=30` gives the shutdown handler ample time to run; the
sequence completes in well under a second on typical workloads. The
[reference unit file](https://github.com/rysweet/Simard/blob/main/scripts/simard-ooda.service) is the source
of truth for production deployments.

---

## Restoring from Backup

As of issue #1973, `cognitive_memory.ladybug` is a **single file**, and
every backup created by `create_verified_backup` is therefore also a
single file (the dir-renamed-aside path in `open()` happens *before*
any backup is taken, so no backup ever contains a legacy KuzuDB
directory). Restoration uses plain `cp`.

```bash
# 1. Stop the daemon
sudo systemctl stop simard-ooda

# 2. Identify the most recent paired backup
ls -lt ~/.simard/backups/cognitive_memory.ladybug.* | head

# 3. Restore the DB file and any paired WAL files
TS=1762800000   # the timestamp suffix from above
rm -f ~/.simard/cognitive_memory.ladybug
cp ~/.simard/backups/cognitive_memory.ladybug.${TS} \
   ~/.simard/cognitive_memory.ladybug

# WAL siblings — either, both, or neither may exist for a given TS.
for w in cognitive_memory.ladybug.wal cognitive_memory.wal; do
  src=~/.simard/backups/${w}.${TS}
  if [ -e "$src" ]; then
    cp "$src" ~/.simard/${w}
  fi
done

# 4. Restart and check the journal
sudo systemctl start simard-ooda
journalctl -u simard-ooda -n 50
```

If the restored backup itself appears corrupt, fall back to the
next-most-recent timestamp. Note that `create_verified_backup` already
opens each backup read-only and runs `verify_db_health` before
declaring it a success, so corrupt backups should be extremely rare.

---

## Public API

> All three items below already exist in the codebase. This section
> documents them; it does not propose them.

### `CognitiveMemoryOps::checkpoint`

```rust
// src/cognitive_memory/ops.rs
pub trait CognitiveMemoryOps {
    /// Force a WAL checkpoint, collapsing the WAL into the main DB
    /// directory. Default impl delegates to
    /// [`NativeCognitiveMemory::checkpoint`] (issue #1631).
    fn checkpoint(&self) -> SimardResult<()> { /* ... */ }
}
```

### `NativeCognitiveMemory::checkpoint`

```rust
// src/cognitive_memory/mod.rs
impl NativeCognitiveMemory {
    /// Force a WAL checkpoint, collapsing the WAL into the main DB.
    ///
    /// Idempotent and safe under concurrent reads. The periodic backup
    /// loop logs failures at warn level and continues; the shutdown
    /// path also logs and continues under `signal_driven=true`.
    pub fn checkpoint(&self) -> SimardResult<()>;
}
```

### `memory_ipc::clear_in_process_writer`

```rust
// src/memory_ipc/launcher.rs (re-exported as
// memory_ipc::clear_in_process_writer from src/memory_ipc/mod.rs)
pub fn clear_in_process_writer();
```

Drops the in-process cognitive-memory writer registration. Audited
callers:

- `operator_commands_ooda::daemon::shutdown_daemon` (step 4 above)
- the in-process tests under `src/memory_ipc/tests_launcher.rs`

> The function lives in `memory_ipc`, **not** `cognitive_memory`. The
> daemon shutdown handler imports it as `memory_ipc::clear_in_process_writer`.

### `operator_commands_ooda::daemon::shutdown_daemon`

```rust
// src/operator_commands_ooda/daemon/mod.rs
fn shutdown_daemon(
    state_root: &std::path::Path,
    shared_mem: &Arc<dyn CognitiveMemoryOps>,
    state: &mut OodaState,
    bridges: &mut OodaBridges,
    signal_driven: bool,
) -> Result<(), Box<dyn std::error::Error>>;
```

Runs the graceful-shutdown sequence (steps 1–5 above). Called from both
the normal-exit path (`signal_driven=false`, errors propagate) and the
shutdown-flag observation path (`signal_driven=true`, errors are
logged and the next step still runs).

### Private: `NativeCognitiveMemory::post_write_barrier` (issue #1973)

```rust
// src/cognitive_memory/mod.rs
impl NativeCognitiveMemory {
    /// Flush the most recent mutation to stable storage.
    ///
    /// Called by every mutating method in `CognitiveMemoryOps`
    /// implementations. The `op` label is a compile-time literal used
    /// only for error context and log correlation; it is **not** a
    /// caller-supplied string.
    ///
    /// # Order (SAFETY)
    /// 1. `self.checkpoint()`        — collapse WAL into data file
    /// 2. `fsync(self.path)`         — flush data file to device
    /// 3. `fsync(self.parent_dir)`   — flush directory entry
    ///
    /// `self.path` is the existing `PathBuf` field (the lbug DB file
    /// path). `self.parent_dir` is a new `PathBuf` field added by
    /// issue #1973, cached at construction time to avoid an allocation
    /// + `parent()` call on every write.
    ///
    /// Reordering or omitting any step re-introduces the lost-write
    /// window that motivated issue #1973. Do not "optimize" this
    /// function without re-running the SIGKILL integration test.
    ///
    /// # Returns
    /// `Ok(())` when `self.durable_writes == false` (in-memory backend)
    /// without performing any I/O. Otherwise propagates the first
    /// failing step's typed `SimardError` (see [Error Mapping] above).
    fn post_write_barrier(&self, op: &'static str) -> SimardResult<()>;
}
```

Intentionally **not** `pub`: the barrier is an implementation detail
of `NativeCognitiveMemory`'s mutating methods. Callers of
`CognitiveMemoryOps` cannot and should not invoke it directly — every
mutating trait method that needs it already does so.

### `cognitive_memory::backup::atomic_copy_with_fsync` (hardened in #1973)

```rust
// src/cognitive_memory/backup.rs
pub(crate) fn atomic_copy_with_fsync(
    src: &Path,
    dst: &Path,
) -> SimardResult<()>;
```

Copies `src` to `dst` atomically (via `dst.tmp` rename) and:

1. `fsync`s the destination file,
2. `fsync`s the destination's parent directory,
3. **Re-opens the destination read-only and verifies a SHA-256
   digest against the source** (issue #1973 — defeats page-cache
   aliasing and silent corruption),

returning `PersistentStoreIo { action: "verify-readback", .. }` on
hash mismatch. Used by `create_verified_backup` (forward path) and
`try_restore_from_backup` (recovery path; the recovery call carries
the `recovery-replay-fsync` action label downstream).

---

## Tests

| Test | Location | Purpose |
|---|---|---|
| `sigkill_preserves_last_write` | `tests/cognitive_memory_crash_durability.rs` | **#1973** — Spawn helper binary, write one marker fact, wait for `WROTE` line, `SIGKILL` the helper, reopen the store from a fresh process, assert the marker is present. Direct proof of the per-write barrier guarantee. |
| `sigkill_run_is_observable_for_negative_control` | `tests/daemon_sigterm_durability.rs` | **#1973** — Promoted in this PR from `assert!(count <= N_FACTS)` to `assert_eq!(count, N_FACTS)`. With the barrier in place, the SIGKILL path must now preserve all acknowledged writes; the test fails if the barrier is regressed. |
| `in_memory_barrier_is_noop` | `src/cognitive_memory/mod.rs` (in-module `#[cfg(test)]`) | **#1973** — Asserts that the in-memory backend's `post_write_barrier` returns `Ok(())` without opening `self.path` or `self.parent_dir` for fsync, even after a write sequence. |
| `post_write_barrier_propagates_fsync_failures` | `src/cognitive_memory/mod.rs` (in-module `#[cfg(test)]`) | **#1973** — Forces an fsync failure (read-only parent dir) and asserts the typed `PersistentStoreIo { action: "fsync-parent-dir", .. }` variant is returned, not swallowed. |
| `atomic_copy_with_fsync_rejects_hash_mismatch` | `src/cognitive_memory/backup.rs` (in-module `#[cfg(test)]`) | **#1973** — Injects a corrupted destination after the copy and asserts the readback hash check returns `PersistentStoreIo { action: "verify-readback", .. }`. |
| `daemon_sigterm_writes_survive` | `tests/daemon_sigterm_durability.rs` | **#1631** — Spawn daemon, write 10 facts, send `SIGTERM`, restart, assert 10/10 survive. |
| `checkpoint_succeeds_after_writes` | `src/cognitive_memory/tests_mod.rs:260` | Existing — store two facts, checkpoint twice, verify search returns the writes. |
| `checkpoint_on_fresh_db_is_safe` | `src/cognitive_memory/tests_mod.rs:275` | Existing — checkpoint a fresh DB returns Ok. |
| `create_verified_backup_*` / `prune_old_backups_*` | `src/cognitive_memory/backup.rs` (in-module) | Existing — backup creation, pairing, pruning, verification. |

### Crash-recovery test harness

`tests/cognitive_memory_crash_durability.rs` ships with a reusable
helper block clearly marked:

```rust
// ─────────────────────────────────────────────────────────────
//   EXTRACTABLE FOR #1974
//   Functions below are duplicated in
//   tests/daemon_sigterm_durability.rs and should be moved to a
//   shared module (tests/support/crash_recovery.rs) under #1974.
// ─────────────────────────────────────────────────────────────
fn helper_binary() -> PathBuf { /* resolve via CARGO_BIN_EXE_* or cargo target dir */ }
fn spawn_helper(data_dir: &Path) -> Child { /* ... */ }
fn read_ready_pid(child: &mut Child) -> u32 { /* parse "READY <pid>\n" */ }
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> { /* ... */ }
fn count_facts(data_dir: &Path) -> usize { /* open store, count rows */ }
// ─────────────────────────────────────────────────────────────
```

The companion helper binary is `examples/cognitive_memory_crash_helper.rs`
(see [Crash Helper Binary](#crash-helper-binary) below). Issue #1974
will move these helpers into `tests/support/crash_recovery.rs` and
have both `cognitive_memory_crash_durability.rs` and
`daemon_sigterm_durability.rs` import them.

### Crash Helper Binary

`examples/cognitive_memory_crash_helper.rs` is a small standalone
binary used exclusively by integration tests. Its contract:

| Aspect | Specification |
|---|---|
| Argv | `<data_dir>` — one positional argument, an absolute path. |
| Safety | Canonicalizes `data_dir` and asserts it is under `std::env::temp_dir().canonicalize()`. Aborts with `exit(2)` otherwise. |
| Signal handlers | **None installed.** The binary explicitly does not register any handler that could absorb `SIGKILL`. |
| Behavior | Opens cognitive memory at `data_dir`, calls `store_fact` with a fixed marker (`fact_id="crash-helper-marker"`, `content="WROTE"`), prints `READY <pid>\n` to stdout (flushed), prints `WROTE\n` to stdout (flushed), then calls `thread::park()` to await `SIGKILL`. |
| Exit | Never exits normally; the test sends `SIGKILL` to its pid. |

Tests invoke it via `Command::new(env!("CARGO_BIN_EXE_cognitive_memory_crash_helper"))`
when available, falling back to resolving the binary in the cargo
target directory.

> **Caution**: do not add a `Drop` impl, a panic hook, or any other
> cleanup path to this binary. The whole point of the test is to
> simulate a process that dies without running any cleanup. The
> existing helper has a `#[deny(unsafe_code)]` and a code-review
> checklist comment to that effect.

> **Caution (test side)**: integration tests that spawn the helper
> **must** wrap the `Child` handle in a kill-on-drop RAII guard (e.g.,
> a small struct whose `Drop` calls `child.kill()` and
> `child.wait()`). Without it, a panicking assertion in the test will
> leave the helper parked in `thread::park()` and the test process
> will hang until CI's job-level timeout fires. The shared helpers in
> `tests/cognitive_memory_crash_durability.rs` (extractable for
> #1974) provide such a guard; new tests should reuse it.

---

## Operational Runbook

### "I just restarted the daemon and the journal does not show the shutdown banner"

The signal handler may not have fired. Check:

```bash
# Look for the shutdown trace in recent logs
journalctl -u simard-ooda --since "10 min ago" \
  | grep -E "shutdown sequence start|shutdown complete"

# If absent, check for SIGKILL escalation by systemd
journalctl -u simard-ooda --since "10 min ago" \
  | grep -iE "killed|SIGKILL"
```

If systemd escalated to SIGKILL, increase `TimeoutStopSec` in the unit
file. Otherwise, file a `durability` issue with the journal excerpt.

### "Backup directory is growing without bound"

Check the consecutive-failure error log:

```bash
journalctl -u simard-ooda | grep "DB backup failed"
```

If you see `ERROR: DB backup failed N consecutive times`, the daemon
has been unable to write a verified backup for at least 3 cycles
(roughly 15 minutes at the default interval). Inspect the named
backup directory for permissions / disk-space / corruption issues, fix
the underlying cause, and the next successful backup will reset the
counter.

If the directory is growing despite successful backups, verify
`SIMARD_DB_BACKUP_KEEP` is non-zero — `0` disables pruning entirely.

### "I see `durability barrier failed` in the journal"

This is the per-write fsync barrier (#1973) reporting an I/O failure
on a mutating cognitive-memory op. The structured fields identify
the failing step:

```bash
journalctl -u simard-ooda --since "10 min ago" \
  | grep 'durability barrier failed'

# Or filter by the structured store field:
journalctl -u simard-ooda --since "10 min ago" \
  | grep 'store="cognitive-memory"'
```

Decode the `action` field:

| `action` | Meaning | First check |
|---|---|---|
| `fsync-data-file` | `sync_all()` on the main data file failed. | Disk full? Read-only remount? `dmesg` for block-device errors. |
| `fsync-parent-dir` | `sync_all()` on the data directory failed. | Directory permissions; parent FS readonly. |
| `verify-readback` | Post-fsync SHA-256 hash mismatch. | Possible silent disk corruption; capture the `path` and the full digests from `tracing` output and file a `durability` issue immediately. |
| `recovery-replay-fsync` | The same barrier failing during backup restoration. | Inspect the source backup integrity and the destination filesystem state. |

The caller of the mutating op already received an `Err(...)` return,
so the in-memory state is consistent with disk. The daemon will
continue to attempt subsequent writes; if they also fail, the
underlying I/O fault must be resolved before durability is restored.

### "I need to disable backups temporarily (e.g., during a migration)"

```bash
sudo systemctl edit simard-ooda
# Add (or merge into existing):
[Service]
Environment=SIMARD_DB_BACKUP_INTERVAL_SECS=86400
# Save, then:
sudo systemctl restart simard-ooda
```

Set the interval to `86400` (one day) rather than disabling outright;
that preserves a daily floor of durability. Setting
`SIMARD_DB_BACKUP_KEEP=0` disables pruning but does **not** disable
backup creation — to halt backup creation entirely, you must stop the
daemon.

### "After upgrading, my cognitive-memory data is missing and there is a `cognitive_memory.ladybug.kuzu-backup` directory"

You upgraded from a prototype build that stored cognitive memory as
a KuzuDB directory. On first open, `NativeCognitiveMemory::open`
detected the directory at `~/.simard/cognitive_memory.ladybug`,
renamed it aside to `cognitive_memory.ladybug.kuzu-backup`, and
created a fresh empty lbug DB file. The legacy KuzuDB data was
**not** auto-imported — the on-disk formats are incompatible. The
journal will show:

```
[simard] migrating old KuzuDB directory → /.../cognitive_memory.ladybug.kuzu-backup
```

The kuzu-backup directory is preserved on disk for manual
inspection or one-shot Cypher export. If you do not need it, it is
safe to `rm -rf` once you have confirmed the new lbug DB is the
intended source of truth.

---

## See Also

- [`docs/memory.md`](../memory.md) — cognitive-memory data model
- [`docs/daemon-mode.md`](../daemon-mode.md) — OODA daemon overview
- [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) — pre-commit, merge policy,
  retention disclosure
- [`docs/operations/meeting-handoffs.md`](meeting-handoffs.md) —
  meeting REPL & handoff ingestion
- [GitHub issue #1973](https://github.com/rysweet/Simard/issues/1973) —
  per-write fsync barrier + crash-recovery test (this feature)
- [GitHub issue #1972](https://github.com/rysweet/Simard/issues/1972) —
  improve-cognitive-memory-persistence epic (goals G1 and G2)
- [GitHub issue #1974](https://github.com/rysweet/Simard/issues/1974) —
  extract the reusable crash-recovery test helpers (next cycle)
- [GitHub issue #1975](https://github.com/rysweet/Simard/issues/1975) —
  audit and fix swallowed `let _ = .sync_all()` sites outside the
  cognitive-memory write path
