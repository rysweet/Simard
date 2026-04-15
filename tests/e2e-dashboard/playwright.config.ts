import { defineConfig, devices } from '@playwright/test';

const PORT = Number(process.env.SIMARD_DASHBOARD_PORT ?? 18787);

export default defineConfig({
  testDir: './specs',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? 'github' : 'list',
  timeout: 30_000,
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: 'on-first-retry',
    headless: true,
  },
  projects: [
    {
      name: 'structural',
      testMatch: /.*\.spec\.ts$/,
      grep: /@structural/,
      use: { ...devices['Desktop Chrome'] },
      timeout: 30_000,
    },
    {
      name: 'smoke',
      testMatch: /.*\.spec\.ts$/,
      grep: /@smoke/,
      use: { ...devices['Desktop Chrome'] },
      timeout: 120_000,
      retries: 2,
    },
  ],
  webServer: {
    command: process.env.SIMARD_BIN
      ? `${process.env.SIMARD_BIN} dashboard serve --port=${PORT}`
      : `cargo run --release -- dashboard serve --port=${PORT}`,
    url: `http://localhost:${PORT}/login`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
