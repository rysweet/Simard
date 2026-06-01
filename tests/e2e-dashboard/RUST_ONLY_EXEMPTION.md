# Rust-Only Exemption: Playwright E2E Tests

**Issue**: [#2159](https://github.com/rysweet/Simard/issues/2159)
**Epic**: [#2155](https://github.com/rysweet/Simard/issues/2155) (enforce Rust-only)
**Status**: EXEMPT

## Rationale

These are **test tooling**, not production code. Playwright is the
industry-standard browser automation framework and has no Rust equivalent
with comparable maturity. These tests validate the dashboard UI, which is
itself a web application requiring JavaScript. Rewriting browser automation
tests in Rust would sacrifice test quality for language purity.

## Scope

All `.ts` files under `tests/e2e-dashboard/` are covered by this exemption:

- `playwright.config.ts` — test configuration
- `fixtures/*.ts` — test fixtures
- `pages/*.ts` — page object models
- `specs/*.ts` — test specifications

The `smoke_python/` subdirectory contains Python smoke tests that are
separately tracked under the Rust-only epic.

## CI Enforcement

A pre-commit hook (`no-new-js-ts`) prevents new `.js`/`.ts` files from being
added outside the exempted directories (`npm/`, `tests/e2e-dashboard/`, and
the root-level distribution shims). See `.pre-commit-config.yaml`.
