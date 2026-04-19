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

// --- Issue #948: live activity surfaces (agent-live-status, recent-actions, open-prs) ---

const MOCK_ACTIVITY = {
  daemon: {
    status: 'healthy',
    last_heartbeat: new Date().toISOString(),
    current_cycle: 42,
    actions_taken: 7,
  },
  recent_cycles: [
    {
      cycle_number: 42,
      report: {
        cycle_number: 42,
        outcomes: [
          {
            success: true,
            action_kind: 'edit',
            action_description: 'Fixed bug in parser',
            detail: 'detail-text',
          },
        ],
        priorities: [
          { goal_id: 'g1', reason: 'top-priority', urgency: 0.8 },
        ],
      },
    },
  ],
  open_prs: [
    {
      number: 100,
      title: 'Test PR title',
      url: 'https://example.com/pr/100',
      createdAt: new Date(Date.now() - 60_000).toISOString(),
      headRefName: 'fix/test',
    },
  ],
  assigned_issues: [],
  timestamp: new Date().toISOString(),
};

test.describe('Dashboard Overview - live activity surfaces @structural', () => {
  let overview: OverviewPage;

  test.beforeEach(async ({ authenticatedPage }) => {
    // Mock the activity endpoint BEFORE navigation to prevent the real fetch
    // from racing with the mock registration.
    await authenticatedPage.route('**/api/activity', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_ACTIVITY),
      }),
    );
    // Also stub status/issues so the overview page renders cleanly without a
    // live backend (mirrors the parent describe's pattern).
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
        body: JSON.stringify([]),
      }),
    );

    await authenticatedPage.goto('/');
    overview = new OverviewPage(authenticatedPage);
  });

  test('agent-live-status card renders with daemon health and current cycle', async ({
    authenticatedPage,
  }) => {
    await expect(overview.agentLiveStatusCard).toBeVisible();
    // Wait until the loading placeholder is replaced with rendered content.
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('agent-live-status');
        return !!el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    await expect(overview.agentLiveStatus).toBeVisible();
    const text = await overview.agentLiveStatus.textContent();
    expect(text).toContain('OODA Loop Active');
    expect(text).toContain('#42');
  });

  test('recent-actions-list renders cycle outcomes', async ({ authenticatedPage }) => {
    await expect(overview.recentActionsCard).toBeVisible();
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('recent-actions-list');
        return !!el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    await expect(overview.recentActionsList).toBeVisible();
    const text = await overview.recentActionsList.textContent();
    expect(text).toContain('Fixed bug in parser');
    expect(text).toContain('edit');
    expect(text).toContain('#42');
  });

  test('open-prs-list renders open pull requests', async ({ authenticatedPage }) => {
    await expect(overview.openPrsCard).toBeVisible();
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('open-prs-list');
        return !!el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    await expect(overview.openPrsList).toBeVisible();
    const text = await overview.openPrsList.textContent();
    expect(text).toContain('#100');
    expect(text).toContain('Test PR title');
  });
});
