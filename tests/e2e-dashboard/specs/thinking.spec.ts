import { test, expect } from '../fixtures/simard-dashboard';

test.describe('Thinking Tab @structural', () => {
  test('thinking tab loads with cycle reports', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/ooda-thinking', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          reports: [
            {
              cycle_number: 5,
              observation: 'Observed 3 open issues',
              priorities: ['Fix dashboard auth', 'Add tests'],
              actions: ['advance-goal: fix-auth'],
              outcomes: ['Success: auth middleware fixed'],
              timestamp: new Date().toISOString(),
            },
          ],
        }),
      }),
    );

    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="thinking"]').click();
    await expect(authenticatedPage.locator('#tab-thinking')).toBeVisible();

    expect(errors).toEqual([]);
  });

  test('thinking tab shows empty state when no reports', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/ooda-thinking', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ reports: [] }),
      }),
    );

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="thinking"]').click();
    await expect(authenticatedPage.locator('#tab-thinking')).toBeVisible();
  });
});

test.describe('Thinking Tab Live @smoke', () => {
  test('thinking tab loads real data without errors', async ({
    authenticatedPage,
  }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="thinking"]').click();

    await authenticatedPage.waitForResponse(
      (resp) => resp.url().includes('/api/ooda-thinking') && resp.status() === 200,
      { timeout: 10_000 },
    );

    await expect(authenticatedPage.locator('#tab-thinking')).toBeVisible();
    expect(errors).toEqual([]);
  });
});
