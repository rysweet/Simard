# Dashboard Tab-Identity Smoke Test

A small Playwright/pytest suite that verifies the **Tab Identity Contract**
for the Simard operator dashboard:

1. Every tab has a **unique, non-empty browser `<title>`** in the uniform format `"{Label} · Simard"`.
2. Every tab has a **unique, non-empty visible `<h1>`**.
3. Every tab has a **non-empty `<p class="page-lede">`** sentence under the H1.
4. **No lede contains banned jargon** (`OODA`, `Observe-Orient-Decide-Act`,
   `synergize`, `leverage`, `ideate`). Simard-internal domain vocabulary
   (`facilitator`, `consolidation`, …) is allowed — the bar is "no
   consultant-speak", not "no jargon at all".

The contract is documented in [`docs/dashboard.md`](../../../docs/dashboard.md#tab-identity-contract)
and is the primary acceptance criterion for issues #1993, #1994, and #1995.

## Why a second test layer

Rust unit tests in `tests_tab_meta.rs` prove the `TAB_METADATA` source-of-truth
table is internally consistent. This smoke test proves that the *rendered,
running, authenticated* dashboard actually obeys the contract — including the
client-side `document.title` swap on tab activation and the panel-visibility
invariant ("exactly one `.tab-panel` on-screen at a time") that makes H1
uniqueness a real property of the live DOM, not just of the source table.

The smoke test relies on Playwright's `expect(locator).to_be_visible()` rather
than a specific CSS class name (such as `.active`), so it is decoupled from
the exact mechanism the tab handler uses to hide the inactive panels.

## Layout

```
smoke_python/
├── README.md            # this file
├── requirements.txt     # playwright==1.59.0, pytest, pytest-playwright, requests
├── conftest.py          # session-scoped login: reads SIMARD_DASHKEY or
│                        # ~/.simard/.dashkey, POSTs JSON {"code": …} to
│                        # /api/login, installs simard_session cookie in the
│                        # browser context; honors SIMARD_DASHBOARD_URL
│                        # (default http://localhost:8080)
└── test_tab_clarity.py  # walks every nav button, asserts contract
```

## Run locally

```bash
# From the repo root
pip install -r tests/e2e-dashboard/smoke_python/requirements.txt
python -m playwright install --with-deps chromium

# Start the dashboard in another terminal (or background):
simard dashboard serve --port=8080

# Run:
pytest tests/e2e-dashboard/smoke_python/ -v
```

The default target URL is `http://localhost:8080`. Override with:

```bash
SIMARD_DASHBOARD_URL=http://localhost:9999 \
  pytest tests/e2e-dashboard/smoke_python/ -v
```

## Authentication

The fixture reads the dashboard login code from `~/.simard/.dashkey` (the same
file `simard dashboard serve` writes on first start, or override with the
`SIMARD_DASHKEY` env var used in CI) and POSTs it to `/api/login` as a JSON
body (`Content-Type: application/json`, field name `code`) to match the existing
route handler. The resulting `simard_session` cookie is reused for the duration
of the test session. No credentials are echoed to stdout or to the evidence
table.

## Evidence output

On a green run, the test prints a markdown table of the form:

```
| slug      | title                  | h1         | lede |
|-----------|------------------------|------------|------|
| overview  | Overview · Simard      | Overview   | What the daemon did this cycle … |
| goals     | Goals · Simard         | Goals      | The list of work items Simard is … |
| workboard | Workboard · Simard     | Workboard  | A shared scratch canvas for notes … |
| …         | …                      | …          | … |
```

The PR template for dashboard changes pastes this table into the description as
proof that the four invariants hold for every tab.

## CI

These tests run in the existing `e2e-dashboard` GitHub Actions job, after the
TypeScript Playwright suite has started the dashboard server and provisioned
`~/.simard/.dashkey`. See [`.github/workflows/verify.yml`](../../../.github/workflows/verify.yml)
for the exact step list.

A failing assertion fails the job. The evidence table is captured in the job log.

## Adding a new tab

You do **not** need to edit this suite when adding a new tab. The test discovers
tabs by querying every `[data-tab]` attribute in the rendered nav, so any tab
declared in `TAB_METADATA` (see `src/operator_commands_dashboard/index_html/tab_meta.rs`)
is exercised automatically.

If you add a new banned-jargon term, update both:
- the Rust constant `BANNED_JARGON` in `tab_meta.rs`, and
- the `BANNED_JARGON` list in `test_tab_clarity.py`.

The two lists are intentionally duplicated (no shared format file) — they are
short and rarely change. The "Adding a new tab" checklist in
[`docs/dashboard.md`](../../../docs/dashboard.md#adding-a-new-tab) reminds
contributors to touch both at once.
