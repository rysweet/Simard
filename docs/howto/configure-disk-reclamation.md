---
title: Configure and run disk reclamation
description: Operator guide for Simard's agentic disk-reclamation capability — running dry-run and live reclamation from the CLI, reading the report and human-review list, tuning the SIMARD_DISK_RECLAIM_PCT threshold, and understanding the self-healing daemon trigger.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/agentic-disk-reclamation.md
  - ../reference/disk-reclaim-api.md
  - ../reference/disk-reclaim-deterministic-enumeration.md
  - ../reference/disk-reclaim-telemetry.md
  - ./configure-disk-health-check.md
  - ./reclaim-disk-space-and-run-low-space-rust-builds.md
  - ./inspect-and-clean-engineer-worktrees.md
---

# Configure and run disk reclamation

Simard reclaims disk agentically: an agent inspects live host state, reasons
about what is safely reclaimable, and a deterministic Rust executor performs the
reclamation behind non-bypassable safety rails. It self-heals when disk usage
crosses a configurable threshold, and you can also run it by hand.

This guide shows how to run it, read its reports, tune it, and diagnose it. For
*why* it works this way, see
[Agentic disk reclamation](../concepts/agentic-disk-reclamation.md).

## When to use this

Use this guide when:

- disk is filling and you want Simard to reclaim it now,
- you want to preview (dry-run) what reclamation *would* remove,
- you want to change the `85%` trigger/target threshold,
- the daemon logged a reclamation run and you want to read the report,
- a candidate landed on the **human-review** list and you need to act on it.

## Run reclamation from the CLI

The operator entry point is `simard disk-reclaim`. **It is dry-run by default** —
running it with no flags performs the full agentic analysis and guard vetting
but makes **zero destructive changes**:

```bash
# Dry-run (default): full would-remove report, nothing deleted.
simard disk-reclaim
```

Typical output:

```text
disk-reclaim (dry-run) — home partition 88% used, target 85%
WOULD REMOVE  tracked_worktree  ~/.simard/engineer-worktrees/goal-1841-...   3.9G  pr #1841 merged, idle
WOULD REMOVE  stale_build_cache /home/azureuser/src/Simard/worktrees/feat-x/target  6.1G  stale target/
WOULD REMOVE  orphan_dir        ~/.simard/engineer-worktrees/leftover-9f3   1.2G  de-registered, no gitdir
SKIP (review) tracked_worktree  ~/src/amplihack-rs/worktrees/wip-parser     2.0G  unpushed commits not in a merged/closed PR
SKIP (review) tracked_worktree  ~/src/Simard/worktrees/main                 —     protected path (daemon WorkingDirectory)
projected: 11.2G reclaimable → 85% used after; 2 candidates need human review
```

Nothing is deleted in dry-run. `WOULD REMOVE` shows what a live run would
reclaim (largest-first); `SKIP (review)` shows candidates a rail refused —
these go to the human-review list, never auto-deleted.

To actually reclaim, pass `--apply`:

```bash
# Live: perform guarded reclamation until under the target %-used.
simard disk-reclaim --apply
```

```text
disk-reclaim (apply) — home partition 88% used, target 85%
REMOVED  stale_build_cache /home/azureuser/src/Simard/worktrees/feat-x/target  6.1G
REMOVED  tracked_worktree  ~/.simard/engineer-worktrees/goal-1841-...          3.9G  (git worktree remove --force)
REMOVED  orphan_dir        ~/.simard/engineer-worktrees/leftover-9f3           1.2G  (rm -rf)
reclaimed 11.2G — home partition now 84% used (target 85% reached); 2 candidates left for human review
```

The executor reclaims **largest-first and stops** as soon as usage drops under
the target, so it removes the minimum needed. Everything it removes has passed
**all** safety rails re-checked immediately before deletion.

### Machine-readable report

For scripting or telemetry pipelines:

```bash
simard disk-reclaim --report-json | jq .
```

```json
{
  "mode": "dry_run",
  "used_pct_before": 88,
  "used_pct_after": 88,
  "target_pct": 85,
  "bytes_freed": 0,
  "removed": [],
  "would_remove": [
    { "path": "/home/azureuser/src/Simard/worktrees/feat-x/target",
      "kind": "stale_build_cache", "est_bytes": 6543212544, "reason": "stale target/" }
  ],
  "skipped": [
    { "path": "/home/azureuser/src/amplihack-rs/worktrees/wip-parser",
      "kind": "tracked_worktree", "reject_reason": "uncommitted_or_unpushed" },
    { "path": "/home/azureuser/src/Simard/worktrees/main",
      "kind": "tracked_worktree", "reject_reason": "protected_path" }
  ],
  "failures": []
}
```

`--report-json` is compatible with `--apply` (`"mode": "apply"`, populated
`removed`/`bytes_freed`).

### Override the target threshold for one run

```bash
# Reclaim down to 80% instead of the configured default.
simard disk-reclaim --apply --target-pct 80
```

`--target-pct` accepts `1`–`99`; out-of-range values are clamped. It overrides
`SIMARD_DISK_RECLAIM_PCT` for that invocation only.

## Configure the threshold

The single knob is the `%-used` trigger and target, set via environment:

| Variable | Effect | Default |
| -------- | ------ | ------- |
| `SIMARD_DISK_RECLAIM_PCT` | `df` `%-used` at which the daemon trigger fires, and the target the executor reclaims down to. Clamped to `[1, 99]`. | `85` |
| `SIMARD_DISK_RECLAIM_DAEMON_APPLY` | Whether the **daemon** self-heal trigger is allowed to *delete*. Unset/`0` = daemon runs dry-run + human-review only (default, ships disabled until OS-level recipe-step confinement is implemented). `1` = daemon reclaims for real. Does not affect the CLI (`--apply` is always honored there). | unset (dry-run) |
| `SIMARD_DISK_HEALTH_INTERVAL_SECS` | Cadence of the cheap daemon `df` probe that decides whether to launch reclamation (reused from the disk-health check). | `900` |
| `SIMARD_GIT_PROTECTED_REPOS` | Comma-separated extra repo roots added to the protected deny-set (never reclaimable). | unset |
| `SIMARD_STATE_ROOT` | State root (`~/.simard`) — where engineer worktrees, backups, and shared cargo targets live. | `$HOME/.simard` |

### Deterministic reclaimable-set thresholds

Routine reclaim also runs a **deterministic enumerator** that always proposes the
regenerable space hogs routine reclaim previously ignored (the idle
`self-deploy-target` build tree, the shared state-root build caches, and stale
engineer worktrees) so a cycle frees real space *before* emergency thresholds —
it no longer logs `freed 0 bytes` while `%-used` climbs. These knobs tune it;
both have conservative defaults and a **safe floor** (`0`/empty/invalid never
means "purge now"):

| Variable | Effect | Default |
| -------- | ------ | ------- |
| `SIMARD_DISK_RECLAIM_BUILD_IDLE_DAYS` | An idle build tree (`self-deploy-target`/`cargo-target`/`shared-target`) is proposed only if older than this **and** no live PID references it. | `1` |
| `SIMARD_DISK_RECLAIM_WORKTREE_IDLE_DAYS` | A stale engineer worktree is proposed only if idle beyond this (still subject to dirty/unpushed/PR-state vetoes). | `7` |

Snapshot / backup / corruption-quarantine retention is **not** configured here —
those directories are owned by the maintenance thread and its
`SIMARD_MAINTENANCE_KEEP_SNAPSHOTS` / `_KEEP_BACKUPS` / `_KEEP_CORRUPT` knobs.

See [Deterministic reclaimable-set enumeration](../reference/disk-reclaim-deterministic-enumeration.md)
for the full contract, the maintenance-ownership boundary, and the live-state
protections (live `cognitive`/`.wal`/`.shadow` and all snapshot/backup/corrupt
dirs are never enumerated by this path).

Set the threshold for the daemon by exporting it in the service environment:

```bash
# e.g. reclaim earlier, at 80% instead of 85%
systemctl --user set-environment SIMARD_DISK_RECLAIM_PCT=80
systemctl --user restart simard-ooda
```

There is **no** threshold buried in the recipe prompt and no Rust recompile
needed — the daemon reads `SIMARD_DISK_RECLAIM_PCT` each maintenance tick.

## The self-healing daemon trigger

Once per `SIMARD_DISK_HEALTH_INTERVAL_SECS`, in the OODA/overseer maintenance
path, the daemon runs a cheap deterministic probe:

```bash
df --output=pcent /home | tail -1     # the same probe the daemon uses
```

If usage exceeds `SIMARD_DISK_RECLAIM_PCT`, it launches the agentic reclaim
capability automatically.

### Dry-run + human-review by default

**The daemon trigger ships in dry-run + human-review mode.** By default it does
the full analysis and guard vetting and logs what it *would* reclaim, but
deletes **nothing** until you opt in:

```bash
# Promote the daemon to closed-loop self-healing (only after the OS-level
# recipe-step confinement described in the concept/API docs is implemented).
systemctl --user set-environment SIMARD_DISK_RECLAIM_DAEMON_APPLY=1
systemctl --user restart simard-ooda
```

Until that flag is set, use `simard disk-reclaim --apply` to reclaim by hand
after reviewing the daemon's would-remove report. `emergency_cleanup` still runs
as the deterministic hard stop regardless of this flag.

### Observe it

The daemon writes a dashboard-visible line to `~/.simard/ooda.log` (also
surfaced in the dashboard **Logs** tab) via `daemon_log`:

```bash
grep "disk reclaim" ~/.simard/ooda.log | tail -5
```

```text
[2026-07-07T15:42:01Z] disk reclaim (dry-run): 88% used, would free ~12026531840 bytes across 3 paths, 2 skipped for review
[2026-07-07T15:43:02Z] disk reclaim: 84% used, under threshold, no run
```

With `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`, the apply-mode line looks like:

```text
[2026-07-07T15:42:01Z] disk reclaim: 88% -> 84% used, freed 12026531840 bytes, 3 paths removed, 2 skipped for review
```

For the full per-candidate detail, read the machine-readable report
(`simard disk-reclaim --report-json`) or the on-disk metrics snapshot — see
[Disk reclaim telemetry](../reference/disk-reclaim-telemetry.md).

> **Note:** The admission `ReclaimFirst` continuation (when engineer admission
> defers a spawn due to disk pressure) also drives the reclaim capability, so a
> deferred spawn triggers a reclamation attempt before retrying. That path
> warn-logs failures via `tracing` (`target: simard::ooda_brain`) and never
> fails the cycle.

## Read the human-review list

Candidates a rail refused are **never deleted** and are surfaced for a human.
They appear as `SKIP (review)` in the CLI, in `skipped[]` of `--report-json`,
and as `WARN` tracing from the daemon. Each carries a `reject_reason`:

| `reject_reason` | Meaning | What to do |
| --------------- | ------- | ---------- |
| `protected_path` | `worktrees/main` or a daemon working directory | nothing — correctly protected |
| `live_process` | a live PID references the path | wait for the process to exit, or stop it |
| `uncommitted_or_unpushed` | dirty tree, or commits not in a merged/closed PR | push/commit the work, or confirm it is disposable and remove by hand |
| `active_worktree` | an active recipe/engineer worktree (tmux/PID) | let it finish |
| `outside_allow_root` | not under an allow-root, or symlink/canonicalize refused | inspect manually; the tool will not touch it |
| `unknown_pr_state` | the agent could not confirm the PR is merged/closed | check the PR (`gh pr view`); reclamation refuses to guess |

To resolve an `uncommitted_or_unpushed` case, inspect the worktree and, if the
work is genuinely disposable, remove it with the guarded operator GC (which
applies the same uncommitted/unpushed guard):

```bash
git -C /home/azureuser/src/Simard worktree list --porcelain | grep -A2 wip-parser
simard worktree-gc --apply   # still refuses if it detects unsaved/unpushed work
```

## Run just the analysis (candidate proposal)

Routine reclaim gathers candidates from **two** sources, merged together and both
re-vetted by the guard: a **deterministic Rust enumerator** (always proposes the
regenerable set — see
[Deterministic reclaimable-set enumeration](../reference/disk-reclaim-deterministic-enumeration.md))
and the **recipe agent** (additive proposals; only ever *proposes*, never
deletes). If the recipe fails, it is non-fatal — the deterministic enumerator
still yields candidates, so reclaim frees space anyway.

To see the raw candidate JSON the agent emits (useful when diagnosing why
something was or was not nominated):

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-reclaim.yaml \
  --output-format json \
  -c state_root="$HOME/.simard" \
  -c repo_root="/home/azureuser/src/Simard" \
  | jq -r '.step_results[0].output' \
  | grep -E '^(DISK_USED_PCT|CANDIDATES_JSON|CANDIDATES_SCHEMA)='
```

The executor consumes this proposal and re-vets every candidate. To feed a
candidate file straight into the guarded executor (the path the recipe uses
internally):

```bash
# @file reads from a file; @- reads candidate JSON from stdin.
simard disk-reclaim exec --candidates @candidates.json           # dry-run
simard disk-reclaim exec --candidates @candidates.json --apply   # live, guarded
```

`exec` re-validates **every** path through `guard::vet_candidate` regardless of
what the JSON claims — a hand-edited candidate list cannot make the executor
delete a protected path.

## Safety notes

- **Dry-run is the default.** You must pass `--apply` for any deletion.
- **Refuses to run as root.** `--apply` (and the daemon apply path) refuse when
  `geteuid() == 0`; running as root would nullify the path-ownership policy.
- **No force-flags on git.** The executor never passes `--admin` or
  `--no-verify` to any git command.
- **Nothing is guessed.** Any inconclusive signal → skip → human review.

## Diagnose a failed run

If the daemon logs `disk reclaim failed` or the CLI exits non-zero:

### 1. `recipe-runner-rs` not installed

```bash
which recipe-runner-rs
```

If missing, reclamation cannot propose candidates. The daemon warns and
continues; `disk_pressure` and `emergency_cleanup` remain as hard stops.

### 2. Recipe YAML missing

```bash
ls -la prompt_assets/simard/recipes/disk-reclaim.yaml
```

Missing → `AdapterInvocationFailed`, no fallback; the daemon warns and continues.

### 3. Candidate JSON did not parse

If the run reports a parse error, the agent step's `CANDIDATES_JSON=` marker was
malformed. Inspect it:

```bash
recipe-runner-rs prompt_assets/simard/recipes/disk-reclaim.yaml \
  --output-format json -c state_root="$HOME/.simard" \
  -c repo_root="/home/azureuser/src/Simard" | python3 -m json.tool
```

A malformed **array** is a hard error (no reclamation); a single malformed
**element** is skipped and reported — the run continues with the valid ones.

### 4. Persistent pressure after a full run

If reclamation runs, reclaims everything it safely can, and disk is still above
the target, the largest remaining consumers are on the human-review list or are
the main worktree's `target/`. See
[Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md)
for the manual `scripts/reclaim-build-space` path, and check for non-Simard
consumers:

```bash
du -sh /home/azureuser/* 2>/dev/null | sort -h | tail -10
```

## Related

- [Reclaim disk with the simard disk tool](./reclaim-disk-with-the-simard-disk-tool.md) — the agent-facing `simard disk reclaim`/`report` tool the disk-health recipe calls to act
- [The simard disk tool (reference)](../reference/simard-disk-tool.md) — CLI grammar, exit codes, guard reasons
- [Agentic disk reclamation (concept)](../concepts/agentic-disk-reclamation.md) — design rationale, the rails, "agent proposes, Rust disposes"
- [Disk reclaim API (reference)](../reference/disk-reclaim-api.md) — module API, guard, executor, recipe contract
- [Deterministic reclaimable-set enumeration (reference)](../reference/disk-reclaim-deterministic-enumeration.md) — the enumerator, keep-N/age thresholds, emergency alignment
- [Disk reclaim telemetry (reference)](../reference/disk-reclaim-telemetry.md) — emitted metrics
- [Configure the disk health check](./configure-disk-health-check.md) — the superseded per-cycle check
- [Reclaim disk space and run low-space Rust builds](./reclaim-disk-space-and-run-low-space-rust-builds.md) — manual build-artifact scripts
- [Inspect and clean engineer worktrees](./inspect-and-clean-engineer-worktrees.md) — manual worktree operations
