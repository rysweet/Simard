import { type Page, type Locator } from '@playwright/test';

export class LoginPage {
  readonly page: Page;
  readonly form: Locator;
  readonly codeInput: Locator;
  readonly errorDiv: Locator;

  constructor(page: Page) {
    this.page = page;
    this.form = page.locator('#login-form');
    this.codeInput = page.locator('#code');
    this.errorDiv = page.locator('#error');
  }

  async navigate(): Promise<void> {
    await this.page.goto('/login');
    await this.form.waitFor({ state: 'visible' });
  }

  async submitCode(code: string): Promise<void> {
    await this.codeInput.fill(code);
    await this.form.evaluate((form: HTMLFormElement) => form.requestSubmit());
  }

  async getErrorText(): Promise<string> {
    await this.errorDiv.waitFor({ state: 'visible', timeout: 5_000 });
    return (await this.errorDiv.textContent()) ?? '';
  }

  async isErrorVisible(): Promise<boolean> {
    return this.errorDiv.isVisible();
  }

  async waitForRedirect(): Promise<void> {
    await this.page.waitForURL('/', { timeout: 10_000 });
  }
}
