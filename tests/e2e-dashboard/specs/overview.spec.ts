import { test, expect } from '../fixtures/simard-dashboard';
import { OverviewPage } from '../pages/overview.page';

test.describe('Dashboard Overview @structural', () => {
  let overview: OverviewPage;

  test.beforeEach(async ({ authenticatedPage }) => {
    // Mock API endpoints so page renders without a live backend
    await authenticatedPage.route('**/api/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          version: '0.7.1.test',
          git_hash: 'abc1234',
          ooda_status: 'idle',
          uptime_secs: 3600,
        }),
      }),
    );
    await authenticatedPage.route('**/api/issues', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          { number: 1, title: 'Test issue', state: 'open', html_url: '#' },
        ]),
      }),
    );

    await authenticatedPage.goto('/');
    overview = new OverviewPage(authenticatedPage);
  });

  test('overview page displays dashboard heading', async () => {
    await expect(overview.heading).toBeVisible();
  });

  test('overview tab is active by default', async ({ authenticatedPage }) => {
    const overviewTab = authenticatedPage.locator('.tab[data-tab="overview"]');
    await expect(overviewTab).toHaveClass(/active/);
    await expect(authenticatedPage.locator('#tab-overview')).toBeVisible();
  });

  test('System Status card renders', async () => {
    await expect(overview.statusCard).toBeVisible();
    await expect(overview.statusCard.locator('h2')).toContainText('System Status');
  });

  test('Open Issues card renders', async () => {
    await expect(overview.issuesCard).toBeVisible();
    await expect(overview.issuesCard.locator('h2')).toContainText('Open Issues');
  });

  test('status div populates from API', async ({ authenticatedPage }) => {
    // Wait for fetchStatus to complete and render
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('status');
        return el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    const text = await overview.statusDiv.textContent();
    expect(text).toContain('0.7.1');
  });

  test('issues list populates from API', async ({ authenticatedPage }) => {
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('issues-list');
        return el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    const text = await overview.issuesList.textContent();
    expect(text).toContain('Test issue');
  });

  test('all 10 tabs are present', async () => {
    const names = await overview.getTabNames();
    expect(names).toEqual([
      'Overview',
      'Goals',
      'Traces',
      'Logs',
      'Processes',
      'Memory',
      'Costs',
      'Chat',
      'Whiteboard',
      '🧠 Thinking',
    ]);
  });
});
