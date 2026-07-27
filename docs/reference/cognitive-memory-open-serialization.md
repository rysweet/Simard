# Cognitive-Memory Open Serialization (Lock-Contention Safety Net)

## What this protects against

The cognitive store is backed by the upstream `amplihack-memory-lib`
lbug/LadybugDB engine. Its resilient open path
(`CognitiveMemory::open_persistent` → `lbug_store::open_resilient`) treats **any**
failed strict open of the main database as *catalog corruption*: it quarantines
the store to `<path>/cognitive.corrupt-<unix_ts>` and rebuilds a **fresh, empty**
store (`recovered_records = 0`).

That self-heal is correct for a genuinely unopenable catalog. It is **wrong** for
a *transient lock conflict*. lbug takes a POSIX/PID lock on the store, so a
**second process** opening a store that a **first process already holds open**
fails to open with:

```
Could not set lock on file: <state_root>/cognitive (Lock is held by PID N)
```

The library does not distinguish that lock-held error from real corruption, so
the second opener quarantines the live database and rebuilds it empty —
**destroying all cognitive memory**. This produced dozens of
`cognitive.corrupt-*` quarantines on the daemon main store and corrupted
short-lived engineer/probe stores that shared one path.

The mis-classification itself lives in the library. Simard cannot change it from
here, but it **can** stop the library from ever seeing a concurrent open on the
same path — which is what this safety net does.

## How it works

`cognitive_memory::open_guard::CognitiveOpenGuard` serializes opens at Simard's
`LibraryCognitiveMemory::open` seam, **before** the library is touched.

1. **Sidecar advisory `flock`.** Before opening the store, the guard takes an
   exclusive `flock` on `<state_root>/cognitive.open.lock` — a sibling of the
   `cognitive` store directory (lbug never touches it). The file is created if
   absent and is intentionally left on disk (unlinking it would let a concurrent
   opener `flock` a different inode and defeat the mutual exclusion).

2. **Bounded exponential backoff.** Acquisition retries with jittered
   exponential backoff up to a budget (default **15 s**, overridable via
   `SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS`, primarily for tests).

   * **Acquired** → proceed to `open_persistent`. A near-simultaneous open race
     simply waits a few milliseconds and then proceeds.
   * **Budget expires while another live process still holds it** → **fail loud**
     with `SimardError::PersistentStoreIo { action: "acquire_open_lock", … }`.
     Returning an error here is the whole point: it stops the caller from
     entering the library's lock-conflict-as-corruption rebuild. **Failing loud
     is strictly better than silently wiping.**

3. **Held for the handle lifetime, released last.** The guard is held for the
   lifetime of the `LibraryCognitiveMemory` handle and is dropped *after* the
   inner store closes, so no other process can slip in while lbug is still
   releasing its own PID lock. `flock` is released automatically by the kernel
   if the holding process dies, so a crashed holder never wedges the store (no
   manual stale-lock reaping required).

4. **Same-process re-entrancy.** lbug's PID lock is re-entrant within one
   process (a daemon writer and a same-process reader view can both be live).
   `flock`, by contrast, blocks a same-process second open. To preserve the
   library's semantics, the guard keeps a **process-global registry** keyed by
   the canonical lock path: the first open in a process takes the real `flock`;
   concurrent same-process opens of the same path share it via a
   reference-counted handle (no second `flock`, no wait). The registry
   check and the `flock` attempt are performed atomically under one mutex, so a
   cold-open race between threads can never make a loser spin to a false failure.

## What you will see

* **Normal operation:** nothing. The lock file exists as a zero/one-line
  `pid=…` marker; opens succeed immediately.
* **Contention (a second opener of a live store):** the open fails with a clear
  error naming the holding PID and advising the correct remedies — route access
  through the daemon IPC, or use an isolated state root for the run. **No**
  `cognitive.corrupt-*` quarantine is created and **no** memory is lost.

Legitimate cross-process access already routes through the daemon over its Unix
socket (`memory_ipc` launcher tiers), so a direct second open of a live store is
exactly the hazardous case this guard converts from *silent wipe* into *loud,
recoverable failure*.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_COGNITIVE_OPEN_LOCK_TIMEOUT_MS` | `15000` | Max backoff budget (ms) before a contended open fails loud. Mainly a test knob. |

## Related

- [Engineer Cognitive-Access Degradation](./engineer-cognitive-access-degradation-api.md)
  — how concurrent OODA engineers avoid *reaching* a contended open (IPC shared
  read) and degrade gracefully to deferred/read-only cognition instead of
  hard-exiting when they do.
- [Cognitive Memory — Library Adapter](../architecture/cognitive-memory-library-adapter.md)
- [Cognitive-Memory Durability](../operations/cognitive-memory-durability.md)
- [Cognitive-Memory WAL Recovery Runbook](../operations/cognitive-memory-wal-recovery-runbook.md)
- [serial(cognitive_memory) Isolation](../testing/cognitive-memory-serial-isolation.md)

## Verification

Covered by the qa-team scenario
`tests/qa-scenarios/cognitive-memory-open-lock-contention-no-wipe.yaml`
(validated with `gadugi-test validate`, run with `gadugi-test run`), which drives
these hermetic tempdir unit tests:

- `cognitive_memory::tests_library_parity::lock_contention_no_wipe::concurrent_open_of_same_path_never_wipes_records`
  — end-to-end: a contended open fails loud, creates no quarantine, and the
  winner's record survives.
- `cognitive_memory::open_guard::tests::contended_by_foreign_holder_fails_loud_within_budget`
- `cognitive_memory::open_guard::tests::same_process_reentrant_acquire_does_not_block`
- `cognitive_memory::open_guard::tests::concurrent_cold_open_race_all_succeed_same_process`
