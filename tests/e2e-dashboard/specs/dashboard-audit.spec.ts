import { test, expect } from '../fixtures/simard-dashboard';
import type { Page } from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';

// Self-understanding audit harness (issues #1170 / #1630 / epic #1992).
//
// Logs in, walks every dashboard tab, and captures a full-page screenshot plus
// the visible innerText of each panel into a gitignored output directory. This
// is an evidence/diagnostic tool. It is tagged `@structural` so it can be run
// on demand via the structural project, but the CI e2e job invokes only an
// explicit allow-list of spec files, so this audit never runs unattended.
//
// Run against a locally built dashboard (pick a free port to avoid reusing a
// stale server):
//   SIMARD_BIN=target/release/simard SIMARD_DASHBOARD_PORT=18797 CI=true \
//     npx playwright test --config=tests/e2e-dashboard/playwright.config.ts \
//     --project=structural tests/e2e-dashboard/specs/dashboard-audit.spec.ts
//
// Output: tests/e2e-dashboard/.audit-output/{<slug>.png,<slug>.txt,REPORT.md}

const OUT_DIR = path.join(__dirname, '..', '.audit-output');

function ensureOutDir(): void {
  fs.mkdirSync(OUT_DIR, { recursive: true });
}

async function discoverTabs(page: Page): Promise<string[]> {
  return page.$$eval('.tab[data-tab]', (els) =>
    els
      .map((e) => e.getAttribute('data-tab') ?? '')
      .filter((s) => s.length > 0),
  );
}

test.describe('Dashboard self-understanding audit @structural', () => {
  test('capture every tab: screenshot + visible text', async ({
    authenticatedPage,
  }) => {
    test.setTimeout(180_000);
    ensureOutDir();

    const page = authenticatedPage;
    await page.goto('/');
    await page.waitForLoadState('networkidle').catch(() => {});

    const tabs = await discoverTabs(page);
    expect(tabs.length).toBeGreaterThanOrEqual(9);

    const report: string[] = [
      '# Dashboard audit',
      '',
      `Captured: ${new Date().toISOString()}`,
      `Tabs discovered: ${tabs.length}`,
      '',
    ];

    for (const slug of tabs) {
      const tab = page.locator(`.tab[data-tab="${slug}"]`);
      await tab.click();
      await expect(page.locator(`#tab-${slug}`)).toBeVisible();
      // Give per-tab fetches and jargon annotation a moment to settle.
      await page.waitForTimeout(1_200);

      const png = path.join(OUT_DIR, `${slug}.png`);
      await page.screenshot({ path: png, fullPage: true });

      const text =
        (await page.locator(`#tab-${slug}`).innerText().catch(() => '')) ?? '';
      fs.writeFileSync(path.join(OUT_DIR, `${slug}.txt`), text, 'utf-8');

      report.push(`## ${slug}`);
      report.push(`- screenshot: ${slug}.png`);
      report.push(`- visible text: ${text.length} chars`);
      report.push('');
    }

    fs.writeFileSync(path.join(OUT_DIR, 'REPORT.md'), report.join('\n'), 'utf-8');

    // Sanity: the Logs tab text dump must include the renamed, plain-English
    // panel heading (jargon pass) rather than the old "Daemon Log" label.
    const logsText = fs.readFileSync(path.join(OUT_DIR, 'logs.txt'), 'utf-8');
    expect(logsText).toContain('Background Service Log');
    expect(logsText).not.toContain('Daemon Log');
  });
});
