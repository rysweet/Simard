# Test New Dashboard Overview Elements

The dashboard overview tab renders eight operator-facing panels that are
covered by structural Playwright tests in
`tests/e2e-dashboard/specs/overview-new-elements.spec.ts`. This guide
describes what the suite asserts, how to run it locally, and how it is
wired into CI.

## Covered elements

The suite exercises the following DOM nodes rendered by the dashboard
overview (see `src/operator_commands_dashboard/routes.rs`):

| Element ID            | Panel                       | Data source       |
|-----------------------|-----------------------------|-------------------|
| `#agent-live-status`  | Autonomous agent banner     | `/api/status`     |
| `#recent-actions-list`| Recent Actions card         | `/api/activity`   |
| `#open-prs-list`      | Open Pull Requests card     | `/api/prs`        |
| `#cluster-topology`   | Cluster Topology card       | `/api/distributed`|
| `#remote-vms`         | Remote VMs card             | `/api/distributed`|
| `#hosts-list`         | Azlin Hosts card            | `/api/hosts`      |
| `#host-name`          | Host name (within Hosts)    | `/api/hosts`      |
| `#host-rg`            | Host resource group         | `/api/hosts`      |

Each element family is covered by three cases:

1. **visible** — element is present and visible on the default overview
   tab without user interaction.
2. **populated** — when the relevant `/api/*` route is mocked with a
   non-empty payload, the element renders the expected entries.
3. **empty / error** — when the mock returns `[]` or HTTP 500, the
   element renders a sanitized empty-state message rather than raising.

All tests are tagged `@structural` and run against mocked APIs only —
no real backend, LLM, or Azure call is made.

## Run locally

```bash
# One-time setup
npm install
npx playwright install --with-deps chromium

# Build the binary the Playwright web-server fixture launches
cargo build --release --bin simard

# Run only the new-elements spec
SIMARD_BIN=./target/release/simard \
  npx playwright test --config tests/e2e-dashboard/playwright.config.ts \
  specs/overview-new-elements.spec.ts

# Or run the full structural tier
npm run test:e2e:structural
```

Typical runtime: **5–10 seconds** for the new-elements spec alone.

## How it works

The spec uses the existing `authenticatedPage` fixture from
`tests/e2e-dashboard/fixtures/` and the extended `OverviewPage`
page-object, which exposes a `Locator` per element ID listed above.

A `beforeEach` block installs default `page.route()` handlers for every
`/api/*` endpoint the overview reads, plus a `**/api/**` catch-all that
returns `{}` so unmocked endpoints cannot hang the test. Individual
tests override the handler they care about.

Mock payloads use only synthetic identifiers (`test-host-01`,
`octocat/hello-world`, `example.com`). Real tenant IDs, subscription
GUIDs, and internal hostnames are forbidden — a code-review check greps
for these before merge.

## CI integration

The suite runs in a dedicated `e2e-dashboard` job in
`.github/workflows/verify.yml`. The job:

1. Checks out the repo with `permissions: contents: read`.
2. Restores the Rust build cache (`Swatinem/rust-cache`).
3. Builds the dashboard binary: `cargo build --release --bin simard --jobs 2`.
4. Sets up Node 20 and runs `npm ci`.
5. Installs Chromium: `npx playwright install --with-deps chromium`.
6. Runs `SIMARD_BIN=./target/release/simard npm run test:e2e:structural`.
7. On failure, uploads the `playwright-report/` directory as a workflow
   artifact for triage.

The job triggers on every pull request and on pushes to the default
branch.

## Adding a new overview element

When you add a new panel to the overview tab:

1. Give the root element a stable `id` attribute.
2. Add a `readonly` `Locator` to `OverviewPage` in
   `tests/e2e-dashboard/pages/overview.page.ts`.
3. Add a `describe` block to `overview-new-elements.spec.ts` with the
   visible / populated / empty triplet.
4. If the panel reads a new `/api/*` endpoint, add a default mock to the
   spec's `beforeEach`.

## See also

- [Run Dashboard End-to-End Tests](run-dashboard-e2e-tests.md) — full
  structural and smoke test workflow.
- `tests/e2e-dashboard/playwright.config.ts` — runner configuration.
- `src/operator_commands_dashboard/routes.rs` — server-rendered overview HTML and client-side fetchers for `/api/*`.
