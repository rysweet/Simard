# Verified Backups of the Live Cognitive Store

> Issue #2420. The daemon backs up the **live** cognitive-memory store, captures
> the **whole** store (not a capped subset), **verifies** every backup before
> pruning, and keeps the corrupt-quarantine population bounded. It complements
> [Cognitive Memory Durability](cognitive-memory-durability.md), which covers
> SIGTERM-safe shutdown and the WAL/CHECKPOINT durability the library backend
> owns.

## Why this exists

The OODA daemon opens its cognitive store through the library backend at
`state_root/cognitive` (in production, `~/.simard/cognitive`). After the
`lbug` 0.17.x de-fork migration (#2307) the live store moved into this
directory; the legacy single-file store at `state_root/cognitive_memory.ladybug`
is no longer the source of truth.

Two coupled regressions broke resilience:

1. **Backups targeted the stale path.** The file-copy backup copied the legacy
   single-file store, so once the store migrated no fresh verified backup was
   produced — the newest backup on the host was Jun 20 while the live store grew
   past 10k memories.
2. **The snapshot export was capped.** The snapshot path capped at 1000 facts
   (`MAX_EXPORT_FACTS`), so even a correctly-targeted backup would silently drop
   everything past the first 1000 memories once the store grew larger.

The verified-backup pipeline guarantees four properties that together make a
silent backup-rot regression impossible:

1. **Backup source == live store.** The backup reads the live store through the
   client, resolved through one migration-aware function
   ([`live_store_path`](#live-store-path-resolution)). A regression test asserts
   the backup source equals the path the daemon opens, so they cannot silently
   diverge again.
2. **Whole store, never capped.** The backup uses an **uncapped** export so it
   holds every fact and procedure regardless of how large the store grows. A
   test stores more than the legacy cap and asserts a full round-trip.
3. **Verified before prune.** Every backup is re-opened and validated (checksum
   + manifest self-consistency) before any retention prune runs. A failed
   verification is logged loudly and leaves prior good backups untouched.
4. **Bounded quarantine.** Corrupt/shadow quarantine artifacts are capped by
   both age **and** count, so a corruption burst can never fill the disk.

---

## Concepts

### Live-store path resolution

`live_store_path(state_root) -> PathBuf` is the single source of truth for the
on-disk location of the cognitive store. It is migration-aware:

| Condition | Resolved path |
|---|---|
| `state_root/cognitive` exists (post-migration, normal) | `state_root/cognitive` |
| only the legacy `state_root/cognitive_memory.ladybug` exists | `state_root/cognitive_memory.ladybug` |
| neither exists (fresh install) | `state_root/cognitive` (created on open) |

The daemon's store-open path (`LibraryCognitiveMemory::open`) and the
`live_store_path` resolver are anchored to the same `LIVE_STORE_SUBDIR`
constant, so "what the daemon opens" and "what the resolver reports" are
guaranteed equal. The verified backup then reads that live store *through the
client* (a logical snapshot), so it inherently captures exactly the store the
daemon opened — never a stale file path. The resolver is exported from
`cognitive_memory`.

```rust
use simard::cognitive_memory::live_store_path;

let state_root = dirs::home_dir().unwrap().join(".simard");
let store_path = live_store_path(&state_root);
// -> ~/.simard/cognitive  (post-migration)
```

### Logical, not file-copy, backups

A backup is a **logical snapshot** taken through the live client, not a raw copy
of the open LadybugDB store. Copying an open store file is unsafe; reading
through `CognitiveMemoryOps` is consistent and inherently targets the migrated
path. Each backup is a timestamped directory under `state_root/backups/`:

```text
~/.simard/backups/20260627_231000/
  cognitive_snapshot.json   # facts + procedures exported via the client
  memory_records.json       # file-backed memory records
  manifest.json             # counts + SHA-256 checksum over the two files above
```

### Whole-store export (no cap)

`remote_transfer::export_full_memory_snapshot` exports the **entire** store —
every fact and every procedure — by requesting with the maximum limit, the same
unbounded retrieval `graph_stats` already performs safely. It is distinct from
the capped `export_memory_snapshot`, which keeps replication/migration payloads
bounded at `MAX_EXPORT_FACTS` / `MAX_EXPORT_PROCEDURES`. `backup_memory` uses the
full export so the backup is a faithful copy of the live store, no matter how
large it grows.

> **Snapshot completeness (issue #2550).** The export is extended to include
> **all durable memory types** — episodes and prospective/triggers, alongside
> facts and procedures — so a snapshot is a complete restore source for the
> cognitive-memory *nodes*. Provenance/similarity **edges** are not serialized:
> they are reconstructable and their endpoints change across a content-dedup
> restore. The export stays size-bounded for large stores. Snapshots written
> before #2550 hold facts + procedures only; a restore from one of those is
> partial by construction.

### The verify-before-prune gate

`backup_memory_verified` wraps backup creation with a hard verification gate:

1. Produce the backup (`backup_memory`) — captures the full snapshot + records
   and records their counts and a SHA-256 checksum in `manifest.json`.
2. Re-open and validate it (`ensure_backup_valid` → `verify_backup`): the
   manifest is present, the snapshot/records files exist, the checksum matches,
   and the recorded counts match the serialized data.
3. **Gate:** return the manifest only if the backup is `Valid`; otherwise return
   a loud `SimardError::MemoryIntegrityError`.

Callers prune **only** after this returns `Ok`. The daemon's
`run_verified_backup` does exactly that: `backup_memory_verified` then
`prune_old_backups`, so a failed or partial backup can never cause the
last-known-good backup to be reclaimed.

For callers holding an **independent** expected count, `verify_backup_memory_count(dir, expected_total)`
re-opens a backup and fails loudly unless its total memory count (facts +
procedures + records) equals `expected_total` — the explicit "confirm the
expected memory count before pruning" check. The daemon does not re-query an
independent live count each cycle (which would race with concurrent writes); the
manifest counts are written from the same read that produced the snapshot and
are confirmed self-consistent by `ensure_backup_valid`.

### Bounded corrupt-quarantine retention

When LadybugDB detects a corrupt store or WAL it quarantines the bad bytes in
place, **next to the live store**. Because the live cognitive store lives at
`<state_root>/state/cognitive` (see [Live-store path resolution](#live-store-path-resolution)),
fresh quarantines land in `<state_root>/state/` — e.g. `cognitive.corrupt-<ts>`,
`cognitive.wal.corrupt-<ts>`, recursively nested `…cognitive.wal.corrupt-<ts>`
chains, corrupt-snapshot shadow side-files that carry the infix
(`cognitive.corrupt-<ts>.shadow`), and `.corrupt-<ts>.bak` copies. The
legacy native backend also left `cognitive_memory.corrupt-<ts>` snapshots
directly under the top-level state root (`~/.simard`).

`simard cleanup` therefore reclaims quarantines in **both** directories:

| Directory | Helper | Contents |
|---|---|---|
| `<state_root>` (`~/.simard`) | `simard_state_root()` | legacy native `cognitive_memory.corrupt-*` snapshots |
| `<state_root>/state/` (`~/.simard/state/`) | `resolve_subdir("state")` | live-backend `cognitive*.corrupt-*` quarantines (where the store actually lives) |

Both paths are resolved through the [`state_root`](../reference/state-root-resolution.md)
helpers — there is **no hardcoded duplicate path**. A cleanup entry is a
quarantine iff its name **starts with** `cognitive.` or `cognitive_memory.` **and
contains** the `.corrupt-<ts>` infix; the live store files (`cognitive`,
`cognitive.wal`, `cognitive.shadow`) lack that infix, so they are never matched.

> **Why the second directory matters.** Before this fix, cleanup only scanned the
> **top-level** `~/.simard` and never read `~/.simard/state/`, so once the store
> moved into the `state/` subdir (de-fork Phase 2b, #2307) library-backend
> quarantines accumulated **unbounded** there — one host reached 62 stale
> artifacts dating back months. Reclaiming `state/` with the same bounds closes
> that regression.

`remove_old_corrupt_dbs` applies **three bounds, independently per directory**
(each directory is listed and ranked on its own — the `state/` branch can never
erode the top-level directory's `keep-N` or largest-asset protection, and vice
versa):

| Bound | Constant | Default | Effect |
|---|---|---|---|
| Age | `CORRUPT_DB_MAX_AGE_DAYS` | `30` days | Remove any quarantine older than N days |
| Count | `CORRUPT_DB_KEEP` | `5` | Keep only the newest N; remove the older surplus immediately |
| Largest-asset floor | `CORRUPT_DB_PROTECT_MIN_BYTES` | `1 MiB` | Never sweep the single largest quarantine at/above the floor — the most likely recovery asset — regardless of age or rank |

A quarantine is removed if it fails **either** of the age/count checks (it is
older than the age cap **or** it is not among the newest `CORRUPT_DB_KEEP`),
*unless* it is the protected largest asset. The count cap closes the gap where a
corruption burst inside the age window could accumulate unbounded (this host saw
88 MB / 112 artifacts pile up). Neither the age nor count bound *protects* an
entry the other would drop — a quarantine within the newest N but older than 30
days is still deleted by the age check.

The **`no_quarantine` self-health probe scans this same live-store directory**
(`<state_root>/state/`, via the same `state_root` helpers) and flags only *fresh*
quarantines created since the deploy window — so the retained snapshots this
retention policy keeps do **not** fail the health check. See
[`NoQuarantineProbe`](../reference/self-deploy-api.md#noquarantineprobe).

> **Preserve recovery assets (issue #2550).** These caps must never sweep away
> the inputs an operator needs to recover from a corruption-reset. The retention
> floors keep the quarantined `cognitive.corrupt-*` store and recent
> `cognitive_snapshot.json` backups long enough to recover from before either
> cap reclaims them. During an active incident, do **not** run `simard cleanup`
> until you have restored or copied the quarantine and newest snapshots out —
> see the [Cognitive-Memory WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md).

### WAL durability — recovery must be durable

The library backend's write-ahead-log durability (corrupt-WAL recovery +
checkpoint) is present in the pinned `amplihack-memory` dependency. The
WAL-durability commit (`7b81590`, PR #89) is an **ancestor** of the current pin
(`26d49bf8`, lbug-0.17.1), so no dependency bump was needed for the *recovery
attempt* itself.

> **Issue #2550.** The 2026-07-04 incident showed that recovery *attempting* is
> not enough: a prefix-recovery salvaged 40,488 records, but the **checkpoint
> after recovery failed** and a later open **reset the store to empty**. The
> current pinned library (`26d49bf8`) already recovers durably — it quarantines
> the corrupt WAL *before* the resilient open and **never** resets a store that
> recovered records (a reset only happens for a genuinely corrupt main DB with
> zero recovered records, and it moves the store aside, never deletes). The
> #2550 change **locks that contract with a boundary regression test** and adds
> snapshot import + startup auto-restore as a backstop for the genuine-reset
> case. (The dependency bump in this change adds a read-only prospective
> enumerator for complete snapshots — not a recovery change.) See
> [Cognitive Memory Durability](cognitive-memory-durability.md) for the
> WAL/CHECKPOINT model and the
> [WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md) for the
> incident and operator recovery.

---

## How it runs (daemon)

The daemon's periodic backup was removed in the de-fork (#2307); #2420
reintroduces it as a **verified** loop. At the top of each OODA cycle the daemon
compares the elapsed time since the last backup against the configured interval;
when the interval has passed it runs `run_verified_backup` before cycle work.
Running at the quiescent top of the loop keeps the snapshot consistent.

Per tick (when due):

1. Open the file-backed store at `state_root/memory_records.json`.
2. `backup_memory_verified(client, store, "simard", &config)` — full snapshot +
   verify.
3. On `Ok`: log `verified backup OK` with counts, then `prune_old_backups`.
4. On `Err`: log `WARN: verified backup FAILED, prune skipped` and continue the
   cycle. Prior backups are preserved.

A backup failure never aborts the OODA cycle — durability is best-effort and
fail-loud, never fail-silent.

> The first verified backup runs promptly after daemon start (the timer is
> back-dated), so a freshly-deployed daemon produces a backup within its first
> cycle rather than one full interval later.

---

## Configuration

| Setting | Default | Override | Notes |
|---|---|---|---|
| Backup interval (seconds) | `86400` (daily) | `SIMARD_BACKUP_INTERVAL_SECS=N` | Read at daemon start; per-cycle elapsed-time check. A zero/invalid value falls back to the default |
| Backup directory | `<state_root>/backups/` | — | Created on first backup |
| Retention (days) | `30` | `BackupConfig::retention_days` | Age-based pruning of backups |
| Minimum backups kept | `3` | `BackupConfig::min_backups_to_keep` | Never prune below this count |
| Corrupt quarantine age cap (days) | `7` | `CORRUPT_DB_MAX_AGE_DAYS` (compile-time) | Removes quarantines older than N days |
| Corrupt quarantine count cap | `5` | `CORRUPT_DB_KEEP` (compile-time) | Keeps the newest N; older surplus removed (age cap still applies) |

> `SIMARD_BACKUP_INTERVAL_SECS` is the operator-facing runtime knob. The
> `BackupConfig` fields come from `BackupConfig::default()` and the quarantine
> caps are compile-time constants — changing them requires a rebuild.

### Sample systemd unit excerpt

```ini
[Service]
WorkingDirectory=/home/azureuser/.simard/repo
Environment=SIMARD_BACKUP_INTERVAL_SECS=86400
KillSignal=SIGTERM
TimeoutStopSec=30
ExecStart=/usr/local/bin/simard ooda daemon
Restart=always
```

> The live store is `~/.simard/cognitive`; the daemon's `WorkingDirectory` is
> `~/.simard/repo`. Operators own deployment — changing backup configuration
> requires an operator-driven restart; the daemon is never live-redeployed from
> automation.

---

## Public API

All items live in `simard::memory_backup` unless noted.

### `live_store_path` — `simard::cognitive_memory`

```rust
pub fn live_store_path(state_root: &Path) -> PathBuf
```

Migration-aware resolver for the live cognitive store path. Anchored to the
same `LIVE_STORE_SUBDIR` constant as the daemon store-open
(`LibraryCognitiveMemory::open`) so the two cannot diverge.

### `export_full_memory_snapshot` — `simard::remote_transfer`

```rust
pub fn export_full_memory_snapshot(
    client: &dyn CognitiveMemoryOps,
    agent_name: &str,
) -> SimardResult<MemorySnapshot>
```

Exports the **complete** store (every fact + procedure), uncapped. Used by
`backup_memory`. Distinct from the capped `export_memory_snapshot` used for
bounded replication payloads.

### `backup_memory_verified`

```rust
pub fn backup_memory_verified(
    client: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    agent_name: &str,
    config: &BackupConfig,
) -> SimardResult<BackupManifest>
```

Creates a backup and verifies it re-opens cleanly before returning. Returns
`SimardError::MemoryIntegrityError` if the backup is not `Valid`. **Callers must
only prune after this returns `Ok`.**

### `verify_backup_memory_count`

```rust
pub fn verify_backup_memory_count(backup_dir: &Path, expected_total: usize) -> SimardResult<()>
```

Re-opens a backup and fails loudly unless its total memory count (facts +
procedures + records) equals `expected_total`. The explicit count gate for
callers holding an independent expected count.

### `ensure_backup_valid`

```rust
pub fn ensure_backup_valid(backup_dir: &Path) -> SimardResult<BackupManifest>
```

Returns the manifest only if `verify_backup` reports `Valid`; otherwise a loud
`MemoryIntegrityError`. The verify-before-prune primitive used by
`backup_memory_verified`.

### `backup_memory`

```rust
pub fn backup_memory(
    client: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    agent_name: &str,
    config: &BackupConfig,
) -> SimardResult<BackupManifest>
```

Low-level backup creation: exports the **full** cognitive snapshot and
file-backed records into a timestamped directory and writes `manifest.json`.
Prefer `backup_memory_verified` for the daemon path — it adds the verify gate.

### `verify_backup`

```rust
pub fn verify_backup(backup_dir: &Path) -> SimardResult<BackupVerification>
```

Re-reads a backup and validates it against its manifest (file presence, recorded
counts, SHA-256 checksum). Returns a `BackupVerification` whose `status` is
`Valid`, `Corrupted { reason }`, or `Incomplete { missing }`.

### `restore_from_backup`

```rust
pub fn restore_from_backup(
    client: &dyn CognitiveMemoryOps,
    store: &dyn MemoryStore,
    backup_dir: &Path,
) -> SimardResult<usize>
```

Verifies the backup, then imports the cognitive snapshot and file-backed records
into the live store. Returns the total number of items restored. Refuses to
restore from a `Corrupted` or `Incomplete` backup.

> **Additive, not wipe-and-replace.** Restore imports on top of whatever the
> target store already holds; it does **not** clear the target first. Restoring
> into a **fresh/empty** store reproduces the backed-up count exactly. Restoring
> into a **non-empty** store **merges** the backup onto the existing data, and the
> resulting count may exceed the backup's. For a clean recovery, restore into an
> empty target (see the runbook below).

### `list_backups` / `prune_old_backups`

```rust
pub fn list_backups(config: &BackupConfig) -> SimardResult<Vec<BackupVerification>>
pub fn prune_old_backups(config: &BackupConfig) -> SimardResult<usize>
```

`list_backups` lists backups newest-first with verification status.
`prune_old_backups` removes backups older than `retention_days`, never dropping
below `min_backups_to_keep`. Call **only** after a successful verified backup.

### Types

```rust
pub struct BackupConfig { pub backup_dir: PathBuf, pub retention_days: u32, pub min_backups_to_keep: usize }
pub struct BackupManifest { /* backup_dir, created_at, counts, checksum, ... */ }
pub struct BackupVerification { pub manifest: BackupManifest, pub status: BackupStatus, pub verified_at: DateTime<Utc> }
pub enum   BackupStatus { Valid, Corrupted { reason: String }, Incomplete { missing: Vec<String> } }
```

### Cleanup API

In `simard::cmd_cleanup::disk`:

```rust
pub const CORRUPT_DB_MAX_AGE_DAYS: u64 = 7;
pub const CORRUPT_DB_KEEP: usize = 5;

/// Remove quarantines older than CORRUPT_DB_MAX_AGE_DAYS OR beyond the newest
/// CORRUPT_DB_KEEP.
pub fn remove_old_corrupt_dbs(report: &mut CleanupReport);
```

`simard cleanup` calls `remove_old_corrupt_dbs`, which applies both bounds in a
single pass.

---

## Examples

### Produce a verified backup of the live store

```rust
use simard::memory_backup::{backup_memory_verified, BackupConfig};

let config = BackupConfig { backup_dir: state_root.join("backups"), ..BackupConfig::default() };
let manifest = backup_memory_verified(
    &*clients.memory,          // live cognitive client
    &file_backed_store,        // <state_root>/memory_records.json
    "simard",
    &config,
)?;
println!("verified backup: {} facts -> {}", manifest.cognitive_facts_count, manifest.backup_dir.display());
```

### Restore round-trip

```rust
use simard::memory_backup::{backup_memory_verified, restore_from_backup};

// 1. Back up the live store (full + verified).
let manifest = backup_memory_verified(&*clients.memory, &file_backed_store, "simard", &config)?;

// 2. Restore into a fresh (empty) store and confirm the count round-trips.
//    A fresh target is required for exact equality: restore is additive.
let restored = restore_from_backup(&*fresh_client, &fresh_store, &manifest.backup_dir)?;
let expected = manifest.cognitive_facts_count
    + manifest.cognitive_procedures_count
    + manifest.memory_records_count;
assert_eq!(restored, expected);
```

### Inspecting backups on disk

```bash
# Newest-first listing of backups
ls -1dt ~/.simard/backups/*/ | head

# Inspect one backup's manifest (counts + checksum)
cat ~/.simard/backups/20260627_231000/manifest.json \
  | jq '{created_at, cognitive_facts_count, cognitive_procedures_count, memory_records_count}'
```

---

## Operational runbook

### "Is a fresh verified backup being produced?"

```bash
# Most recent backup directory and its age
ls -1dt ~/.simard/backups/*/ | head -1

# Journal confirmation that the gate passed
journalctl -u simard-ooda --since "1 day ago" | grep -E "verified backup OK|verified backup FAILED"
```

A current timestamp plus a `verified backup OK` line means the live store is
being backed up and verified. A `verified backup FAILED, prune skipped` line is
a paging event — investigate before the next interval.

### "Backups are failing verification"

`backup_memory_verified` failed the verify gate. The error names the backup
directory and the reason. Check:

- Disk space / permissions on `~/.simard/backups/`.
- Whether the live store opened correctly this cycle.

Prior backups are intact because prune is skipped on failure — restore from the
last good backup if needed.

### "`~/.simard` is filling with `cognitive.corrupt-*` files"

Fresh quarantines land in `~/.simard/state/` (next to the live store); the legacy
native backend left `cognitive_memory.corrupt-*` snapshots directly under
`~/.simard`. Both are bounded by age (30 days), count (newest 5), and the
largest-asset floor, **independently per directory**. Reclaim immediately:

```bash
simard cleanup    # applies age + count + largest-asset caps to quarantines in
                  # BOTH ~/.simard and ~/.simard/state/, among other artifacts
```

If new `.corrupt-<ts>` files appear every cycle, that is a corruption signal —
capture a sample and file a `durability` issue rather than only deleting them.

### "I need to restore the live store from a backup"

Restore is a library operation (`restore_from_backup`) invoked with the daemon
stopped so nothing writes to the live store concurrently. It verifies the backup
first and refuses corrupt/incomplete backups.

Restore is **additive** — it imports onto whatever the target already holds and
does not wipe it first. After corruption the live `~/.simard/cognitive` may still
hold partial data, so restoring directly onto it **merges** the backup onto that
partial data. For a clean recovery, move the corrupt/partial store aside so the
restore lands on an empty target:

```bash
sudo systemctl stop simard-ooda
# Identify the newest verified backup directory:
ls -1dt ~/.simard/backups/*/ | head -1
# Move the corrupt/partial live store aside so restore lands on a clean target:
mv ~/.simard/cognitive ~/.simard/cognitive.prerestore-$(date +%Y%m%d_%H%M%S)
# Run restore via operator tooling / a one-shot harness that calls
# memory_backup::restore_from_backup(client, store, backup_dir) against a fresh
# ~/.simard/cognitive, then:
sudo systemctl start simard-ooda
journalctl -u simard-ooda -n 50
```

`restore_from_backup` verifies the backup before importing; if a backup is
rejected as corrupt/incomplete, fall back to the next-most-recent directory.

Since issue #2550 there is a first-class operator command that wraps this path —
`simard memory import <snapshot.json>` — so you no longer need a one-shot Rust
harness. It is idempotent (dedup by content), so it is safe to re-run and safe to
run onto a partially-populated store:

```bash
sudo systemctl stop simard-ooda
mv ~/.simard/cognitive ~/.simard/cognitive.prerestore-$(date +%Y%m%d_%H%M%S)
simard memory import "$(ls -1dt ~/.simard/backups/*/ | head -1)cognitive_snapshot.json"
sudo systemctl start simard-ooda
journalctl -u simard-ooda -n 50
```

The full incident-driven procedure (detect, preserve assets, choose a snapshot,
verify) is the
[Cognitive-Memory WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md).

---

## Tests

#2420 adds the following regression coverage:

- **Path guard** — `live_store_path` resolves to `state_root/cognitive`
  post-migration, falls back to the legacy file only when `cognitive` is absent,
  and equals the path `LibraryCognitiveMemory::open` uses (both anchored to
  `LIVE_STORE_SUBDIR`).
- **Whole-store export** — a store with more facts than the legacy cap exports
  fully via `export_full_memory_snapshot` while the capped export truncates; a
  verified backup of a >cap store round-trips the full count on restore.
- **Verify-before-prune gate** — a faithful backup passes; a backup whose count
  is short of the expected count (or whose status is not `Valid`) fails loudly.
- **Daemon routine** — `run_verified_backup` produces a `Valid` backup under
  `state_root/backups` (and on an empty store); the interval parser falls back to
  the default on missing/invalid/zero values.
- **Quarantine bounds** — with more than `CORRUPT_DB_KEEP` young quarantines,
  only the newest N survive; the live `cognitive` / `cognitive.wal` files are
  never removed; the age path remains independently functional.

---

## See Also

- [Cognitive Memory Durability](cognitive-memory-durability.md) — SIGTERM-safe
  shutdown, WAL/CHECKPOINT model, quarantine background
- [Cognitive-Memory WAL Recovery Runbook](cognitive-memory-wal-recovery-runbook.md)
  — corrupt-WAL recovery, `simard memory import`, startup auto-restore (#2550)
- [Memory introspection CLI](../reference/simard-memory-cli.md) —
  `simard memory stats` / `dump` / `import`
- [Operations index](index.md)
- [GitHub issue #2420](https://github.com/rysweet/Simard/issues/2420) — verified
  backups of the live store + quarantine bounding (this feature)
- [GitHub issue #2550](https://github.com/rysweet/Simard/issues/2550) — durable
  WAL recovery + snapshot import/auto-restore + complete snapshots
