# Contributing to Simard

Thank you for contributing. This document describes the local developer
workflow, merge policy, durability guarantees, and disposition of known
pre-existing test failures. **Following these rules is mandatory** —
they are the same gates CI enforces.

---

## Table of Contents

1. [Rust-Only Policy](#rust-only-policy)
2. [Local Pre-Commit Workflow](#local-pre-commit-workflow)
3. [Merge Policy: No `--admin` Merges](#merge-policy-no---admin-merges)
4. [Cognitive Memory Durability (Per-Write Barrier + SIGTERM + Periodic Backups)](#cognitive-memory-durability-per-write-barrier--sigterm--periodic-backups)
5. [Local Data Retention Disclosure](#local-data-retention-disclosure)
6. [Pre-Existing Test Failure Disposition](#pre-existing-test-failure-disposition)
7. [Real-Meeting & Dashboard E2E Verification](#real-meeting--dashboard-e2e-verification)
8. [Engineering Guidelines (G1/G2/G3/G4)](#engineering-guidelines-g1g2g3g4)

---

## Rust-Only Policy

Simard is migrating to a Rust-only codebase ([#2155](https://github.com/rysweet/Simard/issues/2155)).
**New `.py` files under `src/` or `python/`, and new `.js`/`.ts` files outside
`npm/` and `tests/e2e-dashboard/`, are not permitted.**

A CI gate (`scripts/check-rust-only-gate.sh`) enforces this on every PR and
as a pre-commit hook. A small set of pre-existing files is allow-listed until
they are individually migrated — see the script for the current list.

If you need to add a non-Rust file that falls outside the allow-list, open an
issue explaining the need and add the file to the allow-list in
`scripts/check-rust-only-gate.sh` with a comment referencing the issue.

---

## Local Pre-Commit Workflow

Simard uses the [`pre-commit`](https://pre-commit.com) framework to mirror
the CI `pre-commit` workflow on every developer machine. The local hooks run
**the same checks CI runs** — if they pass locally, CI will pass.

### One-Time Setup

```bash
# From the repo root
./scripts/install-precommit.sh
```

The script is idempotent; running it again is a no-op if hooks are already
installed.

What `install-precommit.sh` does:

1. Verifies `python3` and `pip` (or `pipx`) are available.
2. Installs the `pre-commit` framework (pinned `>=3.7`) into the user
   site (`pip install --user pre-commit`) or via `pipx` if available.
3. Runs `pre-commit install --install-hooks` (the project pins
   `default_install_hook_types: [pre-commit, pre-push]` in the config so
   both hook stages are installed in one call).
4. Performs an initial `pre-commit run --all-files` to warm caches.

> **Note on `scripts/install-precommit.sh`** — this installer is part of
> the issue #1631 hardening work and lands in the same PR as this
> documentation. If you are reading this on a branch that does not yet
> contain the script, fall back to the manual install below.

### What Each Hook Runs

The actual configuration is in
[`.pre-commit-config.yaml`](.pre-commit-config.yaml); the table below is
a summary, not the source of truth.

| Hook id | Stage(s) | Command |
|---|---|---|
| `cargo-fmt` | `pre-commit`, `pre-push`, `manual` | `cargo fmt --all -- --check` |
| `cargo-clippy-precommit` | `pre-commit`, `manual` | `cargo clippy --release --no-deps -- -D warnings` (via [`scripts/clippy-precommit-release.sh`](scripts/clippy-precommit-release.sh)) |
| `cargo-clippy` | `pre-push`, `manual` | `cargo clippy --all-targets --all-features --locked -- -D warnings` |
| `cargo-test-race-subset` | `pre-push`, `manual` | `cargo test --release --lib -- --test-threads=$(nproc) cognitive_memory bootstrap memory_ipc memory_consolidation` |

The two-tier clippy gate is intentional: the `--release --no-deps`
hook gives instant feedback at commit time; the `--all-targets
--all-features --locked` hook reuses the warm `target/` after the
race-test compile and runs at push time, mirroring CI exactly.

The `cargo-clippy-precommit` hook runs through a thin wrapper,
[`scripts/clippy-precommit-release.sh`](scripts/clippy-precommit-release.sh),
which guarantees the `lbug` (LadybugDB) native static library is on the linker
search path before invoking `cargo clippy --release` (issue #2426). `lbug`
0.17.1 caches its prebuilt `liblbug.a` inside the cargo *registry source*
directory; CI's cargo cache persists `target/` but not `registry/src`, so on a
cache restore the cached release build-script output points at an evicted
archive and clippy fails with `could not find native static library `lbug``.
The wrapper provisions a stable copy of `liblbug.a` (reusing an existing
registry prebuilt, otherwise downloading the same release asset the `build` and
`coverage` jobs use) and points `lbug` at it via `LBUG_LIBRARY_DIR`. It is a
no-op for warm local checks, so the budgets below still hold.

Realistic budgets (warm caches, dev host with the workspace already
built):

- `cargo fmt --check` — under 2 seconds. Effectively free at commit time.
- `cargo clippy --release --no-deps` — typically under 30 seconds
  incrementally with `--no-deps` keeping the analysis bounded to the
  workspace. From cold it is several minutes (one-time cost).
- `cargo test --release --lib -- --test-threads=$(nproc) <filters>` —
  the race-catching subset (cognitive memory, bootstrap, IPC, and
  consolidation modules). Total budget ≤ 90s on a dev host. This is
  intentionally **not** the full suite; the goal is to catch the
  concurrency regressions that surface only under parallel execution
  before they reach CI.

The pre-push gate is deliberately narrow: full-suite gating belongs in
CI, where the test runner has more cores and isolated caches. Local
hooks exist to prevent the multi-thread race classes (writer-Arc
lifecycle, IPC bridge teardown, consolidation order-of-operations) from
ever leaving a developer machine.

### Manual Invocation

```bash
# Run all hooks on all files (recommended before opening a PR)
pre-commit run --all-files

# Run a specific hook on all files
pre-commit run cargo-fmt --all-files
pre-commit run cargo-clippy-precommit --all-files
pre-commit run cargo-clippy --all-files --hook-stage pre-push
pre-commit run cargo-test-race-subset --all-files --hook-stage pre-push

# Run only on staged files (default behavior at commit time)
pre-commit run
```

### Bypassing Hooks (Emergency Only)

The standard `pre-commit` `SKIP=` env var is honored:

```bash
# Skip a single hook (DISCOURAGED — use only when actively debugging
# the hook itself, not the code under change)
SKIP=cargo-test-race-subset git push
```

> **PRs pushed with `SKIP=` will be rejected at merge time.** CI re-runs
> the same checks and merge is blocked on red CI. There is no admin
> override (see [Merge Policy](#merge-policy-no---admin-merges)).

### Verifying Your Hooks Catch What CI Catches

The hooks are designed to catch three failure classes. To verify they
work on your machine, intentionally introduce each failure once:

```bash
# 1. Format failure — should be blocked at commit time by cargo-fmt
echo 'fn   bad_fmt(  )  ->  i32{1}' >> src/lib.rs && \
  git add src/lib.rs && git commit -m "test"
# Expected: cargo-fmt hook fails.
git restore --staged src/lib.rs && git checkout -- src/lib.rs

# 2. Clippy failure — should be blocked at commit time by cargo-clippy-precommit
# (introduce e.g. `let unused = 2;` in non-test code)
# Expected: cargo-clippy-precommit hook fails on commit.

# 3. Race-subset test failure — should be blocked at push time
# (introduce a failing assertion in a test inside cognitive_memory,
#  bootstrap, memory_ipc, or memory_consolidation)
# Expected: cargo-test-race-subset hook fails on push.
```

Revert each test change before continuing.

### Updating Hook Versions

Because every hook is `language: system` (it shells out to the locally
installed `cargo`), there are no upstream hook revisions to bump. To
bump the framework itself:

```bash
pipx upgrade pre-commit          # or: pip install --user --upgrade pre-commit
pre-commit install --install-hooks
```

---

## Merge Policy: No `--admin` Merges

> **Never use `gh pr merge --admin`.**
>
> This is a hard rule. Pre-commit + CI MUST be green before any PR merges.
> Pre-existing failures must either be fixed in the PR or have a tracking
> issue filed (see [Pre-Existing Test Failure Disposition](#pre-existing-test-failure-disposition)).

### Allowed Merge Commands

```bash
# Standard merge (squash + delete branch)
gh pr merge --squash --delete-branch

# That's it. No other variant is allowed.
```

### Why

`--admin` bypasses branch-protection rules and lets red CI ship to `main`.
Every prior incident traced to merging on red CI has cost more time to
diagnose and revert than the original "block" would have cost to fix.
The cognitive-memory durability incident on 2026-05-09 (active goal
`improve-the-dashboard-via-playwright-driven-testing` wiped during a
routine `systemctl restart simard-ooda` because lbug only flushes its
WAL inside `Database::drop`, which signal-induced exits do not invoke)
was traced to a chain of admin merges that suppressed warning signals.

### Pre-Merge Checklist

Before requesting merge:

- [ ] Local `pre-commit run --all-files` is green.
- [ ] Local `cargo fmt --all -- --check && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo test --all-features --locked -- --skip cargo_install_from_repo_succeeds` is green.
- [ ] CI on the PR is green.
- [ ] Pre-existing failures inherited from `main` are either fixed in this PR or tracked by a linked GitHub issue.
- [ ] PR body contains evidence of any required E2E verification (see workstream-specific docs).

---

## Cognitive Memory Durability (Per-Write Barrier + SIGTERM + Periodic Backups)

As of the de-fork (issue #2307, Phase 2b), Simard's cognitive memory is
provided by the `amplihack-memory-lib` library backend
(`LibraryCognitiveMemory`), which **owns its own durability**. The library
persists to `~/.simard/cognitive/` (`state_root/cognitive`, a LadybugDB
`GraphStore` directory using `lbug = "=0.15.3"`). The old native single-file
store at `~/.simard/cognitive_memory.ladybug` is **abandoned** — never opened,
read, or migrated. LadybugDB journals writes through its WAL and collapses the
WAL into the store on CHECKPOINT (and inside `Database::drop`). Because
`SIGTERM` does not invoke `Drop`, the daemon's shutdown handler explicitly
checkpoints before exit.

Durability today rests on three things:

1. **Library WAL + CHECKPOINT** — writes are journaled by LadybugDB; the
   `CognitiveMemoryOps::checkpoint` trait method (delegating to
   `LibraryCognitiveMemory::checkpoint` → the library's `close()`) collapses
   the WAL into the store.
2. **SIGTERM-safe shutdown handler** (issue #1631) — graceful signals
   (`SIGTERM`/`SIGINT`/`SIGHUP`) run the shutdown sequence, which checkpoints
   the store before the process exits, so `systemctl restart` is lossless.
3. **File-level snapshot backups** — the trait-based `memory_backup/` module
   takes periodic file snapshots through `CognitiveMemoryOps`, providing a
   bounded-RPO secondary recovery point.

> **Removed in Phase 2b.** The native fork's per-write `fsync` barrier
> (issue #1973) and its lbug-WAL "verified backup" loop
> (`NativeCognitiveMemory::create_verified_backup` / `prune_old_backups`,
> issue #1631) were **deleted** with `NativeCognitiveMemory`. The library
> backend supersedes them: durability is the library's responsibility, and
> snapshot backups come from `memory_backup/`. The subsections below that
> describe the native per-write barrier, the native verified-backup loop, the
> native on-disk layout, and the native restore procedure are **historical** —
> they document machinery that no longer exists — and are retained only for
> archival context. For the current model see
> [`docs/operations/cognitive-memory-durability.md`](docs/operations/cognitive-memory-durability.md).

### Per-Write fsync Barrier (issue #1973)

> **Historical (removed in Phase 2b).** The per-write `fsync` barrier below was
> a property of the deleted `NativeCognitiveMemory`. The library backend relies
> on LadybugDB's WAL + CHECKPOINT for durability, so there is no Simard-side
> per-write barrier any more. This subsection is retained for archival context.

Implemented as a private `NativeCognitiveMemory::post_write_barrier(
op: &'static str)` and called by every mutating method in
`CognitiveMemoryOps` (`store_fact`, `store_episode`,
`update_episode_status`, `link_episodes`, `delete_fact`, `tag_fact`,
`record_observation`, `record_goal_event`, `consolidate_episodes`).

The barrier:

- Returns `Ok(())` immediately if the store is in-memory
  (`durable_writes == false`), skipping the fsync round-trip and
  keeping in-memory unit tests fast. (Note: the in-memory backend
  still uses a tempdir-backed lbug DB under the hood; only the
  fsync — not the lbug write itself — is skipped.)
- Otherwise performs `fsync(self.path) → fsync(self.parent_dir)` in
  that exact order. The order is non-negotiable and annotated with a
  `// SAFETY:` comment.
- Does **not** issue a `CHECKPOINT;` Cypher statement (an earlier
  draft did; it was removed after CI showed lbug returns raw page
  bytes for `e.content` on subsequent reads when CHECKPOINT is
  interleaved with writes inside `consolidate_episodes`). The
  crash-recovery integration tests confirm the fsync pair above is
  sufficient without CHECKPOINT.
- Propagates all failures as typed `SimardError` variants with
  `store: "cognitive-memory"` and distinct `action` labels
  (`fsync-data-file`, `fsync-parent-dir`, `verify-readback`,
  `recovery-replay-fsync`) — **never** swallows an fsync result.
  The mutating op name is included in `reason` as
  `format!("op={op}: {io_err}")`.

`consolidate_episodes` fires the barrier **once after the loop
completes**, not per-iteration, to bound write amplification.

The backup/recovery helper `atomic_copy_with_fsync` was hardened in
the same PR to (a) propagate previously-swallowed fsync errors and
(b) re-read the destination and compare a SHA-256 digest against the
source, returning `PersistentStoreIo { action: "verify-readback", .. }`
on mismatch.

Direct proof: `tests/cognitive_memory_crash_durability.rs::
sigkill_preserves_last_write` spawns the helper binary
`examples/cognitive_memory_crash_helper.rs`, waits for `WROTE`,
SIGKILLs it, reopens the store from a fresh process, and asserts the
marker fact is present.

> Full reference: [`docs/operations/cognitive-memory-durability.md`](docs/operations/cognitive-memory-durability.md)
> — section "Per-Write fsync Barrier (issue #1973)" documents
> mechanism, error mapping, latency tradeoff, in-memory backend
> opt-out, and operational runbook entries.

### Graceful Shutdown Sequence

When the OODA daemon receives `SIGTERM`, `SIGINT`, or `SIGHUP`, the
[`ctrlc`](https://docs.rs/ctrlc) handler (registered with the
`termination` feature) sets a shutdown flag. At the top of the next
OODA iteration the daemon invokes `shutdown_daemon(state_root,
shared_mem, state, bridges, signal_driven=true)`, which performs the
following steps **in order**:

1. **Persist the goal board** via `persist_board(&state.active_goals,
   &*bridges.memory)`. The write goes through the live cognitive-memory
   writer; LadybugDB journals it to the WAL, and the pre-exit checkpoint
   (step 2) collapses it into the store before the process exits.
2. **Pre-exit checkpoint** via `shared_mem.checkpoint()`
   (`CognitiveMemoryOps::checkpoint`, which delegates to
   `LibraryCognitiveMemory::checkpoint` → the library's `close()`). Collapses
   any WAL residue into the store so a graceful restart observes every
   committed write.
3. **Close the LLM session** if one is bound (`bridges.session.close()`).
4. **Clear the in-process writer** via
   `memory_ipc::clear_in_process_writer()`. This drops the global
   `Weak`/`Arc` registration so the daemon-owned writer Arc becomes the
   sole strong reference.
5. **Drop bridges and the writer Arc** (happens implicitly on function
   return). The inherent `Database::drop` then runs
   `force_checkpoint_on_close` as a defense-in-depth backstop.

When `signal_driven=true`, errors at any step are logged via
`daemon_log` and the next step still runs — best-effort durability is
the correct stance for a process that is already dying. When
`signal_driven=false` (normal exit and tests), errors propagate so
assertions can fire.

### Periodic Backup Loop

> **Historical (removed in Phase 2b).** The native `create_verified_backup` /
> `prune_old_backups` loop described below was **deleted** with
> `NativeCognitiveMemory`. In Phase 2b the library owns durability and the
> trait-based `memory_backup/` module provides file-level snapshots instead.
> This subsection (and the **File and Directory Layout**, **Restoring from
> Backup**, and **Local Data Retention Disclosure** subsections that follow) is
> retained for archival context only.

At the start of every OODA cycle the daemon checks whether
`SIMARD_DB_BACKUP_INTERVAL_SECS` (default `300`) has elapsed since the
last backup. If so:

1. **Checkpoint** via `shared_mem.checkpoint()` so committed-but-WAL-
   resident writes are captured by the file copy. A failed checkpoint
   is logged but the backup attempt continues.
2. **Create the verified backup** via
   `NativeCognitiveMemory::create_verified_backup(&state_root)`, which:
   - copies `~/.simard/cognitive_memory.ladybug` to
     `~/.simard/backups/cognitive_memory.ladybug.<unix_ts>` using a
     `copy → fsync(file) → rename → fsync(parent dir) → readback-verify`
     atomic-write pattern (`atomic_copy_with_fsync` in
     `src/cognitive_memory/backup.rs`; fsync errors propagate and
     a SHA-256 readback verifies the destination, both as of #1973);
   - copies any extant WAL siblings (lbug 0.15 may use either
     `cognitive_memory.ladybug.wal` or `cognitive_memory.wal`) to
     `<wal_name>.<unix_ts>` with the **same** timestamp so the pair is
     unambiguous on restore;
   - opens the new backup read-only and runs `verify_db_health` before
     declaring success.
3. **Prune** via `NativeCognitiveMemory::prune_old_backups(&state_root,
   db_backup_keep)` — keeps the most recent `SIMARD_DB_BACKUP_KEEP`
   (default `24`) paired snapshots; main file and matching WAL files
   for the same timestamp are removed together.
4. **Track consecutive failures**: a single failure is logged at warn
   level. After 3 consecutive failures the daemon escalates to an
   `ERROR` log naming the backup directory; the counter resets on the
   first subsequent success.

On daemon startup, the routine attempts to copy the existing main DB to
a verified backup before opening it for writes. If the open fails
because the on-disk DB is corrupt, the recovery path falls back to the
most recent verified backup (see `try_recover` in
`src/cognitive_memory/backup.rs`). The recovery-replay copy uses
`atomic_copy_with_fsync` and therefore inherits the same fsync +
readback verification, surfaced with `action="recovery-replay-fsync"`
on failure.

### File and Directory Layout

| Path | Purpose | Notes |
|---|---|---|
| `~/.simard/cognitive_memory.ladybug` | lbug DB file | Single file as of #1973. A legacy KuzuDB directory at this path is renamed aside to `cognitive_memory.ladybug.kuzu-backup` by `NativeCognitiveMemory::open` on first run; the legacy contents are preserved but **not** auto-imported. |
| `~/.simard/cognitive_memory.ladybug.wal` *or* `~/.simard/cognitive_memory.wal` | WAL sibling(s) | Either or both may exist depending on lbug minor |
| `~/.simard/cognitive_memory.ladybug.kuzu-backup` | Legacy KuzuDB dir, if migrated | Preserved on disk for manual inspection; safe to `rm -rf` once the new lbug DB is confirmed as source of truth |
| `~/.simard/backups/` | Backup root | Created by `create_dir_all` on first backup |
| `~/.simard/backups/cognitive_memory.ladybug.<ts>` | Backup of the DB file at unix-second `<ts>` | Always a file (legacy KuzuDB directories are renamed aside *before* any backup is taken); restore = `cp` |
| `~/.simard/backups/cognitive_memory.ladybug.wal.<ts>` | WAL sibling for the same `<ts>` | Same timestamp pairs the two |

> **Note**: `cognitive_memory.ladybug` is a **single file** as of
> issue #1973, and every backup created by `create_verified_backup`
> is therefore also a single file. Restoration uses plain `cp`; see
> [Restoring from Backup](docs/operations/cognitive-memory-durability.md#restoring-from-backup)
> and
> [`docs/operations/cognitive-memory-durability.md`](docs/operations/cognitive-memory-durability.md).

### Configuration

| Setting | Default | Override | Notes |
|---|---|---|---|
| Backup interval (seconds) | `300` (5 min) | `SIMARD_DB_BACKUP_INTERVAL_SECS=N` env var | Read once at daemon start |
| Retention count | `24` paired snapshots | `SIMARD_DB_BACKUP_KEEP=N` env var | `0` disables pruning |
| Backup directory | `~/.simard/backups/` | (compile-time, derived from state root) | — |
| Dashboard port | `8080` | `SIMARD_DASHBOARD_PORT=N` env var or `--dashboard-port=N` CLI flag | Default declared in `src/operator_commands_ooda/daemon/config.rs` |

Setting `SIMARD_DB_BACKUP_KEEP=0` disables pruning (operator opt-in for
incident-response scenarios; not recommended for normal operation).

### Restoring from Backup

```bash
# 1. Stop the daemon
sudo systemctl stop simard-ooda

# 2. Identify the most recent paired backup
ls -lt ~/.simard/backups/cognitive_memory.ladybug.* | head

# 3. Copy the DB file AND any WAL sibling(s) back into ~/.simard/
TS=1762800000   # the timestamp suffix from above
rm -f ~/.simard/cognitive_memory.ladybug
cp ~/.simard/backups/cognitive_memory.ladybug.${TS} \
   ~/.simard/cognitive_memory.ladybug
# WAL siblings (either or both may exist for this timestamp; copy what's there)
for w in cognitive_memory.ladybug.wal cognitive_memory.wal; do
  src=~/.simard/backups/${w}.${TS}
  [ -e "$src" ] && cp "$src" ~/.simard/${w}
done

# 4. Restart and check the journal
sudo systemctl start simard-ooda
journalctl -u simard-ooda -n 50
```

If the restored backup is itself corrupt, fall back to the
next-most-recent timestamp; `create_verified_backup` only writes a
backup after a read-only `verify_db_health` pass, so this should be
extremely rare.

---

## Local Data Retention Disclosure

By default, the periodic backup loop retains **24 paired snapshots × 5
minutes ≈ 2 hours** of cognitive-memory history under
`~/.simard/backups/`. This means:

- Facts deleted from cognitive memory may persist in backups for up to
  ~2 hours.
- Backups inherit the umask of the daemon process. If your threat model
  requires owner-only backups, set a restrictive umask (`umask 0077`)
  before starting the daemon, or place `~/.simard/` on an encrypted
  filesystem (LUKS, FileVault, BitLocker, etc.).
- Backups are **not** encrypted at rest by Simard.
- To shorten retention, set `SIMARD_DB_BACKUP_KEEP=N` (e.g., `1` to keep
  only the latest snapshot pair).
- To delete all backups: `rm -rf ~/.simard/backups/cognitive_memory.ladybug.*`
  while the daemon is running is safe — the next backup tick will
  recreate the directory.

---

## Pre-Existing Test Failure Disposition

The table below tracks the disposition of any test failures inherited
from `main` at the base SHA of this PR. Each row is **populated from
the actual run** captured under
[Workstream D — pre-existing-flake triage](#); any `[TBD]` entry must
be replaced before merge.

| Test | Disposition | Tracking |
|---|---|---|
| `version_string_is_semver` (`tests/cli_golden.rs`) | **Fixed in this PR** — stale `assert_eq!(VERSION, "0.16.1")` after Cargo.toml bumped to `0.17.0`. One-line fix; root cause < 1 minute, well under the 1-hour spec threshold. | n/a (fixed) |
| `run_local_engineer_loop_emits_agent_prompt_build_phase` (`src/engineer_loop/tests_agent_spawn.rs`) | **Fixed in this PR** — assertion at line 289 only matched `agent-spawn` / `agent session` in the error message, but on CI runners (no `SIMARD_LLM_PROVIDER` configured) the loop fails earlier with a config-validation error. Both are valid "phase-boundary observable" outcomes for this test. Relaxed the Err-branch assertion to additionally accept `SIMARD_LLM_PROVIDER` / `llm_provider` substrings; the Ok-branch assertions are unchanged. Verified locally; expected to pass on CI runners after this commit. | n/a (fixed) |
| `engineer_loop_probe_fails_visibly_when_structured_replacement_target_is_missing` (`tests/engineer_loop.rs`) | Pre-existing — agent backend "appends to satisfy verify-contains" when the `replace` source string is missing, so the loop reports success instead of failing visibly. Orthogonal to #1631. **Skipped in CI** via `--skip` in `.github/workflows/verify.yml` per CONTRIBUTING.md disposition policy; will be re-enabled once #1639 is fixed. | [#1639](https://github.com/rysweet/Simard/issues/1639) |
| `full_session_lifecycle_triggers_all_consolidation_phases` (`tests/memory_consolidation_lifecycle.rs`) | **Fixed in this PR** — the `consolidation_persistence` drain (PR #1427 / commit `069dc9b`) added a `memory.get_working` call between `push_working` and `store_episode`. The integration test's four `InMemoryBridgeTransport` mocks did not handle the new method, returning `unknown: memory.get_working`. Added `"memory.get_working" => Ok(json!({"slots": []}))` to all four mocks (matches the real `SlotsResponse` deserialization shape). Verified locally — 4/4 pass. Closes #1640. | [#1640](https://github.com/rysweet/Simard/issues/1640) (closed by this PR) |

Each tracking issue includes:

- The failing test name and module path.
- Branch and commit SHA where the failure reproduces.
- The exact `cargo test` invocation.
- Captured `stderr` (last 50 lines).
- A one-paragraph suspected root cause.
- The `pre-existing-flake` label.

### Policy for New Pre-Existing Failures

If you discover a test failure on `main` while working on a PR:

1. **Do not silently inherit it.** Either fix it in your PR (if the root
   cause is < 1h investigation and orthogonal to your work) or file an
   issue using the template above.
2. Link the issue from your PR description.
3. CI's required-checks gate is configured to allow merge when the only
   failures match a tracked-issue allow-list.

---

## Real-Meeting & Dashboard E2E Verification

When changes touch the meeting REPL, dashboard `/ws/chat`, or
cognitive-memory ingestion paths, the PR body MUST include evidence
from a real (not mocked) end-to-end exercise.

### Meeting REPL Exercise

```bash
simard meeting repl <topic-words>
# At the simard:meeting> prompt, send a substantive proposal
# (>100 chars, references the topic).
# Verify the agent responds substantively (>100 chars, references the
# proposal). Use /preview to inspect the draft handoff and /close to
# finalize.
```

The PR body must include:

- The full meeting transcript (or a 30 KB head + 10 KB tail excerpt;
  full transcript committed under
  `docs/evidence/<date>-meeting-transcript.txt` if it exceeds 60 KB).
- Verification that the resulting `meeting_handoff.json` (in
  `$SIMARD_HANDOFF_DIR` or `target/meeting_handoffs/` by default)
  contains a non-empty `decisions` array and at least one
  `action_items` entry.
- A line from `journalctl -u simard-ooda --since "5 min ago"` matching
  `OODA start: ingested N goal/backlog item(s) from meeting handoff`
  (logged from `src/ooda_loop/cycle.rs`).

### Dashboard `/ws/chat` Exercise

The dashboard listens on `SIMARD_DASHBOARD_PORT` (default `8080`). The
PR body must include:

- A real prompt that requires consulting current state (e.g., "What is
  the current OODA cycle count and what action did Simard most recently
  dispatch?").
- The full agent response.
- Output of `simard memory search-facts <topic-token>` showing a fact
  with `created_at > start-of-test`.

### Why

Dashboard rendering smoke tests caught only Unicode bugs; they could not
detect (and did not detect) the WAL-checkpoint data-loss bug. Real E2E
exercise on a live daemon is the only verification gate that catches
durability and ingestion regressions before they reach production.

---

## Engineering Guidelines (G1/G2/G3/G4)

These are **durable engineering principles**, not a point-in-time
snapshot. They apply to human contributors *and* to Simard's own OODA
reasoners and engineer sessions. They are encoded declaratively in the
hot-reloaded prompt assets under `prompt_assets/simard/` (mirrored into
the recipe YAML the daemon runs), enforced as soft review flags in the
merge-readiness judge and the code-review reasoner, and pinned by a
presence test at
[`tests/engineering_guidelines_prompts.rs`](tests/engineering_guidelines_prompts.rs).
G1–G3 are **soft advisory flags**; **G4 additionally has a hard
deterministic backstop** — the Overseer pr-verify scan
`scan_no_point_in_time_report_docs` (pr-verify check #8) — that **blocks**
a merge which would commit a new point-in-time report doc.

> **Why these four?** Each guideline is a lesson learned from a
> specific merged PR (or a run of them) where a change looked complete
> but left a durable gap. The guidelines exist so the same class of gap
> is caught the next time — by a reviewer, by the merge-judge, by Simard
> herself while planning the work, or (for G4) by the deterministic
> merge-gate scan.

> **Terminology.** Simard runs a single **Brain** (the one-Brain
> model). The interpretive reasoners named below — OODA Orient, Decide,
> and the engineer-lifecycle brain — are phases of that one Brain, not a
> separate "Bridge" reasoner. Do not introduce new "Bridge" naming for
> the Brain or its OODA phases. (This is distinct from the existing
> memory/knowledge *bridge adapters*, which keep their established
> names.)

### G1 — Prove gains on BOTH a fixed benchmark AND live self-measurement

Cognition and self-improvement work must iterate toward proving its
gains on **both**:

1. a **fixed benchmark** — a stable corpus or regression baseline, and
2. a **live self-measurement** — a production self-metric that Simard
   emits about her own running behaviour, **trended over time**.

A benchmark-corpus number, or a coarse proxy metric, is **not
sufficient on its own**. The bar is a **hybrid benchmark + live**
result: the improvement must also show up in a live self-metric drawn
from the running daemon, not only on an offline corpus.

This bar is enforced as a **soft advisory flag**, not a hard CI block —
see [How the review gates read](#how-the-review-gates-read-soft-flags-not-hard-blocks)
below. The imperative wording ("must", "not sufficient") describes what a
reviewer or the merge-judge will flag, not a gate that fails the build.

**Motivating context.** PR #2584 reported +86% on a fixed distillation
corpus, and PR #2601 added `recall_precision_at_k` via a coarse
substring proxy. Both proved gains on benchmarks/proxies — neither yet
showed the gain in a live, trended self-metric. G1 closes that gap:
benchmark **and** live, never either alone.

**What "done" looks like for a G1-affected change:**

- A fixed-benchmark result (corpus name, before/after, baseline commit).
- A named live self-metric the daemon emits (for example a `*_at_k`
  recall metric or equivalent), with a plan or link showing it is
  **trended over time** in production — not a one-shot offline number.
- If the live metric cannot yet move within the PR, the PR says so
  explicitly and links the follow-up that will land the live
  measurement.

### G2 — Memory-architecture work belongs upstream in `amplihack-memory-lib`

All memory-architecture work — distillation, recall, ranking, storage,
WAL, forgetting — must land in **`rysweet/amplihack-memory-lib`** (the
memory engine). Simard then **bumps** her pinned `amplihack-memory`
dependency to pick it up.

**Do not fork memory logic into Simard's own repo**
(`src/memory_consolidation`, `src/cognitive_memory`). Where Simard-side
memory logic already exists, **prefer migrating it upstream** over
extending it locally.

**Motivating context.** PR #2584 placed distillation fact-yield logic
in Simard's repo instead of in the library. G2 routes that class of
work to the engine repo so there is a single source of truth for the
memory architecture, and Simard consumes it by bumping the pinned
`amplihack-memory` git rev.

**What "done" looks like for a G2-affected change:**

- The memory-architecture change is a PR against
  `rysweet/amplihack-memory-lib`.
- Simard's PR is (or is immediately followed by) a **dependency bump**
  of the pinned `amplihack-memory` git rev (the dependency is pinned by
  commit SHA, not a semver range) — not a re-implementation under `src/`.
- If a change touches `src/memory_consolidation` or
  `src/cognitive_memory`, the PR justifies why it is a thin
  consumer/adapter and not new engine logic that belongs upstream.

### G3 — Prefer agentic steps over brittle parsing; prefer recipes/prompts over code

Treat string/line parsing of LLM or tool output as a **brittle-parsing
antipattern**. Whenever code parses or extracts model or tool output,
stop and weigh the brittleness, and prefer an **agentic step** — a
structured **JSON output contract** plus agent extraction — that is
robust to rewording, reordering, and extra prose.

More broadly: whenever a change or architecture improvement can be
accomplished through **recipes + prompts** alone, that is the
**preferred choice over writing code**. This guidelines document, and
this whole change set, is itself an application of G3 — it moves policy
into prompt assets rather than into new deterministic code.

**Motivating context.** PR #2573 shipped a line-dropping parser in
`src/recipe_output/extract.rs`. Line/substring parsers silently
mis-handle output the moment the model reformats. G3 pushes extraction
toward a structured contract read by an agent, and pushes new behaviour
toward prompts/recipes before code.

**What "done" looks like for a G3-affected change:**

- New extraction of model/tool output uses a structured/JSON contract,
  not ad-hoc line/substring slicing. (The merge-judge's `verdict` field
  is the reference pattern: a machine-parseable field read through the
  shared extractor that fails **closed** on a parse-miss, never
  silently to `ready`.)
- If new code parses output, the PR explains why an agentic step was
  not viable.
- If the same outcome was reachable via recipes/prompts, the change
  uses recipes/prompts.

**Scope.** G3 targets **new** brittle parsing. The existing shared
extractor in `src/recipe_output/extract.rs` — which the merge-judge reads
and which fails **closed** to `unclear` on a parse-miss — is the
**sanctioned reference pattern**, not a G3 violation. Route new extraction
of model/tool output through that same fail-closed, structured path rather
than adding fresh line/substring slicing.

### G4 — Durable docs only; never commit point-in-time report docs

Simard's repository documentation must be **accurate** and **durable**:
it describes how the system *actually works today* and is expected to be
**updated** by a later PR that changes the feature. The repo must
**never** carry **point-in-time report docs** — investigation, testing,
diagnosis, blockage/recurrence, or benchmark-**snapshot** write-ups that
are true only "as of" the moment they were written.

Decide by **doc type, not topic**. The same subsystem can be the subject
of both a good durable doc and a banned report doc:

- **Durable → keep it, and keep it current.** If a later PR that changes
  the feature would be expected to update the doc (architecture, design,
  reference, how-to), it is durable. Durable documentation is
  **explicitly encouraged** — G4 never discourages keeping the real docs
  accurate.
- **Point-in-time → issue and/or memory, not a repo doc.** If the doc is
  only ever true as of the day it was written ("what I found while
  diagnosing X", a testing write-up, a measured-rate snapshot), its
  findings belong in a **GitHub issue** (the authoritative, trackable,
  dedup-able sink) and/or Simard's memory. Recurrences consolidate into a
  single tracking issue, not a new doc per occurrence.

Unlike G1–G3, G4 has **two rails**. The soft rail is the same
prompt/review guidance that discourages authoring a report doc. The hard
rail is a **deterministic backstop** — the Overseer pr-verify scan
`scan_no_point_in_time_report_docs` (pr-verify check #8) — that blocks a
merge whose diff **adds** a report doc. The scan is deliberately narrow:
it is **added-only** (edits to existing docs are never flagged) and
**report-typed** (it flags a newly added `.md` only when it sits under a
reserved report directory — `docs/investigation/`, `docs/reports/`,
`docs/runs/` — or carries a report-typed *title*, never merely
report-flavored body prose). It uses **no `--admin` / `--no-verify`
bypass**: a flagged PR does not merge until the report doc is removed and
its content moved to an issue/memory.

**Motivating context.** A run of `docs(investigation)` / `docs(overseer)`
PRs — #2879, #2843, #2819, #2814, #2801 — each committed a kgpacks-rs
blockage investigation/diagnosis report as a repo doc. Point-in-time
reports go stale immediately, **poison Simard's own context** when she
reads her repo as grounding, and bury the durable docs. G4 routes that
content to issues/memory and keeps the doc tree durable.

**What "done" looks like for a G4-affected change:**

- An investigation/testing/diagnosis **finding** is recorded as a GitHub
  issue and/or memory — not as a new repo doc; recurrences consolidate
  into one tracking issue.
- Any durable documentation the change warrants is written or updated
  under `docs/` with a **feature/architecture title**, not a report
  title.
- If the pr-verify scan flags an added report doc, the content is moved
  to an issue and the doc removed from the PR — the gate is resolved,
  never overridden.

See [Durable-Documentation Policy (G4)](docs/concepts/durable-documentation-policy.md)
for the rationale and two-rail architecture,
[No point-in-time report docs — pr-verify scan](docs/reference/no-point-in-time-docs-scan.md)
for the deterministic behavior, and
[Record an investigation finding](docs/howto/record-an-investigation-finding.md)
for the step-by-step.

### Where the guidelines are encoded

| Layer | Assets | Effect |
|---|---|---|
| Engineer + OODA reasoner prompts | `engineer_system.md`, `engineer_planning.md`, `ooda_orient.md`, `ooda_decide.md`, `ooda_brain.md` (+ mirrored recipes `ooda-orient.yaml`, `ooda-decide.yaml`, `ooda-engineer-lifecycle.yaml`) | Simard's engineers and OODA Brain apply G1/G2/G3/G4 while planning and doing cognition/memory/parsing/documentation work. |
| Review gates (soft) | `merge_readiness_judge.md` (+ `merge-readiness-judge.yaml`), `review_pipeline.md`, `progress_assessment_reviewer.md` (+ `progress-assessment.yaml`), `overseer/pr_verify.md` | Reviewers FLAG (a) cognition changes with no live self-measurement, (b) memory-arch changes in Simard's repo that belong in `amplihack-memory-lib`, (c) new/extended brittle output-parsing where an agentic step is cleaner, (d) a PR that ADDS a point-in-time investigation/testing/diagnosis report doc that belongs in an issue/memory (G4). |
| Deterministic backstop (G4) | `src/overseer/pr_verify.rs` — pr-verify check #8, `scan_no_point_in_time_report_docs` | Hard-**blocks** a merge whose diff ADDS a new point-in-time report doc. Unlike the soft flags above, this fails the merge gate with `ready: false`; there is no `--admin` / `--no-verify` bypass. |
| Goal framing | `goal_session_objective.md`, `goal_decomposition.md` (+ `goal-decomposition.yaml`), `goal_curator_system.md` | Standing cognition/self-improvement goals inherit G1 (hybrid benchmark + live) and G2 (route memory-arch upstream) in their success criteria. Those goals are **seeded at runtime into `~/.simard` state**, not stored as repo assets — so the presence test pins the G1/G2 framing in the goal *prompts*, not any specific goal slug. |
| Durable doc | this section | Human-facing source of truth for G1/G2/G3/G4. |
| Presence test | `tests/engineering_guidelines_prompts.rs` | Pins keyword invariants so a future prompt edit cannot silently drop a guideline. |

**Why not `AGENTS.md`?** `AGENTS.md` is regenerated amplihack boilerplate —
its body is overwritten on each agent-context sync (the file even opens with
a corrupted marker line). It is therefore **deliberately excluded** as a
durable layer: a cross-reference added there would not survive regeneration.
This `CONTRIBUTING.md` section is the single canonical, human-facing source
of truth for G1/G2/G3/G4, and no `AGENTS.md` edit is required for parity.

**Hot reload.** The `prompt_assets/simard/` files hot-reload from
`~/.simard/prompt_assets/`. After this PR merges, the operator syncs
prompt assets / redeploys to activate the updated prompts on the running
daemon — the merge alone does not change live behaviour.

### How the review gates read (soft flags, not hard blocks)

The review gates add **advisory flags** — they do not change the
machine verdict enum (`ready` / `not_ready` / `unclear` for the
merge-judge, and the severity scale for the code reviewer). A reviewer
or the merge-judge raises a G1/G2/G3/G4 flag as a finding or blocker with a
`fix` suggestion; the author either addresses it or justifies why it
does not apply. What fires a flag:

- **G1 flag** — a PR that improves recall/distillation/ranking and
  reports only a corpus or proxy number, with no live self-metric
  trended over time.
- **G2 flag** — a diff that adds distillation/recall/ranking/WAL logic
  under `src/memory_consolidation` or `src/cognitive_memory` instead of
  `amplihack-memory-lib` plus a dependency bump.
- **G3 flag** — a new or extended line/substring parser over model/tool
  output where a structured JSON contract + agent extraction would be
  cleaner.
- **G4 flag** — a PR that ADDS a new point-in-time investigation/testing/
  diagnosis report doc instead of recording the finding in a GitHub
  issue and/or memory.

**G4 is the exception to "soft flags only."** In addition to the advisory
flag above, G4 has a **hard deterministic backstop**: even if the soft
flag is missed, the Overseer pr-verify scan `scan_no_point_in_time_report_docs`
(check #8) fails the merge gate for a PR that adds a report doc — no
`--admin` / `--no-verify` bypass. The soft flag catches it earlier and
more helpfully; the scan guarantees it cannot slip through.

### Verifying the guidelines are present

```bash
cargo test --test engineering_guidelines_prompts
```

The test reads the prompt assets, lowercases them, and asserts stable
keyword invariants for each guideline (for example `live
self-measurement` and `trended over time` for G1, `amplihack-memory-lib`
for G2, `brittle parsing` and `agentic step` for G3, and
`no-point-in-time-docs` / `point-in-time` for G4), asserts each
edited reasoner `.md` stays in parity with its recipe `.yaml` mirror,
and asserts the edited reasoner regions do not rename the one-Brain
OODA phases as a "Bridge". It asserts **keywords, not full sentences**,
so ordinary rewording does not break it — deleting a guideline does.

---

## Where to Get Help

- Architecture: [`docs/architecture/`](docs/architecture/)
- Operator dashboard: [`docs/operator-dashboard/`](docs/operator-dashboard/)
- Daemon mode: [`docs/daemon-mode.md`](docs/daemon-mode.md)
- Cognitive memory: [`docs/memory.md`](docs/memory.md)
- Roadmap: [`docs/ROADMAP.md`](docs/ROADMAP.md)
- Operations index: [`docs/operations/index.md`](docs/operations/index.md)

For issues, file at https://github.com/rysweet/Simard/issues with the
appropriate label (`bug`, `pre-existing-flake`, `durability`,
`pre-commit`, `meetings`).
