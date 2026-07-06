"""Tab-Identity Contract smoke test (#1993 / #1994 / #1995).

Walks every nav button in the live dashboard and asserts the four
invariants of the Tab-Identity Contract:

1. Unique, non-empty browser ``<title>`` per tab.
2. Unique, non-empty visible ``<h1>`` per tab.
3. Non-empty plain-English ``<p class="page-lede">`` per tab.
4. No banned consultant-speak/acronym jargon in any lede.

The test discovers tabs from the rendered DOM (``[data-tab]`` attributes)
rather than from a hardcoded list, so adding a new tab in
``src/operator_commands_dashboard/index_html/tab_meta.rs`` is picked up
here automatically — no second place to update.

The companion Rust test layer
(``src/operator_commands_dashboard/index_html/tests_tab_meta.rs``) proves
the source-of-truth table is internally consistent. *This* test proves
the *rendered, running, authenticated* dashboard actually obeys the
contract.
"""

from __future__ import annotations

import sys

import pytest
from playwright.sync_api import Page, expect

# Mirrors ``operator_commands_dashboard::index_html::tab_meta::BANNED_JARGON``.
# Kept duplicated on purpose — the list is short and rarely changes; the
# "Adding a new tab" checklist in ``docs/dashboard.md`` reminds contributors
# to touch both at once.
BANNED_JARGON: tuple[str, ...] = (
    "OODA",
    "Observe-Orient-Decide-Act",
    "spawn_engineer",
    "LadybugDB",
    "cognitive memory",
    "synergize",
    "leverage",
    "ideate",
)

# The nine canonical tabs after the #2627 consolidation, in nav-render order.
# Views that were formerly standalone tabs now live as sub-sections inside one
# of these parents (see ``CANONICAL_TABS`` / the alias map below).
CANONICAL_SLUGS: tuple[str, ...] = (
    "overview",
    "goals",
    "activity",
    "workers",
    "pull-requests",
    "resources",
    "chat",
    "overseer",
    "journal",
)

# Every retired top-level slug maps to the parent tab it now lives under, so an
# old bookmark / deep link keeps working instead of 404-ing. Mirrors the Rust
# ``TAB_ALIASES`` allowlist and ``docs/dashboard.md#deep-links-and-tab-aliases``.
RETIRED_SLUG_PARENTS: dict[str, str] = {
    "status": "overview",
    "workboard": "goals",
    "logs": "activity",
    "traces": "activity",
    "thinking": "activity",
    "brain-failures": "activity",
    "processes": "workers",
    "terminal": "workers",
    "merge-decisions": "pull-requests",
    "pr-readiness": "pull-requests",
    "memory": "resources",
    "costs": "resources",
}


def _discover_tab_slugs(page: Page) -> list[str]:
    """Return every tab slug declared in the nav, in render order."""
    slugs = page.eval_on_selector_all(
        ".tab[data-tab]",
        "els => els.map(e => e.getAttribute('data-tab'))",
    )
    # De-duplicate but preserve order. Filter falsy/empty.
    seen: set[str] = set()
    out: list[str] = []
    for s in slugs:
        if s and s not in seen:
            seen.add(s)
            out.append(s)
    return out


@pytest.fixture
def loaded_dashboard(page: Page, dashboard_url: str) -> Page:
    page.goto(f"{dashboard_url}/")
    page.wait_for_selector(".tab[data-tab]", timeout=10_000)
    return page


def test_at_least_nine_tabs_discoverable(loaded_dashboard: Page) -> None:
    """Sanity: the nav exposes exactly the nine consolidated tabs (#2627)."""
    slugs = _discover_tab_slugs(loaded_dashboard)
    assert len(slugs) >= 9, (
        f"expected >=9 tabs, discovered {len(slugs)}: {slugs}"
    )
    # The nine canonical slugs must all be present (drift detector for #2627).
    required = set(CANONICAL_SLUGS)
    missing = required - set(slugs)
    assert not missing, f"required canonical tabs missing from nav: {sorted(missing)}"
    # The retired 17-tab slugs must NOT reappear as top-level nav buttons — they
    # now live as sub-sections reachable via their deep-link alias.
    retired = set(RETIRED_SLUG_PARENTS) & set(slugs)
    assert not retired, (
        f"retired slugs must not be top-level tabs after consolidation: "
        f"{sorted(retired)}"
    )


def test_tab_identity_contract_for_every_tab(
    loaded_dashboard: Page,
) -> None:
    """Walk every nav button and assert the four contract invariants."""
    page = loaded_dashboard
    slugs = _discover_tab_slugs(page)
    assert slugs, "no nav buttons discovered"

    rows: list[tuple[str, str, str, str]] = []
    titles: dict[str, str] = {}
    h1s: dict[str, str] = {}

    for slug in slugs:
        nav = page.locator(f'.tab[data-tab="{slug}"]')
        expect(nav).to_be_visible()
        nav.click()
        # Wait for the panel to be visible. We deliberately don't
        # depend on the class name (`.active`) so the contract survives
        # tab-handler refactors.
        panel = page.locator(f'#tab-{slug}')
        expect(panel).to_be_visible(timeout=5_000)

        title = page.title()
        assert title, f"tab {slug!r} has empty <title>"

        h1_loc = panel.locator("h1.page-h1")
        expect(h1_loc).to_be_visible()
        h1 = (h1_loc.first.text_content() or "").strip()
        assert h1, f"tab {slug!r} has empty <h1 class=page-h1>"

        lede_loc = panel.locator("p.page-lede")
        expect(lede_loc).to_be_visible()
        lede = (lede_loc.first.text_content() or "").strip()
        assert lede, f"tab {slug!r} has empty <p class=page-lede>"
        assert len(lede) >= 40, (
            f"tab {slug!r} lede is suspiciously short "
            f"({len(lede)} chars): {lede!r}"
        )

        # Invariant 4: lede is jargon-free.
        for banned in BANNED_JARGON:
            assert banned not in lede, (
                f"tab {slug!r} lede contains banned jargon {banned!r}: "
                f"{lede!r}"
            )

        # Invariants 1+2: uniqueness across the entire dashboard.
        if title in titles.values():
            other = next(s for s, v in titles.items() if v == title)
            raise AssertionError(
                f"duplicate <title> {title!r} on tabs {other!r} and {slug!r}"
            )
        if h1 in h1s.values():
            other = next(s for s, v in h1s.items() if v == h1)
            raise AssertionError(
                f"duplicate <h1> {h1!r} on tabs {other!r} and {slug!r}"
            )
        titles[slug] = title
        h1s[slug] = h1
        rows.append((slug, title, h1, lede))

    # Evidence dump — printed to stdout so CI captures it in the job log
    # and the PR description can copy it verbatim.
    print(file=sys.stderr)
    print("=== Tab-Identity Contract evidence ===", file=sys.stderr)
    print(file=sys.stderr)
    print("| slug | title | h1 | lede |", file=sys.stderr)
    print("|------|-------|----|------|", file=sys.stderr)
    for slug, title, h1, lede in rows:
        # Truncate the lede so the table stays terminal-friendly. The
        # full text was already asserted to satisfy the contract.
        short_lede = lede if len(lede) <= 100 else lede[:97] + "..."
        print(
            f"| {slug} | {title} | {h1} | {short_lede} |",
            file=sys.stderr,
        )
    print(file=sys.stderr)


# ----- #2627: deep-link continuity for retired slugs -----


@pytest.mark.parametrize(
    ("retired", "parent"),
    sorted(RETIRED_SLUG_PARENTS.items()),
)
def test_retired_slug_deep_link_resolves_to_parent(
    page: Page,
    dashboard_url: str,
    retired: str,
    parent: str,
) -> None:
    """A ``#<retired-slug>`` deep link activates its new parent tab.

    Old bookmarks, browser history, and links in bug reports must keep working
    after the consolidation: navigating to ``/#logs`` should land on the
    **Activity** tab (which now hosts the Logs sub-section) rather than 404 or
    show a blank page.
    """
    page.goto(f"{dashboard_url}/#{retired}")
    page.wait_for_selector(".tab[data-tab]", timeout=10_000)
    # The parent tab's panel becomes visible via the client-side alias resolver.
    parent_panel = page.locator(f"#tab-{parent}")
    expect(parent_panel).to_be_visible(timeout=5_000)
    # The retired slug must NOT resolve to a panel of its own (it is a
    # sub-section now, not a top-level tab).
    assert page.locator(f"#tab-{retired}").count() == 0, (
        f"retired slug {retired!r} must not have its own top-level panel"
    )


def test_unknown_hash_falls_back_to_overview(
    page: Page,
    dashboard_url: str,
) -> None:
    """An unknown/malformed ``#hash`` falls back to Overview with no injection.

    The resolver treats ``location.hash`` as untrusted input: it validates the
    value against ``^[a-z-]+$``, matches it against the allowlist, and defaults
    to the Overview tab on any miss — it never concatenates the hash into a DOM
    selector or element id.
    """
    page.goto(f"{dashboard_url}/#definitely-not-a-real-tab")
    page.wait_for_selector(".tab[data-tab]", timeout=10_000)
    expect(page.locator("#tab-overview")).to_be_visible(timeout=5_000)

    # A malformed hash containing selector metacharacters must also fall back to
    # Overview and must not inject any element into the DOM.
    page.goto(f"{dashboard_url}/#<img src=x onerror=alert(1)>")
    page.wait_for_selector(".tab[data-tab]", timeout=10_000)
    expect(page.locator("#tab-overview")).to_be_visible(timeout=5_000)
    assert page.locator("img[onerror]").count() == 0, (
        "a malformed deep-link hash must never inject markup into the DOM"
    )
