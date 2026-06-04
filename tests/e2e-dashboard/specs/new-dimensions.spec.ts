// Issue #2137 — regression specs for the three new dashboard dimensions:
// merge-judge decisions, per-PR readiness, and brain-failure surfacing.
// Structural tests use mocked APIs; smoke tests hit the live backend.

import { test, expect } from '../fixtures/simard-dashboard';

// ─── Mock data ───────────────────────────────────────────────────────────────

const MOCK_MERGE_JUDGE = {
  decisions: [
    {
      pr_number: 2102,
      verdict: 'approved',
      reasoning: 'All CI checks pass, two approvals, no blocking comments',
      timestamp: '2026-05-27T10:00:00Z',
    },
    {
      pr_number: 2087,
      verdict: 'deferred',
      reasoning: 'Pending review from code owner',
      timestamp: '2026-05-27T09:30:00Z',
    },
  ],
  persistence_available: true,
  persistence_reason: 'sqlite',
  summary: '2 decisions recorded',
  timestamp: new Date().toISOString(),
};

const MOCK_PRS = {
  prs: [
    {
      number: 2150,
      title: 'feat: add memory growth trends',
      ci_status: 'passing',
      review_state: 'approved',
      blockers: [],
      url: 'https://github.com/rysweet/Simard/pull/2150',
    },
    {
      number: 2160,
      title: 'fix: OODA cycle timing',
      ci_status: 'failing',
      review_state: 'changes_requested',
      blockers: ['CI red', 'Review changes requested'],
      url: 'https://github.com/rysweet/Simard/pull/2160',
    },
  ],
  summary: '2 open PRs',
  timestamp: new Date().toISOString(),
};

const MOCK_BRAIN_FAILURES = {
  failures: [
    {
      failure_type: 'EMPTY_RESPONSE_SENTINEL',
      component: 'ooda_decide',
      timestamp: '2026-05-27T08:15:00Z',
      recovered: true,
      recovery_action: 'fell back to deterministic rule',
    },
    {
      failure_type: 'JSON_PARSE_ERROR',
      component: 'goal_prioritizer',
      timestamp: '2026-05-27T07:45:00Z',
      recovered: false,
      recovery_action: null,
    },
  ],
  summary: '2 failures (1 recovered)',
  timestamp: new Date().toISOString(),
};

// ─── Structural tests (mocked API) ──────────────────────────────────────────

test.describe('Merge Decisions Tab @structural', () => {
  test.beforeEach(async ({ authenticatedPage }) => {
    await authenticatedPage.route('**/api/merge-judge', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_MERGE_JUDGE),
      }),
    );
    await authenticatedPage.goto('/');
  });

  test('merge-decisions tab renders and shows decisions', async ({
    authenticatedPage,
  }) => {
    const tab = authenticatedPage.locator('.tab[data-tab="merge-decisions"]');
    await expect(tab).toBeVisible();
    await tab.click();
    await expect(authenticatedPage.locator('#tab-merge-decisions')).toBeVisible();

    // Panel should contain decision data
    const panel = authenticatedPage.locator('#tab-merge-decisions');
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('tab-merge-decisions');
        return el && el.innerText.length > 10;
      },
      { timeout: 10_000 },
    );
    const text = await panel.textContent();
    expect(text).toContain('approved');
  });

  test('merge-decisions tab shows verdict and PR number', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.locator('.tab[data-tab="merge-decisions"]').click();
    const panel = authenticatedPage.locator('#tab-merge-decisions');
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('tab-merge-decisions');
        return el && el.innerText.length > 10;
      },
      { timeout: 10_000 },
    );
    const text = await panel.textContent();
    // Should contain at least one PR number and verdict from mock data
    expect(text).toMatch(/2102|2087/);
    expect(text).toMatch(/approved|deferred/i);
  });

  test('no JS errors on merge-decisions tab', async ({ authenticatedPage }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.locator('.tab[data-tab="merge-decisions"]').click();
    await authenticatedPage.waitForTimeout(2000);
    expect(errors).toEqual([]);
  });
});

test.describe('PR Readiness Tab @structural', () => {
  test.beforeEach(async ({ authenticatedPage }) => {
    await authenticatedPage.route('**/api/prs', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_PRS),
      }),
    );
    await authenticatedPage.goto('/');
  });

  test('pr-readiness tab renders and shows PR data', async ({
    authenticatedPage,
  }) => {
    const tab = authenticatedPage.locator('.tab[data-tab="pr-readiness"]');
    await expect(tab).toBeVisible();
    await tab.click();
    await expect(authenticatedPage.locator('#tab-pr-readiness')).toBeVisible();

    const panel = authenticatedPage.locator('#tab-pr-readiness');
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('tab-pr-readiness');
        return el && el.innerText.length > 10;
      },
      { timeout: 10_000 },
    );
    const text = await panel.textContent();
    expect(text).toContain('2150');
  });

  test('pr-readiness shows CI status and review state', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.locator('.tab[data-tab="pr-readiness"]').click();
    const panel = authenticatedPage.locator('#tab-pr-readiness');
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('tab-pr-readiness');
        return el && el.innerText.length > 10;
      },
      { timeout: 10_000 },
    );
    const text = await panel.textContent();
    expect(text).toMatch(/passing|failing/i);
  });

  test('no JS errors on pr-readiness tab', async ({ authenticatedPage }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.locator('.tab[data-tab="pr-readiness"]').click();
    await authenticatedPage.waitForTimeout(2000);
    expect(errors).toEqual([]);
  });
});

test.describe('Brain Failures Tab @structural', () => {
  test.beforeEach(async ({ authenticatedPage }) => {
    await authenticatedPage.route('**/api/brain-failures', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_BRAIN_FAILURES),
      }),
    );
    await authenticatedPage.goto('/');
  });

  test('brain-failures tab renders and shows failure data', async ({
    authenticatedPage,
  }) => {
    const tab = authenticatedPage.locator('.tab[data-tab="brain-failures"]');
    await expect(tab).toBeVisible();
    await tab.click();
    await expect(authenticatedPage.locator('#tab-brain-failures')).toBeVisible();

    const panel = authenticatedPage.locator('#tab-brain-failures');
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('tab-brain-failures');
        return el && el.innerText.length > 10;
      },
      { timeout: 10_000 },
    );
    const text = await panel.textContent();
    expect(text).toMatch(/EMPTY_RESPONSE_SENTINEL|JSON_PARSE_ERROR/);
  });

  test('brain-failures tab shows component and recovery info', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.locator('.tab[data-tab="brain-failures"]').click();
    const panel = authenticatedPage.locator('#tab-brain-failures');
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('tab-brain-failures');
        return el && el.innerText.length > 10;
      },
      { timeout: 10_000 },
    );
    const text = await panel.textContent();
    expect(text).toMatch(/ooda_decide|goal_prioritizer/);
  });

  test('no JS errors on brain-failures tab', async ({ authenticatedPage }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.locator('.tab[data-tab="brain-failures"]').click();
    await authenticatedPage.waitForTimeout(2000);
    expect(errors).toEqual([]);
  });
});

// ─── Smoke tests (live backend) ─────────────────────────────────────────────

test.describe('New Dimensions API Endpoints @smoke', () => {
  test('/api/merge-judge returns 200 with expected shape', async ({
    authenticatedPage,
  }) => {
    const resp = await authenticatedPage.request.get('/api/merge-judge');
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body).toHaveProperty('decisions');
    expect(body).toHaveProperty('timestamp');
  });

  test('/api/prs returns 200 with expected shape', async ({
    authenticatedPage,
  }) => {
    const resp = await authenticatedPage.request.get('/api/prs');
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body).toHaveProperty('prs');
    expect(body).toHaveProperty('timestamp');
  });

  test('/api/brain-failures returns 200 with expected shape', async ({
    authenticatedPage,
  }) => {
    const resp = await authenticatedPage.request.get('/api/brain-failures');
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body).toHaveProperty('failures');
    expect(body).toHaveProperty('timestamp');
  });
});

test.describe('New Dimensions Tab Rendering @smoke', () => {
  test('merge-decisions tab loads real data without JS errors', async ({
    authenticatedPage,
  }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="merge-decisions"]').click();
    await expect(authenticatedPage.locator('#tab-merge-decisions')).toBeVisible();
    await authenticatedPage.waitForResponse(
      (resp) => resp.url().includes('/api/merge-judge') && resp.status() === 200,
      { timeout: 10_000 },
    );
    expect(errors).toEqual([]);
  });

  test('pr-readiness tab loads real data without JS errors', async ({
    authenticatedPage,
  }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="pr-readiness"]').click();
    await expect(authenticatedPage.locator('#tab-pr-readiness')).toBeVisible();
    await authenticatedPage.waitForResponse(
      (resp) => resp.url().includes('/api/prs') && resp.status() === 200,
      { timeout: 10_000 },
    );
    expect(errors).toEqual([]);
  });

  test('brain-failures tab loads real data without JS errors', async ({
    authenticatedPage,
  }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="brain-failures"]').click();
    await expect(authenticatedPage.locator('#tab-brain-failures')).toBeVisible();
    await authenticatedPage.waitForResponse(
      (resp) => resp.url().includes('/api/brain-failures') && resp.status() === 200,
      { timeout: 10_000 },
    );
    expect(errors).toEqual([]);
  });
});
