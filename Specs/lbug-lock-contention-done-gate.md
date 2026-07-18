# lbug Lock-Contention Never Wipes Memory — Done-Gate Specification

## Purpose

The goal **"stop lbug lock contention from being mistaken for catalog
corruption"** (slug `stop-lbug-lock-contention-from-being-mistaken-f-0ebf1bc7`)
stayed `Blocked` cycle after cycle with the same diagnosis: **no tracked
PR/issue the done-gate could verify** (why = `UNCLEAR-CRITERIA`). The blocker was
**not** technical — the fix already shipped in **merged PR #4317** ("serialize
opens so lock-contention never wipes memory"). The blocker was that the goal's
finish condition had **no machine-checkable definition**, so every cycle
re-observed it as unfinished and produced `NO ACTION`.

This spec fixes that WHY. It makes the done-criteria **measurable** by binding
the goal's finish condition to a **single command a daemon can run and score
automatically**:

```
scripts/check-lbug-lock-contention-done-gate.sh
```

The command exits `0` only when the exact regression tests that prove a
contended open **fails loud instead of wiping records** still pass; otherwise it
exits non-zero and prints the failing check. This turns "stop lbug lock
contention from being mistaken for corruption" from a prose judgement into a
check the done-gate can confirm — and, because this is a **standing
regression-protection goal**, keeps confirming it every cycle rather than
closing it out and losing the guarantee.

## What the problem was

`lbug` (the `amplihack-memory-lib` storage engine) mis-classified a transient
cross-process file-lock conflict (`"Lock is held by PID N"`) as **catalog
corruption**: it quarantined the store to `cognitive.corrupt-<ts>` and rebuilt
it **empty**. A second process opening a live store therefore destroyed all
cognitive memory — 57 such quarantines were observed on the daemon's main store.
The lock-conflict-as-corruption mis-read looked exactly like data corruption,
which is why the goal is phrased "stop lock contention from being **mistaken
for**" corruption.

## What fixed it (merged PR #4317)

The mis-classification lives in the external library (out of scope). PR #4317
closed the door at Simard's **own open seam**:

| Layer | Location |
|-------|----------|
| Cross-process open guard (advisory `flock` + fail-loud, bounded backoff) | [`src/cognitive_memory/open_guard.rs`](../src/cognitive_memory/open_guard.rs) |
| Guard wired into the open seam | [`src/cognitive_memory/library_adapter.rs`](../src/cognitive_memory/library_adapter.rs) `LibraryCognitiveMemory::open` / `in_memory` |
| Regression: two concurrent opens of one path never wipe records | [`src/cognitive_memory/tests_library_parity.rs`](../src/cognitive_memory/tests_library_parity.rs) `lock_contention_no_wipe::concurrent_open_of_same_path_never_wipes_records` |
| Guard unit tests (fail-loud, re-entrancy, registry cleanup) | [`src/cognitive_memory/open_guard.rs`](../src/cognitive_memory/open_guard.rs) `tests` |
| Outside-in qa scenario | [`tests/qa-scenarios/cognitive-memory-open-lock-contention-no-wipe.yaml`](../tests/qa-scenarios/cognitive-memory-open-lock-contention-no-wipe.yaml) |
| Reference doc | [`docs/reference/cognitive-memory-open-serialization.md`](../docs/reference/cognitive-memory-open-serialization.md) |

A contended open now **fails loud** (`PersistentStoreIo`) instead of proceeding
into lbug's destructive rebuild; same-process opens stay re-entrant.

## Measurable done-criteria

The goal is DONE when every criterion below passes. Each is asserted by an
existing test shipped in merged PR #4317, re-run by
`scripts/check-lbug-lock-contention-done-gate.sh`.

| ID | Criterion | Checked by |
|----|-----------|-----------|
| LC-1 | **fails-loud-when-contended** — a contended open returns an error instead of rebuilding the store | `open_guard::tests::contended_by_foreign_holder_fails_loud_within_budget` |
| LC-2 | **acquire/release round-trip** — the guard acquires and releases the sidecar open-lock cleanly | `open_guard::tests::acquire_and_release_roundtrip` |
| LC-3 | **same-process re-entrant** — a second same-process open does not block (two-live-handle parity preserved) | `open_guard::tests::same_process_reentrant_acquire_does_not_block` |
| LC-4 | **concurrent same-process cold opens** — racing cold opens in one process all succeed | `open_guard::tests::concurrent_cold_open_race_all_succeed_same_process` |
| LC-5 | **registry cleanup** — the process-global registry entry clears after the last guard drops | `open_guard::tests::registry_entry_cleared_after_last_guard_drops` |
| LC-6 | **never-wipes-records regression** — two concurrent opens of one path leave the winner's records intact and produce zero quarantines | `tests_library_parity::lock_contention_no_wipe::concurrent_open_of_same_path_never_wipes_records` |

## Definition of "done" (the done-gate)

The goal is **done** when this single command exits `0`:

```
scripts/check-lbug-lock-contention-done-gate.sh
```

It re-asserts the LC-* criteria above via `cargo test`. This is the concrete
artifact the goal's done-criteria points at — the done-gate can run it every
cycle and certify the goal as long as lock-contention can no longer be mistaken
for corruption. Optionally, `--full` additionally confirms the outside-in qa
scenario asset is present.

Because the guard protects against a **recurring** failure mode, this is a
standing goal: the gate is expected to stay green, and turns red the instant the
open-serialization protection regresses.

## Progress log

- **2026-07-18** — Bound the goal's finish condition to the machine-checkable
  regression tests delivered by merged PR #4317 via
  `scripts/check-lbug-lock-contention-done-gate.sh`. The open-serialization guard
  is delivered and its tests are green; the done-gate can now observe and certify
  the goal instead of re-stalling on unmeasurable criteria.
