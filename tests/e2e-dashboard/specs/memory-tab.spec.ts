import { test, expect } from '../fixtures/simard-dashboard';
import type { Page } from '@playwright/test';

/*
 * Dedicated "Memory" tab contract (issue #2627).
 *
 * REGRESSION: the ~17->9 tab consolidation dropped the memory-graph
 * visualization (folded into a collapsed <details> inside Resources and
 * demoted to a deep-link alias). This restores it as its OWN top-level tab
 * wired to the LIVE /api/memory/graph read path.
 *
 * TDD note: these are expected to FAIL against the current server — there is
 * no `.tab[data-tab="memory"]` nav button, no `#tab-memory` panel, and the
 * `#mem-graph-canvas` is buried in `#tab-resources`. They pass once the Memory
 * tab is registered and the viz is promoted into its panel.
 */

// A live-shaped graph payload: the six memory types (per-item nodes clustered
// under type hubs) plus a stats block, mirroring the rebuilt handler.
const LIVE_GRAPH = {
  available: true,
  stats: { working: 2, semantic: 3, episodic: 4, procedural: 1, prospective: 1, sensory: 5 },
  nodes: [
    { id: 'hub:SemanticFact', type: 'SemanticFact', label: 'Facts learned', content: 'Facts learned' },
    { id: 'fact:1', type: 'SemanticFact', label: 'rust ownership', content: 'ZZLIVEFACT the borrow checker' },
    { id: 'hub:EpisodicMemory', type: 'EpisodicMemory', label: 'Events remembered', content: 'Events remembered' },
    { id: 'ep:1', type: 'EpisodicMemory', label: 'merged PR', content: 'ZZLIVEEPISODE merged a PR' },
    { id: 'hub:ProceduralMemory', type: 'ProceduralMemory', label: 'Known procedures', content: 'Known procedures' },
    { id: 'hub:ProspectiveMemory', type: 'ProspectiveMemory', label: 'Planned actions', content: 'Planned actions' },
    { id: 'hub:WorkingMemory', type: 'WorkingMemory', label: 'Currently thinking about', content: '2 slots' },
    { id: 'hub:SensoryBuffer', type: 'SensoryBuffer', label: 'Recent observations', content: '5 buffered' },
  ],
  edges: [
    { source: 'hub:SemanticFact', target: 'fact:1' },
    { source: 'hub:EpisodicMemory', target: 'ep:1' },
  ],
};

const BODIES: Record<string, unknown> = {
  '/api/status': { version: '0.8.0', git_hash: 'test', ooda_status: 'idle', uptime_secs: 0 },
  '/api/status/snapshot': {
    data: { schema_version: 1, generated_at: '2026-07-06T00:00:00Z' },
    rendered: 'SIMARD STATUS',
    generated_at: '2026-07-06T00:00:00Z',
  },
  '/api/memory/graph': LIVE_GRAPH,
  '/api/memory': { overview: {}, files: [] },
  '/api/memory/recent': { items: [], total: 0, last_hour_count: 0, server_time: new Date().toISOString() },
  '/api/memory/history': { points: [] },
  '/api/costs': { daily: [], weekly: [] },
};

function installRoutes(page: Page): Record<string, number> {
  const counts: Record<string, number> = {};
  page.route('**/api/**', async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    counts[pathname] = (counts[pathname] ?? 0) + 1;
    const body = pathname in BODIES ? BODIES[pathname] : {};
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
  });
  return counts;
}

test.describe('Dedicated Memory tab @structural', () => {
  test('a top-level Memory nav button activates a dedicated Memory panel', async ({
    authenticatedPage,
  }) => {
    installRoutes(authenticatedPage);
    await authenticatedPage.goto('/');

    const navButton = authenticatedPage.locator('.tab[data-tab="memory"]');
    await expect(navButton, 'a top-level "Memory" nav button must exist').toBeVisible();
    await expect(navButton).toHaveText(/Memory/);

    await navButton.click();
    await expect(
      authenticatedPage.locator('#tab-memory'),
      'clicking Memory must reveal the #tab-memory panel',
    ).toBeVisible();
    await expect(authenticatedPage.locator('#tab-memory h1.page-h1')).toHaveText('Memory');
  });

  test('the Memory tab renders the graph canvas and all six type filters', async ({
    authenticatedPage,
  }) => {
    installRoutes(authenticatedPage);
    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="memory"]').click();

    const panel = authenticatedPage.locator('#tab-memory');
    await expect(panel.locator('#mem-graph-canvas')).toBeVisible();
    for (const ty of [
      'WorkingMemory',
      'SemanticFact',
      'EpisodicMemory',
      'ProceduralMemory',
      'ProspectiveMemory',
      'SensoryBuffer',
    ]) {
      await expect(
        panel.locator(`.mem-filter[data-type="${ty}"]`),
        `the ${ty} filter must live inside the Memory panel`,
      ).toHaveCount(1);
    }
  });

  test('the Memory tab fetches LIVE data from /api/memory/graph and shows the counts', async ({
    authenticatedPage,
  }) => {
    const counts = installRoutes(authenticatedPage);
    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="memory"]').click();

    // The live graph endpoint must be hit for the Memory tab.
    await expect
      .poll(() => counts['/api/memory/graph'] ?? 0, {
        message: 'activating the Memory tab must fetch the live /api/memory/graph',
        timeout: 15_000,
      })
      .toBeGreaterThanOrEqual(1);

    // The stats line reflects the live counts from the payload (no placeholder).
    await expect(authenticatedPage.locator('#mem-graph-stats')).toContainText('Facts:3');
    await expect(authenticatedPage.locator('#mem-graph-stats')).toContainText('Events:4');
  });

  test('the Memory graph canvas is not duplicated in Resources', async ({ authenticatedPage }) => {
    installRoutes(authenticatedPage);
    await authenticatedPage.goto('/');
    // Moved, not copied: exactly one canvas in the whole document.
    await expect(authenticatedPage.locator('#mem-graph-canvas')).toHaveCount(1);
  });
});
