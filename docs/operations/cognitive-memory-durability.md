# Cognitive Memory Durability

This page documents the SIGTERM-safe shutdown handler and the periodic
backup loop that together guarantee Simard's cognitive memory survives
graceful and forced restarts of the OODA daemon.

> Quick reference for contributors: see the
> [Cognitive Memory Durability section in CONTRIBUTING.md](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md#cognitive-memory-durability-sigterm--periodic-backups).

---

## Background

Simard's cognitive memory is stored in
`~/.simard/cognitive_memory.ladybug/` (an
[`lbug`](https://docs.rs/lbug) database **directory**, not a single
file) using lbug `0.15.x`. lbug only flushes its WAL inside
`Database::drop`. Process termination via `SIGTERM` (the default signal
`systemctl restart` sends) does not invoke `Drop`, so unflushed writes
can be lost. This was the root cause of the 2026-05-09 incident in
which the active goal
`improve-the-dashboard-via-playwright-driven-testing` was wiped during
a routine `systemctl restart simard-ooda`.

The hardening described below ensures data loss of that class cannot
recur and that even total daemon failure (SIGKILL, OOM, power loss)
loses at most ~5 minutes of writes.

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
+---------------------------+
            |
            v
+---------------------------+
| ~/.simard/                |
|   cognitive_memory.ladybug/        (lbug DB directory)
|   cognitive_memory.ladybug.wal     (WAL sibling, may exist)
|   cognitive_memory.wal             (alt WAL name, may exist)
|   backups/                |
|     cognitive_memory.ladybug.<ts>          (backup dir copy)
|     cognitive_memory.ladybug.wal.<ts>      (paired WAL backup)
|     cognitive_memory.wal.<ts>              (paired alt WAL backup)
+---------------------------+
```

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
| 2 | `shared_mem.checkpoint()` — collapses the WAL into the main DB directory through the `CognitiveMemoryOps::checkpoint` trait method (delegates to `NativeCognitiveMemory::checkpoint`). | Logged, next step still runs. |
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

`SIGKILL` cannot be intercepted; the daemon dies immediately. In that
case:

- The most recent periodic backup (≤ `SIMARD_DB_BACKUP_INTERVAL_SECS`
  old, default ~5 minutes) is the recovery point.
- Restore via the [Restoring from Backup](#restoring-from-backup)
  procedure.

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
   copies `~/.simard/cognitive_memory.ladybug/` to a sibling under
   `~/.simard/backups/` using `atomic_copy_with_fsync`:
   - `std::fs::copy(src, &dst.tmp)`
   - `OpenOptions::open(dst.tmp).sync_all()`
   - `std::fs::rename(dst.tmp, dst)`
   - `File::open(parent_dir).sync_all()`
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
cognitive_memory.ladybug.<unix_ts>           # main DB directory backup
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

Because `cognitive_memory.ladybug` is an lbug **directory**, restoration
uses `cp -r`, not `cp`.

```bash
# 1. Stop the daemon
sudo systemctl stop simard-ooda

# 2. Identify the most recent paired backup
ls -lt ~/.simard/backups/cognitive_memory.ladybug.* | head

# 3. Restore the DB directory and any paired WAL files
TS=1762800000   # the timestamp suffix from above
rm -rf ~/.simard/cognitive_memory.ladybug
cp -r ~/.simard/backups/cognitive_memory.ladybug.${TS} \
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

---

## Tests

| Test | Location | Purpose |
|---|---|---|
| `daemon_sigterm_writes_survive` | `tests/daemon_sigterm_durability.rs` | Spawn daemon, write 10 facts, send `SIGTERM`, restart, assert 10/10 survive. (To be added under issue #1631.) |
| `daemon_sigkill_negative_control` | `tests/daemon_sigterm_durability.rs` | Same harness but `SIGKILL` — proves the test setup is meaningful (without the fix, durability would not hold). (To be added under issue #1631.) |
| `checkpoint_succeeds_after_writes` | `src/cognitive_memory/tests_mod.rs:260` | Existing — store two facts, checkpoint twice, verify search returns the writes. |
| `checkpoint_on_fresh_db_is_safe` | `src/cognitive_memory/tests_mod.rs:275` | Existing — checkpoint a fresh DB returns Ok. |
| `create_verified_backup_*` / `prune_old_backups_*` | `src/cognitive_memory/backup.rs` (in-module) | Existing — backup creation, pairing, pruning, verification. |
| `shutdown_daemon_ordering` | `src/operator_commands_ooda/daemon/mod.rs` (in-module) | To be added under issue #1631 — assert steps 1–5 execute in documented order. |

> **Note**: tests `daemon_sigterm_writes_survive`,
> `daemon_sigkill_negative_control`, and `shutdown_daemon_ordering`
> are part of the issue #1631 hardening work and land in the same PR
> as this documentation.

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

---

## See Also

- [`docs/memory.md`](../memory.md) — cognitive-memory data model
- [`docs/daemon-mode.md`](../daemon-mode.md) — OODA daemon overview
- [`CONTRIBUTING.md`](https://github.com/rysweet/Simard/blob/main/CONTRIBUTING.md) — pre-commit, merge policy,
  retention disclosure
- [`docs/operations/meeting-handoffs.md`](meeting-handoffs.md) —
  meeting REPL & handoff ingestion
