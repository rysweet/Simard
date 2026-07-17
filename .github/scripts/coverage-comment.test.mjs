// TDD (issue #4231): resilient coverage-comment posting.
//
// The `coverage` job's "Post coverage comment on PR" step calls three GitHub
// REST APIs (listComments, deleteComment, createComment) with NO retry and NO
// tolerance, so a transient GitHub 5xx fails the whole REQUIRED `coverage`
// check and blocks otherwise-green PRs (observed as coverage:FAILURE on #4230).
//
// Fix contract: extract the inline `actions/github-script` body into this
// sibling module `coverage-comment.mjs` (which `coverage.yml` then loads), so it
// can be unit-tested, and make it resilient:
//
//   export function retry(fn, opts?) -> Promise
//     * awaits fn(attempt) (1-based); returns its value on success.
//     * retries ONLY transient failures: HTTP status >= 500, status === 429,
//       or a network error (no status + a network `code` such as ECONNRESET).
//     * NEVER retries deterministic 4xx (e.g. 403/404).
//     * gives up after `retries` attempts and rethrows the last error.
//     * `opts.sleep(ms)` is injectable so tests incur no real delay.
//
//   export async function postCoverageComment(deps) -> { posted, reason? }
//     deps: { github, context, fs, summaryPath, log?, sleep? }
//     * missing summary file  -> { posted: false, reason: 'no-summary' }, no API calls.
//     * happy path            -> upserts the comment, returns { posted: true }.
//     * PERSISTENT API failure -> catches, warns via `log.warn`, and returns
//       { posted: false, reason: 'api-error' } WITHOUT throwing — the comment is
//       a side effect and must never fail the required coverage gate.
//
// These tests are written FIRST and MUST FAIL until `coverage-comment.mjs`
// exists and exports `retry` + `postCoverageComment`.

import test from 'node:test';
import assert from 'node:assert/strict';

import { retry, postCoverageComment } from './coverage-comment.mjs';

const noSleep = async () => {};

function httpError(status) {
  const e = new Error(`HTTP ${status}`);
  e.status = status;
  return e;
}

function networkError(code) {
  const e = new Error(`network ${code}`);
  e.code = code;
  return e;
}

// A minimal valid `cargo llvm-cov --json --summary-only` payload.
const COVERAGE_JSON = JSON.stringify({
  data: [
    {
      totals: { lines: { count: 100, covered: 80 } },
      files: [
        { filename: 'src/overseer/mod.rs', summary: { lines: { count: 60, covered: 50 } } },
        { filename: 'src/typed_ooda/ledger.rs', summary: { lines: { count: 40, covered: 30 } } },
      ],
    },
  ],
});

function fakeFs({ exists = true, content = COVERAGE_JSON } = {}) {
  return {
    existsSync: () => exists,
    readFileSync: () => content,
  };
}

function fakeContext() {
  return { repo: { owner: 'rysweet', repo: 'Simard' }, issue: { number: 4230 } };
}

function recordingLog() {
  const warnings = [];
  return { log: { warn: (m) => warnings.push(m), info: () => {} }, warnings };
}

// ── retry ────────────────────────────────────────────────────────────────────

test('retry returns immediately when the call succeeds', async () => {
  let calls = 0;
  const out = await retry(async () => {
    calls += 1;
    return 'ok';
  }, { sleep: noSleep });
  assert.equal(out, 'ok');
  assert.equal(calls, 1);
});

test('retry retries transient 5xx then succeeds', async () => {
  let calls = 0;
  const out = await retry(async () => {
    calls += 1;
    if (calls < 3) throw httpError(502);
    return 'recovered';
  }, { retries: 5, sleep: noSleep });
  assert.equal(out, 'recovered');
  assert.equal(calls, 3);
});

test('retry retries transient network errors', async () => {
  let calls = 0;
  const out = await retry(async () => {
    calls += 1;
    if (calls < 2) throw networkError('ECONNRESET');
    return 'recovered';
  }, { retries: 5, sleep: noSleep });
  assert.equal(out, 'recovered');
  assert.equal(calls, 2);
});

test('retry does NOT retry deterministic 4xx', async () => {
  let calls = 0;
  await assert.rejects(
    retry(async () => {
      calls += 1;
      throw httpError(404);
    }, { retries: 5, sleep: noSleep }),
    (e) => e.status === 404,
  );
  assert.equal(calls, 1, '4xx must fail fast, not retry');
});

test('retry gives up and rethrows after exhausting attempts on persistent 5xx', async () => {
  let calls = 0;
  await assert.rejects(
    retry(async () => {
      calls += 1;
      throw httpError(503);
    }, { retries: 3, sleep: noSleep }),
    (e) => e.status === 503,
  );
  assert.equal(calls, 3, 'must attempt exactly `retries` times');
});

// ── postCoverageComment ───────────────────────────────────────────────────────

test('postCoverageComment posts a comment on the happy path', async () => {
  const created = [];
  const github = {
    rest: {
      issues: {
        listComments: async () => ({ data: [] }),
        deleteComment: async () => ({}),
        createComment: async (args) => {
          created.push(args);
          return {};
        },
      },
    },
  };
  const { log } = recordingLog();

  const result = await postCoverageComment({
    github,
    context: fakeContext(),
    fs: fakeFs(),
    summaryPath: 'target/ci-logs/coverage-summary.json',
    log,
    sleep: noSleep,
  });

  assert.deepEqual(result, { posted: true });
  assert.equal(created.length, 1);
  assert.match(created[0].body, /Coverage Summary/);
});

test('postCoverageComment upserts by deleting a prior coverage comment', async () => {
  const deleted = [];
  const github = {
    rest: {
      issues: {
        listComments: async () => ({
          data: [
            { id: 11, body: '## 📊 Coverage Summary\nold' },
            { id: 22, body: 'unrelated comment' },
          ],
        }),
        deleteComment: async (args) => {
          deleted.push(args.comment_id);
          return {};
        },
        createComment: async () => ({}),
      },
    },
  };
  const { log } = recordingLog();

  const result = await postCoverageComment({
    github,
    context: fakeContext(),
    fs: fakeFs(),
    summaryPath: 'target/ci-logs/coverage-summary.json',
    log,
    sleep: noSleep,
  });

  assert.deepEqual(result, { posted: true });
  assert.deepEqual(deleted, [11], 'only the prior coverage comment is deleted');
});

test('postCoverageComment is non-fatal on persistent API failure', async () => {
  const github = {
    rest: {
      issues: {
        // Every attempt 5xxs — a real transient GitHub outage that never clears.
        listComments: async () => {
          throw httpError(500);
        },
        deleteComment: async () => ({}),
        createComment: async () => {
          throw httpError(500);
        },
      },
    },
  };
  const { log, warnings } = recordingLog();

  // MUST NOT throw — a failed comment post must not fail the required check.
  const result = await postCoverageComment({
    github,
    context: fakeContext(),
    fs: fakeFs(),
    summaryPath: 'target/ci-logs/coverage-summary.json',
    log,
    sleep: noSleep,
  });

  assert.equal(result.posted, false);
  assert.equal(result.reason, 'api-error');
  assert.ok(warnings.length >= 1, 'a persistent failure must be surfaced as a warning');
});

test('postCoverageComment skips cleanly when the summary file is missing', async () => {
  let apiCalled = false;
  const github = {
    rest: {
      issues: {
        listComments: async () => {
          apiCalled = true;
          return { data: [] };
        },
        deleteComment: async () => ({}),
        createComment: async () => {
          apiCalled = true;
          return {};
        },
      },
    },
  };
  const { log } = recordingLog();

  const result = await postCoverageComment({
    github,
    context: fakeContext(),
    fs: fakeFs({ exists: false }),
    summaryPath: 'target/ci-logs/coverage-summary.json',
    log,
    sleep: noSleep,
  });

  assert.deepEqual(result, { posted: false, reason: 'no-summary' });
  assert.equal(apiCalled, false, 'no GitHub API calls when there is no summary');
});
