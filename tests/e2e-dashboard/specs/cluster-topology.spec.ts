import { test, expect } from '../fixtures/simard-dashboard';
import type { Page } from '@playwright/test';

// WS-4 — Cluster Topology dashboard: event bus stats panel.
//
// FAILING SPEC (TDD). Asserts the additive `event_bus` block surfaced via
// `/api/distributed` is rendered inside `#cluster-topology` (or its tab
// container) using the agreed `data-testid` selectors. Implementation lands
// in P2/P3.
//
// Contract under test (from design §6 DOM Contract):
//   data-testid="event-bus-total-subscribers"
//   data-testid="event-bus-events-per-min"
//   data-testid="event-bus-last-event"
//   data-testid="event-bus-topic-fact_promoted"
//   data-testid="event-bus-topic-fact_imported"
//   data-testid="event-bus-topic-node_joined"
//   data-testid="event-bus-topic-node_left"
//   data-testid="event-bus-topic-memory_sync_requested"

const KNOWN_TOPICS = [
  'fact_promoted',
  'fact_imported',
  'node_joined',
  'node_left',
  'memory_sync_requested',
] as const;

const POPULATED_DISTRIBUTED = {
  topology: [],
  vms: [],
  event_bus: {
    topics: {
      fact_promoted: {
        subscribers: 3,
        events_per_min: 1.2,
        last_event_timestamp: '2026-04-19T15:50:00Z',
      },
      fact_imported: {
        subscribers: 3,
        events_per_min: 0.0,
        last_event_timestamp: null,
      },
      node_joined: {
        subscribers: 3,
        events_per_min: 0.4,
        last_event_timestamp: '2026-04-19T15:48:00Z',
      },
      node_left: {
        subscribers: 3,
        events_per_min: 0.0,
        last_event_timestamp: null,
      },
      memory_sync_requested: {
        subscribers: 3,
        events_per_min: 0.0,
        last_event_timestamp: null,
      },
    },
    total_subscribers: 3,
    events_per_min: 1.6,
    last_event_timestamp: '2026-04-19T15:50:00Z',
  },
};

// Minimal mocks so the dashboard renders without a backend; only
// `/api/distributed` is interesting for this spec.
async function mockDashboard(page: Page, distributedBody: unknown) {
  const emptyJson = { status: 200, contentType: 'application/json', body: '[]' };
  const emptyObj = { status: 200, contentType: 'application/json', body: '{}' };

  await page.route('**/api/status', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        version: '0.0.0',
        git_hash: 'test',
        ooda_status: 'idle',
        uptime_secs: 0,
      }),
    }),
  );
  await page.route('**/api/issues', (route) => route.fulfill(emptyJson));
  await page.route('**/api/distributed', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(distributedBody),
    }),
  );
  await page.route('**/api/hosts', (route) => route.fulfill(emptyJson));
  await page.route('**/api/registry', (route) => route.fulfill(emptyJson));
  await page.route('**/api/build-lock', (route) => route.fulfill(emptyObj));
  await page.route('**/api/metrics', (route) => route.fulfill(emptyObj));
}

test.describe('Cluster Topology — Event Bus Stats @structural', () => {
  test('renders event bus aggregate stats with stable testids', async ({
    authenticatedPage,
  }) => {
    await mockDashboard(authenticatedPage, POPULATED_DISTRIBUTED);
    await authenticatedPage.goto('/');

    const total = authenticatedPage.locator(
      '[data-testid="event-bus-total-subscribers"]',
    );
    const rate = authenticatedPage.locator(
      '[data-testid="event-bus-events-per-min"]',
    );
    const last = authenticatedPage.locator('[data-testid="event-bus-last-event"]');

    await expect(total).toBeVisible();
    await expect(rate).toBeVisible();
    await expect(last).toBeVisible();

    await expect(total).toContainText('3');
    await expect(rate).toContainText('1.6');
    await expect(last).toContainText('2026-04-19T15:50:00Z');
  });

  test('renders one entry per known topic', async ({ authenticatedPage }) => {
    await mockDashboard(authenticatedPage, POPULATED_DISTRIBUTED);
    await authenticatedPage.goto('/');

    for (const topic of KNOWN_TOPICS) {
      const el = authenticatedPage.locator(
        `[data-testid="event-bus-topic-${topic}"]`,
      );
      await expect(el, `topic row '${topic}' must render`).toBeVisible();
    }

    // Spot-check the active topic carries its rate + timestamp.
    const promoted = authenticatedPage.locator(
      '[data-testid="event-bus-topic-fact_promoted"]',
    );
    await expect(promoted).toContainText('1.2');
    await expect(promoted).toContainText('2026-04-19T15:50:00Z');
  });

  test('renders silent topics with em-dash for null timestamps', async ({
    authenticatedPage,
  }) => {
    await mockDashboard(authenticatedPage, POPULATED_DISTRIBUTED);
    await authenticatedPage.goto('/');

    // U+2014 EM DASH per design §3 (docs/operator-dashboard/event-bus-stats.md).
    const emDash = '\u2014';
    const silent = authenticatedPage.locator(
      '[data-testid="event-bus-topic-fact_imported"]',
    );
    await expect(silent).toContainText(emDash);
  });

  test('degrades gracefully when event_bus key is absent', async ({
    authenticatedPage,
  }) => {
    // Older server: no `event_bus`. Optional chaining must skip the block.
    await mockDashboard(authenticatedPage, { topology: [], vms: [] });
    const errors: string[] = [];
    authenticatedPage.on('pageerror', (err) => errors.push(err.message));
    await authenticatedPage.goto('/');

    await expect(
      authenticatedPage.locator('[data-testid="event-bus-total-subscribers"]'),
    ).toHaveCount(0);
    expect(errors).toEqual([]);
  });
});
