import { test, expect } from '../fixtures/simard-dashboard';
import * as fs from 'node:fs';
import * as path from 'node:path';

// Human-as-operator audit (epic #1992): logs in and walks every dashboard tab,
// capturing a full-page screenshot of each into tests/e2e-dashboard/artifacts/.
// Artifacts are git-ignored; this spec generates the evidence referenced in the
// audit, it is not a pass/fail contract for individual tab contents.

const ARTIFACT_DIR = path.join(__dirname, '..', 'artifacts');

test.describe('Dashboard human audit @smoke', () => {
  test('walk every tab and screenshot it', async ({ authenticatedPage }) => {
    fs.mkdirSync(ARTIFACT_DIR, { recursive: true });

    await authenticatedPage.goto('/');
    await expect(authenticatedPage.locator('.tab[data-tab]').first()).toBeVisible();

    // Discover tabs from the DOM rather than hardcoding (mirrors the
    // tab-identity contract in docs/dashboard.md).
    const slugs = await authenticatedPage
      .locator('.tab[data-tab]')
      .evaluateAll((els) =>
        els.map((e) => (e as HTMLElement).dataset.tab).filter((s): s is string => !!s),
      );

    const captured: string[] = [];
    for (const slug of slugs) {
      try {
        await authenticatedPage.locator(`.tab[data-tab="${slug}"]`).click();
        await expect(authenticatedPage.locator(`#tab-${slug}`)).toBeVisible({
          timeout: 8_000,
        });
        // Give async panels a moment to populate.
        await authenticatedPage.waitForTimeout(750);
        const file = path.join(ARTIFACT_DIR, `${String(captured.length).padStart(2, '0')}-${slug}.png`);
        await authenticatedPage.screenshot({ path: file, fullPage: true });
        captured.push(slug);
      } catch (err) {
        // Best-effort: a flaky tab must not abort the audit.
        console.warn(`audit: skipped tab "${slug}": ${(err as Error).message}`);
      }
    }

    console.log(`audit: captured ${captured.length} tab screenshots -> ${ARTIFACT_DIR}`);
    expect(captured.length).toBeGreaterThan(0);
  });
});
