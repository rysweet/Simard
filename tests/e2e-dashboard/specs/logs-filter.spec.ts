import { test, expect } from '../fixtures/simard-dashboard';

// Proves the fix for issue #1687: the Logs tab severity dropdown
// (All levels / Errors / Warnings / Info) must actually filter the rendered
// daemon-log tail by per-line log level, instead of doing a naive substring
// match that made plain "[simard] …" daemon lines vanish under "Info".

// A deterministic mix: 4 informational lines (3 plain daemon lines with no
// level token + 1 tracing INFO line), 1 WARN line, and 2 ERROR lines.
const PLAIN_INFO = [
  '2026-06-21T04:42:19Z [simard] RSS health: 379 MiB',
  '2026-06-21T04:48:20Z [simard] OODA cycle #5: 4 priorities, 4 actions (4/4 succeeded)',
  '2026-06-21T05:04:38Z [simard] disk health: 32% used, no cleanup needed',
];
const TRACING_INFO =
  '2026-06-21T16:13:12.043717Z  INFO simard::base_type_copilot: Copilot adapter: received response';
const WARN_LINE =
  '2026-06-21T16:14:00.000000Z  WARN simard::engineer_worktree: worktree sweep skipped';
const ERROR_LINES = [
  '2026-06-21T16:15:00.000000Z ERROR simard::brain: brain-error fallback engaged',
  '2026-06-21T16:16:00.000000Z ERROR simard::ooda: goal-action parse failed',
];

const ALL_LINES = [...PLAIN_INFO, TRACING_INFO, WARN_LINE, ...ERROR_LINES];

async function mockLogs(page: import('@playwright/test').Page) {
  await page.route('**/api/logs', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        daemon_log_lines: ALL_LINES,
        ooda_transcripts: [],
        terminal_transcripts: [],
        cost_log_lines: [],
        cycle_reports: [],
        timestamp: new Date().toISOString(),
      }),
    }),
  );
}

async function openLogsTab(page: import('@playwright/test').Page) {
  await page.goto('/');
  await page.locator('.tab[data-tab="logs"]').click();
  await expect(page.locator('#tab-logs')).toBeVisible();
  // Wait until the mocked tail has rendered.
  await expect(page.locator('#log-line-count')).toHaveText(
    `${ALL_LINES.length}/${ALL_LINES.length} lines`,
  );
}

test.describe('Logs severity filter @structural', () => {
  test('"All levels" shows every captured line', async ({ authenticatedPage }) => {
    await mockLogs(authenticatedPage);
    await openLogsTab(authenticatedPage);

    const log = authenticatedPage.locator('#daemon-log');
    for (const line of ALL_LINES) {
      await expect(log).toContainText(line);
    }
  });

  test('"Info" keeps plain daemon lines instead of hiding them (#1687)', async ({
    authenticatedPage,
  }) => {
    await mockLogs(authenticatedPage);
    await openLogsTab(authenticatedPage);

    await authenticatedPage.locator('#log-level-filter').selectOption('info');

    // The regression: under the old substring filter, selecting "Info" matched
    // the literal text "info", which is absent from "[simard] …" lines, so all
    // four informational lines disappeared. They must now remain.
    await expect(authenticatedPage.locator('#log-line-count')).toHaveText('4/7 lines');
    const log = authenticatedPage.locator('#daemon-log');
    await expect(log).not.toHaveText('(no matching log lines)');
    for (const line of [...PLAIN_INFO, TRACING_INFO]) {
      await expect(log).toContainText(line);
    }
    // No error/warn lines leak into the Info view.
    await expect(log).not.toContainText('ERROR');
    await expect(log).not.toContainText('WARN');
  });

  test('"Errors" surfaces only ERROR-level lines', async ({ authenticatedPage }) => {
    await mockLogs(authenticatedPage);
    await openLogsTab(authenticatedPage);

    await authenticatedPage.locator('#log-level-filter').selectOption('error');

    await expect(authenticatedPage.locator('#log-line-count')).toHaveText('2/7 lines');
    const log = authenticatedPage.locator('#daemon-log');
    for (const line of ERROR_LINES) {
      await expect(log).toContainText(line);
    }
    // Informational chatter must not appear under "Errors".
    await expect(log).not.toContainText('RSS health');
    await expect(log).not.toContainText('disk health');
  });

  test('"Warnings" surfaces only WARN-level lines', async ({ authenticatedPage }) => {
    await mockLogs(authenticatedPage);
    await openLogsTab(authenticatedPage);

    await authenticatedPage.locator('#log-level-filter').selectOption('warn');

    await expect(authenticatedPage.locator('#log-line-count')).toHaveText('1/7 lines');
    const log = authenticatedPage.locator('#daemon-log');
    await expect(log).toContainText(WARN_LINE);
    await expect(log).not.toContainText('ERROR');
    await expect(log).not.toContainText('RSS health');
  });

  test('severity filter composes with the text filter', async ({ authenticatedPage }) => {
    await mockLogs(authenticatedPage);
    await openLogsTab(authenticatedPage);

    await authenticatedPage.locator('#log-level-filter').selectOption('error');
    await authenticatedPage.locator('#log-filter').fill('parse failed');

    await expect(authenticatedPage.locator('#log-line-count')).toHaveText('1/7 lines');
    await expect(authenticatedPage.locator('#daemon-log')).toContainText(
      'goal-action parse failed',
    );
  });
});

test.describe('Logs severity filter — live data @smoke', () => {
  test('"Info" is non-empty against the real daemon log (#1687)', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="logs"]').click();
    await expect(authenticatedPage.locator('#tab-logs')).toBeVisible();

    await authenticatedPage.waitForResponse(
      (resp) => resp.url().includes('/api/logs') && resp.status() === 200,
      { timeout: 10_000 },
    );

    // Wait for the JS to finish rendering the tail before reading the counter
    // (waitForResponse resolves before the response body is processed).
    const counter = authenticatedPage.locator('#log-line-count');
    await expect(counter).toHaveText(/^\d+\/\d+ lines$/, { timeout: 10_000 });

    const countText = (await counter.textContent()) ?? '0/0';
    const total = Number(countText.split('/')[1]?.split(' ')[0] ?? '0');
    test.skip(total === 0, 'no daemon log lines available on this host');

    await authenticatedPage.locator('#log-level-filter').selectOption('info');
    await expect(counter).toHaveText(/^\d+\/\d+ lines$/);
    const shown = Number(((await counter.textContent()) ?? '0/0').split('/')[0]);
    expect(shown).toBeGreaterThan(0);
    await expect(authenticatedPage.locator('#daemon-log')).not.toHaveText(
      '(no matching log lines)',
    );
  });
});
