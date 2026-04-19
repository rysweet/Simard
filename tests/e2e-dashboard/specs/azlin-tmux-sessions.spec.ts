import { test, expect } from '../fixtures/simard-dashboard';

// WS-1 AZLIN-TMUX-SESSIONS-LIST — @structural E2E (TDD, Step 7).
//
// These tests EXPECT TO FAIL until Step 8 ships:
//   - GET  /api/azlin/tmux-sessions   (snapshot route)
//   - WS   /ws/tmux_attach/{host}/{session}
//   - <section id="azlin-sessions-panel"> rendered inside #tab-terminal
//   - data-testid hooks: tmux-table-{host}, tmux-open-{host}-{session},
//     tmux-last-refreshed
//
// The spec mocks every backend call so it runs hermetically against the
// existing dashboard server (the panel just needs the markup + JS to exist).

test.describe('Azlin Tmux Sessions Panel @structural', () => {
  const consoleErrors: string[] = [];

  test.beforeEach(async ({ authenticatedPage }) => {
    consoleErrors.length = 0;
    authenticatedPage.on('console', (msg) => {
      if (msg.type() === 'error') consoleErrors.push(msg.text());
    });

    // Mock canonical hosts list (used elsewhere on the dashboard).
    await authenticatedPage.route('**/api/hosts', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          hosts: [
            { name: 'vm-1', resource_group: 'rg1' },
            { name: 'vm-2', resource_group: 'rg2' },
          ],
        }),
      }),
    );

    // Mock the new snapshot route: 1 reachable (2 sessions), 1 unreachable.
    await authenticatedPage.route('**/api/azlin/tmux-sessions', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          hosts: [
            {
              host: 'vm-1',
              reachable: true,
              error: null,
              sessions: [
                { name: 'main', created: 1700000000, attached: false, windows: 3 },
                { name: 'work', created: 1700000500, attached: true, windows: 1 },
              ],
            },
            {
              host: 'vm-2',
              reachable: false,
              error: 'connection timed out',
              sessions: [],
            },
          ],
          refreshed_at: new Date().toISOString(),
        }),
      }),
    );
  });

  test('renders a per-host table with sessions and a last-refreshed timestamp', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.goto('/');
    await authenticatedPage
      .locator('[data-tab="terminal"], button:has-text("Terminal")')
      .first()
      .click();

    // The new companion panel must be visible inside the Terminal tab.
    const panel = authenticatedPage.locator('#azlin-sessions-panel');
    await expect(panel).toBeVisible({ timeout: 5_000 });

    // Per-host table for vm-1 (reachable) renders both sessions.
    const vm1Table = authenticatedPage.locator('[data-testid="tmux-table-vm-1"]');
    await expect(vm1Table).toBeVisible();
    await expect(vm1Table).toContainText('main');
    await expect(vm1Table).toContainText('work');
    await expect(vm1Table).toContainText('3'); // windows for "main"

    // Open buttons exist for each session.
    await expect(
      authenticatedPage.locator('[data-testid="tmux-open-vm-1-main"]'),
    ).toBeVisible();
    await expect(
      authenticatedPage.locator('[data-testid="tmux-open-vm-1-work"]'),
    ).toBeVisible();

    // Unreachable host renders with its error text and no Open buttons.
    const vm2Block = authenticatedPage.locator('[data-testid="tmux-table-vm-2"]');
    await expect(vm2Block).toBeVisible();
    await expect(vm2Block).toContainText('connection timed out');

    // Last-refreshed timestamp surfaced.
    await expect(
      authenticatedPage.locator('[data-testid="tmux-last-refreshed"], #tmux-last-refreshed'),
    ).toBeVisible();

    expect(consoleErrors, `unexpected console errors: ${consoleErrors.join(' | ')}`).toEqual(
      [],
    );
  });

  test('clicking Open opens /ws/tmux_attach/{host}/{session} and streams into xterm', async ({
    authenticatedPage,
  }) => {
    let wsUrl: string | null = null;

    await authenticatedPage.routeWebSocket('**/ws/tmux_attach/vm-1/main', (ws) => {
      wsUrl = ws.url();
      // Send a canned tmux-style server frame.
      ws.send('tmux-attached: vm-1:main\r\n');
    });

    await authenticatedPage.goto('/');
    await authenticatedPage
      .locator('[data-tab="terminal"], button:has-text("Terminal")')
      .first()
      .click();

    await authenticatedPage.locator('[data-testid="tmux-open-vm-1-main"]').click();

    // Verify the WS opened to the expected path.
    await expect.poll(() => wsUrl, { timeout: 5_000 }).not.toBeNull();
    expect(wsUrl).toMatch(/\/ws\/tmux_attach\/vm-1\/main(\?|$)/);

    // Server frame appears in xterm host (existing #xterm-host element reused).
    const host = authenticatedPage.locator('#xterm-host');
    await expect(host).toContainText('tmux-attached: vm-1:main', { timeout: 5_000 });

    // Status surface updated to reflect the attached session.
    await expect(authenticatedPage.locator('#agent-log-name')).toHaveValue(/vm-1[:/]main/);
  });
});
