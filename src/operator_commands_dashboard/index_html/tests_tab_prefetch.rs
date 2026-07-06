//! Contract tests for the **background tab prefetch / refresh** feature
//! (issue #2649). These assert the static markers of the client-side
//! scheduler, loader registry, `apiFetch` in-flight de-dupe, per-tab
//! freshness indicators, and the global `visibilitychange` back-off gate
//! are present in the rendered `INDEX_HTML`.
//!
//! This is the Rust half of the Background-Prefetch contract; the
//! behavioural half is `tests/e2e-dashboard/specs/tab-prefetch.spec.ts`.
//!
//! TDD note: every assertion here is expected to **fail** until the
//! background scheduler is implemented — the markers below are absent
//! from the current `runTabFetches`/`activateTab`/`apiFetch` code. They
//! define the contract the implementation must satisfy.

#![cfg(test)]

use super::INDEX_HTML;

// Per-slug loader coverage ("every canonical tab has a registered loader")
// is verified at runtime in tests/e2e-dashboard/specs/tab-prefetch.spec.ts
// by introspecting `Object.keys(window.TAB_LOADERS)`; a static Rust string
// check cannot distinguish a registry key from a slug mentioned elsewhere
// (TAB_ALIASES, CANONICAL_TABS), so it is intentionally not duplicated here.

/// REQ-1: a single-source-of-truth loader registry (`slug -> loaders`)
/// replaces the `runTabFetches` slug-branch chain.
#[test]
fn tab_prefetch_registry_is_declared() {
    assert!(
        INDEX_HTML.contains("const TAB_LOADERS="),
        "rendered INDEX_HTML missing the TAB_LOADERS registry — background \
         prefetch needs a slug->loaders map as its single source of truth"
    );
}

/// REQ-1: the registry must be reachable from the page global scope so the
/// e2e contract test can introspect `Object.keys(window.TAB_LOADERS)` and
/// assert every canonical tab has a loader.
#[test]
fn tab_prefetch_registry_exposed_on_window() {
    assert!(
        INDEX_HTML.contains("window.TAB_LOADERS"),
        "TAB_LOADERS must be exposed on window so the registry coverage can \
         be verified at runtime (tab-prefetch.spec.ts reads window.TAB_LOADERS)"
    );
}

/// REQ-1: the scheduler entry point must be defined *and* invoked from the
/// dashboard init so every tab starts loading on page load, not on click.
#[test]
fn tab_prefetch_scheduler_defined_and_invoked() {
    assert!(
        INDEX_HTML.contains("function startBackgroundScheduler("),
        "missing startBackgroundScheduler() definition — the scheduler that \
         enqueues a background load for every tab on init"
    );
    assert!(
        INDEX_HTML.contains("startBackgroundScheduler()"),
        "startBackgroundScheduler() is never called from init — background \
         prefetch would never start"
    );
}

/// REQ-2 (efficiency): initial background loads must be bounded and
/// staggered so ~10 tabs never hammer the daemon at once.
#[test]
fn tab_prefetch_bounded_concurrency_and_stagger() {
    assert!(
        INDEX_HTML.contains("MAX_CONCURRENCY=3"),
        "missing MAX_CONCURRENCY=3 — background loads must be capped at 3 \
         concurrent fetchers so the initial wave does not DoS the daemon"
    );
    assert!(
        INDEX_HTML.contains("STAGGER_MS=150"),
        "missing STAGGER_MS=150 — initial background dispatch must be \
         staggered so the daemon never sees every endpoint at once"
    );
}

/// REQ-2 (efficiency): concurrent duplicate GETs to the same endpoint must
/// be de-duped at the apiFetch layer, and the in-flight entry must be
/// cleared on settle (success AND failure) so a rejected promise cannot
/// poison the lock. The 401 -> /login guard must survive the refactor.
#[test]
fn tab_prefetch_apifetch_dedupes_inflight_and_preserves_auth() {
    assert!(
        INDEX_HTML.contains("inFlightFetches"),
        "apiFetch has no inFlightFetches map — concurrent duplicate GETs \
         (background + manual refresh) would not be de-duped"
    );
    assert!(
        INDEX_HTML.contains("inFlightFetches.delete"),
        "inFlightFetches entry is never deleted — the in-flight promise must \
         be cleared on settle so a failed fetch cannot poison the de-dupe lock"
    );
    assert!(
        INDEX_HTML.contains("window.location.href='/login'"),
        "the 401 -> /login redirect guard must be preserved through the \
         apiFetch de-dupe refactor — a backgrounded 401 must still log out"
    );
}

/// REQ-3 (correctness / no silent staleness): a per-endpoint "last OK"
/// timestamp store drives the freshness indicators.
#[test]
fn tab_prefetch_freshness_timestamp_store() {
    assert!(
        INDEX_HTML.contains("lastOk"),
        "missing the lastOk freshness store — the dashboard must record when \
         each endpoint last succeeded to render a per-tab data-age indicator"
    );
}

/// REQ-3: each tab renders a subtle "Updated <relative>" indicator with a
/// stable `data-testid="{slug}-updated"` so the operator can tell data age
/// and the e2e test can locate it.
#[test]
fn tab_prefetch_freshness_indicator_testid() {
    assert!(
        INDEX_HTML.contains("-updated"),
        "missing per-tab freshness indicator — each tab needs a subtle \
         data-testid=\"{{slug}}-updated\" element showing when its data was \
         last refreshed (no silent staleness)"
    );
}

/// REQ-2 (back-off): a global visibility gate must suspend background
/// refresh when the browser tab is hidden and resume when it is visible.
#[test]
fn tab_prefetch_visibility_gate() {
    assert!(
        INDEX_HTML.contains("visibilitychange"),
        "missing a visibilitychange listener — background refresh must pause \
         when the browser tab is hidden to avoid wasted work"
    );
    assert!(
        INDEX_HTML.contains("document.visibilityState"),
        "missing a document.visibilityState check — the scheduler must gate \
         refresh on tab visibility and resume promptly when visible again"
    );
}
