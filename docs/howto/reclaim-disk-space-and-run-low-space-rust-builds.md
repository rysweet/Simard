---
title: "How to reclaim disk space and run low-space Rust builds"
description: Preview and remove stale Rust build artifacts, then run Cargo through one shared low-space target directory across Simard worktrees.
last_updated: 2026-03-31
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../tutorials/run-your-first-local-session.md
  - ../reference/runtime-contracts.md
---

# How to reclaim disk space and run low-space Rust builds

Simard worktrees can consume a lot of disk because each worktree can accumulate its own `target/` tree.

This repository now ships two explicit helpers:

- `scripts/reclaim-build-space` previews or deletes Rust build artifact directories under the shared Simard git common dir
- `scripts/cargo-low-space` runs Cargo through one shared `target-shared/` directory, disables incremental builds by default, and strips debug info unless you opt back in

These helpers are local build tooling only. They do not change the runtime contract or the shipped `simard` CLI surface.

## When to use this

Use this guide when:

- `/home` or your workspace disk is filling up
- you have many Simard worktrees
- you want one shared Rust target dir instead of per-worktree duplication
- you need to keep building, testing, or running Simard on a tighter disk budget

## Step 1: Preview reclaimable build artifacts

From any Simard worktree:

```bash
scripts/reclaim-build-space
```

By default this prints:

- the repo-level `target/`
- the repo-level `target-shared/`
- every per-worktree `target/`
- except the current worktree `target/`, which it keeps by default

This is preview-only.

## Step 2: Remove the previewed build artifacts

If the preview looks right:

```bash
scripts/reclaim-build-space --apply
```

If you also want to remove the current worktree `target/`:

```bash
scripts/reclaim-build-space --apply --include-current
```

This only deletes build artifact directories. It does **not** remove whole worktrees, branches, or tracked source files.

## Step 3: Run Cargo through the low-space wrapper

Use the wrapper instead of raw Cargo when you want a shared lower-disk build path:

```bash
scripts/cargo-low-space test --quiet
scripts/cargo-low-space run --quiet -- engineer terminal-recipe-list
```

Default behavior:

- shared target dir: `target-shared/` under the Simard git common root
- `CARGO_INCREMENTAL=0` unless you already set it explicitly
- `RUSTFLAGS` gains `-C debuginfo=0` unless you opt back in

That means multiple worktrees can reuse one build output tree instead of each storing their own separate `target/`.

## Optional overrides

Choose a different shared target dir:

```bash
SIMARD_CARGO_TARGET_DIR=/tmp/simard-target scripts/cargo-low-space test --quiet
```

Keep debug info:

```bash
SIMARD_LOW_SPACE_DEBUGINFO=1 scripts/cargo-low-space test --quiet
```

Show the wrapper's resolved settings:

```bash
SIMARD_LOW_SPACE_VERBOSE=1 scripts/cargo-low-space test --quiet
```

## What this does not automate

This guide does **not** automatically remove old worktrees.

Whole-worktree cleanup still needs a judgment call about:

- whether the worktree is clean
- whether its branch has already been merged or is otherwise no longer needed
- whether nested dirty worktrees exist underneath it

Use `git worktree list --porcelain`, `git status --porcelain`, and branch/merge checks before removing whole worktrees.

## Suggested operating pattern

For routine low-space work:

1. reclaim stale build artifacts with `scripts/reclaim-build-space`
2. use `scripts/cargo-low-space ...` for new builds/tests
3. remove old clean merged worktrees separately when you no longer need them
