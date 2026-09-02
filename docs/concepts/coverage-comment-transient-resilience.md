---
title: The coverage comment survives transient GitHub 5xx without failing the required check
description: Why a transient GitHub 500/502/503 while posting the coverage comment no longer red-Xes the required `coverage` check and blocks an otherwise-green PR; how the "Post coverage comment" step now retries the GitHub API calls with backoff on 5xx/network errors and treats a persistent post failure as non-fatal (an internal try/catch that warns and returns success) — while the coverage computation and threshold gate remain fully authoritative, and continue-on-error is deliberately NOT used.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ../reference/coverage-comment-resilience.md
  - ../operations/coverage-check.md
---

# The coverage comment survives transient GitHub 5xx

> **Status: implemented (issue #4231).** The `coverage` workflow's
> "Post coverage comment on PR" step now retries the GitHub API on transient
> errors and treats a **persistent** comment-post failure as non-fatal, so a
> flaky GitHub API no longer fails the required check. The coverage threshold
> gate is unchanged. Primary source:
> [`.github/workflows/coverage.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/coverage.yml).
> API details:
> [Coverage comment resilience reference](../reference/coverage-comment-resilience.md).

## The defect this fixes

The required `coverage` check ends by posting a per-module coverage table as a PR
comment via `actions/github-script`. That step made three GitHub REST calls —
`listComments`, `deleteComment` (upsert), `createComment` — with **no** retry or
error handling. A single transient GitHub `5xx` on any of them threw, failed the
step, and therefore failed the whole **required** check — blocking an otherwise
green PR from merging (observed as `coverage: FAILURE` on PR #4230).

This conflates two unrelated things: **computing/gating coverage** (which must
gate merges) and **posting a cosmetic comment** (a best-effort side effect that
should tolerate a GitHub outage).

## The fix: retry transient errors, tolerate persistent failure

The comment-posting script now:

1. **Retries with backoff on transient errors only.** Each GitHub API call is
   wrapped in a small retry helper that re-attempts on HTTP `5xx` (500/502/503/504)
   and network errors, with exponential backoff and a bounded attempt count.
   `4xx` (e.g. permission, validation) is **not** retried — it fails fast.
2. **Treats a persistent post failure as non-fatal.** The whole
   comment-upsert block is wrapped in an internal `try/catch`. If the calls still
   fail after retries, the script `console.warn`s the reason and **returns
   normally**, so the step — and the required check — succeed.

Crucially:

- **`continue-on-error` is NOT used.** That would de-require the check entirely
  (it would report success even on a real coverage/threshold failure). Instead
  the tolerance is scoped inside the script to the comment side effect only.
- **The coverage computation and threshold gate remain authoritative.** Only the
  comment-posting is made tolerant; a genuine coverage regression still fails the
  check.
- The `pull_request` trigger and `pull-requests: write` permission are unchanged;
  the token is never logged.

## Behaviour matrix

| Situation                                   | Before (#4231)     | After (#4231)                          |
| ------------------------------------------- | ------------------ | -------------------------------------- |
| Transient 5xx on a comment API call         | Check **FAILS**    | Retried; usually succeeds              |
| Persistent GitHub outage (all retries fail) | Check **FAILS**    | `warn` + check **PASSES** (no comment) |
| 4xx (permissions/validation)                | Check fails        | Fails fast (not retried)               |
| Real coverage/threshold regression          | Check fails        | Check **still fails** (unchanged)      |

## Verifying the behaviour

Because this is workflow JavaScript, validation is a dry-run/mock of the retry
helper:

- **Retries then succeeds** — a mock API that returns 503 twice then 200 results
  in a single posted comment and step success.
- **Persistent failure is non-fatal** — a mock API that always returns 503
  results in a `warn` and step success, with no thrown error.
- **4xx fails fast** — a mock 403 is not retried.
- **Gate preserved** — the threshold/computation path is untouched; a coverage
  regression still fails the check.

See the [reference doc](../reference/coverage-comment-resilience.md) for the
retry helper contract and the exact step wiring.
