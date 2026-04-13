import { test, expect } from '../fixtures/simard-dashboard';

test.describe('Multi-Turn Context Awareness @structural', () => {
  test.beforeEach(async ({ chatPage, authenticatedPage }) => {
    let turnCount = 0;
    const history: string[] = [];

    await authenticatedPage.routeWebSocket('**/ws/chat', (ws) => {
      ws.send(
        JSON.stringify({
          role: 'system',
          content: 'Connected to Simard. Speak naturally — /help for commands, /close to end.',
        }),
      );

      ws.onMessage((msg) => {
        const text = typeof msg === 'string' ? msg : msg.toString();
        const trimmed = text.trim();
        turnCount++;
        history.push(trimmed);

        if (trimmed === '/close') {
          ws.send(
            JSON.stringify({
              role: 'system',
              content: `Meeting closed. ${turnCount} messages. Summary: discussed ${history.join(', ')}`,
            }),
          );
          ws.close();
          return;
        }

        // Simulate context-aware responses that reference prior turns
        if (turnCount === 1) {
          ws.send(
            JSON.stringify({
              role: 'assistant',
              content: `I'll help with "${trimmed}". What specific aspect interests you?`,
            }),
          );
        } else {
          ws.send(
            JSON.stringify({
              role: 'assistant',
              content: `Building on your earlier question about "${history[0]}", regarding "${trimmed}": here's more detail.`,
            }),
          );
        }
      });
    });

    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
    await chatPage.clickReconnect();
    await chatPage.waitForConnected();
    // Consume greeting
    await chatPage.waitForResponse(5_000);
  });

  test('second turn references first turn context', async ({ chatPage }) => {
    await chatPage.sendMessage('error handling');
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('show me examples');
    const resp = await chatPage.waitForResponse(5_000);
    expect(resp.role).toBe('assistant');
    expect(resp.content).toContain('error handling');
    expect(resp.content).toContain('show me examples');
  });

  test('three-turn conversation maintains thread', async ({ chatPage }) => {
    await chatPage.sendMessage('performance');
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('benchmarks');
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('optimization');
    const resp = await chatPage.waitForResponse(5_000);
    expect(resp.content).toContain('performance');
  });

  test('/close summary includes conversation history', async ({ chatPage }) => {
    await chatPage.sendMessage('topic-alpha');
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('topic-beta');
    await chatPage.waitForResponse(5_000);

    await chatPage.sendMessage('/close');
    const closeMsg = await chatPage.waitForSystemMessage(5_000);
    expect(closeMsg).toContain('Meeting closed');
    expect(closeMsg).toContain('topic-alpha');
    expect(closeMsg).toContain('topic-beta');
  });

  test('message list grows with each turn', async ({ chatPage }) => {
    const beforeCount = (await chatPage.getMessages()).length;

    await chatPage.sendMessage('first');
    await chatPage.waitForResponse(5_000);
    const midCount = (await chatPage.getMessages()).length;
    // user msg + assistant response = +2
    expect(midCount).toBe(beforeCount + 2);

    await chatPage.sendMessage('second');
    await chatPage.waitForResponse(5_000);
    const afterCount = (await chatPage.getMessages()).length;
    expect(afterCount).toBe(midCount + 2);
  });
});
