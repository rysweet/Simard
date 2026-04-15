import { test, expect } from '../fixtures/simard-dashboard';

test.describe('System Status API @structural', () => {
  test('/api/status returns JSON with version', async ({
    authenticatedPage,
    baseURL,
  }) => {
    const resp = await authenticatedPage.request.get(`${baseURL}/api/status`);
    expect(resp.status()).toBe(200);
    const body = await resp.json();
    expect(body).toHaveProperty('version');
    expect(body).toHaveProperty('git_hash');
  });

  test('status card updates after fetch', async ({ authenticatedPage }) => {
    await authenticatedPage.route('**/api/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          version: '0.7.1.42',
          git_hash: 'deadbeef',
          ooda_status: 'running',
          uptime_secs: 7200,
        }),
      }),
    );
    // Mock other endpoints the page fetches on load
    await authenticatedPage.route('**/api/issues', (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: '[]' }),
    );

    await authenticatedPage.goto('/');

    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('status');
        return el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );

    const statusText = await authenticatedPage.locator('#status').textContent();
    expect(statusText).toContain('0.7.1.42');
    // Dashboard truncates git hash to 7 chars
    expect(statusText).toContain('deadbee');
  });
});
