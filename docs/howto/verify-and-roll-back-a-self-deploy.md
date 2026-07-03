---
title: How to verify and roll back a self-deploy
description: Operator runbook for the safe self-deploy — confirm a merged self-change is actually running, read the self-health probe, clear stuck engineer orphans, and force a rollback when a deploy goes wrong.
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../concepts/reconcile-and-self-deploy.md
  - ../reference/self-deploy-api.md
  - ../reference/self-deploy-source-prep.md
  - ../howto/run-self-deploy-from-any-directory.md
  - ../safe-self-update.md
  - ../howto/inspect-and-clean-engineer-worktrees.md
---

# How to verify and roll back a self-deploy

> **Status: implemented.** `simard safe-update`, `simard self-test`,
> `simard rollback`, and `simard self-health` all exist today. The self-deploy
> orchestrator and its rollback tail live in
> [`src/self_deploy/`](https://github.com/rysweet/Simard/blob/main/src/self_deploy/mod.rs)
> (see [reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md)).
> The operator drives the deploy; the recipe never restarts a live daemon.

This guide is for an operator who wants to confirm that a merged self-change is
**running**, or to recover when a self-deploy goes wrong. For the design, see
[reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md).

## Prerequisites

- The `simard` daemon is installed at `~/.simard/bin/simard` and managed by
  systemd (`simard-ooda` user unit) on the host.
- You can read `~/.simard/state/` and `~/.simard/bin/`.

## Check whether the running daemon is stale

Ask the daemon directly whether merged work is still un-deployed:

```bash
simard self-health --json | jq '.probes.version_advanced'
```

`"healthy": true` means the running binary's embedded SHA is **compatible** with
the target — equal, or one is a case-insensitive prefix of the other (the
abbreviation tolerance in `commits_compatible`); after a normal self-deploy the
two full SHAs are exactly equal. A `false` means there is deploy
drift — a merged self-change is not yet running. The reconciliation detector
reports the same drift to the brain each cycle (see
[`DeployDrift`](../reference/self-deploy-api.md#deploydrift)).

## Run the full health probe

```bash
simard self-health
```

The human-readable table prints one line per probe:

| Probe | What a failure means |
| --- | --- |
| version advanced | the running binary is behind merged `main` |
| memory intact | the live fact count dropped below the pre-deploy baseline |
| goal board intact | the goal board did not load or lost active goals |
| reasoners LLM-backed | a brain fell back to deterministic output (parse failure) |
| no quarantine | the cognitive-memory store is quarantined |

Exit code `0` means every probe passed. Any non-zero exit means at least one
probe failed — the same condition that makes the orchestrator roll back.

## Clear stuck engineer orphans (the "Text file busy" case)

If a swap reports `OrphanReapTimeout` or you see "Text file busy", a subprocess
is still executing the old binary. List candidates by numeric PID:

```bash
# Inspect in-flight engineer worktrees and their claim PIDs first.
ls ~/.simard/engineer-worktrees/
# Identify processes still bound to the install path and running `engineer run`.
# (The reaper matches exe == install path AND argv contains `engineer run`.)
```

Then let the daemon's reaper run again — the self-deploy step is idempotent, so
re-triggering it (or re-running `simard safe-update`) re-attempts the reap. Only
terminate a process yourself by its specific numeric PID; never use name-based
process killers (repo shell policy). See
[inspect and clean engineer worktrees](../howto/inspect-and-clean-engineer-worktrees.md).

## Force a rollback

A self-deploy rolls back automatically when its post-deploy health check fails.
To roll back manually — for example, if a deploy passed its health check but you
observe a regression — restore the most recent binary backup and restart:

```bash
simard rollback
```

This restores the latest `~/.simard/bin/simard.bak.<utc>` over the install path
and asks systemd to restart `simard-ooda`. It is idempotent and refuses to
overwrite the install with a corrupt backup (`RollbackBackupCorrupt`). Confirm
the result:

```bash
simard self-health --json | jq '.healthy'
```

If `simard rollback` reports `RollbackFailed`, the host could not be returned to
a healthy state automatically — this is the **critical** path. Investigate the
binary backups in `~/.simard/bin/` and the memory snapshot recorded by the deploy
before restarting the service by hand.

## Verify a clean deploy end-to-end

After a successful self-deploy you should see, in order:

1. `simard self-health` exits `0` and `version_advanced.healthy == true`.
2. `memory_intact.healthy == true` (fact count preserved).
3. `brains_llm_backed.fallback_records == 0` over the probe cycle.
4. No `draining.flag` in `~/.simard/state/` (engineer dispatch has resumed).

When all four hold, the merged change is **running and verified** — the loop is
closed.

## See also

- [reconcile-and-self-deploy concept](../concepts/reconcile-and-self-deploy.md)
- [How to run self-deploy from any directory](../howto/run-self-deploy-from-any-directory.md)
- [Self-deploy API reference](../reference/self-deploy-api.md)
- [Self-deploy source-prep reference](../reference/self-deploy-source-prep.md)
- [Safe Self-Update](../safe-self-update.md)
