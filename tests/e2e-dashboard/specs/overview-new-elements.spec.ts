import { test, expect, type Page } from '../fixtures/simard-dashboard';
import { OverviewPage } from '../pages/overview.page';

/**
 * Structural coverage for newly added dashboard overview elements (issue #948).
 *
 * All API endpoints are mocked so tests run without a live backend.
 * For each new element family we assert:
 *   (a) the card is visible on the default overview tab,
 *   (b) it populates correctly when the mocked API returns data,
 *   (c) it shows a graceful empty/error state when the mock returns empty/500.
 *
 * Mock payloads use only synthetic identifiers (octocat/hello-world,
 * test-host-01, example.com, rysweet-linux-vm-pool placeholder etc.) — no real
 * tenant IDs, subscription IDs, or PATs.
 */

type Json = unknown;

async function mockJson(page: Page, urlPattern: string | RegExp, body: Json, status = 200) {
  await page.route(urlPattern, (route) =>
    route.fulfill({
      status,
      contentType: 'application/json',
      body: JSON.stringify(body),
    }),
  );
}

async function mockError(page: Page, urlPattern: string | RegExp, status = 500) {
  await page.route(urlPattern, (route) =>
    route.fulfill({
      status,
      contentType: 'application/json',
      body: JSON.stringify({ error: 'mocked failure' }),
    }),
  );
}

/**
 * Default mocks (empty/healthy responses) installed before every test so
 * unmocked endpoints don't hang network and slow the test down.
 *
 * Individual tests override specific routes with `mockJson` / `mockError`
 * BEFORE calling `page.goto('/')` to take effect on initial load.
 */
async function installDefaultMocks(page: Page) {
  await mockJson(page, '**/api/status', {
    version: '0.0.0-test',
    git_hash: 'deadbee',
    ooda_status: 'idle',
    uptime_secs: 0,
    cpu_pct: 0,
    mem_usage_pct: 0,
    disk_usage_pct: 0,
    timestamp: new Date().toISOString(),
  });
  await mockJson(page, '**/api/issues', []);
  await mockJson(page, '**/api/activity', { daemon: {}, recent_cycles: [], open_prs: [] });
  await mockJson(page, '**/api/prs', []);
  await mockJson(page, '**/api/distributed', {
    topology: 'standalone',
    local: { hostname: 'test-host-01' },
    hive_mind: { protocol: 'DHT+bloom gossip', status: 'standalone' },
    remote_vms: [],
    timestamp: new Date().toISOString(),
  });
  await mockJson(page, '**/api/hosts', { discovered: [], hosts: [] });
  await mockJson(page, '**/api/goals', []);
  await mockJson(page, '**/api/processes', []);
  await mockJson(page, '**/api/memory', {});
  await mockJson(page, '**/api/costs', {});
  await mockJson(page, '**/api/traces', []);
  await mockJson(page, '**/api/logs', []);
  await mockJson(page, '**/api/thinking', []);
}

/**
 * Wait until the loading placeholder inside an element has been replaced.
 * Uses a generous timeout because some renderers wait for multiple fetches.
 */
async function waitForLoaded(page: Page, id: string, timeoutMs = 10_000) {
  await page.waitForFunction(
    (elementId) => {
      const el = document.getElementById(elementId);
      return !!el && !el.querySelector('.loading');
    },
    id,
    { timeout: timeoutMs },
  );
}

test.describe('Dashboard Overview — new elements @structural', () => {
  let overview: OverviewPage;

  test.beforeEach(async ({ authenticatedPage }) => {
    await installDefaultMocks(authenticatedPage);
    overview = new OverviewPage(authenticatedPage);
  });

  // -----------------------------------------------------------------------
  // 1. agent-live-status (autonomous agent banner)
  // -----------------------------------------------------------------------
  test.describe('agent-live-status banner', () => {
    test('is visible on the default overview tab', async ({ authenticatedPage }) => {
      await authenticatedPage.goto('/');
      await expect(overview.agentLiveStatusCard).toBeVisible();
      await expect(overview.agentLiveStatus).toBeVisible();
      await expect(overview.agentLiveStatusCard.locator('h2')).toContainText('Autonomous Agent');
    });

    test('populates when /api/activity returns a healthy daemon', async ({ authenticatedPage }) => {
      await mockJson(authenticatedPage, '**/api/activity', {
        daemon: {
          status: 'healthy',
          last_heartbeat: new Date().toISOString(),
          current_cycle: 42,
        },
        recent_cycles: [
          {
            cycle_number: 42,
            report: {
              cycle_number: 42,
              priorities: [
                { goal_id: 'goal-test-1', reason: 'synthetic priority', urgency: 0.5 },
              ],
              outcomes: [
                {
                  success: true,
                  action_kind: 'build',
                  action_description: 'cargo build succeeded',
                  detail: 'finished in 1s',
                },
              ],
            },
          },
        ],
        open_prs: [],
      });
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'agent-live-status');
      const text = (await overview.agentLiveStatus.textContent()) ?? '';
      expect(text).toContain('OODA Loop Active');
      expect(text).toContain('#42');
      expect(text).toContain('cargo build succeeded');
    });

    test('shows graceful error state when /api/activity returns 500', async ({ authenticatedPage }) => {
      await mockError(authenticatedPage, '**/api/activity', 500);
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'agent-live-status');
      await expect(overview.agentLiveStatus).toContainText('Failed to load agent status');
    });
  });

  // -----------------------------------------------------------------------
  // 2. recent-actions-list (Recent Actions card)
  // -----------------------------------------------------------------------
  test.describe('recent-actions-list card', () => {
    test('is visible on the default overview tab', async ({ authenticatedPage }) => {
      await authenticatedPage.goto('/');
      await expect(overview.recentActionsCard).toBeVisible();
      await expect(overview.recentActionsList).toBeVisible();
      await expect(overview.recentActionsCard.locator('h2')).toContainText('Recent Actions');
    });

    test('populates from /api/activity cycle outcomes', async ({ authenticatedPage }) => {
      await mockJson(authenticatedPage, '**/api/activity', {
        daemon: { status: 'healthy', last_heartbeat: new Date().toISOString(), current_cycle: 7 },
        recent_cycles: [
          {
            cycle_number: 7,
            report: {
              cycle_number: 7,
              outcomes: [
                { success: true, action_kind: 'lint', action_description: 'clippy clean' },
                { success: false, action_kind: 'test', action_description: 'flake in foo_test' },
              ],
            },
          },
        ],
        open_prs: [],
      });
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'recent-actions-list');
      const text = (await overview.recentActionsList.textContent()) ?? '';
      expect(text).toContain('clippy clean');
      expect(text).toContain('flake in foo_test');
      expect(text).toContain('#7');
    });

    test('shows empty-state copy when no outcomes are recorded', async ({ authenticatedPage }) => {
      // Default activity mock already returns recent_cycles: []
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'recent-actions-list');
      await expect(overview.recentActionsList).toContainText('No structured action history yet');
    });
  });

  // -----------------------------------------------------------------------
  // 3. open-prs-list (Open PRs card)
  // -----------------------------------------------------------------------
  test.describe('open-prs-list card', () => {
    test('is visible on the default overview tab', async ({ authenticatedPage }) => {
      await authenticatedPage.goto('/');
      await expect(overview.openPrsCard).toBeVisible();
      await expect(overview.openPrsList).toBeVisible();
      await expect(overview.openPrsCard.locator('h2')).toContainText('Open PRs');
    });

    test('populates when /api/activity returns open_prs', async ({ authenticatedPage }) => {
      await mockJson(authenticatedPage, '**/api/activity', {
        daemon: {},
        recent_cycles: [],
        open_prs: [
          {
            number: 101,
            title: 'feat: synthetic test PR',
            url: 'https://example.com/octocat/hello-world/pull/101',
            createdAt: new Date().toISOString(),
          },
          {
            number: 102,
            title: 'chore: another synthetic PR',
            url: 'https://example.com/octocat/hello-world/pull/102',
            createdAt: new Date().toISOString(),
          },
        ],
      });
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'open-prs-list');
      const text = (await overview.openPrsList.textContent()) ?? '';
      expect(text).toContain('#101');
      expect(text).toContain('synthetic test PR');
      expect(text).toContain('#102');
    });

    test('shows empty-state copy when no PRs are open', async ({ authenticatedPage }) => {
      // Default activity mock returns open_prs: []
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'open-prs-list');
      await expect(overview.openPrsList).toContainText('No open PRs');
    });
  });

  // -----------------------------------------------------------------------
  // 4. cluster-topology (Cluster Topology card)
  //
  // Cluster/remote-vms only render when the user clicks Refresh
  // (fetchDistributed is opt-in due to its 10–30s blocking nature). We assert
  // visibility/empty-state at load, and populated state after explicit refresh.
  // -----------------------------------------------------------------------
  test.describe('cluster-topology card', () => {
    test('is visible on the default overview tab with loading placeholder', async ({ authenticatedPage }) => {
      await authenticatedPage.goto('/');
      await expect(overview.clusterTopologyCard).toBeVisible();
      await expect(overview.clusterTopology).toBeVisible();
      await expect(overview.clusterTopologyCard.locator('h2')).toContainText('Cluster Topology');
    });

    test('populates after Refresh when /api/distributed returns data', async ({ authenticatedPage }) => {
      await mockJson(authenticatedPage, '**/api/distributed', {
        topology: 'mesh',
        local: { hostname: 'test-host-01' },
        hive_mind: { protocol: 'DHT+bloom gossip', status: 'active', peers: 3, facts_shared: 42 },
        remote_vms: [],
        timestamp: new Date().toISOString(),
      });
      await authenticatedPage.goto('/');
      await overview.clusterTopologyCard.locator('button.btn', { hasText: 'Refresh' }).click();
      await waitForLoaded(authenticatedPage, 'cluster-topology');
      const text = (await overview.clusterTopology.textContent()) ?? '';
      expect(text).toContain('mesh');
      expect(text).toContain('test-host-01');
      expect(text).toContain('active');
    });

    test('shows graceful error state when /api/distributed returns 500', async ({ authenticatedPage }) => {
      await mockError(authenticatedPage, '**/api/distributed', 500);
      await authenticatedPage.goto('/');
      await overview.clusterTopologyCard.locator('button.btn', { hasText: 'Refresh' }).click();
      await waitForLoaded(authenticatedPage, 'cluster-topology');
      await expect(overview.clusterTopology).toContainText('Failed to query distributed status');
    });
  });

  // -----------------------------------------------------------------------
  // 5. remote-vms (Remote VMs card)
  // -----------------------------------------------------------------------
  test.describe('remote-vms card', () => {
    test('is visible on the default overview tab', async ({ authenticatedPage }) => {
      await authenticatedPage.goto('/');
      await expect(overview.remoteVmsCard).toBeVisible();
      await expect(overview.remoteVms).toBeVisible();
      await expect(overview.remoteVmsCard.locator('h2')).toContainText('Remote VMs');
    });

    test('populates after Refresh when /api/distributed returns remote_vms', async ({ authenticatedPage }) => {
      await mockJson(authenticatedPage, '**/api/distributed', {
        topology: 'mesh',
        local: { hostname: 'test-host-01' },
        hive_mind: { protocol: 'DHT+bloom gossip', status: 'standalone' },
        remote_vms: [
          {
            vm_name: 'test-vm-alpha',
            status: 'reachable',
            hostname: 'alpha.example.com',
            uptime: '1d',
            load_avg: '0.10 0.10 0.10',
            memory_mb: 2048,
            disk_root_pct: 10,
          },
        ],
        timestamp: new Date().toISOString(),
      });
      await authenticatedPage.goto('/');
      await overview.clusterTopologyCard.locator('button.btn', { hasText: 'Refresh' }).click();
      await waitForLoaded(authenticatedPage, 'remote-vms');
      const text = (await overview.remoteVms.textContent()) ?? '';
      expect(text).toContain('test-vm-alpha');
      expect(text).toContain('alpha.example.com');
      expect(text).toContain('reachable');
    });

    test('shows empty-state copy after Refresh when no remote_vms', async ({ authenticatedPage }) => {
      // Default mock returns remote_vms: []
      await authenticatedPage.goto('/');
      await overview.clusterTopologyCard.locator('button.btn', { hasText: 'Refresh' }).click();
      await waitForLoaded(authenticatedPage, 'remote-vms');
      await expect(overview.remoteVms).toContainText('No remote VMs configured');
    });
  });

  // -----------------------------------------------------------------------
  // 6. hosts-list / host-name / host-rg (Azlin Hosts card)
  // -----------------------------------------------------------------------
  test.describe('Azlin hosts card', () => {
    test('is visible with name + resource-group inputs on the default overview tab', async ({ authenticatedPage }) => {
      await authenticatedPage.goto('/');
      await expect(overview.hostsCard).toBeVisible();
      await expect(overview.hostsList).toBeVisible();
      await expect(overview.hostsCard.locator('h2')).toContainText('Azlin Hosts');
      await expect(overview.hostNameInput).toBeVisible();
      await expect(overview.hostRgInput).toBeVisible();
      // Inputs should not contain real Azure resource-group identifiers — only
      // the synthetic placeholder shipped with the dashboard.
      await expect(overview.hostNameInput).toHaveAttribute('placeholder', /VM name/i);
    });

    test('populates from /api/hosts with discovered + configured entries', async ({ authenticatedPage }) => {
      await mockJson(authenticatedPage, '**/api/hosts', {
        discovered: [
          { name: 'test-host-01', location: 'eastus', resourceGroup: 'rg-synthetic' },
        ],
        hosts: [
          { name: 'test-host-02', resource_group: 'rg-synthetic', added_at: new Date().toISOString() },
        ],
      });
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'hosts-list');
      const text = (await overview.hostsList.textContent()) ?? '';
      expect(text).toContain('test-host-01');
      expect(text).toContain('test-host-02');
      expect(text).toContain('rg-synthetic');
    });

    test('shows graceful error state when /api/hosts returns 500', async ({ authenticatedPage }) => {
      await mockError(authenticatedPage, '**/api/hosts', 500);
      await authenticatedPage.goto('/');
      await waitForLoaded(authenticatedPage, 'hosts-list');
      await expect(overview.hostsList).toContainText('Failed to load hosts');
    });
  });
});
