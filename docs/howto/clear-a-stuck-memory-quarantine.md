---
title: How to clear a stuck memory quarantine
description: Operator runbook for the self-health `no_quarantine` deadlock (#4469) — how to recognize a genuinely-stuck cognitive-memory quarantine that freezes self-deploy, acknowledge it with `simard self-health --acknowledge-quarantine` so the probe clears WITHOUT deleting the #2550 recovery asset, confirm convergence, and reverse the acknowledgement if needed.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: howto
status: implemented
related:
  - ../reference/self-deploy-quarantine-acknowledge.md
  - ../reference/self-deploy-api.md
  - ../concepts/reconcile-and-self-deploy.md
  - ../howto/verify-and-roll-back-a-self-deploy.md
---

# How to clear a stuck memory quarantine

> **Status: implemented.** `simard self-health --acknowledge-quarantine` writes
> a durable `.ack` sidecar next to each cognitive-memory quarantine artifact so
> the `no_quarantine` probe can clear **without** deleting the artifact. The
> underlying convention is documented in the
> [quarantine-acknowledge reference](../reference/self-deploy-quarantine-acknowledge.md).

## When to use this

Use this runbook when self-deploy is frozen **only** because of a quarantine
that can never clear on its own — the deadlock from issue #4469:

- `simard self-health` reports `[FAIL] no_quarantine   quarantined=true`, **and**
- the overseer keeps emitting "DeployDrift — running binary is N commit(s) behind
  merged main", **and**
- the offending artifact is the long-lived **recovery asset** (the largest
  `cognitive*.corrupt-<ts>` file, which `simard cleanup` deliberately never
  deletes — see issue #2550).

If the quarantine is **fresh** (recent corruption you have not yet
investigated), do **not** acknowledge it — investigate the corruption first. The
autonomous auto-ack only ever touches the protected recovery asset once it is
older than the 30-day forensic window; everything else stays red by design.

## Step 1 — Confirm the deadlock

```console
$ simard self-health
simard self-health: UNHEALTHY
  [ok  ] version_advanced   running=<commit> target=<commit>
  [ok  ] memory_intact      live_facts=1206 baseline=n/a
  [ok  ] goal_board_intact  active_goals=5
  [ok  ] brains_llm_backed  fallback_records=0
  [FAIL] no_quarantine      quarantined=true
  [ok  ] entrypoint_parity  path=/home/you/.local/bin/simard version=simard 0.35.0 mismatch=false foreign=false
```

Only `no_quarantine` is red, and the artifact is the retained recovery asset.
Inspect what is present under the state root:

```console
$ ls -1 ~/.simard/ | grep '\.corrupt-'
cognitive.corrupt-20260601T090412Z        # large recovery asset — retained by #2550
```

(If `SIMARD_STATE_ROOT` is set, look there instead — the probe, the
acknowledge path, and `simard cleanup` all resolve the same root.)

## Step 2 — Acknowledge the quarantine

```console
$ simard self-health --acknowledge-quarantine
simard self-health: HEALTHY
  [ok  ] version_advanced   running=<commit> target=<commit>
  [ok  ] memory_intact      live_facts=1206 baseline=n/a
  [ok  ] goal_board_intact  active_goals=5
  [ok  ] brains_llm_backed  fallback_records=0
  [ok  ] no_quarantine      quarantined=false
  [ok  ] entrypoint_parity  path=/home/you/.local/bin/simard version=simard 0.35.0 mismatch=false foreign=false
```

This writes an `.ack` sidecar next to each present quarantine artifact and
re-runs the probe. The artifact is **not** deleted:

```console
$ ls -1 ~/.simard/ | grep '\.corrupt-'
cognitive.corrupt-20260601T090412Z          # still here — recovery asset retained
cognitive.corrupt-20260601T090412Z.ack      # acknowledgement sidecar
```

The command is idempotent — running it again is safe and reports the artifacts
as already acknowledged. Exit code is `0` once every probe is healthy.

## Step 3 — Confirm self-deploy converges

With `no_quarantine` green, `all_healthy()` reaches `true`, the post-deploy
health check passes, and the next deploy is accepted instead of rolled back:

```console
$ simard self-deploy
# … canary + gates pass, swap accepted, health check HEALTHY …

$ simard self-health
simard self-health: HEALTHY
```

The recurring "DeployDrift — N commit(s) behind merged main" signal stops once
the running binary advances to merged `main`.

## Reversing an acknowledgement

Acknowledgement is reversible. Delete the sidecar to make the probe count the
artifact again:

```console
$ rm ~/.simard/cognitive.corrupt-20260601T090412Z.ack
$ simard self-health          # no_quarantine reddens again
```

Deleting the sidecar never affects the quarantine artifact itself.

## What this does *not* do

- It does **not** delete the quarantine artifact — the #2550 recovery asset is
  retained so you can still salvage records from it.
- It does **not** silence *future* corruption. A new corruption event writes a
  new `cognitive.corrupt-<ts>` artifact with no sidecar, so `no_quarantine`
  reddens again immediately and self-deploy blocks — exactly as intended.
- It does **not** change any other probe or the `self-health` exit-code
  convention.

## See also

- [Self-deploy quarantine-acknowledge reference](../reference/self-deploy-quarantine-acknowledge.md)
  — the `.ack` convention, the `quarantine_ack` API, and the guarded auto-ack.
- [Self-deploy API reference](../reference/self-deploy-api.md#simard-self-health)
  — the `simard self-health` subcommand and the six probes.
- [Verify and roll back a self-deploy](../howto/verify-and-roll-back-a-self-deploy.md).
