---
title: Configure and monitor brain introspection
description: Operator guide for Simard's periodic brain self-examination and memory-hygiene pass (#2419) — enabling/disabling the cadence, tuning the prune cap and baseline window, reading the brain-introspection GitHub issue and metrics, manually triggering a run, and diagnosing failures.
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../architecture/brain-introspection.md
  - ../reference/brain-introspection-api.md
  - ./configure-disk-health-check.md
  - ./configure-episode-hygiene-and-promotion.md
---

# Configure and monitor brain introspection

Simard runs a periodic **brain self-examination + memory-hygiene** pass on its
own interval (default daily). Each run examines OODA brain decision quality,
mines recent activity for patterns, safely trims a bounded amount of memory,
consolidates episodes, and writes findings to a GitHub issue.

This guide shows how to enable, tune, observe, and troubleshoot the pass. For
design rationale see [Brain introspection + memory
hygiene](../architecture/brain-introspection.md); for the API contract see the
[Brain introspection API](../reference/brain-introspection-api.md).

## When to use this

Use this guide when:

- You want to change how often the pass runs (or disable it)
- You want to change the safe-prune cap or the regression baseline window
- The daemon logged `brain introspection: …` and you want to read the findings
- The daemon logged `brain introspection failed` and you need to diagnose it
- You want to run the introspection pass manually without waiting for the cadence

## Enable, disable, and tune

All knobs are environment variables, read once at daemon start (set them before
launching the daemon):

| Knob            | Env var                                      | Default | What it controls                                            |
| --------------- | -------------------------------------------- | ------: | ----------------------------------------------------------- |
| Cadence         | `SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS`   | `86400` | Seconds between runs; **`0` disables the pass**             |
| Safe-prune cap  | `SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE`       | `25`    | Ceiling on value-bearing prune **recommendations** per run (does not throttle expired-sensory cleanup) |
| Baseline window | `SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS`   | `7`     | Number of prior runs used as the regression baseline        |

Examples:

```bash
# Run every 6 hours instead of daily
export SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS=21600

# Disable the pass entirely
export SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS=0

# Allow more value-bearing prune recommendations per run (still absolute)
export SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE=50

# Compare against the last 14 runs instead of 7
export SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS=14
```

Confirm the configured interval at daemon start:

```bash
grep "brain introspection interval" ~/.simard/ooda.log | tail -1
# [2026-06-27T09:00:00Z] [simard] OODA daemon: brain introspection interval = 86400s
```

> **The cap is absolute, not a percentage.** A cap of `25` means the recipe may
> recommend at most 25 **value-bearing** memories for removal in a single run,
> regardless of total memory size. This is the core SAFETY bound. It does **not**
> throttle expired-sensory cleanup (`prune_expired_sensory` removes only
> already-expired transient rows — non-discretionary, never capped), and no
> value-bearing memory is auto-deleted in this increment at all (candidates are
> recommended, not removed).

## Observe a run

The daemon logs a one-liner per run:

```bash
grep "brain introspection" ~/.simard/ooda.log | tail -5
```

Typical output:

```
[2026-06-27T09:00:01Z] brain introspection: 3 health findings, 2 patterns, 4 prune candidates, 11 sensory pruned, 6 consolidated, issue=https://github.com/rysweet/Simard/issues/2531
```

The full findings live in the **GitHub issue**, not in the log or a repo doc:

```bash
# Open issues this pass has filed/updated (stable title ⇒ deduped)
gh issue list --repo rysweet/Simard --label brain-introspection
```

The issue contains the brain-health summary, detected patterns, the
`PRUNE_CANDIDATE` list (memories recommended for review, **not** auto-deleted),
and the consolidation result. Repeated runs **update** the same issue rather
than opening new ones.

## Read the metrics

Each run appends to `~/.simard/metrics/metrics.jsonl`:

```bash
grep brain_introspection ~/.simard/metrics/metrics.jsonl | tail -8
```

| Metric                                | Meaning                                        |
| ------------------------------------- | ---------------------------------------------- |
| `brain_introspection_live_memories`   | non-sensory live memory count at run start (working + episodic + semantic + procedural + prospective) |
| `brain_introspection_sensory_pruned`  | already-expired transient sensory rows removed |
| `brain_introspection_prune_requested` | value-bearing prune candidates recommended     |
| `brain_introspection_consolidated`    | facts/procedures added (hook-measured delta)   |

These accumulate the rolling baseline; a run compares itself to the previous
`SIMARD_BRAIN_INTROSPECTION_BASELINE_RUNS` runs and reports `REGRESSION:` lines
in the issue when a signal worsens. The **first run** reports
`BRAIN_HEALTH: no prior baseline`.

## Manually trigger a run

To run the agentic recipe directly (the same way the daemon invokes it), use
`recipe-runner-rs` with the JSON envelope:

```bash
recipe-runner-rs prompt_assets/simard/recipes/brain-introspection.yaml \
  --output-format json \
  -c state_root="$HOME/.simard" \
  -c repo_path="/home/azureuser/src/Simard" \
  -c max_prune=25 \
  -c baseline_runs=7 \
  -c stats='{}'
```

This prints a JSON envelope whose `step_results[*].output` strings contain the
text markers the Rust shim parses:

```json
{
  "success": true,
  "step_results": [
    {
      "step_id": "brain-health",
      "output": "BRAIN_HEALTH: fallback rate 1.1% (baseline 1.0%) — nominal\nREGRESSION: 0-succeeded-action cycles up 3x vs baseline\n"
    },
    {
      "step_id": "output",
      "output": "PRUNE_REQUESTED=4\nCONSOLIDATED_FACTS=6\nISSUE_URL=https://github.com/rysweet/Simard/issues/2531\n"
    }
  ]
}
```

> **Note:** Running the recipe directly exercises only the *agentic* half
> (analysis + issue). The deterministic memory operations
> (`get_statistics`, `prune_expired_sensory`, `consolidate_episodes`)
> run in the Rust hook (`run_brain_introspection`), which the daemon calls on its
> interval. To exercise the **whole** pass on demand (without waiting a day),
> start the daemon with a short interval, observe one run, then restore the
> default:
>
> ```bash
> # Fire the full hook ~1 minute after daemon start, for dev/testing only
> SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS=60 simard daemon
> grep "brain introspection" ~/.simard/ooda.log | tail -1
> ```
>
> A first-class on-demand `simard` subcommand that runs the hook once is a
> documented follow-up; until then the short-interval override is the way to
> exercise the deterministic half end-to-end.

To see only the recipe summary line (no step output), omit `--output-format json`:

```bash
recipe-runner-rs prompt_assets/simard/recipes/brain-introspection.yaml \
  -c state_root="$HOME/.simard" -c repo_path="/home/azureuser/src/Simard"
# Recipe: brain-introspection SUCCESS
```

## Diagnose a failed run

If the daemon logs `brain introspection failed`, check these in order:

### 1. `recipe-runner-rs` not installed

```bash
which recipe-runner-rs
```

If missing, the pass cannot run but the daemon continues (fail-open). The
deterministic hygiene (steps 1–3) still runs because it executes **before** the
recipe spawn. Install `recipe-runner-rs` from the amplihack toolchain.

### 2. Recipe YAML missing

```bash
ls -la prompt_assets/simard/recipes/brain-introspection.yaml
```

If absent (deleted, or a detached worktree without it), the shim returns
`AdapterInvocationFailed` and the daemon warns and continues. The hook also
checks the hot-reload path `~/.simard/prompt_assets/simard/recipes/brain-introspection.yaml`
first.

### 3. `gh` not authenticated (issue step fails)

```bash
gh auth status
```

The `output` step uses `gh issue` to create/update the brain-introspection
issue. If `gh` is unauthenticated, the run still completes the analysis and
hygiene but emits no `ISSUE_URL=`; `summary()` shows `issue=none`.

### 4. Parse error mentioning BRAIN_HEALTH

The parser **requires** at least one non-empty `BRAIN_HEALTH:` line. If the
agent emitted none (e.g. it failed to read `metrics.jsonl`), the shim returns a
parse error. Inspect the raw output:

```bash
recipe-runner-rs prompt_assets/simard/recipes/brain-introspection.yaml \
  --output-format json \
  -c state_root="$HOME/.simard" -c repo_path="/home/azureuser/src/Simard" \
  | python3 -m json.tool
```

Check that `~/.simard/metrics/metrics.jsonl` and `~/.simard/ooda.log` exist and
are readable.

## Verify safety

The cap bounds the number of **value-bearing** prune *recommendations* per run,
and no value-bearing memory is auto-deleted in this increment at all. To confirm
the bound is respected, check the recommendation count against the cap:

```bash
grep brain_introspection_prune_requested ~/.simard/metrics/metrics.jsonl | tail -1
grep brain_introspection_sensory_pruned ~/.simard/metrics/metrics.jsonl | tail -1
```

`prune_requested` is always `<= SIMARD_BRAIN_INTROSPECTION_MAX_PRUNE`.
`sensory_pruned` counts only already-expired transient rows and is **not** bound
by the cap — that cleanup is non-discretionary (the rows are past their TTL).
Superseded and low-value memories are **recommended** in the issue, never
auto-deleted in this increment (see the
[safety model](../architecture/brain-introspection.md#safety-model)).

## Related

- [Brain introspection + memory hygiene (architecture)](../architecture/brain-introspection.md) — design rationale and safety model
- [Brain introspection API (reference)](../reference/brain-introspection-api.md) — module API, structs, markers, config
- [Configure and monitor the disk health check](./configure-disk-health-check.md) — the sibling periodic-recipe pass
- [Configure episode hygiene and promotion](./configure-episode-hygiene-and-promotion.md) — the per-cycle consolidation this pass reuses
