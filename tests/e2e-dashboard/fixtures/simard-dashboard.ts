import { test as base, expect, type Page } from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';
import { LoginPage } from '../pages/login.page';
import { ChatPage } from '../pages/chat.page';

type SimardFixtures = {
  /** The 8-char dashkey read from ~/.simard/.dashkey or SIMARD_DASHKEY env. */
  loginCode: string;
  /** A Page that already has a valid simard_session cookie. */
  authenticatedPage: Page;
  /** LoginPage page-object on a fresh (unauthenticated) page. */
  loginPage: LoginPage;
  /** ChatPage page-object on an authenticated page. */
  chatPage: ChatPage;
};

export const test = base.extend<SimardFixtures>({
  loginCode: [async ({}, use) => {
    // 1. Env override
    const envCode = process.env.SIMARD_DASHKEY;
    if (envCode) {
      await use(envCode);
      return;
    }
    // 2. Read from ~/.simard/.dashkey
    const keyPath = path.join(os.homedir(), '.simard', '.dashkey');
    const code = fs.readFileSync(keyPath, 'utf-8').trim();
    if (!code) throw new Error(`Empty dashkey at ${keyPath}`);
    await use(code);
  }, { scope: 'test' }],

  authenticatedPage: async ({ page, loginCode, baseURL }, use) => {
    // POST /api/login to get a session token, then set the cookie
    const resp = await page.request.post(`${baseURL}/api/login`, {
      data: { code: loginCode },
    });
    expect(resp.status()).toBe(200);

    const setCookie = resp.headers()['set-cookie'] ?? '';
    const tokenMatch = setCookie.match(/simard_session=([^;]+)/);
    expect(tokenMatch).toBeTruthy();
    const token = tokenMatch![1];

    const url = new URL(baseURL!);
    await page.context().addCookies([{
      name: 'simard_session',
      value: token,
      domain: url.hostname,
      path: '/',
    }]);

    await use(page);
  },

  loginPage: async ({ page }, use) => {
    await use(new LoginPage(page));
  },

  chatPage: async ({ authenticatedPage }, use) => {
    await use(new ChatPage(authenticatedPage));
  },
});

export { expect };
