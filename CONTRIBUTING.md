# Contributing to Simard

## Merge policy

**Pull requests must be merged with green CI. `gh pr merge --admin` is not
permitted.** It bypasses required checks (cargo fmt, clippy, the full test
suite) that are there to keep main shippable.

If CI is failing on your PR:

1. **Fix the failure**, even if it looks pre-existing.
2. If the failure is *truly* unrelated to your change and isn't tractable in
   the current PR, file a tracking issue with a concrete reproduction and
   link it from the PR body. The PR still cannot merge until CI is green —
   the path forward is either fixing the issue first, or reverting the
   regression that introduced it.
3. **Never** use `--admin`, `--auto`, or any merge flag that skips checks.

## Pre-commit hooks

This repo ships a `.pre-commit-config.yaml` with two stages that mirror what
CI runs:

- `pre-commit` stage: `cargo fmt --all -- --check`
- `pre-push` stage: `cargo clippy --all-targets --all-features --locked -- -D warnings`
  followed by `cargo test --all-features --locked --test-threads=4`

Install both stages once per checkout:

```sh
pre-commit install --hook-type pre-commit --hook-type pre-push
```

The hooks live in the git common dir. If you're working in a `git worktree`,
install hooks from any worktree — they apply repo-wide.

The pre-push test stage uses `--test-threads=4` deliberately. Single-threaded
runs hide races that surface under CI's parallel scheduler. **Do not** run
local tests with `--test-threads=1` and consider that a green signal.

## Cognitive memory durability

The OODA daemon owns the on-disk LadybugDB. Two invariants must hold:

1. **Periodic verified backups.** The daemon backs up
   `~/.simard/cognitive_memory.ladybug` (and its `.wal` companion) every 5
   minutes by default, retaining 24 backups (≈2 hours of point-in-time
   recovery). Override with `SIMARD_DB_BACKUP_INTERVAL_SECS` /
   `SIMARD_DB_BACKUP_RETENTION` env vars.
2. **Graceful shutdown checkpoints.** SIGTERM/SIGINT triggers a graceful
   shutdown that explicitly persists the goal board and runs a `CHECKPOINT;`
   on the cognitive-memory DB before exit. Skipping the checkpoint loses any
   writes still in the WAL.

Tests in `tests/cognitive_memory_durability.rs` pin both invariants. If you
touch the daemon shutdown path, the IPC server, or the cognitive-memory
`Database`, those tests must still pass.

## Tech debt

We don't park tech debt as "follow-ups." If a fix is identified during a PR
and is in scope of the bug or feature being addressed, fix it now. If it is
truly out of scope, file a tracking issue immediately — don't leave it as a
TODO comment or a future-self note.
