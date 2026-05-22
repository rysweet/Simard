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
    "synergize",
    "leverage",
    "ideate",
)


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


def test_at_least_eleven_tabs_discoverable(loaded_dashboard: Page) -> None:
    """Sanity: the nav exposes the eleven tabs the contract covers."""
    slugs = _discover_tab_slugs(loaded_dashboard)
    assert len(slugs) >= 11, (
        f"expected >=11 tabs, discovered {len(slugs)}: {slugs}"
    )
    # Slugs we know must be present (drift detector for #1995).
    required = {
        "overview", "goals", "traces", "logs", "processes",
        "memory", "costs", "chat", "workboard", "thinking", "terminal",
    }
    missing = required - set(slugs)
    assert not missing, f"required tabs missing from nav: {sorted(missing)}"


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
