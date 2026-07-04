---
title: Cognitive-memory WAL corruption recovery runbook
description: Operator runbook for the 2026-07-04 WAL-corruption data-loss class — durable prefix-recovery, snapshot restore via `simard memory import`, startup auto-restore, and the recovery-asset preservation contract (issue #2550).
last_updated: 2026-07-04
owner: cognitive-memory
doc_type: howto
---

# Cognitive-Memory WAL Corruption Recovery Runbook

> **Issue #2550.** Hardens the library-backed cognitive store against the
> corrupt-WAL → salvage → reset **data-loss** class first observed in production
> on 2026-07-04. It complements
> [Cognitive Memory Durability](cognitive-memory-durability.md) (SIGTERM-safe
> shutdown, WAL/CHECKPOINT model) and
> [Verified Backups of the Live Cognitive Store](verified-backups.md)
> (verify-before-prune, bounded quarantines). Read this page when the daemon
> logs a corrupt-WAL recovery, when `simard memory stats` shows a store that
> collapsed to a near-empty count, or when you need to restore from a snapshot.

---

## The incident this runbook exists for

On **2026-07-04 00:16** the live cognitive store failed a data-loss sequence
with **no automatic recovery path**:

1. **WAL corrupt on open.** LadybugDB tripped an assertion opening the
   write-ahead log:

   ```text
   Assertion failed ... wal/wal_record.cpp line 76: UNREACHABLE_CODE
   ```

2. **Resilient recovery salvaged the good prefix.** The `amplihack-memory`
   backend recovered the store from the corrupt WAL and logged success:

   ```text
   recovered from corrupt WAL, good prefix replayed + checkpointed
   ```

   **40,488 records** were salvaged in memory.

3. **The checkpoint after recovery FAILED.** Persisting the salvaged store back
   to disk did not complete:

   ```text
   Cannot open .../cognitive.wal
   Cannot open .../cognitive.wal.checkpoint
   Error removing .../cognitive.wal
   ```

   The recovered store therefore lived only in memory — nothing durable landed.

4. **A later open re-quarantined and RESET the store to empty.** Because the
   on-disk store still looked corrupt, the next open re-flagged it, quarantined
   the bad bytes, and **reset the live store to a fresh empty database** —
   dropping roughly **20,480 memories to 128**.

5. **No restore path existed.** Periodic backups were present at
   `~/.simard/backups/<ts>/cognitive_snapshot.json`, but they held **only facts
   and procedures** — not episodes or prospective/triggers — and there was **no
   operator command** to import a snapshot back into the store. The reset was
   effectively **permanent**.

The four fixes below close each link in that chain so the same corruption
degrades to a durable prefix-recovery (and, worst case, a self-healing
auto-restore) instead of silent, permanent loss.

---

## What changed (issue #2550)

| # | Fix | Failure it removes |
|---|---|---|
| 1 | **Durable WAL recovery.** After a successful prefix-recovery (`recovered_records > 0`) the salvaged store is checkpointed/persisted so later opens succeed. The "checkpoint after recovery failed / cannot open `cognitive.wal(.checkpoint)`" path is handled, and a store that just recovered records is **never** re-quarantined and reset. | Steps 3–4: recovered records lost, store reset to empty. |
| 2 | **Snapshot restore + startup auto-restore.** `simard memory import <snapshot.json>` ingests a `cognitive_snapshot.json` back into the store (idempotent / dedup by content). On daemon startup, if the live store is **empty** (as a corruption-reset leaves it, before bootstrap seeding) **and** a newer non-empty snapshot exists, the daemon **auto-restores** from the newest good snapshot and logs it. | Step 5: no restore path; a reset stays permanent. |
| 3 | **Complete snapshots.** The periodic snapshot export now includes **all** durable memory types — episodes and prospective/triggers — not just facts + procedures, so a restore is faithful. (Provenance/similarity edges are not serialized: they are reconstructable and their endpoints change across a content-dedup restore.) | Step 5: snapshots too thin to fully restore. |
| 4 | **Preserve recovery assets.** The corruption/cleanup path keeps the quarantined `cognitive.corrupt-*` store and recent `cognitive_snapshot.json` backups long enough to recover from — they are not swept or rotated away prematurely. | Loss of the only forensic + recovery inputs before an operator can act. |

Where each fix lives:

- **Fix 1** is in the store/recovery layer, which lives in
  [`amplihack-memory-lib`](../architecture/cognitive-memory-library-adapter.md)
  (`lbug_store::open_with_recovery`). The pinned dependency (`26d49bf8`)
  **already recovers durably**: it quarantines the corrupt WAL *before* the
  resilient open (copied aside, never deleted), replays the good prefix,
  checkpoints, and — critically — if that checkpoint fails it still **keeps** the
  recovered store rather than resetting. A reset (`RebuiltAfterCorruption`) only
  happens for a genuinely corrupt *main* database with **zero** recovered
  records, and it moves the store aside rather than deleting it. The #2550
  contribution here is a **boundary regression test**
  (`tests/wal_recovery_durability_2550.rs`) that pins the contract — *a store
  that recovered records is persisted, not reset* — so it can never regress. The
  dependency bump in this change adds a read-only prospective enumerator for
  Fix 3, **not** a recovery change.
- **Fix 2** adds `simard memory import` in `src/operator_cli/memory.rs` and the
  startup auto-restore (`memory_backup::auto_restore_if_empty`, wired in
  `src/operator_commands_ooda/daemon/mod.rs`).
- **Fix 3** extends `remote_transfer::export_full_memory_snapshot` (the export
  `backup_memory` uses) to cover episodes and prospective/triggers via new
  `CognitiveMemoryOps::list_all_episodes` / `list_all_prospective` enumerators
  (the latter backed by the library's read-only `get_all_prospective`).
- **Fix 4** widens the quarantine retention age and protects the largest
  (recovery-asset) quarantine from the age/count caps in
  `src/cmd_cleanup/disk.rs`, while the existing verified-backup retention
  (`prune_old_backups`, keep-newest-N + age) preserves recent snapshots.

---

## Detect: am I hitting this?

Look for the recovery / reset signatures in the journal, then confirm the live
count with the read-only introspection CLI (see
[Memory introspection CLI](../reference/simard-memory-cli.md)).

```bash
# 1) Recovery + checkpoint-failure signatures
journalctl -u simard-ooda --since "2 hours ago" \
  | grep -E "corrupt WAL|good prefix|recovered .*records|Cannot open .*cognitive\.wal|reset .*empty|auto-restored"

# 2) Current live count (near-empty after a reset)
simard memory stats            # human table
simard memory stats --json     # scriptable; check counts.total
```

Interpretation:

| Journal evidence | State | Action |
|---|---|---|
| `recovered … records, good prefix replayed + checkpointed` and **no** later `Cannot open cognitive.wal` / `reset` | Durable recovery succeeded (fix 1). | None — the salvaged store persisted. Confirm `simard memory stats` count is healthy. |
| `auto-restored <n> memories from … cognitive_snapshot.json` on startup | Store was empty; daemon self-healed from the newest snapshot (fix 2). | None — verify the restored count; investigate the original corruption. |
| `Cannot open … cognitive.wal.checkpoint` **followed by** a near-empty `simard memory stats` and no auto-restore line | Reset without recovery — manual restore needed. | Follow **Manual recovery** below. |

---

## Manual recovery

Use this when the store reset and auto-restore did not fire (for example on a
build that predates issue #2550, or when no newer snapshot was available). The
recovery inputs are the **preserved quarantine** and the **most recent good
snapshot**. On the 2026-07-04 host the pre-reset assets were copied out of the
live tree to `~/simard-memory-recovery/` — prefer those originals when present.

> **Do not** run these steps against a live daemon. Restore/import is a write
> path; stop the daemon first so nothing writes concurrently.

### 1. Stop the daemon and snapshot current state

```bash
sudo systemctl stop simard-ooda
simard memory stats --json | tee /tmp/memory-before-restore.json   # record the near-empty count
```

### 2. Locate the newest good snapshot

Verified backups are timestamped, newest-first by directory name. Each holds a
`cognitive_snapshot.json`.

```bash
# Newest verified-backup snapshots (host default root):
ls -1dt ~/.simard/backups/*/ | head -5
# Preserved recovery copies from the incident, if present:
ls -1dt ~/simard-memory-recovery/*/ 2>/dev/null | head -5

# Pick the newest snapshot whose item count is non-trivial:
for d in $(ls -1dt ~/.simard/backups/*/); do
  f="$d/cognitive_snapshot.json"
  [ -f "$f" ] && echo "$f  facts=$(grep -c '"concept"' "$f")"
done | head
```

Choose the newest snapshot with a healthy count. Post-#2550 snapshots also carry
`episodes` and `prospective`/triggers, so a restore from one of those round-trips
every durable memory node; older snapshots restore facts + procedures only.

### 3. Import the snapshot

`simard memory import` ingests a `cognitive_snapshot.json` back into the store.
It is **idempotent** — re-running it or importing onto a store that already
holds some of the records deduplicates by content, so it is safe to run more
than once and safe to run onto a partially-populated store.

```bash
simard memory import ~/.simard/backups/<TS>/cognitive_snapshot.json
# → imported N items (M new, K deduplicated)
```

If the live store still holds partial/corrupt data and you want a clean target,
move it aside first so the import lands on an empty store (the quarantine is
preserved for forensics either way):

```bash
mv ~/.simard/cognitive ~/.simard/cognitive.prerestore-$(date +%Y%m%d_%H%M%S)
simard memory import ~/.simard/backups/<TS>/cognitive_snapshot.json
```

### 4. Verify and restart

```bash
simard memory stats            # confirm counts recovered to the expected range
sudo systemctl start simard-ooda
journalctl -u simard-ooda -n 50
```

Confirm `simard memory stats --json` reports a `counts.total` consistent with
the snapshot you imported, then confirm the daemon came up cleanly with no fresh
corrupt-WAL lines.

---

## Preserve the recovery assets

Fix 4 keeps the inputs this runbook depends on, but if you are recovering on an
older build, **copy them out before anything sweeps them**:

```bash
mkdir -p ~/simard-memory-recovery/$(date +%Y%m%d_%H%M%S)
cp -a ~/.simard/cognitive.corrupt-* ~/simard-memory-recovery/$(date +%Y%m%d_%H%M%S)/ 2>/dev/null || true
cp -a ~/.simard/backups            ~/simard-memory-recovery/$(date +%Y%m%d_%H%M%S)/ 2>/dev/null || true
```

Do **not** run `simard cleanup` until you have recovered or copied out the
quarantine and the newest snapshots — cleanup applies the age/count caps to
`cognitive.corrupt-*` quarantines and to backups (see
[Bounded corrupt-quarantine retention](verified-backups.md#bounded-corrupt-quarantine-retention)).
Post-#2550 the retention floors keep enough generations to recover from, but the
caps still eventually reclaim them.

---

## Prevention checklist

- **Keep periodic verified backups healthy.** Confirm a fresh
  `~/.simard/backups/<ts>/` appears on the daemon's backup cadence
  (`SIMARD_BACKUP_INTERVAL_SECS`, default daily) — see
  [Verified Backups](verified-backups.md). A snapshot you never wrote cannot be
  restored.
- **Watch for repeated corruption.** A `cognitive.corrupt-*` quarantine every
  cycle is a hardware/filesystem signal, not just cleanup fodder — capture a
  sample and file a `durability` issue.
- **Do not delete quarantines during an active incident.** They are the only
  forensic copy of the pre-reset store.
- **Prefer `simard memory import` over hand-editing store files.** Import is
  idempotent and content-verified; raw file copies of lbug stores are not a
  supported restore path.

---

## Verification coverage (issue #2550)

The fix ships with regression tests that pin each guarantee (no sleeps, no
network — fakes + temp dirs only):

| Guarantee | Test intent |
|---|---|
| Durable prefix-recovery | A store recovered from a corrupt WAL survives a **re-open**: the recovered records are still present and the store is **not** reset. |
| `memory import` round-trip | Importing a `cognitive_snapshot.json` restores its items; a second import deduplicates (idempotent). |
| Startup auto-restore | With an **empty** live store **and** a newer non-empty snapshot, daemon startup restores from the newest good snapshot and logs it; a store holding any memory is left untouched. |
| Snapshot completeness | The exported snapshot includes episodes and prospective/triggers, not just facts + procedures. |

---

## See Also

- [Cognitive Memory Durability](cognitive-memory-durability.md) — SIGTERM-safe
  shutdown, WAL/CHECKPOINT model, historical incidents
- [Verified Backups of the Live Cognitive Store](verified-backups.md) —
  verify-before-prune, whole-store export, bounded quarantines
- [Memory introspection CLI](../reference/simard-memory-cli.md) —
  `simard memory stats` / `dump` / `import`
- [Cognitive Memory Library Adapter](../architecture/cognitive-memory-library-adapter.md)
  — the `amplihack-memory` backend and `lbug_store`
- [Operations index](index.md)
- [GitHub issue #2550](https://github.com/rysweet/Simard/issues/2550) — this fix
- [GitHub issue #2420](https://github.com/rysweet/Simard/issues/2420) — verified
  backups + bounded quarantines
- [GitHub issue #2307](https://github.com/rysweet/Simard/issues/2307) — native
  cognitive-memory fork deletion (library backend)
