---
title: Configure and monitor the monthly self-quality-audit
description: Operator guide for Simard's recurring ~monthly self-quality-audit — enabling/disabling and tuning the SIMARD_SELF_AUDIT_INTERVAL cadence, understanding the disk-backed last-run persistence that survives restarts, observing fire/completion logs and the audit's pull requests, manually triggering a run, and diagnosing failures.
last_updated: 2026-07-02
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../architecture/monthly-self-quality-audit.md
  - ../reference/self-quality-audit-api.md
  - ./configure-brain-introspection.md
  - ./configure-disk-health-check.md
---

# Configure and monitor the monthly self-quality-audit

Simard runs a recurring **self-quality-audit** on its own interval (default
~monthly). Each run drives five SEEK→VALIDATE→FIX waves of the amplihack
`quality-audit` skill against her own repository, has the `crusty-old-engineer`
skill proxy-review every resulting pull request, and self-merges the PRs that
are crusty-approved and CI-green.

This guide shows how to enable, tune, observe, and troubleshoot the audit. For
design rationale see
[Monthly self-quality-audit](../architecture/monthly-self-quality-audit.md); for
the API contract see the
[Self-quality-audit API](../reference/self-quality-audit-api.md).

## When to use this

Use this guide when:

- You want to change how often the audit runs (or disable it)
- The daemon logged `self quality-audit: starting …` or `self quality-audit:
  complete …` and you want to read the results
- The daemon logged `WARN: self quality-audit failed` and you need to diagnose it
- You want to run the audit manually without waiting for the monthly cadence
- You need to understand why the audit did (or did not) fire after a restart

## Enable, disable, and tune

The single knob is an environment variable, read once at daemon start (set it
before launching the daemon):

| Knob    | Env var                      | Default   | What it controls                                    |
| ------- | ---------------------------- | --------: | --------------------------------------------------- |
| Cadence | `SIMARD_SELF_AUDIT_INTERVAL` | `2592000` | **Seconds** between runs; **`0` disables the audit** |

Examples:

```bash
# Run every 2 weeks instead of ~monthly
export SIMARD_SELF_AUDIT_INTERVAL=1209600

# Disable the audit entirely
export SIMARD_SELF_AUDIT_INTERVAL=0

# Restore the default ~30-day cadence (or just unset it)
unset SIMARD_SELF_AUDIT_INTERVAL
```

> **The value is in seconds, and the name has no `_SECS` suffix.** Unlike the
> sibling `SIMARD_DISK_HEALTH_INTERVAL_SECS` / `SIMARD_BRAIN_INTROSPECTION_INTERVAL_SECS`
> knobs, this one is `SIMARD_SELF_AUDIT_INTERVAL` (still seconds). This is
> intentional — it matches the feature's specification. A garbage value falls
> back to the ~30-day default; only an explicit `0` disables the audit.

Confirm the configured interval at daemon start:

```bash
grep "self quality-audit interval" ~/.simard/ooda.log | tail -1
# [2026-07-02T09:00:00Z] [simard] OODA daemon: self quality-audit interval = 2592000s
```

## How the cadence survives restarts

Unlike the other periodic tasks (which reset an in-memory timer on every
restart), the self-quality-audit **persists its last-run time to disk** so a
~monthly cadence is not reset by deploys, crashes, or reboots.

The last-run wall-clock timestamp (epoch seconds) lives here:

```bash
cat ~/.simard/self_quality_audit_last_run
# 1751446800
date -d @"$(cat ~/.simard/self_quality_audit_last_run)"
# Tue Jul  1 09:00:00 UTC 2026
```

Behavior you can rely on:

- **First run / fresh deploy.** If the file is absent or unreadable at startup,
  the daemon writes `now` and does **not** audit this cycle. The first audit
  fires ~one interval later — a heavy five-wave audit never fires instantly on
  a fresh install.
- **Restart mid-interval.** On restart the daemon reads the recent epoch back,
  sees only a small elapsed time, and correctly declines to fire until a full
  interval of wall-clock time has passed. The audit fires **~monthly, not on
  every restart**.
- **After a run (success or failure).** The daemon rewrites the file to the run
  time regardless of outcome, so a failing audit retries next month — not next
  cycle.

To force the *next* audit to run on the next cycle (e.g. after changing the
recipe), delete the marker and restart the daemon — it will re-init to `now`,
so instead set it to an old epoch:

```bash
# Make the audit "due" immediately on next daemon start
echo 0 > ~/.simard/self_quality_audit_last_run
```

Writing `0` (or any epoch older than one interval ago) makes `elapsed >=
interval` true, so the audit fires on the next cycle. (An *absent* file
re-inits to now and waits a full interval — the two are deliberately different.)

## Observe a run

The daemon logs a **fire** line when the audit starts and a **completion** line
when it finishes:

```bash
grep "self quality-audit" ~/.simard/ooda.log | tail -5
```

Typical output:

```
[2026-08-01T09:00:01Z] [simard] self quality-audit: starting 5-wave crusty-gated self-audit
[2026-08-01T10:14:32Z] [simard] self quality-audit: complete — 5 waves, 4 PRs opened, 3 merged, 1 crusty-unresolved
```

The real results are **pull requests**, not a log line or a repo doc. Inspect
them with `gh`:

```bash
# PRs the audit opened/merged (five-wave quality-audit fixes)
gh pr list --repo rysweet/Simard --state all --search "author:@me" --limit 20

# PRs crusty left unresolved (open, awaiting human follow-up)
gh pr list --repo rysweet/Simard --state open --search "author:@me"
```

A completion line reporting `crusty-unresolved` > 0 means one or more PRs hit
the 3-round crusty budget without approval and were left open **for you** to
review — they are not force-merged.

## Manually trigger a run

To run the agentic recipe directly (the same way the daemon invokes it), use
`recipe-runner-rs`, supplying the `record_path` the hook would derive:

```bash
recipe-runner-rs prompt_assets/simard/recipes/monthly-self-quality-audit.yaml \
  --output-format json \
  -c state_root="$HOME/.simard" \
  -c repo_path="/home/azureuser/src/Simard" \
  -c record_path="$HOME/.simard/self_quality_audit/record.json"
```

The recipe's final ACT step calls `simard cognition record-self-quality-audit`,
which writes the typed record the hook reads back. Inspect it directly:

```bash
cat "$HOME/.simard/self_quality_audit/record.json" | jq .
```

```json
{
  "schema": "self-quality-audit/v1",
  "written_at_epoch": 1793558400,
  "waves_completed": 5,
  "prs_opened": ["https://github.com/rysweet/Simard/pull/2601"],
  "prs_merged": ["https://github.com/rysweet/Simard/pull/2601"],
  "crusty_approved": ["https://github.com/rysweet/Simard/pull/2601"],
  "crusty_unresolved": [],
  "summary_line": "5 waves, 4 PRs opened, 3 merged, 1 crusty-unresolved"
}
```

> **Note:** Running the recipe directly is the *whole* audit in this feature —
> the Rust hook (`run_self_quality_audit`) is a pure invoker with no deterministic
> pre-work, so there is no daemon-side half to miss (unlike brain introspection).
> The only thing the daemon adds is the interval gate, the last-run persistence,
> and the fire/completion logging.

To exercise the full daemon path on demand (interval gate + persistence +
logging), start the daemon with a short interval and a due marker, for dev/test
only:

```bash
echo 0 > ~/.simard/self_quality_audit_last_run           # make it due
SIMARD_SELF_AUDIT_INTERVAL=60 simard ooda run --cycles=2  # fire ~1 min in
grep "self quality-audit" ~/.simard/ooda.log | tail -3
```

To see only the recipe summary line (no step output), omit `--output-format json`:

```bash
recipe-runner-rs prompt_assets/simard/recipes/monthly-self-quality-audit.yaml \
  -c state_root="$HOME/.simard" -c repo_path="/home/azureuser/src/Simard"
# Recipe: monthly-self-quality-audit SUCCESS
```

## Diagnose a failed run

If the daemon logs `WARN: self quality-audit failed`, check these in order.
A failure never blocks the OODA cycle (fail-open) and never hot-loops (last-run
is persisted on failure, so the next attempt is next interval).

### 1. `recipe-runner-rs` not installed

```bash
which recipe-runner-rs
```

If missing, the audit cannot run but the daemon continues. Install
`recipe-runner-rs` from the amplihack toolchain.

### 2. Recipe YAML missing

```bash
ls -la prompt_assets/simard/recipes/monthly-self-quality-audit.yaml
```

If absent (deleted, or a detached worktree without it), the shim returns
`AdapterInvocationFailed` and the daemon warns and continues. The hook also
checks the hot-reload path
`~/.simard/prompt_assets/simard/recipes/monthly-self-quality-audit.yaml` first.

### 3. `gh` not authenticated (waves/merge fail)

```bash
gh auth status
```

The waves open PRs and the self-merge step uses `gh`. If `gh` is
unauthenticated, the run cannot open or merge PRs; the completion summary will
show `0 PRs opened`.

### 4. Record read failure (`R{n}`) in the daemon log

The hook reads the recipe's typed record **fail-closed** over the R1–R7 matrix. A
`WARN: self quality-audit failed: self-quality-audit record R{n}: …` line means the
recipe ran but wrote no valid record. Common cases:

- **R1** — no record at `~/.simard/self_quality_audit/record.json`: the recipe never
  reached its final `record-self-quality-audit` ACT step (e.g. it aborted mid-audit).
- **R5** — bounds break: `waves_completed > 5`, an over-long URL list, or an unknown
  field.
- **R6** — empty `summary_line`: the recipe must record a non-empty terminal summary.

Inspect the record the recipe wrote:

```bash
cat "$HOME/.simard/self_quality_audit/record.json" | jq .
```

See the [R1–R7 read matrix](../reference/record-brain-introspection-self-audit-cli.md#the-fail-closed-read-matrix-r1r7)
for the full table.

### 5. Audit isn't firing when you expect

```bash
# Is it disabled?
grep "self quality-audit interval" ~/.simard/ooda.log | tail -1   # =0s means disabled

# When did it last run?
date -d @"$(cat ~/.simard/self_quality_audit_last_run 2>/dev/null || echo 0)"
```

Remember the **init-to-now** rule: a *missing* last-run file waits a full interval
before the first run. If you want it due now, write an old epoch
(`echo 0 > ~/.simard/self_quality_audit_last_run`) rather than deleting the file.

## Understand the crusty gate

Every PR the waves open is proxy-reviewed by `crusty-old-engineer` on operator
Ryan's behalf, looping up to **3 rounds**:

- **Approved within 3 rounds** → the PR self-merges **iff CI is also green** (recorded
  under `prs_merged`). Branch protection is respected.
- **Still unsatisfied after 3 rounds** → the PR is **left open** (recorded under
  `crusty_unresolved`) for you to review. It is never force-merged.

So the completion summary's `merged` count is always PRs that passed **both**
crusty and CI, and `crusty-unresolved` is your human follow-up queue.

## Related

- [Monthly self-quality-audit (architecture)](../architecture/monthly-self-quality-audit.md) — design rationale, restart-persistence, safety model
- [Self-quality-audit API (reference)](../reference/self-quality-audit-api.md) — module API, structs, typed record, config
- [record-brain-introspection / record-self-quality-audit CLI](../reference/record-brain-introspection-self-audit-cli.md) — the gated record verb, schema, and R1–R7 read matrix
- [Configure and monitor brain introspection](./configure-brain-introspection.md) — the sibling periodic task whose pattern this reuses
- [Configure and monitor the disk health check](./configure-disk-health-check.md) — the pure recipe-invoker sibling
