---
title: "Runbook: land amplihack-rs#893 (Copilot CLI frontmatter regression guard)"
description: >
  Operational runbook for landing rysweet/amplihack-rs#893 — rebasing the
  frontmatter regression-guard PR onto the latest base, re-triggering the
  cancelled/skipped checks, taking mergeStateStatus from BEHIND to CLEAN, and
  merging via gh on green required checks without --admin override.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./cross-repo-merge-authority.md
  - ./pr-finalization-pipeline.md
  - ../operations/index.md
---

# Runbook: land amplihack-rs#893

> **Status: landed.** [rysweet/amplihack-rs#893](https://github.com/rysweet/amplihack-rs/pull/893)
> (Copilot CLI frontmatter regression guard, opened in response to issue #890)
> was rebased onto the latest base branch, its cancelled/skipped checks re-ran to
> completion, and it merged on green required checks with `mergeStateStatus
> CLEAN`. This runbook records the procedure so it is repeatable.

## Context

`amplihack-rs#893` was **MERGEABLE** but stalled at
`mergeStateStatus=BEHIND` with cancelled checks (CANCELLED=2, SKIPPED=2,
SUCCESS=8) — the PR had fallen behind its base branch, so its checks were never
completed. The PR intent (a frontmatter regression guard) is **unchanged**; this
is a delivery/unblock action, not a code rewrite.

`rysweet/amplihack-rs` is a **separate repository** — not part of this checkout.
All steps run via the `gh` CLI against that repo using ambient `gh` auth. Tokens
are never read, echoed, or persisted.

## Procedure

### 1. Confirm the stalled state

```bash
gh pr view 893 --repo rysweet/amplihack-rs \
  --json mergeable,mergeStateStatus,statusCheckRollup
```

Expect `mergeable: MERGEABLE`, `mergeStateStatus: BEHIND`.

### 2. Rebase onto the latest base and re-trigger CI

Update the branch from base so the cancelled/skipped checks re-run to completion:

```bash
# Bring the PR head up to date with base; this re-triggers the checks.
gh pr update-branch 893 --repo rysweet/amplihack-rs --rebase
```

If a rebase surfaces conflicts, resolve them **without changing the PR's intent**
(preserve the frontmatter regression guard), push, and let CI re-run.

### 3. Wait for required checks to complete green

```bash
gh pr checks 893 --repo rysweet/amplihack-rs --watch
```

All **required** checks must finish `SUCCESS`. The previously cancelled/skipped
checks must now run to completion — a skipped required check is not "green".

### 4. Confirm CLEAN, then merge

```bash
gh pr view 893 --repo rysweet/amplihack-rs --json mergeStateStatus  # expect: CLEAN
gh pr merge 893 --repo rysweet/amplihack-rs --squash
```

## Guardrails

- **No `--admin` override.** Merge only when `mergeStateStatus` is `CLEAN` and all
  required checks are green. Never force a merge over failing or stale checks.
- **Intent unchanged.** Do not alter the frontmatter regression-guard behavior;
  the action is limited to rebase + re-trigger + merge.
- **Least privilege.** Rely on ambient `gh` auth; never read/echo/persist tokens.
- **Additive / non-breaking**, PRD preserved, no "Bridge" naming.

## Done when

- `mergeStateStatus` is `CLEAN` (no longer `BEHIND`).
- All required checks are green.
- #893 is merged with its intent unchanged.
