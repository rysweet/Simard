import { type Page, type Locator, expect } from '@playwright/test';

export class ChatPage {
  readonly page: Page;
  readonly chatTab: Locator;
  readonly messagesDiv: Locator;
  readonly chatInput: Locator;
  readonly sendButton: Locator;
  readonly wsStatus: Locator;

  constructor(page: Page) {
    this.page = page;
    this.chatTab = page.locator('.tab[data-tab="chat"]');
    this.messagesDiv = page.locator('#chat-messages');
    this.chatInput = page.locator('#chat-input');
    this.sendButton = page.locator('#chat-send');
    this.wsStatus = page.locator('#ws-status');
  }

  async openChatTab(): Promise<void> {
    await this.chatTab.click();
    await this.messagesDiv.waitFor({ state: 'visible' });
  }

  async clickReconnect(): Promise<void> {
    await this.wsStatus.locator('button').click();
  }

  async waitForConnected(timeout = 15_000): Promise<void> {
    await expect(this.wsStatus).toContainText('Connected', { timeout });
  }

  async connectWebSocket(): Promise<void> {
    await this.clickReconnect();
    await this.waitForConnected();
  }

  async sendMessage(text: string): Promise<void> {
    await this.chatInput.fill(text);
    await this.sendButton.click();
  }

  /** Returns all .chat-msg elements as {role, content} pairs. */
  async getMessages(): Promise<Array<{ role: string; content: string }>> {
    const msgs = this.page.locator('.chat-msg');
    const count = await msgs.count();
    const result: Array<{ role: string; content: string }> = [];
    for (let i = 0; i < count; i++) {
      const el = msgs.nth(i);
      const roleEl = el.locator('.role');
      const roleClasses = await roleEl.getAttribute('class') ?? '';
      const role = roleClasses.replace('role', '').trim();
      const fullText = (await el.textContent()) ?? '';
      const roleText = (await roleEl.textContent()) ?? '';
      const content = fullText.replace(roleText, '').trim();
      result.push({ role, content });
    }
    return result;
  }

  async getLastMessage(): Promise<{ role: string; content: string }> {
    const msgs = await this.getMessages();
    if (msgs.length === 0) throw new Error('No messages in chat');
    return msgs[msgs.length - 1];
  }

  async waitForResponse(timeout = 60_000): Promise<{ role: string; content: string }> {
    const initialCount = await this.page.locator('.chat-msg').count();
    await this.page.waitForFunction(
      (expected: number) => document.querySelectorAll('.chat-msg').length > expected,
      initialCount,
      { timeout },
    );
    return this.getLastMessage();
  }

  async waitForSystemMessage(timeout = 30_000): Promise<string> {
    const initialCount = await this.page.locator('.chat-msg').count();
    await this.page.waitForFunction(
      (expected: number) => {
        const msgs = document.querySelectorAll('.chat-msg');
        return msgs.length > expected &&
          msgs[msgs.length - 1].querySelector('.role.system') !== null;
      },
      initialCount,
      { timeout },
    );
    const last = await this.getLastMessage();
    return last.content;
  }
}
