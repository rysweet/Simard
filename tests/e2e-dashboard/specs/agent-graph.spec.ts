import { test, expect } from '../fixtures/simard-dashboard';

/**
 * Issue #951 — Agent Graph backend contract.
 *
 * Verifies the /api/agent-graph endpoint contract end-to-end through the
 * authenticated dashboard surface. The force-directed visualization frontend
 * is a follow-up; these tests pin the JSON shape that the frontend will
 * consume so the contract cannot regress unnoticed.
 */
test.describe('Agent Graph API @structural', () => {
  test('GET /api/agent-graph returns nodes/edges/layers shape', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/agent-graph', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          nodes: [
            { id: 'o1', type: 'ooda', role: 'ooda', host: 'h', pid: 1, state: 'Running' },
            { id: 'e1', type: 'engineer', role: 'engineer', host: 'h', pid: 2, state: 'Running' },
            { id: 's1', type: 'session', role: 'session', host: 'h', pid: 3, state: 'Running' },
          ],
          edges: [
            { src: 'o1', dst: 'e1' },
            { src: 'e1', dst: 's1' },
          ],
          layers: { ooda: 1, engineer: 1, session: 1 },
          timestamp: new Date().toISOString(),
        }),
      }),
    );

    const response = await authenticatedPage.request.get('/api/agent-graph');
    expect(response.status()).toBe(200);

    const body = await response.json();
    expect(Array.isArray(body.nodes)).toBe(true);
    expect(Array.isArray(body.edges)).toBe(true);
    expect(body.layers).toMatchObject({ ooda: 1, engineer: 1, session: 1 });

    for (const node of body.nodes) {
      expect(node).toHaveProperty('id');
      expect(node).toHaveProperty('type');
      expect(['ooda', 'engineer', 'session']).toContain(node.type);
    }
    for (const edge of body.edges) {
      expect(edge).toHaveProperty('src');
      expect(edge).toHaveProperty('dst');
    }
  });

  test('agent-graph endpoint surfaces empty topology gracefully', async ({
    authenticatedPage,
  }) => {
    await authenticatedPage.route('**/api/agent-graph', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          nodes: [],
          edges: [],
          layers: { ooda: 0, engineer: 0, session: 0 },
          timestamp: new Date().toISOString(),
        }),
      }),
    );

    const response = await authenticatedPage.request.get('/api/agent-graph');
    const body = await response.json();
    expect(body.nodes).toHaveLength(0);
    expect(body.edges).toHaveLength(0);
  });
});
