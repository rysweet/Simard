import { test, expect } from '../fixtures/simard-dashboard';

/**
 * Smoke tests that exercise the real LLM backend.
 * These require a running Simard instance with LLM access.
 * Tagged @smoke so they only run in the 'smoke' project with longer timeouts.
 */
test.describe('Meeting Content Quality @smoke', () => {
  test.beforeEach(async ({ chatPage, authenticatedPage }) => {
    await authenticatedPage.goto('/');
    await chatPage.openChatTab();
    await chatPage.connectWebSocket();
    // Wait for the "Connected to Simard" system greeting
    await chatPage.waitForResponse(15_000);
  });

  test('greeting message appears on connect', async ({ chatPage }) => {
    const msgs = await chatPage.getMessages();
    const greeting = msgs.find((m) => m.role === 'system');
    expect(greeting).toBeDefined();
    expect(greeting!.content).toContain('Connected to Simard');
  });

  test('LLM responds to a natural language question', async ({ chatPage }) => {
    await chatPage.sendMessage('What can you help me with?');
    const resp = await chatPage.waitForResponse(90_000);
    expect(resp.role).toBe('assistant');
    expect(resp.content.length).toBeGreaterThan(10);
  });

  test('/help returns recognized commands', async ({ chatPage }) => {
    await chatPage.sendMessage('/help');
    const resp = await chatPage.waitForSystemMessage(30_000);
    expect(resp).toContain('/status');
    expect(resp).toContain('/close');
  });
});
