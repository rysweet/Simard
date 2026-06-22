import { test, expect } from '../fixtures/simard-dashboard';
import type { Page } from '@playwright/test';

// Regression coverage for issue #1687: the Logs tab "Background Service Log"
// level filter (All / Errors / Warnings / Info) was inert because the daemon
// emits lines with no level token and the old filter matched the literal
// substrings "error" / "warn" / "info". Selecting any level except "All"
// therefore showed zero lines — even "Info", though every line is info-level.
//
// These tests drive the real shipped frontend with a deterministic, mocked
// /api/logs payload so they run in CI without a live daemon.

const ERROR_LINE = '2026-06-20T10:00:00Z [simard] goal-action parse failed for goal abc';
const WARN_LINE = '2026-06-20T10:01:00Z [simard] WARN retrying request after timeout';
const INFO_LINE_1 = '2026-06-20T10:02:00Z [simard] OODA cycle #1: 4 actions (4/4 succeeded)';
const INFO_LINE_2 = '2026-06-20T10:03:00Z [simard] disk health: 82% used, freed 0 bytes';

const ALL_LINES = [ERROR_LINE, WARN_LINE, INFO_LINE_1, INFO_LINE_2];

async function mockLogs(page: Page, withLevels: boolean): Promise<void> {
  const body: Record<string, unknown> = {
    daemon_log_lines: ALL_LINES,
    ooda_transcripts: [],
    terminal_transcripts: [],
    cost_log_lines: [],
    cycle_reports: [],
    timestamp: new Date().toISOString(),
  };
  if (withLevels) {
    body.daemon_log_levels = ['error', 'warn', 'info', 'info'];
  }
  await page.route('**/api/logs', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(body),
    }),
  );
}

async function openLogsTab(page: Page): Promise<void> {
  await page.goto('/');
  await page.locator('.tab[data-tab="logs"]').click();
  // Default ("All levels") shows every line.
  await expect(page.locator('#daemon-log')).toContainText(INFO_LINE_1);
}

async function selectLevel(page: Page, value: string): Promise<void> {
  await page.locator('#log-level-filter').selectOption(value);
}

test.describe('Logs level filter @structural', () => {
  test('selecting "Errors" hides INFO and WARN lines (#1687)', async ({
    authenticatedPage,
  }) => {
    await mockLogs(authenticatedPage, /* withLevels */ true);
    await openLogsTab(authenticatedPage);

    const logBox = authenticatedPage.locator('#daemon-log');
    const count = authenticatedPage.locator('#log-line-count');

    // Baseline: all four lines visible.
    await expect(count).toHaveText('4/4 lines');

    // Errors: only the error line remains; info/warn lines are hidden.
    await selectLevel(authenticatedPage, 'error');
    await expect(logBox).toContainText(ERROR_LINE);
    await expect(logBox).not.toContainText(INFO_LINE_1);
    await expect(logBox).not.toContainText(INFO_LINE_2);
    await expect(logBox).not.toContainText('retrying request');
    await expect(count).toHaveText('1/4 lines');
  });

  test('each level selection filters to that level; "All levels" restores everything', async ({
    authenticatedPage,
  }) => {
    await mockLogs(authenticatedPage, true);
    await openLogsTab(authenticatedPage);

    const logBox = authenticatedPage.locator('#daemon-log');
    const count = authenticatedPage.locator('#log-line-count');

    // Info: the previously-broken case — every info line now shows.
    await selectLevel(authenticatedPage, 'info');
    await expect(logBox).toContainText(INFO_LINE_1);
    await expect(logBox).toContainText(INFO_LINE_2);
    await expect(logBox).not.toContainText(ERROR_LINE);
    await expect(count).toHaveText('2/4 lines');

    // Warnings: only the warn line.
    await selectLevel(authenticatedPage, 'warn');
    await expect(logBox).toContainText('retrying request');
    await expect(logBox).not.toContainText(ERROR_LINE);
    await expect(logBox).not.toContainText(INFO_LINE_1);
    await expect(count).toHaveText('1/4 lines');

    // All levels: back to the full set.
    await selectLevel(authenticatedPage, '');
    await expect(count).toHaveText('4/4 lines');
  });

  test('text search composes with the level filter', async ({
    authenticatedPage,
  }) => {
    await mockLogs(authenticatedPage, true);
    await openLogsTab(authenticatedPage);

    const logBox = authenticatedPage.locator('#daemon-log');
    const count = authenticatedPage.locator('#log-line-count');

    await selectLevel(authenticatedPage, 'info');
    await authenticatedPage.locator('#log-filter').fill('disk health');
    await expect(logBox).toContainText(INFO_LINE_2);
    await expect(logBox).not.toContainText(INFO_LINE_1);
    await expect(count).toHaveText('1/4 lines');
  });

  test('falls back to client-side classification when the backend omits levels', async ({
    authenticatedPage,
  }) => {
    // Old backends (and the tabs mock) return no daemon_log_levels; the
    // frontend must still classify lines so the filter is never inert.
    await mockLogs(authenticatedPage, /* withLevels */ false);
    await openLogsTab(authenticatedPage);

    const logBox = authenticatedPage.locator('#daemon-log');
    const count = authenticatedPage.locator('#log-line-count');

    await selectLevel(authenticatedPage, 'error');
    await expect(logBox).toContainText(ERROR_LINE);
    await expect(logBox).not.toContainText(INFO_LINE_1);
    await expect(count).toHaveText('1/4 lines');
  });
});
