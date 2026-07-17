---
title: "Reference: Coverage Comment Resilience"
description: >
  The contract for transient-failure resilience in the coverage workflow's
  "Post coverage comment on PR" step: the retry-with-backoff helper (5xx/network
  only, bounded attempts, 4xx fails fast), the non-fatal try/catch around the
  comment upsert, the preserved coverage threshold gate, the explicit avoidance
  of continue-on-error, unchanged trigger/permissions, and the dry-run test list.
last_updated: 2026-07-17
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/coverage-comment-transient-resilience.md
---

# Reference: Coverage Comment Resilience

> **Status: implemented (#4231).** Present-tense description of shipped
> behaviour. Primary source:
> [`.github/workflows/coverage.yml`](https://github.com/rysweet/Simard/blob/main/.github/workflows/coverage.yml),
> step **"Post coverage comment on PR"**. Conceptual overview:
> [The coverage comment survives transient GitHub 5xx](../concepts/coverage-comment-transient-resilience.md).

## Retry helper

Inside the `actions/github-script` step, a small helper retries a GitHub API call
on transient failures only:

```js
async function withRetry(fn, { attempts = 4, baseMs = 500 } = {}) {
  for (let i = 0; i < attempts; i++) {
    try {
      return await fn();
    } catch (err) {
      const status = err && err.status;
      const transient = status >= 500 && status <= 599; // 500/502/503/504
      const network = status === undefined;             // fetch/network error
      if (!(transient || network) || i === attempts - 1) throw err; // 4xx fails fast; last attempt rethrows
      await new Promise(r => setTimeout(r, baseMs * 2 ** i)); // exponential backoff
    }
  }
}
```

Contract:

| Input error        | Behaviour                          |
| ------------------ | ---------------------------------- |
| HTTP 5xx           | Retried with exponential backoff.  |
| Network error      | Retried with exponential backoff.  |
| HTTP 4xx           | **Not** retried — rethrown at once.|
| Retries exhausted  | Rethrown to the outer `try/catch`. |

## Non-fatal comment upsert

The three API calls are wrapped with `withRetry` and enclosed in a `try/catch`
that makes a persistent failure non-fatal:

```js
try {
  const { data: comments } = await withRetry(() =>
    github.rest.issues.listComments({ owner, repo, issue_number, per_page: 100 }));
  for (const c of comments) {
    if (c.body && c.body.startsWith(marker)) {
      await withRetry(() =>
        github.rest.issues.deleteComment({ owner, repo, comment_id: c.id }));
    }
  }
  await withRetry(() =>
    github.rest.issues.createComment({ owner, repo, issue_number, body }));
} catch (err) {
  console.warn(`Coverage comment post failed after retries; continuing (non-fatal): ${err && err.message}`);
  // Return normally — the required `coverage` check is NOT failed by a comment outage.
}
```

## What is deliberately NOT changed

- **No `continue-on-error`.** The check stays required; only the comment side
  effect is tolerant. A real coverage/threshold failure still fails the job.
- **Coverage computation and threshold gate** are untouched and authoritative.
- **Trigger and permissions** — `on: pull_request` and `pull-requests: write`
  are unchanged. The token is never logged; only `err.message` is warned.

## Dry-run / mock tests

| Test                                  | Asserts                                                       |
| ------------------------------------- | ------------------------------------------------------------ |
| `retries_then_succeeds`               | 503×2 then 200 ⟹ one comment posted, step succeeds.          |
| `persistent_failure_is_non_fatal`     | Always-503 ⟹ `warn` + step succeeds, no throw.               |
| `four_xx_fails_fast`                  | 403 ⟹ not retried.                                           |
| `backoff_is_bounded`                  | Attempts capped; total delay bounded.                        |
| `threshold_gate_preserved`            | Coverage regression still fails the check.                   |
