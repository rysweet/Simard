import { test, expect } from '../fixtures/simard-dashboard';

// Issue #947 — Terminal widget structural tests.
// Mocks the /ws/agent_log/<name> WebSocket and verifies that:
//   1. The Terminal tab is present and switchable.
//   2. Connecting opens a WS to the correct path.
//   3. Server-sent text frames render as lines in the xterm host.
//   4. Disconnecting closes the WS cleanly with no console errors.
//   5. Invalid agent names are rejected client-side (no WS opened) OR
//      surfaced as a status message after a 400 from the server.

test.describe('Agent Terminal Widget @structural', () => {
  const consoleErrors: string[] = [];

  test.beforeEach(async ({ authenticatedPage }) => {
    consoleErrors.length = 0;
    authenticatedPage.on('console', (msg) => {
      if (msg.type() === 'error') {
        consoleErrors.push(msg.text());
      }
    });
  });

  test('Terminal tab is present and selectable', async ({ authenticatedPage }) => {
    await authenticatedPage.goto('/');
    const terminalTab = authenticatedPage.locator('[data-tab="terminal"], button:has-text("Terminal")').first();
    await expect(terminalTab).toBeVisible();
    await terminalTab.click();
    await expect(authenticatedPage.locator('#tab-terminal')).toBeVisible();
    await expect(authenticatedPage.locator('#xterm-host')).toBeVisible();
  });

  test('connecting to a valid agent name streams lines into xterm', async ({ authenticatedPage }) => {
    let wsOpened = false;
    await authenticatedPage.routeWebSocket('**/ws/agent_log/planner', (ws) => {
      wsOpened = true;
      // Backfill + live frames; client should writeln each.
      ws.send('line one');
      ws.send('line two');
      ws.send('line three');
    });

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('[data-tab="terminal"], button:has-text("Terminal")').first().click();

    // Enter agent name and connect.
    const nameInput = authenticatedPage.locator('#agent-log-name');
    await nameInput.fill('planner');
    await authenticatedPage.locator('#agent-log-connect').click();

    // Wait for all three lines to appear in the xterm host (xterm.js renders into
    // .xterm-rows). We assert against textContent of the xterm host container.
    const host = authenticatedPage.locator('#xterm-host');
    await expect(host).toContainText('line one', { timeout: 5_000 });
    await expect(host).toContainText('line two');
    await expect(host).toContainText('line three');

    expect(wsOpened).toBe(true);
  });

  test('disconnect closes WS cleanly with no console errors', async ({ authenticatedPage }) => {
    let closed = false;
    await authenticatedPage.routeWebSocket('**/ws/agent_log/planner', (ws) => {
      ws.send('hello');
      ws.onClose(() => {
        closed = true;
      });
    });

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('[data-tab="terminal"], button:has-text("Terminal")').first().click();
    await authenticatedPage.locator('#agent-log-name').fill('planner');
    await authenticatedPage.locator('#agent-log-connect').click();
    await expect(authenticatedPage.locator('#xterm-host')).toContainText('hello', { timeout: 5_000 });

    await authenticatedPage.locator('#agent-log-disconnect').click();

    // Allow the close handshake to settle.
    await authenticatedPage.waitForTimeout(250);
    expect(closed).toBe(true);
    expect(consoleErrors).toEqual([]);
  });

  test('invalid agent name is rejected without opening a WS', async ({ authenticatedPage }) => {
    let wsOpened = false;
    await authenticatedPage.routeWebSocket('**/ws/agent_log/**', (_ws) => {
      wsOpened = true;
    });

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('[data-tab="terminal"], button:has-text("Terminal")').first().click();
    await authenticatedPage.locator('#agent-log-name').fill('../etc/passwd');
    await authenticatedPage.locator('#agent-log-connect').click();

    // Status surface should report rejection; either client-side or after 400.
    const status = authenticatedPage.locator('#agent-log-status');
    await expect(status).toContainText(/invalid|reject|disallowed/i, { timeout: 2_000 });
    expect(wsOpened).toBe(false);
  });
});
