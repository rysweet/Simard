---
title: Diagnose duplicate gap-scan issues
description: >
  Operator playbook for when the Overseer gap-scan produces near-identical "Cover
  uncovered backlog workstream(s)" issues for the same gap (#4340/#4341,
  #4337/#4338) — opened by the coverage workstreams it launches. How to confirm
  the duplicate signature, verify the open-issue coverage set is seeding
  correctly, and clean up an existing duplicate cluster.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: how-to
status: reference
related:
  - ../concepts/cross-process-gap-scan-dedup.md
  - ../reference/gap-scan-open-issue-coverage.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ../howto/configure-overseer-gap-scan-backoff.md
  - ./review-overseer-workstream-gaps.md
---

# Diagnose duplicate gap-scan issues

**Symptom.** Two (or more) open GitHub issues describe the *same* gap — e.g.
[#4337](https://github.com/rysweet/Simard/issues/4337) and
[#4338](https://github.com/rysweet/Simard/issues/4338) both *"Cover uncovered
backlog… goal:harden-amplihack-rs-recipes-tool"*. Each is opened by a coverage
workstream the Overseer launched for that gap; a duplicate launch (typically after
a daemon restart) yields a duplicate issue. This is the cross-process duplicate the
gap-scan open-issue coverage set is designed to prevent.

With [cross-process gap-scan dedup](../concepts/cross-process-gap-scan-dedup.md) in
place, two identical gap-scan passes — even across a daemon restart — produce
**exactly one** covering workstream and **exactly one** issue. Use these steps to
confirm the dedup is working and to clean up a pre-existing cluster.

## 1. Confirm the two issues share a signature

Every covering issue embeds `stewardship-signature: workstream-gap:<sig>` in its
body:

```bash
for n in 4337 4338; do
  echo "== #$n =="
  gh issue view "$n" --repo rysweet/Simard --json body \
    --jq '.body' | grep -o 'stewardship-signature: workstream-gap:[^ ]*'
done
```

If both print the **same** `workstream-gap:<sig>` (e.g.
`workstream-gap:goal:harden-amplihack-rs-recipes-tool`), they are genuine
duplicates of one gap.

## 2. Verify the coverage set would now dedup them

List the open issues the gap-scan reads as its coverage set and confirm the
signature is present:

```bash
gh issue list --repo rysweet/Simard --state open --limit 200 --json number,body \
  --jq '.[] | select(.body | test("workstream-gap:goal:harden-amplihack-rs-recipes-tool"))
        | .number'
```

If this returns one (or more) open issue numbers, then on the next gap-scan pass
that signature is in `coverage`, `detect_workstream_gaps` treats the gap as
covered, no coverage workstream is launched, and **no new duplicate is opened**.
See the [open-issue coverage reference](../reference/gap-scan-open-issue-coverage.md).

## 3. Clean up the existing duplicate cluster

The dedup prevents *new* duplicates; it does not retroactively close ones already
open. Keep the lowest-numbered issue and close the rest as duplicates:

```bash
# keep #4337, close #4338 as a duplicate
gh issue close 4338 --repo rysweet/Simard \
  --comment "Duplicate of #4337 (same workstream-gap signature); closing. \
Cross-process gap-scan dedup prevents recurrence."
```

Repeat for the other clusters (`#4340`/`#4341`, and the
`#4297`/`#4301`/`#4304`/`#4306`/`#4316` cluster), keeping one per signature.

## 4. Confirm no fresh duplicate appears

Watch the next few gap-scan passes on the activity feed
([review Overseer workstream gaps](./review-overseer-workstream-gaps.md)). A
healthy scan either does nothing for that gap (it's covered by the surviving open
issue) or, if you close *all* issues for a still-real gap, launches **exactly one**
covering workstream that opens **exactly one** fresh issue — never a pair.

## If duplicates still appear

- **Signatures differ.** If the two issues carry *different* `workstream-gap:<sig>`
  values, they are not the same gap to the dedup — check whether the underlying
  goal id / issue ref / anomaly slug genuinely differs.
- **Coverage query failing.** The coverage set is best-effort; a failing `gh`
  query degrades to the in-memory coverage gate. Check the `overseer::gap_scan`
  tracing target for a "degrading" warning, and confirm `gh` auth/rate limits are healthy.
- **Backoff too short.** Within a single process, re-announcement is also
  rate-limited by the [BackoffGate](../concepts/gap-scan-backoff-dedup.md); tune it
  via [configure Overseer gap-scan backoff](./configure-overseer-gap-scan-backoff.md).

Full rationale:
[cross-process gap-scan dedup](../concepts/cross-process-gap-scan-dedup.md).
