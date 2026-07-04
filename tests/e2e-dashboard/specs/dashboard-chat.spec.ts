import { test, expect } from '../fixtures/simard-dashboard';
import { type Page } from '@playwright/test';

/**
 * Failing TDD e2e spec for the Dashboard Chat feature (issue #2577, Step 7).
 *
 * Encodes the operator-facing acceptance criteria from
 * docs/reference/dashboard-chat.md and the issue:
 *
 *   FR-3/FR-4  Session sidebar lists saved sessions; clicking one loads its
 *              complete history into the panel.
 *   FR-5       The chat panel fills the available vertical/horizontal space
 *              (no fixed 400px card) — responsive full-height layout.
 *   FR-2       Reopening a session replays persisted history (restore frame).
 *   FR-6       Assistant replies render incrementally from chunk/done frames,
 *              with graceful fallback to a single assistant frame.
 *   Security   Stored titles and message content are rendered as text, never
 *              injected as markup (XSS-safe).
 *
 * These reference not-yet-built selectors (#chat-sessions, .chat-session-item,
 * #chat-new) and not-yet-built client frame handling (ready/restore/chunk/done),
 * so they fail until the frontend lands.
 */

const SESSIONS = [
  {
    id: '018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88',
    title: 'How do I unblock a stuck OODA goal?',
    created_at: '2026-07-04T15:20:11Z',
    updated_at: '2026-07-04T15:41:02Z',
  },
  {
    id: '018f3b12-4a9c-7d02-8f31-11ab77e0c210',
    title: "Summarize last cycle's actions",
    created_at: '2026-07-04T14:02:55Z',
    updated_at: '2026-07-04T14:09:40Z',
  },
];

const SESSION_HISTORY = {
  id: SESSIONS[0].id,
  title: SESSIONS[0].title,
  created_at: SESSIONS[0].created_at,
  updated_at: SESSIONS[0].updated_at,
  history: [
    { role: 'user', content: 'How do I unblock a stuck OODA goal?', timestamp: '2026-07-04T15:20:11Z' },
    { role: 'assistant', content: 'Start by inspecting the goal board.', timestamp: '2026-07-04T15:20:19Z' },
  ],
};

/** Register REST mocks for the session list + by-id endpoints. */
async function mockChatRest(page: Page, sessions = SESSIONS, history = SESSION_HISTORY) {
  // Specific by-id route first so it wins over the list route.
  await page.route('**/api/chat/sessions/*', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(history),
    });
  });
  await page.route('**/api/chat/sessions', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ sessions }),
    });
  });
}

test.describe('Dashboard Chat — session sidebar @structural', () => {
  test.beforeEach(async ({ authenticatedPage }) => {
    await mockChatRest(authenticatedPage);
  });

  test('sidebar lists saved sessions newest-first with a New chat control', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await authenticatedPage.goto('/');
    await chatPage.openChatTab();

    const sidebar = authenticatedPage.locator('#chat-sessions');
    await expect(sidebar).toBeVisible();
    await expect(authenticatedPage.locator('#chat-new')).toBeVisible();

    const items = authenticatedPage.locator('.chat-session-item');
    await expect(items).toHaveCount(SESSIONS.length);
    // Newest-first ordering + titles rendered.
    await expect(items.nth(0)).toContainText('How do I unblock a stuck OODA goal?');
    await expect(items.nth(1)).toContainText("Summarize last cycle's actions");
  });

  test('clicking a session loads its complete history into the panel', async ({
    chatPage,
    authenticatedPage,
  }) => {
    // Mock the WS so the follow-on connect does not hang.
    await authenticatedPage.routeWebSocket('**/ws/chat**', (ws) => {
      ws.send(JSON.stringify({ type: 'ready', session_id: SESSIONS[0].id, streaming: true, protocol_version: 1 }));
    });

    await authenticatedPage.goto('/');
    await chatPage.openChatTab();

    await authenticatedPage.locator('.chat-session-item').nth(0).click();

    const msgs = await chatPage.getMessages();
    const contents = msgs.map((m) => m.content).join('\n');
    expect(contents).toContain('How do I unblock a stuck OODA goal?');
    expect(contents).toContain('Start by inspecting the goal board.');
    const roles = msgs.map((m) => m.role);
    expect(roles).toContain('user');
    expect(roles).toContain('assistant');
  });
});

test.describe('Dashboard Chat — full-height layout @structural', () => {
  test('chat panel fills the viewport (no fixed 400px card)', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await mockChatRest(authenticatedPage);
    await authenticatedPage.setViewportSize({ width: 1280, height: 900 });
    await authenticatedPage.goto('/');
    await chatPage.openChatTab();

    const box = await chatPage.messagesDiv.boundingBox();
    expect(box).not.toBeNull();
    // The old design fixed #chat-messages at 400px. Full-height must be well
    // beyond that and take a large share of the 900px viewport.
    expect(box!.height).toBeGreaterThan(500);

    // The transcript scrolls while the input stays anchored: computed overflow-y
    // is auto/scroll, and the fixed 400px height is gone.
    const overflowY = await chatPage.messagesDiv.evaluate(
      (el) => getComputedStyle(el).overflowY,
    );
    expect(['auto', 'scroll']).toContain(overflowY);

    const fixedHeight = await chatPage.messagesDiv.evaluate((el) => {
      // Inline/authored fixed height of exactly 400px indicates the old card.
      return el.style.height;
    });
    expect(fixedHeight).not.toBe('400px');
  });
});

test.describe('Dashboard Chat — resume/restore @structural', () => {
  test('reopening a session replays persisted history from the restore frame', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await mockChatRest(authenticatedPage);
    await authenticatedPage.routeWebSocket('**/ws/chat**', (ws) => {
      ws.send(JSON.stringify({ type: 'ready', session_id: SESSIONS[0].id, streaming: true, protocol_version: 1 }));
      ws.send(
        JSON.stringify({
          type: 'restore',
          messages: SESSION_HISTORY.history,
        }),
      );
    });

    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
    await chatPage.clickReconnect();
    await chatPage.waitForConnected();

    await chatPage.waitForResponse(5_000);
    const msgs = await chatPage.getMessages();
    const contents = msgs.map((m) => m.content).join('\n');
    expect(contents).toContain('How do I unblock a stuck OODA goal?');
    expect(contents).toContain('Start by inspecting the goal board.');
  });
});

test.describe('Dashboard Chat — streaming with fallback @structural', () => {
  test('assistant reply assembles incrementally from chunk/done frames', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await mockChatRest(authenticatedPage);
    await authenticatedPage.routeWebSocket('**/ws/chat**', (ws) => {
      ws.send(JSON.stringify({ type: 'ready', streaming: true, protocol_version: 1 }));
      ws.onMessage(() => {
        ws.send(JSON.stringify({ type: 'chunk', content: 'Start by inspecting ' }));
        ws.send(JSON.stringify({ type: 'chunk', content: 'the goal board with ' }));
        ws.send(JSON.stringify({ type: 'chunk', content: '`simard status`.' }));
        ws.send(JSON.stringify({ type: 'done' }));
      });
    });

    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
    await chatPage.clickReconnect();
    await chatPage.waitForConnected();

    await chatPage.sendMessage('How do I unblock a stuck goal?');
    await chatPage.waitForResponse(5_000);

    const msgs = await chatPage.getMessages();
    const assistant = msgs.filter((m) => m.role === 'assistant');
    expect(assistant.length).toBeGreaterThan(0);
    // The three chunks must coalesce into ONE assistant bubble, not three.
    const last = assistant[assistant.length - 1];
    expect(last.content).toContain('Start by inspecting the goal board with `simard status`.');
  });

  test('non-streaming fallback renders a single assistant frame', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await mockChatRest(authenticatedPage);
    await authenticatedPage.routeWebSocket('**/ws/chat**', (ws) => {
      ws.send(JSON.stringify({ type: 'ready', streaming: false, protocol_version: 1 }));
      ws.onMessage(() => {
        ws.send(
          JSON.stringify({
            role: 'assistant',
            content: 'Inspect the goal board with `simard status`.',
          }),
        );
      });
    });

    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
    await chatPage.clickReconnect();
    await chatPage.waitForConnected();

    await chatPage.sendMessage('How do I unblock a stuck goal?');
    const resp = await chatPage.waitForResponse(5_000);
    expect(resp.role).toBe('assistant');
    expect(resp.content).toContain('Inspect the goal board with `simard status`.');
  });
});

test.describe('Dashboard Chat — XSS safety @structural', () => {
  test('malicious session titles and messages are rendered as text, not markup', async ({
    chatPage,
    authenticatedPage,
  }) => {
    const xssTitle = '<img src=x onerror="window.__xss_title=1">pwn';
    const xssMsg = '<img src=x onerror="window.__xss_msg=1">hello';
    await mockChatRest(
      authenticatedPage,
      [{ ...SESSIONS[0], title: xssTitle }],
      {
        ...SESSION_HISTORY,
        title: xssTitle,
        history: [{ role: 'assistant', content: xssMsg, timestamp: '2026-07-04T15:20:19Z' }],
      },
    );

    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
    await authenticatedPage.locator('.chat-session-item').nth(0).click();

    // No injected <img> from title or message content anywhere in the chat UI.
    expect(await authenticatedPage.locator('#chat-sessions img').count()).toBe(0);
    expect(await authenticatedPage.locator('#chat-messages img').count()).toBe(0);

    // The onerror handlers never fired.
    const firedTitle = await authenticatedPage.evaluate(() => (window as any).__xss_title);
    const firedMsg = await authenticatedPage.evaluate(() => (window as any).__xss_msg);
    expect(firedTitle).toBeFalsy();
    expect(firedMsg).toBeFalsy();

    // The literal markup is present as text.
    await expect(authenticatedPage.locator('.chat-session-item').nth(0)).toContainText('pwn');
  });
});
