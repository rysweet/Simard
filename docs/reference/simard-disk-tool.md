---
title: The simard disk tool
description: Reference for the agent-facing `simard disk` command (reclaim/report), its exit-code contract, argument grammar, guard reject reasons, and the module wiring that makes disk_health.rs a thin exit-status trigger with no recipe-output parsing.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../howto/reclaim-disk-with-the-simard-disk-tool.md
  - ../howto/configure-disk-reclamation.md
  - ../concepts/agentic-disk-reclamation.md
  - ./disk-reclaim-api.md
  - ./disk-health-api.md
---

# The simard disk tool

**CLI adapter:** `src/operator_cli/disk.rs`
**Guarded core:** `src/disk_reclaim/` (`reclaim_candidates`, `guard::vet_candidate`)
**Trigger shim:** `src/disk_health.rs`

`simard disk` is the agent-facing surface the disk-health recipe calls to *act*.
It is a thin, delete-free adapter: it parses arguments, builds
`ReclaimCandidate` values, and delegates to the shared, non-bypassable
`disk_reclaim::reclaim_candidates` guarded executor. **No deletion logic lives in
the adapter** — every removal is performed by the guarded core after re-vetting.

## Command grammar

```
simard disk report  (--path <P> | --paths @<file> | --paths @-)…
simard disk reclaim (--path <P> | --paths @<file> | --paths @-)… [--dry-run]
```

| Flag | Applies to | Meaning |
| ---- | ---------- | ------- |
| `--path <P>` | both | A single candidate path. Repeatable. |
| `--paths @<file>` | both | Read **newline-delimited candidate paths** from a file — one path per line. |
| `--paths @-` | both | Read the same newline-delimited paths from stdin. |
| `--dry-run` | `reclaim` | Vet and report only; perform zero destructive operations. |

- Leading-dash path values are refused; use `--` to terminate flag parsing when a
  path could be ambiguous.
- Paths are canonicalized and symlinked candidates are refused by the guard, not
  pre-resolved by the adapter (avoids TOCTOU).
- Large lists **must** use `--paths @file` / `@-`; do not inline a big list into
  a single argv string.
- **`--paths` takes a path list, not JSON.** This is a deliberate divergence from
  `simard disk-reclaim exec --candidates`, whose loader reads a JSON
  `[ReclaimCandidate]` array. Keeping the two loaders distinct prevents a plain
  path list from ever being misparsed as JSON (or vice-versa); the adapter derives
  each `ReclaimCandidate` (including `kind`) from the path itself. Blank lines and
  `#` comment lines are ignored.

### Default mode differs from `disk-reclaim`

| Command | Default | Opt-in to the other mode |
| ------- | ------- | ------------------------ |
| `simard disk reclaim` | **Apply** (guarded delete) | `--dry-run` |
| `simard disk-reclaim` | Dry-run | `--apply` |

This divergence is intentional and documented in the tool's usage string:
`simard disk reclaim` is what the recipe agent runs to actually free space, so
apply is the default. Apply-as-root is still hard-refused.

## Exit-code contract

| Exit | Name | Condition |
| ---- | ---- | --------- |
| `0` | handled | Candidate reclaimed, **or** safely skipped by a guard rail (per-candidate rejection is **not** a failure). `report` and `--dry-run` also exit `0`. |
| `1` | operational failure | A path was unreadable, a size measurement or git operation failed, or a delete errored mid-run. |
| `2` | refused | `reclaim` apply mode invoked as root (`geteuid() == 0`). |

The recipe interprets the tool — and the disk-health run overall — by exit status
alone. There is no stdout envelope to parse.

> **Implementer contract.** The core reclamation model only *distinguishes* the
> refusal case (`2`, apply-as-root). Exit `1` is this adapter's deliberate
> convention for any operational failure (unreadable path, size/git op failure,
> mid-run delete error) so that a genuine breakage is never silently mapped to
> `0`. `disk.rs` **must** implement this 0/1/2 mapping; per-candidate guard
> rejections are `0`, not `1`.

## Candidate classification (`kind`)

The adapter classifies each path conservatively, because a misclassification
that *shortened* vetting would be a safety hole:

| Detected shape | Assigned `CandidateKind` | Vetting |
| -------------- | ------------------------ | ------- |
| Has a `.git` entry and is a registered git worktree under an allow-root | `TrackedWorktree` | Runs the merged/closed-PR + no-active-session rails. |
| A `target/`-style build cache | `StaleBuildCache` | `rm -rf` after allow-root + live-process rails. |
| Anything else | `OrphanDir` | `rm -rf` after allow-root + live-process rails. |

`kind` is **advisory**. The guard re-derives the real primitive at vet time; a
path with a `.git` marker is *always* vetted as a tracked worktree even if it was
labelled `orphan_dir` or `stale_build_cache`. A mislabel can only ever *deepen*
vetting, never skip it.

## Guard reject reasons

Every skip maps to exactly one closed `RejectReason` (from
`src/disk_reclaim/guard.rs`). All of these route to human-review and exit `0`:

| `reject_reason` | Meaning | Operator action |
| --------------- | ------- | --------------- |
| `protected_path` | `worktrees/main` or a daemon working directory | none — correctly protected |
| `live_process` | a live PID references the path (`/proc/<pid>/cwd`) | wait for / stop the process |
| `uncommitted_or_unpushed` | dirty tree, or commits not in a merged/closed PR | push/commit, or remove by hand if disposable |
| `active_worktree` | an active recipe/engineer worktree (tmux/PID) owns it | let it finish |
| `outside_allow_root` | not under an allow-root, or symlink/canonicalize refused | inspect manually |
| `unknown_pr_state` | the PR could not be positively confirmed merged/closed | check `gh pr view`; the tool refuses to guess |

### No merge-base ancestry test

The staleness decision is a **positively-confirmed merged/closed PR**, not
`git merge-base --is-ancestor <branch> origin/main`. A fresh worktree at
`origin/main` (no commits yet) *is* an ancestor of `origin/main`; using ancestry
would wrongly flag it and delete its live build cache. When the PR state is not
positively merged/closed, `RealTrackedWorktreeProbe::assess` fail-closes to
`unknown_pr_state` and the path is kept. A guard regression test asserts this and
that no `--is-ancestor` path exists in the routing.

## Allow-roots and the deny-set

The adapter delegates scope entirely to the guarded core:

- **Allow-roots** (`disk_reclaim::allow_roots`): `<state_root>/engineer-worktrees`,
  `<repo>/worktrees` for each managed repo, and the shared cargo target dirs
  (`<state_root>/cargo-target`, `<state_root>/shared-target`). The adapter must
  not widen these.
- **Protected deny-set** (`ProtectedDenySet`): the hardcoded `worktrees/main`,
  resolved daemon working directories, and any `SIMARD_GIT_PROTECTED_REPOS`.

## Configuration

The tool inherits the reclamation environment; it adds no new knobs of its own.

| Variable | Effect | Default |
| -------- | ------ | ------- |
| `SIMARD_STATE_ROOT` | State root (`~/.simard`); determines allow-roots. | `$HOME/.simard` |
| `SIMARD_GIT_PROTECTED_REPOS` | Comma-separated extra repo roots added to the deny-set. | unset |
| `AMPLIHACK_AGENT_BINARY` | Agent binary the disk-health recipe subprocess uses; preserved by `disk_health.rs`. | set by launcher |

The disk-health trigger threshold and cadence are the daemon's, not the tool's —
see [Configure the disk health check](../howto/configure-disk-health-check.md).

## `disk_health.rs` — thin exit-status trigger

After the rework, `src/disk_health.rs` no longer parses recipe output.

**Removed:** `RecipeOutput` and `StepResult` (relocated to
`src/disk_reclaim/recipe.rs` as `pub(crate)` for the reclaim proposal flow that
still parses candidate JSON), `parse_disk_health_text`, and their unit tests. The
`--output-format json` flag and the `RecipeOutput`/`StepResult` deserialization
of the child's stdout are gone from this file — nothing in the disk-health path
scrapes recipe output any more.

**Retained:** `emergency_cleanup` (the Tier-1 deterministic hard-stop),
`get_disk_usage_pct`, `dir_size_bytes`, `resolve_recipe_path`, and the
`recipe-runner-rs` spawn wiring (including `AMPLIHACK_AGENT_BINARY`).

**New contract:** `run_disk_health_check` returns success/failure by the child's
exit status only (e.g. `SimardResult<bool>`): `Ok(true)` on exit `0`,
`Ok(false)` on non-zero, `Err(..)` on spawn failure. The deterministic Rust `df`
gate decides *whether* to trigger; it never parses the recipe's output.

### Daemon caller

The daemon's Tier-2 maintenance path consumes the boolean:
`Ok(true)`/`Ok(false)` log success/failure and `Err` warn-logs. The Tier-1
`emergency_cleanup` and Tier-3 agentic `run_disk_reclaim` paths are unchanged.
The df-based decision to trigger stays deterministic Rust.

## Data flow

```
disk-health-check.yaml (agent step)
        │  calls
        ▼
simard disk report / simard disk reclaim --path <P>
        │  build ReclaimCandidate (kind classified conservatively)
        ▼
disk_reclaim::reclaim_candidates
        │  guard::vet_candidate (re-vets EVERY candidate)
        ├── Allow  → primitive delete (git worktree remove --force | rm -rf)
        └── Reject → human-review skip (exit 0)
        ▲
        │ exit status only — NO JSON printed, NO Rust parse
disk_health.rs::run_disk_health_check → SimardResult<bool>
```

## Security surface

The adapter is the primary new attack surface (input arrives from an LLM recipe):

- refuses leading-dash path values; honours `--` argv terminators,
- ingests `@file` / `@-` via the audited loader (never through a shell),
- delegates every safety decision to `guard::vet_candidate` — a single guarded
  entry point with no fast path,
- performs no path pre-resolution (the remover re-asserts canonical containment
  at the syscall to close TOCTOU),
- refuses apply-as-root (exit `2`); `gh`/`git` inherit ambient credentials — the
  adapter adds no token plumbing and never widens allow-roots.

## Related

- [Reclaim disk with the simard disk tool (how-to)](../howto/reclaim-disk-with-the-simard-disk-tool.md)
- [Configure and run disk reclamation](../howto/configure-disk-reclamation.md)
- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md)
- [Disk reclaim API (reference)](./disk-reclaim-api.md) — guard, executor, candidate contract
- [Disk health API (reference)](./disk-health-api.md) — the pre-rework parsing shim (historical)
