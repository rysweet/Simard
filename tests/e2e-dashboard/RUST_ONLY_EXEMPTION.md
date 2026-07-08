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

> The former `smoke_python/` subdirectory (pytest + Playwright) was removed in
> #3181. Simard is a pure-Rust, Python-free daemon; its dashboard tab-identity
> coverage is now the Rust `tests_tab_meta.rs` unit tests plus these TypeScript
> Playwright specs. No Python remains under `tests/e2e-dashboard/`.

## CI Enforcement

The committed native `hooks/pre-commit` (wired via `core.hooksPath`, see
`hooks/README.md`) and the CI `verify.yml` Rust-only gate
(`scripts/check-rust-only-gate.sh`) prevent new `.py` files anywhere, and new
`.js`/`.ts` files outside the exempted directories (`npm/`,
`tests/e2e-dashboard/`, and the root-level distribution shims).
