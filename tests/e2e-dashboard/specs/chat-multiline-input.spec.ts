import { test, expect } from '../fixtures/simard-dashboard';
import { type Page } from '@playwright/test';

/**
 * Failing TDD e2e spec for the multi-line Dashboard Chat input (issue #2690).
 *
 * The dashboard chat box must become a genuine multi-line, auto-growing
 * textarea instead of a fixed single-line-height input, while preserving the
 * existing send semantics.
 *
 * Acceptance contract (from the design spec):
 *   1. #chat-input is a <textarea> whose CSS grows between a min and a max
 *      height, then scrolls (min-height:42px, max-height:150px,
 *      overflow-y:auto, resize:none). The FIXED `height:42px` is gone.
 *   2. Typing multiple lines auto-grows the box (JS `input` listener sets
 *      height=auto then min(scrollHeight, 150)).
 *   3. Growth is capped at max-height and the box becomes scrollable past it.
 *   4. Shift+Enter inserts a newline and does NOT send.
 *   5. Enter sends the trimmed message and the box collapses back to its
 *      resting height (inline height cleared).
 *   6. Multi-line content is transmitted intact (internal newlines preserved).
 *   7. Whitespace-only input is not sent.
 *   8. Busy state still disables the input and the send button.
 *   9. Selectors (#chat-input, #chat-send) and the placeholder are unchanged.
 *
 * The CSS-envelope, auto-grow, cap-and-scroll, and box-collapse rows exercise
 * not-yet-built CSS/JS and therefore FAIL until the frontend lands. The
 * remaining rows guard behaviour that must NOT regress.
 *
 * The magic number 150 mirrors the CSS `max-height` and the JS clamp; keep it
 * in sync with src/operator_commands_dashboard/index_html/{part_00,part_04}.rs.
 *
 * NOTE ON WS MOCKING: the dashboard has a stateful backend (persisted chat
 * sessions can auto-populate the panel), so send-path tests (a) register the
 * WebSocket mock *before* navigation — the only order that reliably intercepts
 * the connection — and (b) assert on *relative* message counts rather than
 * absolute ones.
 */

const MAX_HEIGHT_PX = 150;
const MIN_HEIGHT_PX = 42;
const WS_GLOB = '**/ws/chat*';

/** Rendered pixel height of the textarea (border-box). */
async function boxHeight(page: Page): Promise<number> {
  return page.locator('#chat-input').evaluate(
    (el) => el.getBoundingClientRect().height,
  );
}

/** Echo each received message back as an assistant frame. */
function echoHandler(ws: { onMessage: (cb: (m: unknown) => void) => void; send: (d: string) => void }) {
  ws.onMessage((msg: unknown) => {
    const text = typeof msg === 'string' ? msg : String(msg);
    ws.send(JSON.stringify({ role: 'assistant', content: `echo:\n${text}` }));
  });
}

/** Accept messages but never reply, so the busy state persists. */
function silentHandler(ws: { onMessage: (cb: (m: unknown) => void) => void }) {
  ws.onMessage(() => {
    /* intentionally no reply */
  });
}

/**
 * Register the WS mock BEFORE navigating, open the chat tab, and connect.
 * Registering the route before `goto` is the pattern that reliably intercepts
 * the WebSocket the dashboard opens.
 */
async function openChatWithWs(
  page: Page,
  handler: Parameters<Page['routeWebSocket']>[1],
): Promise<void> {
  await page.routeWebSocket(WS_GLOB, handler);
  await page.goto('/');
  await page.locator('.tab[data-tab="chat"]').click();
  await page.locator('#chat-messages').waitFor({ state: 'visible' });
  await page.locator('#ws-status button').click();
  await expect(page.locator('#ws-status')).toContainText('Connected', { timeout: 15_000 });
}

test.describe('Chat Multi-line Input @structural', () => {
  test.beforeEach(async ({ chatPage, authenticatedPage }) => {
    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
  });

  test('chat input is a textarea with the correct id and placeholder', async ({
    chatPage,
  }) => {
    const tag = await chatPage.chatInput.evaluate((el) => el.tagName.toLowerCase());
    expect(tag).toBe('textarea');
    await expect(chatPage.chatInput).toHaveAttribute(
      'placeholder',
      'Type a message… (/close to end session)',
    );
    await expect(chatPage.sendButton).toBeVisible();
  });

  test('textarea uses a growable height envelope, not a fixed height', async ({
    authenticatedPage,
  }) => {
    const style = await authenticatedPage.locator('#chat-input').evaluate((el) => {
      const cs = getComputedStyle(el);
      return {
        minHeight: cs.minHeight,
        maxHeight: cs.maxHeight,
        overflowY: cs.overflowY,
        resize: cs.resize,
      };
    });

    // A fixed single-line height must NOT be enforced any more; instead the
    // element grows between an explicit min and max.
    expect(style.minHeight).toBe(`${MIN_HEIGHT_PX}px`);
    expect(style.maxHeight).toBe(`${MAX_HEIGHT_PX}px`);
    // Overflow must be auto so content past the cap scrolls.
    expect(style.overflowY).toBe('auto');
    // Manual resize handle stays disabled (JS controls height).
    expect(style.resize).toBe('none');
  });

  test('typing multiple lines auto-grows the textarea', async ({ chatPage, authenticatedPage }) => {
    const empty = await boxHeight(authenticatedPage);

    await chatPage.chatInput.fill('single line');
    const oneLine = await boxHeight(authenticatedPage);

    await chatPage.chatInput.fill(
      ['line one', 'line two', 'line three', 'line four', 'line five'].join('\n'),
    );
    const manyLines = await boxHeight(authenticatedPage);

    // A single line is at the resting height.
    expect(oneLine).toBeLessThanOrEqual(MIN_HEIGHT_PX + 12);
    // Five lines must be visibly taller than one line.
    expect(manyLines).toBeGreaterThan(oneLine + 20);
    // ...but never taller than the cap.
    expect(manyLines).toBeLessThanOrEqual(MAX_HEIGHT_PX + 2);
    // Sanity: an empty box is also at the resting height.
    expect(empty).toBeLessThanOrEqual(MIN_HEIGHT_PX + 12);
  });

  test('growth is capped at the max height and becomes scrollable', async ({
    chatPage,
    authenticatedPage,
  }) => {
    const lots = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join('\n');
    await chatPage.chatInput.fill(lots);

    const height = await boxHeight(authenticatedPage);
    expect(height).toBeLessThanOrEqual(MAX_HEIGHT_PX + 2);
    expect(height).toBeGreaterThanOrEqual(MAX_HEIGHT_PX - 12);

    const { scrollHeight, clientHeight } = await chatPage.chatInput.evaluate(
      (el) => ({ scrollHeight: el.scrollHeight, clientHeight: el.clientHeight }),
    );
    // More content than fits => the box scrolls internally.
    expect(scrollHeight).toBeGreaterThan(clientHeight);
  });

  test('Shift+Enter inserts a newline without sending', async ({ chatPage }) => {
    const before = await chatPage.page.locator('.chat-msg').count();

    await chatPage.chatInput.click();
    await chatPage.chatInput.type('first line');
    await chatPage.chatInput.press('Shift+Enter');
    await chatPage.chatInput.type('second line');

    const value = await chatPage.chatInput.inputValue();
    expect(value).toBe('first line\nsecond line');

    // Nothing was sent: no new chat bubble was appended.
    expect(await chatPage.page.locator('.chat-msg').count()).toBe(before);
    // Input still holds the draft.
    await expect(chatPage.chatInput).not.toHaveValue('');
  });
});

test.describe('Chat Multi-line Send @structural', () => {
  test('Enter sends and the textarea collapses back to its resting height', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await openChatWithWs(authenticatedPage, echoHandler);

    // Build a tall multi-line draft, then confirm it grew.
    await chatPage.chatInput.fill(
      Array.from({ length: 8 }, (_, i) => `draft line ${i + 1}`).join('\n'),
    );
    const grown = await boxHeight(authenticatedPage);
    expect(grown).toBeGreaterThan(MIN_HEIGHT_PX + 20);

    await chatPage.chatInput.press('Enter');

    // Draft cleared (message sent).
    await expect(chatPage.chatInput).toHaveValue('');
    // Inline height override is cleared so the box returns to CSS min-height.
    const inlineHeight = await chatPage.chatInput.evaluate(
      (el) => (el as HTMLTextAreaElement).style.height,
    );
    expect(inlineHeight).toBe('');
    // Rendered height is back at the resting height.
    const collapsed = await boxHeight(authenticatedPage);
    expect(collapsed).toBeLessThanOrEqual(MIN_HEIGHT_PX + 12);
  });

  test('multi-line message is submitted with its newlines intact', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await openChatWithWs(authenticatedPage, echoHandler);

    await chatPage.chatInput.click();
    await chatPage.chatInput.type('first line');
    await chatPage.chatInput.press('Shift+Enter');
    await chatPage.chatInput.type('second line');
    await chatPage.chatInput.press('Enter');

    // The echoed assistant bubble proves the multi-line payload was sent with
    // both lines (and their newline) intact.
    await chatPage.page
      .locator('.chat-msg .role.assistant')
      .first()
      .waitFor({ state: 'visible', timeout: 5_000 });
    const msgs = await chatPage.getMessages();

    const assistant = msgs.find((m) => m.role === 'assistant');
    expect(assistant).toBeDefined();
    expect(assistant!.content).toContain('first line');
    expect(assistant!.content).toContain('second line');

    // The user's own bubble also preserves both lines.
    const user = msgs.find((m) => m.role === 'user');
    expect(user).toBeDefined();
    expect(user!.content).toContain('first line');
    expect(user!.content).toContain('second line');
  });

  test('whitespace-only input is not sent', async ({ chatPage, authenticatedPage }) => {
    await openChatWithWs(authenticatedPage, echoHandler);

    const before = await chatPage.page.locator('.chat-msg').count();
    await chatPage.chatInput.fill('   \n  \n\t ');
    await chatPage.sendButton.click();

    // No bubble appended for whitespace-only content.
    await chatPage.page.waitForTimeout(300);
    expect(await chatPage.page.locator('.chat-msg').count()).toBe(before);
  });

  test('busy state disables the input and send button', async ({
    chatPage,
    authenticatedPage,
  }) => {
    await openChatWithWs(authenticatedPage, silentHandler);

    await chatPage.sendMessage('are you there?');

    await expect(chatPage.chatInput).toBeDisabled();
    await expect(chatPage.sendButton).toBeDisabled();
  });
});
