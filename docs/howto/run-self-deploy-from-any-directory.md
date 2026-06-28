---
title: How to run self-deploy from any directory
description: Operator runbook for the autonomous, fast self-deploy — run `simard self-deploy` from any working directory and have it fetch + check out the merged head and build it into a warm, incremental target dir. Covers the warm directories, the SIMARD_SELF_DEPLOY_REPO override, first-run vs warm-run timing, and how to confirm the merged head (not cwd HEAD) was deployed.
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../reference/self-deploy-source-prep.md
  - ../reference/self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
  - ../reference/state-root-resolution.md
---

# How to run self-deploy from any directory

> **Status: implemented.** `simard self-deploy` fetches and checks out the
> merged head before building, and reuses a warm target directory, so it works
> from **any** working directory and is fast on repeat runs. The
> source-preparation surface lives in
> [`src/self_deploy/source_prep.rs`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/source_prep.rs)
> — see [self-deploy source-prep reference](../reference/self-deploy-source-prep.md).
> The operator drives the deploy; the recipe never live-redeploys a daemon.

This guide is for an operator who wants to deploy the latest **merged** Simard
commit into the running daemon **without** first navigating to an up-to-date
checkout. For the design and the typed API, see
[self-deploy source-prep reference](../reference/self-deploy-source-prep.md).

## What changed

`simard self-deploy` used to build whatever the current directory's `HEAD` was,
into a cold per-run `temp_dir()` (a ~10-minute from-scratch compile). Now it:

1. resolves a canonical Simard source repo (independent of your cwd),
2. `git fetch origin` and `git checkout --detach <merged head>` in that repo,
3. builds **that** merged commit into a **persistent warm** target dir
   (`~/.simard/self-deploy-target/`) — incremental, ~2–3 min on repeat runs,

then runs the unchanged safety sequence (dual backup → drain → orphan-reap →
swap → restart → health-check → rollback).

## Prerequisites

- The `simard` daemon is installed at `~/.simard/bin/simard` (systemd
  `simard-ooda` user unit) on the host.
- `git` and a Rust toolchain are on `PATH` (the build runs `cargo build
  --release`).
- Network access to the Simard origin (for `git fetch`) **or** a pre-populated
  source repo (see [`SIMARD_SELF_DEPLOY_REPO`](#use-an-explicit-source-repo)).

## Check drift first (read-only)

`--check` never mutates anything; it reports running-vs-merged drift:

```bash
simard self-deploy --check
```

```text
simard self-deploy --check:
  running commit : 3f9a1c2…
  merged head    : a17be03…
  behind commits : 4
  drifted pins   : (none)
  needs deploy   : YES
```

`needs deploy: YES` means a merged change is not yet running. For the `merged
head` shown here to be **exactly** the SHA the deploy fetches, checks out, and
builds, `--check` must resolve the merged head against the **same canonical
repo** the deploy uses (the `SIMARD_SELF_DEPLOY_REPO` override or the persistent
clone) — not the cwd checkout. If `--check` is run from a stale or unrelated
checkout while the deploy resolves a different canonical repo, the two SHAs can
diverge; see the design note in
[self-deploy source-prep reference](../reference/self-deploy-source-prep.md#repo-resolution-precedence).
Use `--json` for machine-readable `DeployDrift`:

```bash
simard self-deploy --check --json | jq '.needs_deploy, .behind_commits'
```

## Deploy from any directory

Run it from anywhere — your home directory, `/tmp`, a stale checkout, it does
not matter:

```bash
cd /tmp
simard self-deploy
```

```text
simard self-deploy: deploying merged head a17be03… (binary 4 behind)…
simard self-deploy: SUCCESS — new binary verified running (restarter=systemd, orphans reaped=0).
```

Under the hood the command resolves the canonical source repo, fetches origin,
checks out `a17be03…` **detached**, and builds it into
`~/.simard/self-deploy-target/` — never your cwd's `HEAD`. If the running binary
is already at the merged head, it is a no-op:

```text
simard self-deploy: running binary is already at merged head — nothing to do.
```

## First run vs. warm runs (timing)

| Run | What happens | Approx. time |
| --- | --- | --- |
| First ever | Clones the source into `~/.simard/self-deploy-src/` and does a cold compile into the empty warm dir. | ~10+ min (one-time) |
| Subsequent | Reuses the existing clone (`git fetch` + checkout) and the warm `~/.simard/self-deploy-target/` for an **incremental** build. | ~2–3 min |

The warm directories live under `~/.simard`, outside `/tmp`, so they survive
reboots and disk-cleanup reapers and stay warm between deploys. Do not delete
them to "clean up" — that re-imposes the one-time cold-build cost.

## Use an explicit source repo

To skip the managed clone (for example, an air-gapped host with a mirror, or to
reuse an existing maintained checkout), point `SIMARD_SELF_DEPLOY_REPO` at an
existing git work-tree:

```bash
SIMARD_SELF_DEPLOY_REPO=/opt/simard/src simard self-deploy
```

The path must be **absolute**, contain no `..`, not be a symlink, and be a real
git work-tree. An invalid value aborts loudly with `SourceResolveFailed` — it
never silently falls back to building your current directory.

## Relocate the warm directories

`SIMARD_STATE_ROOT` relocates the durable state root, including both
`self-deploy-src/` and `self-deploy-target/`:

```bash
SIMARD_STATE_ROOT=/data/simard simard self-deploy
# builds into /data/simard/self-deploy-target/, clones to /data/simard/self-deploy-src/
```

See [state-root-resolution](../reference/state-root-resolution.md).

## Confirm the merged head is actually running

After a deploy, verify the running binary advanced to the merged commit:

```bash
simard self-health --json | jq '.probes.version_advanced'
```

`"healthy": true` confirms the running binary's embedded SHA is **compatible**
with the target (equal, or one a case-insensitive prefix of the other — after a
normal deploy the two full SHAs are exactly equal), so the merged head is live.
Re-running `--check` should now report `needs deploy: no`:

```bash
simard self-deploy --check | grep 'needs deploy'
#   needs deploy   : no
```

For the full post-deploy verification checklist and rollback, see
[verify and roll back a self-deploy](./verify-and-roll-back-a-self-deploy.md).

## Troubleshooting

| Symptom | Cause | Action |
| --- | --- | --- |
| `SourceResolveFailed` | Invalid `SIMARD_SELF_DEPLOY_REPO`, undiscoverable origin URL, or first-time clone failed. | Fix the path/URL; ensure the cwd or override points at a repo with an `origin` remote. The deploy aborted **before** touching the daemon. |
| `FetchFailed` | `git fetch origin` failed and the target object is not cached locally. | Check network/credentials to origin. No daemon mutation occurred. |
| `CheckoutFailed` | The merged SHA failed validation or the detached checkout failed (e.g. the object is missing after fetch). | Inspect the warm clone at `~/.simard/self-deploy-src/`; the deploy aborted pre-sequence. |
| Build is slow every time | The warm target dir was deleted or relocated between runs. | Stop deleting `~/.simard/self-deploy-target/`; keep `SIMARD_STATE_ROOT` stable. |

All three `*Failed` aborts happen during build-step 1, **before** any backup,
drain, swap, or restart — the running daemon is untouched, so it is always safe
to fix the cause and re-run.

## See also

- [Self-deploy source-prep reference](../reference/self-deploy-source-prep.md) — trait, preparer, warm dirs, errors, security.
- [Verify and roll back a self-deploy](./verify-and-roll-back-a-self-deploy.md) — post-deploy verification and recovery.
- [Reconcile-and-self-deploy concept](../concepts/reconcile-and-self-deploy.md) — the end-to-end sequence.
- [Self-deploy API reference](../reference/self-deploy-api.md) — the broader self-deploy types and CLI.
