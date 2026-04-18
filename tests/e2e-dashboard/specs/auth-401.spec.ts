import { test, expect } from '../fixtures/simard-dashboard';

test.describe('Auth 401 JSON Body @structural', () => {
  test('unauthenticated API returns JSON error, not empty body', async ({
    page,
    baseURL,
  }) => {
    const resp = await page.request.get(`${baseURL}/api/status`, {
      maxRedirects: 0,
    });
    expect(resp.status()).toBe(401);

    const body = await resp.json();
    expect(body).toHaveProperty('error');
    expect(body.error).toContain('not authenticated');
    expect(body).toHaveProperty('login_url', '/login');
  });

  test('unauthenticated API for goals returns JSON 401', async ({
    page,
    baseURL,
  }) => {
    const resp = await page.request.get(`${baseURL}/api/goals`, {
      maxRedirects: 0,
    });
    expect(resp.status()).toBe(401);

    const body = await resp.json();
    expect(body.error).toContain('not authenticated');
  });

  test('unauthenticated API for workboard returns JSON 401', async ({
    page,
    baseURL,
  }) => {
    const resp = await page.request.get(`${baseURL}/api/workboard`, {
      maxRedirects: 0,
    });
    expect(resp.status()).toBe(401);

    const body = await resp.json();
    expect(body.error).toContain('not authenticated');
  });

  test('unauthenticated API for ooda-thinking returns JSON 401', async ({
    page,
    baseURL,
  }) => {
    const resp = await page.request.get(`${baseURL}/api/ooda-thinking`, {
      maxRedirects: 0,
    });
    expect(resp.status()).toBe(401);

    const body = await resp.json();
    expect(body.error).toContain('not authenticated');
  });

  test('login page does NOT use apiFetch (which is in INDEX_HTML scope)', async ({
    page,
    baseURL,
  }) => {
    const resp = await page.request.get(`${baseURL}/login`);
    const html = await resp.text();

    // Login page should use raw fetch, not apiFetch
    expect(html).not.toContain('apiFetch');
    expect(html).toContain("fetch('/api/login'");
  });
});
