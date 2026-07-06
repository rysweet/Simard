import { test, expect } from '../fixtures/simard-dashboard';
import { OverviewPage } from '../pages/overview.page';

test.describe('Dashboard Overview @structural', () => {
  let overview: OverviewPage;

  test.beforeEach(async ({ authenticatedPage }) => {
    // Mock API endpoints so page renders without a live backend
    await authenticatedPage.route('**/api/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          version: '0.7.1.test',
          git_hash: 'abc1234',
          ooda_status: 'idle',
          uptime_secs: 3600,
        }),
      }),
    );
    await authenticatedPage.route('**/api/issues', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([
          { number: 1, title: 'Test issue', state: 'open', html_url: '#' },
        ]),
      }),
    );

    await authenticatedPage.goto('/');
    overview = new OverviewPage(authenticatedPage);
  });

  test('overview page displays dashboard heading', async () => {
    await expect(overview.heading).toBeVisible();
  });

  test('overview tab is active by default', async ({ authenticatedPage }) => {
    const overviewTab = authenticatedPage.locator('.tab[data-tab="overview"]');
    await expect(overviewTab).toHaveClass(/active/);
    await expect(authenticatedPage.locator('#tab-overview')).toBeVisible();
  });

  test('System Status card renders', async () => {
    await expect(overview.statusCard).toBeVisible();
    await expect(overview.statusCard.locator('h2')).toContainText('System Status');
  });

  test('Open Issues card renders', async () => {
    await expect(overview.issuesCard).toBeVisible();
    await expect(overview.issuesCard.locator('h2')).toContainText('Open Issues');
  });

  test('status div populates from API', async ({ authenticatedPage }) => {
    // Wait for fetchStatus to complete and render
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('status');
        return el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    const text = await overview.statusDiv.textContent();
    expect(text).toContain('0.7.1');
  });

  test('issues list populates from API', async ({ authenticatedPage }) => {
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('issues-list');
        return el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    const text = await overview.issuesList.textContent();
    expect(text).toContain('Test issue');
  });

  test('all dashboard tabs are present', async () => {
    const names = await overview.getTabNames();
    // #2627: the dashboard was consolidated onto a fixed nine-tab taxonomy.
    // This guards that all nine canonical tabs remain present; former
    // standalone tabs now live as sub-sections inside their parent tab.
    expect(names.length).toBeGreaterThanOrEqual(9);
    for (const required of [
      'Overview',
      'Goals',
      'Activity',
      'Workers',
      'Pull Requests',
      'Resources',
      'Chat',
      'Overseer',
      'Journal',
    ]) {
      expect(names).toContain(required);
    }
  });
});

// --- Issue #948: live activity surfaces (agent-live-status, recent-actions) ---
// Note: the Overview "Open PRs" card was removed as a duplicate of Merge
// Readiness (#26); the open-prs surface assertion below now checks its ABSENCE.

const MOCK_ACTIVITY = {
  daemon: {
    status: 'healthy',
    last_heartbeat: new Date().toISOString(),
    current_cycle: 42,
    actions_taken: 7,
  },
  recent_cycles: [
    {
      cycle_number: 42,
      report: {
        cycle_number: 42,
        outcomes: [
          {
            success: true,
            action_kind: 'edit',
            action_description: 'Fixed bug in parser',
          },
          {
            success: true,
            action_kind: 'spawn_engineer',
            action_description: 'Dispatched engineer for task',
          },
        ],
        priorities: [
          { goal_id: 'g1', reason: 'top-priority', urgency: 0.8 },
        ],
      },
    },
  ],
  assigned_issues: [],
  timestamp: new Date().toISOString(),
};

test.describe('Dashboard Overview - live activity surfaces @structural', () => {
  let overview: OverviewPage;

  test.beforeEach(async ({ authenticatedPage }) => {
    // Mock the activity endpoint BEFORE navigation to prevent the real fetch
    // from racing with the mock registration.
    await authenticatedPage.route('**/api/activity', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(MOCK_ACTIVITY),
      }),
    );
    // Also stub status/issues so the overview page renders cleanly without a
    // live backend (mirrors the parent describe's pattern).
    await authenticatedPage.route('**/api/status', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          version: '0.7.1.test',
          git_hash: 'abc1234',
          ooda_status: 'idle',
          uptime_secs: 3600,
        }),
      }),
    );
    await authenticatedPage.route('**/api/issues', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify([]),
      }),
    );

    await authenticatedPage.goto('/');
    overview = new OverviewPage(authenticatedPage);
  });

  test('agent-live-status card renders with daemon health and current cycle', async ({
    authenticatedPage,
  }) => {
    await expect(overview.agentLiveStatusCard).toBeVisible();
    // Wait until the loading placeholder is replaced with rendered content.
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('agent-live-status');
        return !!el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    await expect(overview.agentLiveStatus).toBeVisible();
    const text = await overview.agentLiveStatus.textContent();
    expect(text).toContain('Decision Loop Active');
    expect(text).toContain('#42');
  });

  test('recent-actions-list renders cycle outcomes', async ({ authenticatedPage }) => {
    await expect(overview.recentActionsCard).toBeVisible();
    await authenticatedPage.waitForFunction(
      () => {
        const el = document.getElementById('recent-actions-list');
        return !!el && !el.querySelector('.loading');
      },
      { timeout: 10_000 },
    );
    await expect(overview.recentActionsList).toBeVisible();
    const text = await overview.recentActionsList.textContent();
    expect(text).toContain('Fixed bug in parser');
    expect(text).toContain('Edit');
    expect(text).toContain('Launched sub-agent');
    expect(text).toContain('#42');
  });

  test('duplicative Open PRs card is removed; Merge Readiness is the single PR surface', async () => {
    // #26: the Overview "Open PRs" card duplicated the richer Merge Readiness
    // card (its data was a strict subset), so it was removed — markup, render
    // target, and its /api/activity -> open_prs producer. Neither the card
    // container nor its render target may exist anywhere on the Overview tab.
    await expect(overview.openPrsCard).toHaveCount(0);
    await expect(overview.openPrsList).toHaveCount(0);
    // Merge Readiness — the retained single Overview PR surface — must remain.
    await expect(overview.mergeReadinessCard).toBeVisible();
  });
});

// --- Issue #2358 P2 (item 3): Overview action-detail humanization (behavioral) ---
//
// The Rust (`tests_tab_meta.rs`) and gadugi (`dashboard-jargon-clarity.sh`)
// tests for `humanizeActionDetail` are *structural*: they assert the helper is
// defined and wired into the two render sites, but never run it. The tests
// below close that gap by executing the REAL in-browser functions
// (`humanizeActionDetail`, `renderActionDetail`, `esc`) against concrete
// inputs and asserting their *outputs* — including the escape-last (XSS) and
// Attach-button contracts called out in the PR review.

// Ambient declarations for the dashboard's global page functions/state. These
// are classic-script globals on the served page; the page.evaluate callbacks
// below resolve them in the browser context (never in Node).
declare function humanizeActionDetail(detail: unknown): string;
declare function renderActionDetail(detail: string): string;
declare function esc(s: unknown): string;
declare function rebuildSubagentIndex(): void;
// eslint-disable-next-line @typescript-eslint/no-explicit-any
declare const subagentSessionsCache: { live: any[]; recently_ended: any[]; byId: Record<string, any> };

test.describe('Dashboard Overview - action-detail humanization @structural', () => {
  test.beforeEach(async ({ authenticatedPage }) => {
    await authenticatedPage.goto('/');
    // The inline dashboard script runs during parse; wait until the helpers
    // are actually defined before exercising them.
    await authenticatedPage.waitForFunction(
      () =>
        typeof (window as unknown as { humanizeActionDetail?: unknown }).humanizeActionDetail ===
          'function' &&
        typeof (window as unknown as { renderActionDetail?: unknown }).renderActionDetail ===
          'function',
      { timeout: 10_000 },
    );
  });

  test('maps known machine decision tokens to plain English', async ({ authenticatedPage }) => {
    const results = await authenticatedPage.evaluate(() => ({
      continueSkipping: humanizeActionDetail('brain: continue_skipping'),
      spawnEngineer: humanizeActionDetail('advance-goal: spawn_engineer dispatched'),
      prefixRouted: humanizeActionDetail('ooda-brain: prefix-routed'),
      noLlm: humanizeActionDetail('no-action: no LLM configured'),
    }));
    expect(results.continueSkipping).toBe('continued without acting');
    expect(results.spawnEngineer).toBe('launched a sub-agent');
    expect(results.prefixRouted).toBe('chosen by built-in routing rules');
    expect(results.noLlm).toBe('no language model configured');
  });

  test('strips machine routing prefixes including the generic <x>-brain: form', async ({
    authenticatedPage,
  }) => {
    const results = await authenticatedPage.evaluate(() => ({
      brain: humanizeActionDetail('brain: reviewed the open PRs'),
      advance: humanizeActionDetail('advance-goal: opened a draft PR'),
      noAction: humanizeActionDetail('no-action: nothing to do this cycle'),
      genericBrain: humanizeActionDetail('merge-brain: evaluating PR #12'),
    }));
    expect(results.brain).toBe('reviewed the open PRs');
    expect(results.advance).toBe('opened a draft PR');
    expect(results.noAction).toBe('nothing to do this cycle');
    // The generic <x>-brain: prefix is stripped, leaving the human-readable tail.
    expect(results.genericBrain).toBe('evaluating PR #12');
    expect(results.genericBrain).not.toContain('brain');
  });

  test('removes the "no decision keyword found … defaulting to" boilerplate', async ({
    authenticatedPage,
  }) => {
    const results = await authenticatedPage.evaluate(() => ({
      pureBoilerplate: humanizeActionDetail(
        'no-action: no decision keyword found in model response; defaulting to continue_skipping',
      ),
      mixed: humanizeActionDetail(
        'Reviewing open PRs no decision keyword found defaulting to skip',
      ),
    }));
    // Pure boilerplate collapses to nothing (render sites fall back to the
    // action description), and never leaks the machine phrasing.
    expect(results.pureBoilerplate).toBe('');
    expect(results.pureBoilerplate).not.toContain('no decision keyword');
    // Real content survives; the trailing boilerplate is removed.
    expect(results.mixed).toBe('Reviewing open PRs');
    expect(results.mixed).not.toContain('defaulting to');
  });

  test('strips parenthetical brain-error noise that carries no agent reference', async ({
    authenticatedPage,
  }) => {
    const results = await authenticatedPage.evaluate(() => ({
      brainError: humanizeActionDetail('Did work (brain-error fallback: writer crashed)'),
      parseFailed: humanizeActionDetail('(goal-action parse failed) advanced the goal'),
    }));
    expect(results.brainError).toBe('Did work');
    expect(results.parseFailed).toBe('advanced the goal');
  });

  test("preserves an agent='engineer-…' reference even inside a noisy parenthetical", async ({
    authenticatedPage,
  }) => {
    // SR-D5: the Attach button keys off agent='engineer-…'. A parenthetical
    // that would otherwise be stripped as noise must be KEPT when it carries
    // that reference, so the substring reaches renderActionDetail intact.
    const result = await authenticatedPage.evaluate(() =>
      humanizeActionDetail(
        "done (brain-error fallback while dispatching agent='engineer-bar-9')",
      ),
    );
    expect(result).toContain("agent='engineer-bar-9'");
  });

  test('is null/undefined/empty safe', async ({ authenticatedPage }) => {
    const results = await authenticatedPage.evaluate(() => ({
      nul: humanizeActionDetail(null),
      undef: humanizeActionDetail(undefined),
      empty: humanizeActionDetail(''),
    }));
    expect(results.nul).toBe('');
    expect(results.undef).toBe('');
    expect(results.empty).toBe('');
  });

  test('escape-last holds at Site 1: esc() wraps the humanized output (XSS)', async ({
    authenticatedPage,
  }) => {
    // Mirrors the Site-1 render expression esc(humanizeActionDetail(d).substring(0,120)).
    const out = await authenticatedPage.evaluate(() =>
      esc(humanizeActionDetail('<img src=x onerror=alert(1)>').substring(0, 120)),
    );
    expect(out).toContain('&lt;img');
    expect(out).not.toContain('<img');
    expect(out).not.toContain('onerror=alert(1)>');
  });

  test('escape-last holds at Site 2: renderActionDetail escapes the humanized output (XSS)', async ({
    authenticatedPage,
  }) => {
    const out = await authenticatedPage.evaluate(() =>
      renderActionDetail(humanizeActionDetail('<script>alert(1)</script>')),
    );
    expect(out).toContain('&lt;script&gt;');
    expect(out).not.toContain('<script>');
  });

  test('renderActionDetail still renders the Attach button after humanization', async ({
    authenticatedPage,
  }) => {
    const out = await authenticatedPage.evaluate(() => {
      // Seed a cached subagent session the Attach button can match against.
      subagentSessionsCache.live = [
        {
          agent_id: 'engineer-foo-123',
          session_name: 'sess-foo',
          host: 'local',
          pid: 4242,
          goal_id: 'g1',
        },
      ];
      subagentSessionsCache.recently_ended = [];
      rebuildSubagentIndex();
      const humanized = humanizeActionDetail(
        "advance-goal: dispatched agent='engineer-foo-123'",
      );
      return { humanized, html: renderActionDetail(humanized) };
    });
    // The routing prefix is gone but the agent reference survives humanization…
    expect(out.humanized).toContain("agent='engineer-foo-123'");
    expect(out.humanized).not.toContain('advance-goal:');
    // …so the downstream Attach button still fires with the cached command.
    expect(out.html).toContain('Attach →');
    expect(out.html).toContain('tmux attach -t sess-foo');
  });

  test('no Attach button when no session is cached for the agent reference', async ({
    authenticatedPage,
  }) => {
    // Fresh page (beforeEach re-navigated): the cache is empty, so even a
    // valid agent reference must not produce an Attach button.
    const html = await authenticatedPage.evaluate(() =>
      renderActionDetail(humanizeActionDetail("dispatched agent='engineer-nope-1'")),
    );
    expect(html).toContain("agent='engineer-nope-1'");
    expect(html).not.toContain('Attach →');
  });

  test('processes adversarial input in linear time (ReDoS-immune)', async ({
    authenticatedPage,
  }) => {
    const { dt, ok } = await authenticatedPage.evaluate(() => {
      // The bounded regexes (the documented ReDoS contract) must stay linear:
      // a very long "no decision keyword found …" run that never reaches
      // "defaulting to" would expose catastrophic backtracking if the inner
      // quantifier were unbounded. It is capped at {0,80}, so this stays fast.
      const adversarial =
        'no decision keyword found ' + 'a'.repeat(50_000) + ' brain: continue_skipping';
      const t0 = performance.now();
      const out = humanizeActionDetail(adversarial);
      const dt = performance.now() - t0;
      return { dt, ok: out.indexOf('continued without acting') >= 0 };
    });
    // Linear processing is single-digit milliseconds; the generous bound only
    // catches genuine catastrophic backtracking (seconds-to-minutes).
    expect(dt).toBeLessThan(1500);
    expect(ok).toBe(true);
  });
});
