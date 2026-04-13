import { test, expect } from '../fixtures/simard-dashboard';

test.describe('Dashboard Authentication @structural', () => {
  test('redirects unauthenticated users to /login', async ({ page, baseURL }) => {
    const resp = await page.request.get(`${baseURL}/`, { maxRedirects: 0 });
    expect(resp.status()).toBe(303);
    expect(resp.headers()['location']).toBe('/login');
  });

  test('login page renders form with code input', async ({ loginPage }) => {
    await loginPage.navigate();
    await expect(loginPage.form).toBeVisible();
    await expect(loginPage.codeInput).toBeVisible();
    await expect(loginPage.codeInput).toHaveAttribute('maxlength', '8');
    await expect(loginPage.codeInput).toHaveAttribute('autocomplete', 'off');
  });

  test('invalid code shows error message', async ({ loginPage }) => {
    await loginPage.navigate();
    await loginPage.submitCode('XXXXXXXX');
    const errText = await loginPage.getErrorText();
    expect(errText).toContain('Invalid code');
  });

  test('valid code redirects to dashboard', async ({ loginPage, loginCode }) => {
    await loginPage.navigate();
    await loginPage.submitCode(loginCode);
    await loginPage.waitForRedirect();
    await expect(loginPage.page).toHaveURL('/');
  });

  test('API returns 401 without auth', async ({ page, baseURL }) => {
    const resp = await page.request.get(`${baseURL}/api/status`, {
      maxRedirects: 0,
    });
    expect(resp.status()).toBe(401);
  });

  test('API returns 200 with valid session cookie', async ({
    authenticatedPage,
    baseURL,
  }) => {
    const resp = await authenticatedPage.request.get(`${baseURL}/api/status`);
    expect(resp.status()).toBe(200);
  });

  test('dashboard loads after authentication', async ({ authenticatedPage }) => {
    await authenticatedPage.goto('/');
    await expect(authenticatedPage.locator('text=Simard Dashboard')).toBeVisible();
    await expect(authenticatedPage.locator('#status')).toBeVisible();
  });
});
