# Contributing to Simard

Thank you for contributing. This document describes the local developer
workflow, merge policy, durability guarantees, and disposition of known
pre-existing test failures. **Following these rules is mandatory** —
they are the same gates CI enforces.

---

## Table of Contents

1. [Local Pre-Commit Workflow](#local-pre-commit-workflow)
2. [Merge Policy: No `--admin` Merges](#merge-policy-no---admin-merges)
3. [Cognitive Memory Durability (SIGTERM + Periodic Backups)](#cognitive-memory-durability-sigterm--periodic-backups)
4. [Local Data Retention Disclosure](#local-data-retention-disclosure)
5. [Pre-Existing Test Failure Disposition](#pre-existing-test-failure-disposition)
6. [Real-Meeting & Dashboard E2E Verification](#real-meeting--dashboard-e2e-verification)

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
| `cargo-clippy-precommit` | `pre-commit`, `manual` | `cargo clippy --release --no-deps -- -D warnings` |
| `cargo-clippy` | `pre-push`, `manual` | `cargo clippy --all-targets --all-features --locked -- -D warnings` |
| `cargo-test-race-subset` | `pre-push`, `manual` | `cargo test --release --lib -- --test-threads=$(nproc) cognitive_memory bootstrap memory_ipc memory_consolidation` |

The two-tier clippy gate is intentional: the `--release --no-deps`
hook gives instant feedback at commit time; the `--all-targets
--all-features --locked` hook reuses the warm `target/` after the
race-test compile and runs at push time, mirroring CI exactly.

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

## Cognitive Memory Durability (SIGTERM + Periodic Backups)

Simard's cognitive memory is backed by [`lbug`](https://docs.rs/lbug)
(`0.15.x`, see [`Cargo.toml`](Cargo.toml) for the active minor). lbug
opens its database as a directory at
`~/.simard/cognitive_memory.ladybug/` and only flushes its WAL inside
`Database::drop`, which does **not** run automatically on signal-induced
process exit. To prevent data loss on `systemctl restart simard-ooda`
(and similar SIGTERM-issuing scenarios), the daemon installs a graceful
shutdown handler and runs a periodic backup loop.

### Graceful Shutdown Sequence

When the OODA daemon receives `SIGTERM`, `SIGINT`, or `SIGHUP`, the
[`ctrlc`](https://docs.rs/ctrlc) handler (registered with the
`termination` feature) sets a shutdown flag. At the top of the next
OODA iteration the daemon invokes `shutdown_daemon(state_root,
shared_mem, state, bridges, signal_driven=true)`, which performs the
following steps **in order**:

1. **Persist the goal board** via `persist_board(&state.active_goals,
   &*bridges.memory)`. The write goes through the live cognitive-memory
   writer so the subsequent checkpoint flushes it.
2. **Pre-exit checkpoint** via `shared_mem.checkpoint()`
   (`CognitiveMemoryOps::checkpoint`, which delegates to
   `NativeCognitiveMemory::checkpoint`). Collapses the WAL into the
   main DB directory.
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
     `copy → fsync(file) → rename → fsync(parent dir)` atomic-write
     pattern (`atomic_copy_with_fsync` in
     `src/cognitive_memory/backup.rs`);
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
`src/cognitive_memory/backup.rs`).

### File and Directory Layout

| Path | Purpose | Notes |
|---|---|---|
| `~/.simard/cognitive_memory.ladybug/` | lbug DB directory | Created by `lbug::Database::new` on first use |
| `~/.simard/cognitive_memory.ladybug.wal` *or* `~/.simard/cognitive_memory.wal` | WAL sibling(s) | Either or both may exist depending on lbug minor |
| `~/.simard/backups/` | Backup root | Created by `create_dir_all` on first backup |
| `~/.simard/backups/cognitive_memory.ladybug.<ts>` | Backup of the DB directory at unix-second `<ts>` | Restore = copy back |
| `~/.simard/backups/cognitive_memory.ladybug.wal.<ts>` | WAL sibling for the same `<ts>` | Same timestamp pairs the two |

> **Note**: `cognitive_memory.ladybug` is an **lbug database directory**,
> not a single file. Restoration uses `cp -r` (not `cp`); see
> [Restoring from Backup](#restoring-from-backup) and
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

# 3. Copy both the DB directory AND the WAL sibling(s) back into ~/.simard/
TS=1762800000   # the timestamp suffix from above
rm -rf ~/.simard/cognitive_memory.ladybug
cp -r ~/.simard/backups/cognitive_memory.ladybug.${TS} \
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
| `run_local_engineer_loop_emits_agent_prompt_build_phase` (`src/engineer_loop/tests_agent_spawn.rs`) | **Now passes** when re-run on this branch (intermittent). Re-verified after Workstream B and on isolated cache. No new issue filed; will reopen if it recurs. | n/a (passing) |
| `engineer_loop_probe_fails_visibly_when_structured_replacement_target_is_missing` (`tests/engineer_loop.rs`) | Pre-existing — agent backend "appends to satisfy verify-contains" when the `replace` source string is missing, so the loop reports success instead of failing visibly. Orthogonal to #1631. | [#1639](https://github.com/rysweet/Simard/issues/1639) |
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
