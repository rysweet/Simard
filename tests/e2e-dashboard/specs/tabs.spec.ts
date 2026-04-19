import { test, expect } from '../fixtures/simard-dashboard';

const ALL_TABS = [
  'overview',
  'goals',
  'traces',
  'logs',
  'processes',
  'memory',
  'costs',
  'thinking',
  'workboard',
  'chat',
] as const;

// Mock all API endpoints so tabs render without a live backend
async function mockAllApis(page: import('@playwright/test').Page) {
  const emptyJson = { status: 200, contentType: 'application/json', body: '[]' };
  const emptyObj = { status: 200, contentType: 'application/json', body: '{}' };

  await page.route('**/api/status', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        version: '0.7.1.0',
        git_hash: 'test',
        ooda_status: 'idle',
        uptime_secs: 0,
      }),
    }),
  );
  await page.route('**/api/issues', (route) => route.fulfill(emptyJson));
  await page.route('**/api/distributed', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        topology: [],
        vms: [],
        event_bus: {
          topics: {},
          total_subscribers: 0,
          events_per_min: 0.0,
          last_event_timestamp: null,
        },
      }),
    }),
  );
  await page.route('**/api/hosts', (route) => route.fulfill(emptyJson));
  await page.route('**/api/goals', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ active: [], backlog: [] }),
    }),
  );
  await page.route('**/api/traces', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ otel_status: 'disabled', traces: [] }),
    }),
  );
  await page.route('**/api/logs', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ daemon: '', cost_ledger: '', transcripts: [] }),
    }),
  );
  await page.route('**/api/processes', (route) => route.fulfill(emptyJson));
  await page.route('**/api/memory', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ overview: {}, files: [] }),
    }),
  );
  await page.route('**/api/costs', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ daily: [], weekly: [] }),
    }),
  );
  await page.route('**/api/budget', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ daily: 10, weekly: 50 }),
    }),
  );
  await page.route('**/api/registry', (route) => route.fulfill(emptyJson));
  await page.route('**/api/build-lock', (route) => route.fulfill(emptyObj));
  await page.route('**/api/metrics', (route) => route.fulfill(emptyObj));
  await page.route('**/api/ooda-thinking', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ reports: [] }),
    }),
  );
  await page.route('**/api/workboard', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        cycle: { number: 1, phase: 'idle', interval_secs: 300 },
        goals: [],
        spawned_engineers: [],
        recent_actions: [],
        task_memory: [],
        working_memory: [],
        cognitive_statistics: {},
        uptime_seconds: 0,
        timestamp: new Date().toISOString(),
        next_cycle_eta_seconds: 60,
      }),
    }),
  );
  await page.route('**/api/process-tree', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        root: {
          pid: 1,
          name: 'simard',
          command: 'simard ooda',
          state: 'running',
          cpu_pct: 0.5,
          memory_mb: 64.0,
          children: [],
        },
      }),
    }),
  );
  await page.route('**/api/memory/graph', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ nodes: [], edges: [] }),
    }),
  );
}

test.describe('Tab Navigation @structural', () => {
  test.beforeEach(async ({ authenticatedPage }) => {
    await mockAllApis(authenticatedPage);
    await authenticatedPage.goto('/');
  });

  for (const tabName of ALL_TABS) {
    test(`clicking "${tabName}" tab shows its content without JS errors`, async ({
      authenticatedPage,
    }) => {
      const errors: string[] = [];
      authenticatedPage.on('pageerror', (err) => errors.push(err.message));

      const tab = authenticatedPage.locator(`.tab[data-tab="${tabName}"]`);
      await tab.click();

      // Tab should become active
      await expect(tab).toHaveClass(/active/);

      // Tab content panel should be visible
      const panel = authenticatedPage.locator(`#tab-${tabName}`);
      await expect(panel).toBeVisible();

      // Other panels should be hidden
      for (const other of ALL_TABS) {
        if (other !== tabName) {
          await expect(
            authenticatedPage.locator(`#tab-${other}`),
          ).not.toBeVisible();
        }
      }

      // No JS errors should have fired
      expect(errors).toEqual([]);
    });
  }

  test('tab switching preserves content on return', async ({
    authenticatedPage,
  }) => {
    // Go to Goals tab
    await authenticatedPage.locator('.tab[data-tab="goals"]').click();
    await expect(authenticatedPage.locator('#tab-goals')).toBeVisible();

    // Go to Logs tab
    await authenticatedPage.locator('.tab[data-tab="logs"]').click();
    await expect(authenticatedPage.locator('#tab-logs')).toBeVisible();
    await expect(authenticatedPage.locator('#tab-goals')).not.toBeVisible();

    // Return to Overview
    await authenticatedPage.locator('.tab[data-tab="overview"]').click();
    await expect(authenticatedPage.locator('#tab-overview')).toBeVisible();
  });
});
