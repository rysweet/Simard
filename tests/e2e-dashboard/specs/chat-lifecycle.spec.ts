import { test, expect } from '../fixtures/simard-dashboard';

test.describe('Chat Lifecycle @structural', () => {
  test.beforeEach(async ({ chatPage, authenticatedPage }) => {
    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
  });

  test('chat tab shows disconnected state initially', async ({ chatPage }) => {
    await expect(chatPage.wsStatus).toContainText('Disconnected');
    await expect(chatPage.wsStatus.locator('button')).toBeVisible();
  });

  test('chat elements are present', async ({ chatPage }) => {
    await expect(chatPage.messagesDiv).toBeVisible();
    await expect(chatPage.chatInput).toBeVisible();
    await expect(chatPage.sendButton).toBeVisible();
    await expect(chatPage.chatInput).toHaveAttribute(
      'placeholder',
      'Type a message… (/close to end session)',
    );
  });

  test('sending without connection shows system warning', async ({ chatPage }) => {
    await chatPage.sendMessage('hello');
    const msgs = await chatPage.getMessages();
    const systemMsg = msgs.find((m) => m.role === 'system');
    expect(systemMsg).toBeDefined();
    expect(systemMsg!.content).toContain('Not connected');
  });

  test('Enter key sends message', async ({ chatPage, authenticatedPage }) => {
    // Mock WS to avoid needing a real backend
    await authenticatedPage.routeWebSocket('**/ws/chat', (ws) => {
      ws.onMessage((msg) => {
        ws.send(JSON.stringify({ role: 'assistant', content: `echo: ${msg}` }));
      });
    });

    await chatPage.clickReconnect();
    await chatPage.waitForConnected();
    await chatPage.chatInput.fill('test message');
    await chatPage.chatInput.press('Enter');

    const resp = await chatPage.waitForResponse(5_000);
    expect(resp.role).toBe('assistant');
    expect(resp.content).toContain('echo: test message');
  });
});

test.describe('Chat Commands with Mock WS @structural', () => {
  test.beforeEach(async ({ chatPage, authenticatedPage }) => {
    await authenticatedPage.routeWebSocket('**/ws/chat', (ws) => {
      // Simulate the server greeting
      ws.send(
        JSON.stringify({
          role: 'system',
          content: 'Connected to Simard. Speak naturally — /help for commands, /close to end.',
        }),
      );

      ws.onMessage((msg) => {
        const text = typeof msg === 'string' ? msg : msg.toString();
        const trimmed = text.trim();

        if (trimmed === '/help') {
          ws.send(
            JSON.stringify({
              role: 'system',
              content:
                'Commands: /status, /close, /help. Everything else is natural conversation with Simard.',
            }),
          );
        } else if (trimmed === '/status') {
          ws.send(
            JSON.stringify({
              role: 'system',
              content: 'Topic: Dashboard Chat\nMessages: 1\nStarted: 2026-01-01T00:00:00Z\nOpen: true',
            }),
          );
        } else if (trimmed === '/close') {
          ws.send(
            JSON.stringify({
              role: 'system',
              content: 'Meeting closed. 3 messages. Summary: Test session completed.',
            }),
          );
          ws.close();
        } else {
          ws.send(
            JSON.stringify({ role: 'assistant', content: `Response to: ${trimmed}` }),
          );
        }
      });
    });

    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
    await chatPage.clickReconnect();
    await chatPage.waitForConnected();
  });

  test('/help returns command list', async ({ chatPage }) => {
    // Wait for greeting
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('/help');
    const resp = await chatPage.waitForSystemMessage(5_000);
    expect(resp).toContain('/status');
    expect(resp).toContain('/close');
    expect(resp).toContain('/help');
  });

  test('/status returns session info', async ({ chatPage }) => {
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('/status');
    const resp = await chatPage.waitForSystemMessage(5_000);
    expect(resp).toContain('Topic:');
    expect(resp).toContain('Messages:');
    expect(resp).toContain('Open: true');
  });

  test('/close terminates session', async ({ chatPage }) => {
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('/close');
    const resp = await chatPage.waitForSystemMessage(5_000);
    expect(resp).toContain('Meeting closed');
    expect(resp).toContain('Summary:');
  });

  test('natural conversation gets assistant response', async ({ chatPage }) => {
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('What is the current system status?');
    const resp = await chatPage.waitForResponse(5_000);
    expect(resp.role).toBe('assistant');
    expect(resp.content).toContain('Response to:');
  });
});
