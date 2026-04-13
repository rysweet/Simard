# Run Dashboard End-to-End Tests

The Simard operator dashboard has a Playwright test suite that validates
authentication, chat lifecycle, meeting content quality, and multi-turn
conversation context. Tests run in two tiers — **structural** (mocked
WebSocket, fast, suitable for CI) and **smoke** (real LLM backend, slow,
nightly).

## Prerequisites

| Requirement | Version | Notes |
|---|---|---|
| Node.js | ≥ 18 | For Playwright runner |
| Rust toolchain | stable | To build the dashboard binary |
| Chromium | auto-installed | Via `npx playwright install chromium` |

Install dependencies once:

```bash
npm install
npx playwright install chromium
```

## Run structural tests (no LLM needed)

Structural tests mock the WebSocket layer with `page.routeWebSocket()`.
The dashboard binary still starts (the HTML is embedded in the Rust binary),
but no LLM backend is required.

```bash
npm run test:e2e:structural
```

Typical runtime: **10–30 seconds**.

## Run smoke tests (requires LLM backend)

Smoke tests hit the real Simard agent backend. The dashboard must be able
to reach an LLM provider.

```bash
npm run test:e2e:smoke
```

Smoke tests have a 120-second timeout per test and 2 automatic retries.
Typical runtime: **2–5 minutes** depending on LLM latency.

## Run all tests

```bash
npm run test:e2e
```

This runs both projects sequentially.

## Configuration

### Dashboard port

Set `SIMARD_DASHBOARD_PORT` to override the default port (18787):

```bash
SIMARD_DASHBOARD_PORT=9090 npm run test:e2e:structural
```

### Pre-built binary

Skip the `cargo run` build step by pointing to an existing binary:

```bash
SIMARD_BIN=./target/release/simard npm run test:e2e:structural
```

### Dashboard authentication key

Tests read the dashkey from `~/.simard/.dashkey` by default. Override with:

```bash
SIMARD_DASHKEY=abcd1234 npm run test:e2e:structural
```

### CI mode

When `CI=true` is set, Playwright:

- Uses the `github` reporter (annotations on PR diffs)
- Adds 1 retry for structural tests
- Refuses `test.only` (via `forbidOnly`)
- Does not reuse an existing server

### Traces and debugging

Traces are captured on first retry. View them with:

```bash
npx playwright show-trace test-results/*/trace.zip
```

Run in headed mode for visual debugging:

```bash
npx playwright test --config tests/e2e-dashboard/playwright.config.ts \
  --project structural --headed
```

## Environment summary

| Variable | Default | Purpose |
|---|---|---|
| `SIMARD_DASHBOARD_PORT` | `18787` | Dashboard listen port |
| `SIMARD_BIN` | *(cargo run)* | Path to pre-built binary |
| `SIMARD_DASHKEY` | `~/.simard/.dashkey` | Auth code override |
| `CI` | unset | Enables CI reporter and stricter settings |
