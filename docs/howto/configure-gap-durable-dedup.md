---
title: Configure and verify durable gap dedup
description: >
  How to operate and verify the Overseer's restart-safe, GitHub-side gap-filing
  dedup guard for workstream-gap stewardship issues — confirm a duplicate gap is
  reused rather than re-filed after a daemon restart, read the `overseer::gap_scan` reused/flagged
  logs, understand the fail-loud behaviour when `gh` is unavailable, and check
  the bounded GapCategory taxonomy that keeps titles stable and dedupable.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/overseer-gap-durable-dedup.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../reference/overseer-backoff-gate-api.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ./review-overseer-workstream-gaps.md
  - ./configure-overseer-gap-scan-backoff.md
  - ./file-stewardship-issues-from-orchestrator-runs.md
---

# Configure and verify durable gap dedup

The Overseer files stewardship issues for uncovered backlog work — uncovered
goals, high-signal open issues, and unaddressed telemetry anomalies. It now
dedupes those filings against **GitHub itself**, not just in-process memory, so
a daemon **restart** (or a second daemon) no longer re-files an issue that is
already open. This closes the near-duplicate `[stewardship] workstream_gap:*`
flood (observed on e.g. #4671, #4680, #4685).

For the data model, signature grammar, and guarantees, see the
[durable gap-filing dedup reference](../reference/overseer-gap-durable-dedup.md).

## What changed for operators

- **Before:** the filed `stewardship-signature:` was derived per run
  (`originating-run: overseer-<hash>`), so every restart/re-run minted a fresh
  signature that matched nothing on GitHub, and the only stable-ish guard (the
  in-process gate) went cold on restart → the next tick re-filed a near-duplicate
  stewardship issue.
- **Now:** the signature is a **stable, content-addressed** slug, so the
  Overseer's open-issue search actually matches an already-open issue carrying
  the gap's marker and **reuses** it. The guarantee is now *at most one open
  issue per distinct gap signature*, **across restarts**.
- **Legacy issues:** duplicates filed *before* this rail carry the old per-run
  hash signature (e.g. `stewardship-signature: f6f8480b146b171e`), so the
  slug-based search below only finds post-fix issues; old duplicates are not
  retro-reconciled.

There is **nothing to turn on** — the durable check is always active on the
gap-filing path whenever the Overseer runs. The existing knobs still apply:

| Env var | What it does | Default |
|---|---|---|
| `SIMARD_OVERSEER_GAP_SCAN` | Falsey (`0`/`false`/`no`/`off`) turns the gap-scan off entirely | on |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | Run the scan every *Nth* tick | `1` |

See [configure the gap-scan backoff](./configure-overseer-gap-scan-backoff.md)
for the in-process pre-filter window that sits in front of the durable check.

## Prerequisites

- The `gh` CLI is installed, on `PATH`, and authenticated against `rysweet/Simard`
  (or a token with `issues:write` scope). The durable check needs read (search)
  and, when filing, write.
- The acting Overseer is enabled (`SIMARD_OVERSEER_ENABLED` unset or truthy).

## Verify: a restart does not re-file

This is the core acceptance check for the workstream-gap dedup rail.

1. Let the Overseer file one gap issue and note its number and signature:

   ```bash
   gh issue list -R rysweet/Simard --search 'in:body stewardship-signature: workstream-gap:' \
     --state open --json number,title | jq '.[0]'
   ```

   Confirm the body carries the marker:

   ```bash
   gh issue view <number> -R rysweet/Simard --json body \
     | jq -r '.body' | grep 'stewardship-signature:'
   ```

2. **Restart the daemon** (the in-process gate is now cold).

3. On the next gap-scan tick, confirm **no new issue** was opened for the same
   signature — the open-issue count for that signature stays at **1**:

   ```bash
   gh issue list -R rysweet/Simard --state open \
     --search 'in:body stewardship-signature: workstream-gap:<signature>' \
     --json number | jq 'length'   # → 1, not 2
   ```

4. Confirm the reuse in the logs (see below).

## Read the logs

The rail emits structured `tracing` + OTel only (no `print!`/`println!`), on
`target: "overseer::gap_scan"`:

```text
INFO overseer::gap_scan flagged=0 reused_existing=1 suppressed=0
     key="workstream-gap:goal:g-1873"
     overseer gap-scan: matched an already-open issue; reused instead of re-filing
```

| Field | Meaning |
|---|---|
| `flagged` | New issues created this tick |
| `reused_existing` | Gaps matched to an already-open issue (no new issue) |
| `suppressed` | Gaps dropped by the in-process `WhisperGate` pre-filter |
| `key` | The batch `workstream_gap_key` for the tick |

A restart-then-reuse shows `flagged=0 reused_existing=1` — the proof the durable
check did its job.

## What happens when `gh` is unavailable (fail-loud)

If the open-issue search fails (unauthenticated, rate-limited, offline), the act
path **files nothing** and surfaces the error — it never falls back to a blind
create:

```text
ERROR overseer::gap_scan gh search failed; filing skipped (fail-loud)
      key="workstream-gap:goal:g-1873" reason="gh: HTTP 403 rate limit exceeded"
```

This is deliberate: a fail-*open* fallback would re-create the flood. Fix the
`gh` auth / rate-limit condition (often `gh auth login`) and the next tick
resumes normally. Contrast the in-process backoff gate, which fails *toward
surfacing* — together they mean a genuine gap is never silently dropped **and**
a duplicate is never blindly filed.

## The bounded taxonomy (why titles are now stable)

Duplicates used to slip through because free-form titles drifted between ticks.
Each gap resolves to a bounded `GapCategory` variant with a stable slug that
anchors the signature:

| Gap kind | `GapCategory` | Signature prefix |
|---|---|---|
| Uncovered p1/p2 goal | `GoalUncovered` | `goal:<goal_id>` |
| High-signal open issue | `IssueUncovered` | `issue:<repo>#<n>` |
| Unaddressed anomaly | `AnomalyUnaddressed` | `anomaly:<slug>` |

`GapCategory` is a closed enum of exactly these three kinds, so a gap's title and
signature are stable across ticks. The durable fix did not add kinds — it made
the filed `stewardship-signature:` a stable, content-addressed slug (instead of a
per-run hash) so an already-open issue for the same gap is found and reused. This
change is additive — existing `goal:`/`issue:`/`anomaly:` issues keep matching.

## Common pitfalls

- **A closed issue and the gap recurred → a new issue appeared.** Expected. The
  search uses `--state open`, so a closed cover issue does not suppress a fresh
  filing. Leave the issue open, or fix the underlying gap, to keep it deduped.
- **Two issues for the "same" gap.** Confirm they carry the **same**
  `stewardship-signature:`. If the signatures differ, the gap resolved to two
  distinct keys (e.g. two different goal ids) — that is correct behaviour, not a
  dedup miss.
- **No reuse log after a restart, and a duplicate appeared.** Check the
  `overseer::gap_scan` logs for a `gh search failed` ERROR line around the tick;
  a failed search files nothing, so a duplicate here means the *prior* issue was
  closed or its marker is missing. Verify the body still contains
  `stewardship-signature:`.
- **`gh` not authenticated.** The fail-loud ERROR carries the trimmed `gh`
  stderr — usually a hint to run `gh auth login`.

## See also

- [Durable gap-filing dedup reference](../reference/overseer-gap-durable-dedup.md)
  — signature grammar, flow, fail-loud contract, and security notes.
- [Review the Overseer's workstream gaps](./review-overseer-workstream-gaps.md)
  — where the gaps surface and how to respond.
- [Gap-scan dedup & exponential backoff](../concepts/gap-scan-backoff-dedup.md)
  — the in-process pre-filter this durable check backstops.
- [File stewardship issues from orchestrator runs](./file-stewardship-issues-from-orchestrator-runs.md)
  — the sibling loop whose dedup flow this mirrors.
