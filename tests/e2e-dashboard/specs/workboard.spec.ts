import { test, expect } from '../fixtures/simard-dashboard';

test.describe('Whiteboard Tab @structural', () => {
  test('whiteboard tab loads without JS errors', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/workboard', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          cycle: { number: 5, phase: 'act', interval_secs: 300 },
          goals: [
            { name: 'test-goal', status: 'in_progress', progress_pct: 50 },
          ],
          spawned_engineers: [],
          recent_actions: [
            { action: 'advance-goal', target: 'test-goal', result: 'Working', cycle: 5, at: new Date().toISOString() },
          ],
          task_memory: [],
          working_memory: [],
          cognitive_statistics: { total_facts: 42, total_episodes: 10 },
          uptime_seconds: 3600,
          timestamp: new Date().toISOString(),
          next_cycle_eta_seconds: 120,
        }),
      }),
    );

    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="workboard"]').click();
    await expect(authenticatedPage.locator('#tab-workboard')).toBeVisible();

    expect(errors).toEqual([]);
  });

  test('whiteboard tab shows cycle and goals info', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/workboard', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          cycle: { number: 10, phase: 'observe', interval_secs: 300 },
          goals: [
            { name: 'improve-tests', status: 'in_progress', progress_pct: 75 },
          ],
          spawned_engineers: [],
          recent_actions: [],
          task_memory: [],
          working_memory: [],
          cognitive_statistics: {},
          uptime_seconds: 7200,
          timestamp: new Date().toISOString(),
          next_cycle_eta_seconds: 60,
        }),
      }),
    );

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="workboard"]').click();

    // Should show cycle information
    await expect(authenticatedPage.locator('#tab-workboard')).toContainText('10');
  });
});

test.describe('Whiteboard Tab Live @smoke', () => {
  test('whiteboard loads real data', async ({ authenticatedPage }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="workboard"]').click();

    await authenticatedPage.waitForResponse(
      (resp) => resp.url().includes('/api/workboard') && resp.status() === 200,
      { timeout: 10_000 },
    );

    await expect(authenticatedPage.locator('#tab-workboard')).toBeVisible();
    expect(errors).toEqual([]);
  });
});
