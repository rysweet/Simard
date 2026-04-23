import { test, expect } from '../fixtures/simard-dashboard';

// TDD spec for the Stewardship tab (issue #1172).
// Will FAIL until Step 8 wires the tab markup, JS, and API loaders into
// INDEX_HTML in src/operator_commands_dashboard/routes.rs.

const STEWARDSHIP_PAYLOAD = {
  repos: [
    {
      repo: 'rysweet/Simard',
      role: 'primary',
      last_activity: '2026-04-22',
      notes: 'active dev',
    },
    {
      repo: 'rysweet/lbug',
      role: 'support',
      last_activity: '2026-04-15',
      notes: '',
    },
  ],
};

const SELF_UNDERSTANDING_PAYLOAD = {
  uptime_secs: 12_345,
  metrics: [
    { timestamp: '2026-04-22T00:00:00Z', name: 'cycle_count', value: 42 },
    { timestamp: '2026-04-22T00:00:30Z', name: 'cycle_count', value: 43 },
  ],
  snapshot: {
    session_phase: 'idle',
    topology_summary: 'standalone',
  },
};

async function mockBaselineApis(page: import('@playwright/test').Page) {
  const emptyJson = { status: 200, contentType: 'application/json', body: '[]' };
  const emptyObj = { status: 200, contentType: 'application/json', body: '{}' };
  await page.route('**/api/status', (r) =>
    r.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        version: '0.7.1.test',
        git_hash: 'abc1234',
        ooda_status: 'idle',
        uptime_secs: 0,
      }),
    }),
  );
  await page.route('**/api/issues', (r) => r.fulfill(emptyJson));
  await page.route('**/api/distributed', (r) =>
    r.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        topology: [],
        vms: [],
        event_bus: {
          topics: {},
          total_subscribers: 0,
          events_per_min: 0,
          last_event_timestamp: null,
        },
      }),
    }),
  );
  await page.route('**/api/hosts', (r) => r.fulfill(emptyJson));
  await page.route('**/api/metrics', (r) => r.fulfill(emptyObj));
  await page.route('**/api/build-lock', (r) => r.fulfill(emptyObj));
  await page.route('**/api/registry', (r) => r.fulfill(emptyJson));
  await page.route('**/api/processes', (r) => r.fulfill(emptyJson));
}

test.describe('Stewardship Tab @structural', () => {
  test.beforeEach(async ({ authenticatedPage }) => {
    await mockBaselineApis(authenticatedPage);
    await authenticatedPage.route('**/api/stewardship', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(STEWARDSHIP_PAYLOAD),
      }),
    );
    await authenticatedPage.route('**/api/self-understanding', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(SELF_UNDERSTANDING_PAYLOAD),
      }),
    );
    await authenticatedPage.goto('/');
  });

  test('Stewardship tab is present in the tab bar', async ({
    authenticatedPage,
  }) => {
    const tab = authenticatedPage.locator('.tab[data-tab="stewardship"]');
    await expect(tab).toBeVisible();
    await expect(tab).toContainText(/stewardship/i);
  });

  test('clicking Stewardship tab activates its panel without JS errors', async ({
    authenticatedPage,
  }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));

    await authenticatedPage.locator('.tab[data-tab="stewardship"]').click();
    const panel = authenticatedPage.locator('#tab-stewardship');
    await expect(panel).toBeVisible();
    await expect(
      authenticatedPage.locator('.tab[data-tab="stewardship"]'),
    ).toHaveClass(/active/);

    expect(errors).toEqual([]);
  });

  test('Stewardship panel renders both Repos and Self-Understanding cards', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.locator('.tab[data-tab="stewardship"]').click();
    const panel = authenticatedPage.locator('#tab-stewardship');

    await expect(panel.getByText(/Repos Under Stewardship/i)).toBeVisible();
    await expect(panel.getByText(/Self-Understanding/i)).toBeVisible();
  });

  test('Stewardship card lists repos returned by /api/stewardship', async ({
    authenticatedPage,
  }) => {
    const requested = authenticatedPage.waitForRequest('**/api/stewardship');
    await authenticatedPage.locator('.tab[data-tab="stewardship"]').click();
    await requested;

    const panel = authenticatedPage.locator('#tab-stewardship');
    await expect(panel.getByText('rysweet/Simard')).toBeVisible();
    await expect(panel.getByText('rysweet/lbug')).toBeVisible();
    await expect(panel.getByText(/primary/i)).toBeVisible();
    await expect(panel.getByText(/support/i)).toBeVisible();
  });

  test('Self-Understanding card surfaces uptime + recent metric values', async ({
    authenticatedPage,
  }) => {
    const requested = authenticatedPage.waitForRequest(
      '**/api/self-understanding',
    );
    await authenticatedPage.locator('.tab[data-tab="stewardship"]').click();
    await requested;

    const panel = authenticatedPage.locator('#tab-stewardship');
    // Uptime must be visible (formatted or raw — both acceptable contracts).
    await expect(panel).toContainText(/uptime/i);
    // Most recent metric value must appear somewhere in the card.
    await expect(panel).toContainText('cycle_count');
    await expect(panel).toContainText('43');
  });

  test('Stewardship card handles empty repos list gracefully', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.unroute('**/api/stewardship');
    await authenticatedPage.route('**/api/stewardship', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ repos: [] }),
      }),
    );
    await authenticatedPage.reload();

    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.locator('.tab[data-tab="stewardship"]').click();

    const panel = authenticatedPage.locator('#tab-stewardship');
    await expect(panel).toBeVisible();
    // Empty-state messaging is acceptable; no JS errors must fire.
    expect(errors).toEqual([]);
  });

  test('captures Stewardship tab screenshot for PR evidence', async ({
    authenticatedPage,
  }, testInfo) => {
    await authenticatedPage.locator('.tab[data-tab="stewardship"]').click();
    await expect(
      authenticatedPage.locator('#tab-stewardship'),
    ).toBeVisible();
    // Wait for both loaders to settle before snapshotting.
    await authenticatedPage.waitForLoadState('networkidle');
    const buf = await authenticatedPage.screenshot({ fullPage: true });
    await testInfo.attach('stewardship-tab.png', {
      body: buf,
      contentType: 'image/png',
    });
  });
});
