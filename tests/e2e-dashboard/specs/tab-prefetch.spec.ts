import { test, expect } from '../fixtures/simard-dashboard';
import type { Page } from '@playwright/test';

/*
 * Background tab prefetch / refresh contract (issue #2649).
 *
 * Requirement: every dashboard tab must load AND keep refreshing in the
 * BACKGROUND on page load — not only when its tab becomes active — so
 * switching tabs renders already-fetched, continuously-refreshed data.
 *
 * TDD note: these tests are expected to FAIL against the current server.
 * Today `runTabFetches` runs only the active tab's loaders and wipes all
 * timers on every tab switch, so non-active tabs are never prefetched,
 * there is no apiFetch in-flight de-dupe, no visibility back-off, and no
 * per-tab freshness indicator. They pass once the background scheduler
 * described in docs/reference/dashboard-background-tab-prefetch.md lands.
 */

// Canonical tab -> a "signature" endpoint hit ONLY by that tab's loader
// (never by the always-on dashboard init), so a non-zero request count
// proves that tab was background-loaded even though it was never clicked.
const SIGNATURE_ENDPOINT: Record<string, string> = {
  overview: '/api/status/snapshot',
  goals: '/api/workboard',
  activity: '/api/ooda-cycles',
  workers: '/api/subagent-sessions',
  'pull-requests': '/api/merge-judge',
  resources: '/api/memory/graph',
  chat: '/api/chat/sessions',
  overseer: '/api/overseer',
  journal: '/api/journal/dates',
  'creative-ideas': '/api/creative-ideas',
};

const CANONICAL_TABS = Object.keys(SIGNATURE_ENDPOINT);

// A fast-refreshing endpoint (workers subagent poll = 5s floor) used to
// observe the hidden-tab back-off and visible resume.
const FAST_ENDPOINT = '/api/subagent-sessions';

// Structured bodies for the handful of endpoints whose renderers read
// nested fields; everything else falls back to safeDefault().
const BODIES: Record<string, unknown> = {
  '/api/status': { version: '0.8.0', git_hash: 'test', ooda_status: 'idle', uptime_secs: 0 },
  '/api/status/snapshot': {
    data: { schema_version: 1, generated_at: '2026-07-06T00:00:00Z' },
    rendered: 'SIMARD STATUS',
    generated_at: '2026-07-06T00:00:00Z',
  },
  '/api/goals': { active: [], backlog: [] },
  '/api/workboard': {
    cycle: { number: 1, phase: 'idle', interval_secs: 300 },
    goals: [],
    spawned_engineers: [],
    recent_actions: [],
    task_memory: [],
    working_memory: [],
    cognitive_statistics: {},
    uptime_seconds: 0,
    timestamp: new Date().toISOString(),
    next_cycle_eta_seconds: 60,
  },
  '/api/distributed': {
    topology: [],
    vms: [],
    event_bus: { topics: {}, total_subscribers: 0, events_per_min: 0.0, last_event_timestamp: null },
  },
  '/api/traces': { otel_status: 'disabled', traces: [] },
  '/api/logs': { daemon: '', cost_ledger: '', transcripts: [] },
  '/api/memory': { overview: {}, files: [] },
  '/api/memory/recent': { items: [], total: 0, last_hour_count: 0, server_time: new Date().toISOString() },
  '/api/memory/graph': { nodes: [], edges: [] },
  '/api/memory/history': { points: [] },
  '/api/costs': { daily: [], weekly: [] },
  '/api/budget': { daily: 10, weekly: 50 },
  '/api/ooda-thinking': { reports: [] },
  '/api/ooda-cycles': { cycles: [] },
  '/api/brain-failures': { failures: [], summary: 'No failures', timestamp: new Date().toISOString() },
  '/api/merge-judge': {
    decisions: [],
    persistence_available: false,
    persistence_reason: 'mock',
    summary: 'No decisions',
    timestamp: new Date().toISOString(),
  },
  '/api/prs': { prs: [], summary: 'No open PRs', timestamp: new Date().toISOString() },
  '/api/merge-readiness': {
    prs: [],
    summary: { objective_ready: 0, objective_pending: 0, objective_blocked: 0, total_open: 0 },
    timestamp: new Date().toISOString(),
  },
  '/api/activity': { agents: [], summary: {} },
  '/api/cognition/recall-precision': { precision: 0, recall: 0, samples: 0 },
  '/api/overseer': { checks: [], summary: 'ok', timestamp: new Date().toISOString() },
  '/api/journal/dates': { dates: [] },
  '/api/creative-ideas': { ideas: [] },
  '/api/subagent-sessions': { sessions: [] },
  '/api/azlin/tmux-sessions': { sessions: [] },
  '/api/chat/sessions': { sessions: [] },
};

// Endpoints whose renderers expect a bare JSON array.
const ARRAY_ENDPOINTS = new Set(['/api/issues', '/api/hosts', '/api/processes', '/api/registry']);

function safeDefault(pathname: string): unknown {
  return ARRAY_ENDPOINTS.has(pathname) ? [] : {};
}

// Installs a single catch-all router over /api/** that COUNTS requests per
// pathname and returns a safe body. Returns the live counts map. Any
// endpoints in `hold` are delayed by `holdMs` (kept in-flight) so overlap
// / de-dupe can be observed deterministically.
function installCountingRoutes(
  page: Page,
  opts: { hold?: Set<string>; holdMs?: number } = {},
): Record<string, number> {
  const counts: Record<string, number> = {};
  const hold = opts.hold ?? new Set<string>();
  const holdMs = opts.holdMs ?? 0;

  page.route('**/api/**', async (route) => {
    const pathname = new URL(route.request().url()).pathname;
    counts[pathname] = (counts[pathname] ?? 0) + 1;
    if (hold.has(pathname) && holdMs > 0) {
      await new Promise((r) => setTimeout(r, holdMs));
    }
    const body = pathname in BODIES ? BODIES[pathname] : safeDefault(pathname);
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
  });

  return counts;
}

test.describe('Background tab prefetch @structural', () => {
  test('background-loads EVERY tab on initial load, not just the active tab', async ({
    authenticatedPage,
  }) => {
    const counts = installCountingRoutes(authenticatedPage);
    await authenticatedPage.goto('/');

    // Do NOT click any tab. Overview is the only active tab; every OTHER
    // tab's signature endpoint must still be hit by the background scheduler.
    for (const slug of CANONICAL_TABS) {
      if (slug === 'overview') continue;
      const ep = SIGNATURE_ENDPOINT[slug];
      await expect
        .poll(() => counts[ep] ?? 0, {
          message: `expected background prefetch to fetch ${ep} for tab "${slug}" without it being clicked`,
          timeout: 15_000,
        })
        .toBeGreaterThanOrEqual(1);
    }
  });

  test('every canonical tab has a registered background loader', async ({ authenticatedPage }) => {
    installCountingRoutes(authenticatedPage);
    await authenticatedPage.goto('/');

    const registered = await authenticatedPage.evaluate(() => {
      const reg = (window as unknown as { TAB_LOADERS?: Record<string, unknown> }).TAB_LOADERS;
      return reg ? Object.keys(reg) : null;
    });

    expect(registered, 'window.TAB_LOADERS registry must be exposed').not.toBeNull();
    for (const slug of CANONICAL_TABS) {
      expect(registered, `TAB_LOADERS is missing a loader for canonical tab "${slug}"`).toContain(slug);
    }
  });

  test('switching to a background-loaded tab renders immediately from cache', async ({
    authenticatedPage,
  }) => {
    const counts = installCountingRoutes(authenticatedPage);
    await authenticatedPage.goto('/');

    // Wait for goals to be prefetched in the background (before any click).
    await expect
      .poll(() => counts['/api/workboard'] ?? 0, { timeout: 15_000 })
      .toBeGreaterThanOrEqual(1);
    const prefetchedBeforeClick = counts['/api/workboard'];

    // Activating the tab renders the already-fetched data immediately.
    await authenticatedPage.locator('.tab[data-tab="goals"]').click();
    await expect(authenticatedPage.locator('#tab-goals')).toBeVisible();

    expect(
      prefetchedBeforeClick,
      'goals data must have been prefetched in the background BEFORE the tab was activated',
    ).toBeGreaterThanOrEqual(1);
  });

  test('each tab shows a per-tab "last updated" freshness indicator', async ({
    authenticatedPage,
  }) => {
    const counts = installCountingRoutes(authenticatedPage);
    await authenticatedPage.goto('/');

    await expect
      .poll(() => counts['/api/workboard'] ?? 0, { timeout: 15_000 })
      .toBeGreaterThanOrEqual(1);

    await authenticatedPage.locator('.tab[data-tab="goals"]').click();
    const indicator = authenticatedPage.locator('[data-testid="goals-updated"]');
    await expect(indicator).toBeVisible();
    await expect(indicator).toContainText(/updated|ago|just now|refresh/i);
  });

  test('concurrent duplicate GET fetches are de-duped into ONE network request', async ({
    authenticatedPage,
  }) => {
    // Probe endpoint no tab loader touches, held in-flight so two concurrent
    // calls overlap; de-dupe at the apiFetch layer must collapse them to one.
    //
    // Playwright resolves overlapping routes in reverse registration order —
    // the LAST-registered matching handler runs first. installCountingRoutes
    // registers a catch-all `**/api/**`, so the specific `**/api/dedupe-probe`
    // handler must be registered AFTER it to take precedence; otherwise the
    // catch-all would shadow the probe and probeHits would never increment.
    let probeHits = 0;
    installCountingRoutes(authenticatedPage);
    await authenticatedPage.route('**/api/dedupe-probe', async (route) => {
      probeHits += 1;
      await new Promise((r) => setTimeout(r, 300));
      await route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' });
    });
    await authenticatedPage.goto('/');

    const bothResolved = await authenticatedPage.evaluate(async () => {
      const api = (window as unknown as { apiFetch?: (u: string) => Promise<unknown> }).apiFetch;
      if (!api) throw new Error('apiFetch not found on window');
      const [a, b] = [api('/api/dedupe-probe'), api('/api/dedupe-probe')];
      const [ra, rb] = await Promise.all([a, b]);
      return JSON.stringify(ra) === JSON.stringify(rb);
    });

    expect(bothResolved, 'both callers should resolve from the shared in-flight promise').toBe(true);
    expect(probeHits, 'two concurrent GETs to the same URL must collapse to a single network request').toBe(1);
  });

  test('hidden browser tab pauses background refresh and resumes when visible', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.clock.install();
    const counts = installCountingRoutes(authenticatedPage);
    await authenticatedPage.goto('/');

    // Drain the staggered initial prefetch queue and a few refresh ticks.
    await authenticatedPage.clock.runFor(6_000);
    await expect
      .poll(() => counts[FAST_ENDPOINT] ?? 0, { timeout: 15_000 })
      .toBeGreaterThanOrEqual(1);

    // Flush any in-flight responses, then snapshot the baseline. With the
    // clock installed, page timers only fire during runFor, so no interval
    // fires between here and the visibility change.
    await new Promise((r) => setTimeout(r, 400));

    // Go hidden.
    await authenticatedPage.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
      Object.defineProperty(document, 'hidden', { value: true, configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    const hiddenBaseline = counts[FAST_ENDPOINT] ?? 0;

    // Advance well past several refresh intervals while hidden.
    await authenticatedPage.clock.runFor(30_000);
    await new Promise((r) => setTimeout(r, 400));
    expect(
      counts[FAST_ENDPOINT] ?? 0,
      'a hidden browser tab must not keep polling in the background',
    ).toBe(hiddenBaseline);

    // Become visible again — intervals resume and the active tab refreshes.
    await authenticatedPage.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
      Object.defineProperty(document, 'hidden', { value: false, configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await authenticatedPage.clock.runFor(8_000);
    await expect
      .poll(() => counts[FAST_ENDPOINT] ?? 0, { timeout: 15_000 })
      .toBeGreaterThan(hiddenBaseline);
  });
});
