import { test, expect } from '../fixtures/simard-dashboard';

test.describe('Goals Tab @structural', () => {
  test('goals tab renders with active goals and current activity column', async ({
    authenticatedPage,
    baseURL,
  }) => {
    // Mock goals API with current_activity and wip_refs
    await authenticatedPage.route('**/api/goals', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active: [
            {
              id: 'test-goal-1',
              description: 'Test goal with activity',
              priority: 1,
              status: 'in-progress(50%)',
              assigned_to: null,
              current_activity: 'advance-goal: Working on PR #123',
              wip_refs: [
                { kind: 'pr', ref_id: '123', label: 'PR #123: Add tests', url: 'https://github.com/test/repo/pull/123' },
                { kind: 'issue', ref_id: '456', label: 'Issue #456: Bug fix', url: 'https://github.com/test/repo/issues/456' },
              ],
            },
            {
              id: 'test-goal-2',
              description: 'Idle goal',
              priority: 2,
              status: 'not-started',
              assigned_to: null,
              current_activity: null,
              wip_refs: [],
            },
          ],
          backlog: [],
          active_count: 2,
          backlog_count: 0,
        }),
      }),
    );

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="goals"]').click();
    await expect(authenticatedPage.locator('#tab-goals')).toBeVisible();

    // Should have Current Activity column header
    await expect(authenticatedPage.locator('#goals-active th:has-text("Current Activity")')).toBeVisible();

    // Should show the activity text for goal 1
    await expect(authenticatedPage.locator('#goals-active').getByText('Working on PR #123')).toBeVisible();

    // Should show WIP reference links
    await expect(authenticatedPage.locator('#goals-active a:has-text("PR #123: Add tests")')).toBeVisible();
    await expect(authenticatedPage.locator('#goals-active a:has-text("Issue #456: Bug fix")')).toBeVisible();

    // Idle goal should show dash
    const rows = authenticatedPage.locator('#goals-active table tr');
    const idleRow = rows.filter({ hasText: 'Idle goal' });
    await expect(idleRow).toBeVisible();
  });

  test('goals tab shows active lifecycle breakdown next to the count', async ({
    authenticatedPage,
  }) => {
    // The active board mixes in-progress, blocked, and already-completed goals.
    // The count line must break them down so the operator can see what Simard
    // is ACTUALLY working on right now — not just a conflated "3 active goal(s)".
    await authenticatedPage.route('**/api/goals', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active: [
            { id: 'g1', description: 'In flight', priority: 1, status: 'in-progress(50%)', status_progress: { InProgress: { percent: 50 } }, current_activity: null, wip_refs: [] },
            { id: 'g2', description: 'Waiting on upstream', priority: 2, status: 'blocked: upstream', status_progress: { Blocked: 'upstream' }, current_activity: null, wip_refs: [] },
            { id: 'g3', description: 'Finished but not archived', priority: 3, status: 'completed', status_progress: 'Completed', current_activity: null, wip_refs: [] },
          ],
          backlog: [],
          active_count: 3,
          backlog_count: 0,
          active_status_breakdown: {
            proposed: 0, not_started: 0, in_progress: 1, blocked: 1, paused: 0, completed: 1,
          },
        }),
      }),
    );

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="goals"]').click();

    const countLine = authenticatedPage.locator('#goals-active').getByText('3 active goal(s)');
    await expect(countLine).toBeVisible();
    // Only nonzero buckets, in priority order (in progress · blocked · completed).
    await expect(countLine).toContainText('1 in progress');
    await expect(countLine).toContainText('1 blocked');
    await expect(countLine).toContainText('1 completed');
    // Zero buckets must be omitted (no "0 paused" noise).
    await expect(countLine).not.toContainText('paused');
    await expect(countLine).not.toContainText('proposed');
  });

  test('goals tab shows backlog with promote button', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/goals', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active: [],
          backlog: [
            { id: 'backlog-1', description: 'Future work', source: 'auto', score: 0.8 },
          ],
          active_count: 0,
          backlog_count: 1,
        }),
      }),
    );

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="goals"]').click();

    await expect(authenticatedPage.locator('#goals-backlog').getByText('Future work')).toBeVisible();
    await expect(authenticatedPage.locator('#goals-backlog button:has-text("Promote")')).toBeVisible();
  });

  test('goals tab shows empty state message when no goals', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/goals', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          active: [],
          backlog: [],
          active_count: 0,
          backlog_count: 0,
        }),
      }),
    );

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="goals"]').click();

    await expect(authenticatedPage.locator('#goals-active').getByText('No active goals')).toBeVisible();
  });
});

test.describe('Goals Tab Live @smoke', () => {
  test('goals tab loads real data without JS errors', async ({
    authenticatedPage,
  }) => {
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));

    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="goals"]').click();
    await expect(authenticatedPage.locator('#tab-goals')).toBeVisible();

    // Wait for goals to load
    await authenticatedPage.waitForResponse(
      (resp) => resp.url().includes('/api/goals') && resp.status() === 200,
      { timeout: 10_000 },
    );

    // Should have either goals or empty state — no errors
    const active = authenticatedPage.locator('#goals-active');
    await expect(active).not.toBeEmpty();
    expect(errors).toEqual([]);
  });
});
