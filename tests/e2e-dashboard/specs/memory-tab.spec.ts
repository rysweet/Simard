import { test, expect } from '../fixtures/simard-dashboard';
import type { Page } from '@playwright/test';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

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

/*
 * Fail-LOUD contract (issue #2627): the memory graph must never silently blank.
 * A data-load failure surfaces a VISIBLE on-canvas #mem-graph-error overlay (not
 * a tiny stats-line note, not a blank canvas). See
 * docs/reference/dashboard-memory-graph-fail-loud.md.
 *
 * TDD note: RED against the current renderer — there is no #mem-graph-error
 * overlay; fetchMemoryGraph writes `Error: …` to the low-visibility
 * #mem-graph-stats line and returns, leaving the canvas untouched. Passes once
 * the overlay + mgError render path land.
 */
test.describe('Memory tab fail-loud error state @structural', () => {
  const ERROR_GRAPH = {
    error: 'Cognitive memory reader is unavailable.',
    available: false,
    nodes: [],
    edges: [],
    stats: { working: 0, semantic: 0, episodic: 0, procedural: 0, prospective: 0, sensory: 0 },
  };

  function installErrorRoutes(page: Page): void {
    page.route('**/api/**', async (route) => {
      const pathname = new URL(route.request().url()).pathname;
      const body =
        pathname === '/api/memory/graph'
          ? ERROR_GRAPH
          : pathname in BODIES
            ? BODIES[pathname]
            : {};
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(body),
      });
    });
  }

  test('a data-load error paints the visible #mem-graph-error overlay, not a silent blank', async ({
    authenticatedPage,
  }) => {
    installErrorRoutes(authenticatedPage);
    await authenticatedPage.goto('/');
    await authenticatedPage.locator('.tab[data-tab="memory"]').click();

    const overlay = authenticatedPage.locator('#tab-memory #mem-graph-error');
    await expect(
      overlay,
      'a graph-load error must surface a VISIBLE #mem-graph-error overlay (fail-loud, never a silent blank)',
    ).toBeVisible();
    await expect(
      overlay,
      'the overlay must show the sanitized error message from the payload',
    ).toContainText('Cognitive memory reader is unavailable.');
  });
});

/*
 * Live acceptance gate (issue #2627): against the REAL dashboard (webServer runs
 * `dashboard serve`), an authenticated GET /api/memory/graph must return a
 * non-empty graph when the live cognitive store has content — the end-to-end
 * proof the Memory tab shows Simard's live memory, not a stale/empty stub.
 *
 * Gated + skipped cleanly when there is no dashkey or the live store is empty, so
 * it never flakes in CI without a live daemon (per the design's live-path risk).
 */
function readDashkeyOrEmpty(): string {
  if (process.env.SIMARD_DASHKEY) return process.env.SIMARD_DASHKEY;
  try {
    return fs.readFileSync(path.join(os.homedir(), '.simard', '.dashkey'), 'utf-8').trim();
  } catch {
    return '';
  }
}

test.describe('Memory tab live acceptance @smoke', () => {
  test('authenticated GET /api/memory/graph returns a non-empty graph when the live store has content', async ({
    page,
    baseURL,
  }) => {
    const code = readDashkeyOrEmpty();
    test.skip(!code, 'no ~/.simard/.dashkey or SIMARD_DASHKEY — live cognitive store unavailable');

    const login = await page.request.post(`${baseURL}/api/login`, { data: { code } });
    test.skip(login.status() !== 200, 'dashkey did not authenticate against the live dashboard');

    const resp = await page.request.get(`${baseURL}/api/memory/graph`);
    expect(resp.status(), 'the live graph endpoint must respond 200 to an authenticated GET').toBe(200);
    const g = await resp.json();

    // Fail-loud: a data-load error must be a non-empty string, never a silent blank.
    if (g.error !== undefined && g.error !== null) {
      expect(typeof g.error, 'a data-load `error` must be a string when present').toBe('string');
      expect(String(g.error).length, 'a data-load `error` must be non-empty (fail-loud)').toBeGreaterThan(0);
      test.skip(true, `live reader reported an error state (fail-loud, not asserting content): ${g.error}`);
    }

    const s = g.stats ?? {};
    const enumerable = (s.semantic ?? 0) + (s.episodic ?? 0) + (s.procedural ?? 0) + (s.prospective ?? 0);
    test.skip(enumerable === 0, 'live store holds no enumerable content (empty store) — nothing to render');

    // ACCEPTANCE GATE: a populated live store must surface item nodes + edges,
    // never a silent-empty / hub-only graph (the #2627 regression).
    const nodes: Array<{ hub?: boolean }> = Array.isArray(g.nodes) ? g.nodes : [];
    const edges: unknown[] = Array.isArray(g.edges) ? g.edges : [];
    const itemNodes = nodes.filter((n) => !n.hub);
    expect(
      itemNodes.length,
      'a populated live store (~7700 facts) must emit item nodes, not just the six type hubs',
    ).toBeGreaterThan(0);
    expect(
      edges.length,
      'live item nodes must be linked to their type hubs (non-empty edges)',
    ).toBeGreaterThan(0);
  });
});
