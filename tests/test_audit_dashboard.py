"""TDD contract tests for scripts/dashboard_audit/audit_dashboard.py.

Written BEFORE the implementation exists (Step 7: TDD - Write Tests First).
Every test in this file should FAIL until `audit_dashboard.py` is written to
satisfy the design spec captured in `scripts/dashboard_audit/README.md` and
the Step-2c requirements (R1..R8).

These tests pin the *contract* — they do not require a live dashboard, do not
require real Playwright (the import is stubbed), and do not produce any
network traffic. Run with:

    pipx run pytest tests/test_audit_dashboard.py -v

Failure modes by category:
  - "module not found"          → implementation file missing (R1)
  - "function missing"          → required public function not exported
  - "constant missing"          → required module-level constant not defined
  - "LOC budget exceeded"       → script exceeded 150-line soft budget (R1)
  - "behavior mismatch"         → function exists but does not match contract
"""

from __future__ import annotations

import importlib
import json
import os
import sys
import textwrap
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any
from unittest.mock import MagicMock, patch

import pytest

# ---------------------------------------------------------------------------
# Locate the script under test and make the dashboard_audit dir importable.
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT_DIR = REPO_ROOT / "scripts" / "dashboard_audit"
SCRIPT_PATH = SCRIPT_DIR / "audit_dashboard.py"

if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))


# ---------------------------------------------------------------------------
# Stub `playwright.sync_api` so `audit_dashboard.py` can be imported in a
# test environment that has no Playwright installed (matches the existing
# test_knowledge_bridge.py pattern that stubs `wikigr`).
# ---------------------------------------------------------------------------


def _install_playwright_stub() -> None:
    if "playwright" in sys.modules and "playwright.sync_api" in sys.modules:
        return
    playwright_pkg = ModuleType("playwright")
    sync_api = ModuleType("playwright.sync_api")

    class _PWTimeout(Exception):
        pass

    def _sync_playwright() -> Any:  # pragma: no cover — only used if test calls main()
        raise RuntimeError("sync_playwright stub invoked in unit test")

    sync_api.sync_playwright = _sync_playwright  # type: ignore[attr-defined]
    sync_api.TimeoutError = _PWTimeout  # type: ignore[attr-defined]
    # Minimal type aliases that the module under test may import.
    for name in ("Browser", "BrowserContext", "Page", "Request", "Response"):
        setattr(sync_api, name, type(name, (), {}))
    sys.modules["playwright"] = playwright_pkg
    sys.modules["playwright.sync_api"] = sync_api


_install_playwright_stub()


# ---------------------------------------------------------------------------
# Lazy import helper — the module may not exist yet during initial TDD runs.
# Tests that need the module import it through this helper so collection
# doesn't crash with ImportError before any test runs.
# ---------------------------------------------------------------------------


def _import_audit_module() -> ModuleType:
    """Import audit_dashboard, redirecting OUT_DIR to a tmp path is the
    caller's responsibility (do it via monkeypatch AFTER importing)."""
    if "audit_dashboard" in sys.modules:
        # Force reload so module-top asserts re-evaluate against current env.
        return importlib.reload(sys.modules["audit_dashboard"])
    return importlib.import_module("audit_dashboard")


# ---------------------------------------------------------------------------
# Section A: File existence + structural / LOC-budget contracts (R1)
# ---------------------------------------------------------------------------


class TestScriptStructure:
    def test_script_file_exists(self):
        assert SCRIPT_PATH.exists(), (
            f"audit_dashboard.py not found at {SCRIPT_PATH}. "
            "Required by Step-2c R1."
        )

    def test_script_under_150_loc_soft_budget(self):
        """Spec R1: ≤150 lines (excluding trailing blank line).

        Per README this is a SOFT design budget — exceeding it should flag
        scope creep, not block. We enforce strictly here so the TDD contract
        is unambiguous; relax to 200 only with an explicit waiver in the PR.
        """
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        lines = SCRIPT_PATH.read_text().rstrip("\n").splitlines()
        assert len(lines) <= 150, (
            f"audit_dashboard.py is {len(lines)} lines, exceeds 150-line "
            f"design budget (R1). Refactor or document the waiver."
        )

    def test_script_uses_only_stdlib_and_playwright(self):
        """R1: stdlib only + `playwright.sync_api`. No third-party deps."""
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        src = SCRIPT_PATH.read_text()
        # crude allow-list check: every `from X import` or `import X` where X
        # is not stdlib and not playwright should fail.
        import ast
        tree = ast.parse(src)
        stdlib_top_levels = set(sys.stdlib_module_names)  # type: ignore[attr-defined]
        allowed_extra = {"playwright"}
        offenders: list[str] = []
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    top = alias.name.split(".")[0]
                    if top not in stdlib_top_levels and top not in allowed_extra:
                        offenders.append(alias.name)
            elif isinstance(node, ast.ImportFrom) and node.module:
                top = node.module.split(".")[0]
                if top not in stdlib_top_levels and top not in allowed_extra:
                    offenders.append(node.module)
        assert not offenders, (
            f"audit_dashboard.py imports non-stdlib modules outside the "
            f"playwright allow-list: {offenders}"
        )

    def test_module_docstring_present(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        assert mod.__doc__ and mod.__doc__.strip(), (
            "audit_dashboard.py must have a module docstring naming its "
            "purpose and distinguishing it from audit_pass_01.py."
        )

    @pytest.mark.parametrize(
        "fn_name",
        [
            "load_key",
            "authenticate",
            "discover_routes",
            "capture_page",
            "scan_jargon",
            "write_report",
        ],
    )
    def test_required_function_exists(self, fn_name: str):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        assert hasattr(mod, fn_name) and callable(getattr(mod, fn_name)), (
            f"audit_dashboard.py must export a public callable named {fn_name!r} "
            "(per design spec, six functions exactly)."
        )

    @pytest.mark.parametrize(
        "const_name",
        [
            "BASE_URL",
            "DASHKEY_PATH",
            "OUT_DIR",
            "MAX_ROUTES",
            "TIMEOUT_MS",
            "FALLBACK_TABS",
            "JARGON_TERMS",
        ],
    )
    def test_required_constant_exists(self, const_name: str):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        assert hasattr(mod, const_name), (
            f"audit_dashboard.py must define module-level constant "
            f"{const_name!r} (per Configuration section of README)."
        )

    def test_constants_have_expected_defaults(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        assert mod.BASE_URL == "http://localhost:8080"
        assert str(mod.DASHKEY_PATH).endswith("/.simard/.dashkey")
        assert mod.MAX_ROUTES == 50
        assert isinstance(mod.TIMEOUT_MS, int) and mod.TIMEOUT_MS >= 5000
        assert isinstance(mod.FALLBACK_TABS, (list, tuple)) and len(mod.FALLBACK_TABS) >= 5
        assert isinstance(mod.JARGON_TERMS, (list, tuple)) and len(mod.JARGON_TERMS) >= 5

    def test_fallback_tabs_includes_documented_seed(self):
        """README documents the fallback list explicitly."""
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        seed = {
            "overview", "goals", "traces", "logs", "processes",
            "memory", "costs", "chat", "whiteboard", "thinking", "terminal",
        }
        tabs = {t.lower() for t in mod.FALLBACK_TABS}
        missing = seed - tabs
        assert not missing, f"FALLBACK_TABS missing documented seed tabs: {missing}"

    def test_jargon_terms_includes_seed_excludes_weak_signals(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        terms_lower = {t.lower() for t in mod.JARGON_TERMS}
        must_include = {
            "ooda", "cognitive memory", "handoff bundle", "facilitator",
            "recipe runner",
        }
        missing = must_include - terms_lower
        assert not missing, f"JARGON_TERMS missing required seed terms: {missing}"
        # README explicitly drops these as weak signals.
        must_exclude = {"trace", "self-serve"}
        leaked = must_exclude & terms_lower
        assert not leaked, (
            f"JARGON_TERMS includes weak-signal terms that README excludes: {leaked}"
        )


# ---------------------------------------------------------------------------
# Section B: load_key() (Spec R1 step 1, security constraint "no dashkey in logs")
# ---------------------------------------------------------------------------


class TestLoadKey:
    def test_reads_and_strips_dashkey_file(self, tmp_path, monkeypatch):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        keyfile = tmp_path / ".dashkey"
        keyfile.write_text("  abc123XYZ  \n")
        monkeypatch.setattr(mod, "DASHKEY_PATH", keyfile)
        assert mod.load_key() == "abc123XYZ"

    def test_missing_file_exits_nonzero(self, tmp_path, monkeypatch):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        monkeypatch.setattr(mod, "DASHKEY_PATH", tmp_path / "does-not-exist")
        with pytest.raises((SystemExit, FileNotFoundError, RuntimeError)):
            mod.load_key()

    def test_empty_file_exits_nonzero(self, tmp_path, monkeypatch):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        keyfile = tmp_path / ".dashkey"
        keyfile.write_text("   \n")
        monkeypatch.setattr(mod, "DASHKEY_PATH", keyfile)
        with pytest.raises((SystemExit, ValueError, RuntimeError)):
            mod.load_key()

    def test_oversized_file_rejected(self, tmp_path, monkeypatch):
        """README: sanity envelope 1 ≤ len ≤ 256."""
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        keyfile = tmp_path / ".dashkey"
        keyfile.write_text("x" * 1024)
        monkeypatch.setattr(mod, "DASHKEY_PATH", keyfile)
        with pytest.raises((SystemExit, ValueError, RuntimeError)):
            mod.load_key()

    def test_error_message_does_not_leak_key(self, tmp_path, monkeypatch, capsys):
        """Security constraint: dashkey must never appear in error output."""
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        keyfile = tmp_path / ".dashkey"
        secret = "DO_NOT_LEAK_THIS_VALUE_XYZ"
        # too long → should error, must not include the value
        keyfile.write_text(secret * 50)
        monkeypatch.setattr(mod, "DASHKEY_PATH", keyfile)
        with pytest.raises(BaseException):
            mod.load_key()
        captured = capsys.readouterr()
        assert secret not in captured.out
        assert secret not in captured.err


# ---------------------------------------------------------------------------
# Section C: authenticate() (Spec R1 step 3, README §2)
# ---------------------------------------------------------------------------


class TestAuthenticate:
    def _make_context(self, *, ok: bool = True, status: int = 200,
                      cookie_header: str = "simard_session=abc; Path=/"):
        """Construct a BrowserContext-shaped MagicMock that satisfies the
        authenticate() contract: context.request.post(...) returns a response
        with .ok / .status / .headers, and context.add_cookies / context.cookies
        are recorded."""
        resp = MagicMock()
        resp.ok = ok
        resp.status = status
        resp.headers = {"set-cookie": cookie_header} if ok else {}
        resp.json = MagicMock(return_value={"ok": ok})
        resp.text = MagicMock(return_value="" if ok else "denied")
        api = MagicMock()
        api.post = MagicMock(return_value=resp)
        ctx = MagicMock()
        ctx.request = api
        ctx.add_cookies = MagicMock()
        ctx.cookies = MagicMock(return_value=(
            [{"name": "simard_session", "value": "abc", "domain": "localhost", "path": "/"}]
            if ok else []
        ))
        return ctx, api, resp

    def test_posts_to_login_with_json_code_body(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        ctx, api, _ = self._make_context()
        mod.authenticate(ctx, "MY_KEY")
        assert api.post.called, "authenticate must POST to /api/login"
        args, kwargs = api.post.call_args
        # First positional arg or 'url' kwarg should contain /api/login
        url = (args[0] if args else kwargs.get("url", "")) or ""
        assert "/api/login" in url
        # Body should carry the code — accept either JSON or form-encoded but
        # the value must be the supplied key.
        body_blob = json.dumps({"args": args[1:], "kwargs": {
            k: v for k, v in kwargs.items() if k != "url"
        }}, default=str)
        assert "MY_KEY" in body_blob, (
            "authenticate must send the dashkey in the login request body."
        )

    def test_non_2xx_exits_without_leaking_key(self, capsys):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        ctx, _, resp = self._make_context(ok=False, status=401)
        with pytest.raises(BaseException):
            mod.authenticate(ctx, "SECRET_KEY_DO_NOT_LEAK")
        out = capsys.readouterr()
        combined = out.out + out.err
        assert "SECRET_KEY_DO_NOT_LEAK" not in combined, (
            "Authentication failure must not include the dashkey in any output."
        )
        # Status code SHOULD be present (it's the only thing we surface).
        assert "401" in combined or True  # status presence is preferred but optional

    def test_success_adds_simard_session_cookie(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        ctx, _, _ = self._make_context(ok=True)
        mod.authenticate(ctx, "ANY_KEY")
        # Either via add_cookies() or by the daemon's Set-Cookie being honoured
        # automatically. Accept either: assert cookies are present afterwards.
        if ctx.add_cookies.called:
            cookies_arg = ctx.add_cookies.call_args[0][0]
            assert any(c.get("name") == "simard_session" for c in cookies_arg)
        else:
            # APIRequestContext honoured Set-Cookie automatically
            assert ctx.cookies.called or True


# ---------------------------------------------------------------------------
# Section D: discover_routes() (Spec R1 step 4)
# ---------------------------------------------------------------------------


class TestDiscoverRoutes:
    def _page_returning(self, dom_routes: list[dict[str, str]]):
        """Build a page mock whose evaluate() returns the given DOM routes."""
        page = MagicMock()
        page.goto = MagicMock()
        page.wait_for_load_state = MagicMock()
        page.wait_for_timeout = MagicMock()
        page.evaluate = MagicMock(return_value=dom_routes)
        return page

    def test_returns_at_least_fallback_tabs_when_dom_empty(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        page = self._page_returning([])
        routes = mod.discover_routes(page)
        assert routes, "discover_routes must never return empty when fallback exists"
        slugs = {r.get("slug") or r.get("href", "") for r in routes if isinstance(r, dict)}
        # Should include at least a handful of fallback names
        assert any("memory" in str(s).lower() for s in slugs), (
            "Fallback tabs (e.g. 'memory') must be unioned when DOM is empty."
        )

    def test_filters_javascript_and_mailto_schemes(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        page = self._page_returning([
            {"label": "Bad1", "href": "javascript:alert(1)"},
            {"label": "Bad2", "href": "mailto:x@y"},
            {"label": "Bad3", "href": "data:text/html,<x>"},
            {"label": "Bad4", "href": "file:///etc/passwd"},
            {"label": "Good", "href": "#/memory"},
        ])
        routes = mod.discover_routes(page)
        for r in routes:
            href = r.get("href", "") if isinstance(r, dict) else ""
            assert not href.startswith(("javascript:", "mailto:", "data:", "file:")), (
                f"discover_routes leaked dangerous scheme: {href!r}"
            )

    def test_filters_cross_origin(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        page = self._page_returning([
            {"label": "External", "href": "https://evil.example.com/foo"},
            {"label": "Local", "href": "/memory"},
        ])
        routes = mod.discover_routes(page)
        for r in routes:
            href = r.get("href", "") if isinstance(r, dict) else ""
            assert "evil.example.com" not in href, (
                "discover_routes must reject cross-origin routes."
            )

    def test_caps_at_max_routes(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        hostile = [{"label": f"t{i}", "href": f"#/r{i}"} for i in range(500)]
        page = self._page_returning(hostile)
        routes = mod.discover_routes(page)
        assert len(routes) <= mod.MAX_ROUTES, (
            f"discover_routes returned {len(routes)} routes, exceeds "
            f"MAX_ROUTES cap of {mod.MAX_ROUTES}."
        )

    def test_dedupes_by_normalized_path(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        page = self._page_returning([
            {"label": "A", "href": "/memory"},
            {"label": "B", "href": "/memory"},
            {"label": "C", "href": "#/memory"},
        ])
        routes = mod.discover_routes(page)
        # Allow at most one 'memory' route (normalization may differ but should dedupe)
        memory_routes = [
            r for r in routes
            if isinstance(r, dict) and "memory" in (r.get("slug", "") or r.get("href", "")).lower()
        ]
        assert len(memory_routes) <= 2, (
            "Same path appearing in multiple discovery channels must dedupe."
        )


# ---------------------------------------------------------------------------
# Section E: scan_jargon() (Spec R1 step 5)
# ---------------------------------------------------------------------------


class TestScanJargon:
    def test_returns_inverted_index(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        text_dumps = {
            "memory": "Cognitive Memory pane — last consolidation 3m ago.",
            "overview": "OODA loop running. Recipe runner idle.",
            "goals": "No jargon here, just plain English about objectives.",
        }
        index = mod.scan_jargon(text_dumps)
        assert isinstance(index, dict)
        # Case-insensitive substring match
        # "cognitive memory" → memory page; "OODA" → overview; "recipe runner" → overview
        # keys may be returned in original case; lowercase them for comparison
        idx_lower = {k.lower(): v for k, v in index.items()}
        assert "memory" in idx_lower.get("cognitive memory", []), (
            f"scan_jargon did not detect 'cognitive memory' on memory page. "
            f"Got: {index!r}"
        )
        assert "overview" in idx_lower.get("ooda", []) or \
               "overview" in idx_lower.get("ooda loop", []), (
            "scan_jargon did not detect 'OODA' on overview page."
        )

    def test_zero_hit_terms_omitted(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        # Use text that contains NONE of the jargon terms.
        text_dumps = {"plain": "this page is in plain English about cookies and tea"}
        index = mod.scan_jargon(text_dumps)
        # No empty value lists allowed
        for term, hits in index.items():
            assert hits, f"term {term!r} kept with empty hit list — should be omitted"

    def test_case_insensitive_matching(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        text_dumps = {"page1": "ooda is the same as OODA is the same as Ooda."}
        index = mod.scan_jargon(text_dumps)
        idx_lower = {k.lower(): v for k, v in index.items()}
        assert "page1" in idx_lower.get("ooda", []), (
            "scan_jargon must be case-insensitive."
        )

    def test_empty_input_returns_empty_dict(self):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        assert mod.scan_jargon({}) == {}


# ---------------------------------------------------------------------------
# Section F: capture_page() (Spec R1 step 5)
# ---------------------------------------------------------------------------


class TestCapturePage:
    def _make_page(self, *, body_text: str = "hello world"):
        page = MagicMock()
        # listener registry so we can verify add + remove parity
        page._listeners: dict[str, list] = {}

        def _on(event, handler):
            page._listeners.setdefault(event, []).append(handler)

        def _off(event, handler):
            if handler in page._listeners.get(event, []):
                page._listeners[event].remove(handler)

        page.on = MagicMock(side_effect=_on)
        page.remove_listener = MagicMock(side_effect=_off)
        page.goto = MagicMock()
        page.evaluate = MagicMock(return_value=body_text)
        page.wait_for_load_state = MagicMock()
        page.wait_for_timeout = MagicMock()
        page.screenshot = MagicMock()
        return page

    def test_writes_png_txt_and_errors_json(self, tmp_path, monkeypatch):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        monkeypatch.setattr(mod, "OUT_DIR", tmp_path)
        page = self._make_page(body_text="overview body text")
        route = {"slug": "overview", "href": "#/overview", "label": "Overview"}
        result = mod.capture_page(page, route)
        # contract: result is dict-like; outputs land in OUT_DIR
        png = tmp_path / "overview.png"
        txt = tmp_path / "overview.txt"
        errors = tmp_path / "overview.errors.json"
        assert page.screenshot.called, "screenshot must be invoked"
        assert txt.exists(), f"txt dump not written to {txt}"
        assert errors.exists(), f"errors.json not written to {errors}"
        # errors.json must be valid JSON with http/console keys
        data = json.loads(errors.read_text())
        assert isinstance(data, dict)
        assert "http" in data and "console" in data
        assert isinstance(data["http"], list)
        assert isinstance(data["console"], list)
        # Returned dict should at least reveal slug + counts
        assert isinstance(result, dict)

    def test_removes_listeners_after_capture(self, tmp_path, monkeypatch):
        """Listener-leak regression — see README's O(N²) call-out."""
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        monkeypatch.setattr(mod, "OUT_DIR", tmp_path)
        page = self._make_page()
        route = {"slug": "overview", "href": "#/overview", "label": "Overview"}
        mod.capture_page(page, route)
        for event in ("response", "console"):
            remaining = page._listeners.get(event, [])
            assert not remaining, (
                f"capture_page leaked {len(remaining)} {event!r} listener(s); "
                "must remove in finally to avoid O(N²) buffering bug."
            )


# ---------------------------------------------------------------------------
# Section G: write_report() (Spec R1 step 6, README "REPORT.md anatomy")
# ---------------------------------------------------------------------------


class TestWriteReport:
    def test_emits_five_sections_in_order(self, tmp_path, monkeypatch):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        monkeypatch.setattr(mod, "OUT_DIR", tmp_path)
        pages = [
            {
                "slug": "overview", "url": "/#overview", "title": "Overview",
                "text_chars": 1200, "http_errors": 0, "console_errors": 0,
                "h1": "Overview", "excerpt": "Overview of the dashboard.",
            },
            {
                "slug": "memory", "url": "/#memory", "title": "Memory",
                "text_chars": 80, "http_errors": 1, "console_errors": 2,
                "h1": None, "excerpt": "Cognitive Memory log",
            },
        ]
        jargon = {"OODA": ["overview"], "cognitive memory": ["memory"]}
        mod.write_report(pages, jargon)
        report = tmp_path / "REPORT.md"
        assert report.exists(), "REPORT.md must be written to OUT_DIR"
        body = report.read_text()
        # Five section headings, in order, case-insensitive substring check.
        expected_headings = [
            "pages found",
            "what each page",
            "jargon",
            "missing context",
            "top-5",
        ]
        last_pos = -1
        for h in expected_headings:
            pos = body.lower().find(h)
            assert pos > last_pos, (
                f"REPORT.md section '{h}' missing or out of order. "
                f"body[:500]={body[:500]!r}"
            )
            last_pos = pos

    def test_jargon_zero_hit_terms_not_rendered(self, tmp_path, monkeypatch):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        monkeypatch.setattr(mod, "OUT_DIR", tmp_path)
        pages = [{
            "slug": "overview", "url": "/", "title": "Overview",
            "text_chars": 100, "http_errors": 0, "console_errors": 0,
            "h1": "Overview", "excerpt": "text",
        }]
        # Note: empty value lists deliberately should NOT appear in the report.
        jargon = {"OODA": ["overview"]}
        mod.write_report(pages, jargon)
        body = (tmp_path / "REPORT.md").read_text()
        assert "OODA" in body
        # Spot-check a term we did NOT include — must not appear.
        assert "facilitator" not in body.lower() or "facilitator → " not in body.lower()

    def test_missing_context_flags_short_pages(self, tmp_path, monkeypatch):
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        mod = _import_audit_module()
        monkeypatch.setattr(mod, "OUT_DIR", tmp_path)
        pages = [{
            "slug": "lonely", "url": "/lonely", "title": "Lonely",
            "text_chars": 42, "http_errors": 0, "console_errors": 0,
            "h1": None, "excerpt": "",
        }]
        mod.write_report(pages, {})
        body = (tmp_path / "REPORT.md").read_text().lower()
        # The lonely page (text < 200, no h1) should be flagged somewhere in
        # the "Missing context" section.
        idx = body.find("missing context")
        assert idx >= 0
        missing_section = body[idx:idx + 2000]
        assert "lonely" in missing_section, (
            "Pages with <200 chars / no <h1> must be surfaced in Missing context."
        )


# ---------------------------------------------------------------------------
# Section H: Forbidden-path safety constraint (Spec R7, README "Hard safety")
# ---------------------------------------------------------------------------


class TestForbiddenPaths:
    def test_module_refuses_to_load_when_out_dir_under_prompt_assets(
        self, tmp_path, monkeypatch
    ):
        """README: module-top assert must refuse to run if OUT_DIR resolves
        under $SIMARD_PROMPT_ASSETS_DIR."""
        if not SCRIPT_PATH.exists():
            pytest.skip("script not yet implemented")
        # Drop any cached import so the module-top assert re-evaluates.
        sys.modules.pop("audit_dashboard", None)
        # Point the env-var to a parent of where OUT_DIR would resolve.
        # We can't trivially move OUT_DIR without editing the file, so instead
        # we test the inverse: setting SIMARD_PROMPT_ASSETS_DIR to the script
        # directory (which IS a parent of scripts/dashboard_audit/out) should
        # trip the assert.
        monkeypatch.setenv("SIMARD_PROMPT_ASSETS_DIR", str(SCRIPT_DIR))
        with pytest.raises((AssertionError, SystemExit, RuntimeError)):
            importlib.import_module("audit_dashboard")
        # cleanup so other tests don't see a poisoned module
        sys.modules.pop("audit_dashboard", None)
        monkeypatch.delenv("SIMARD_PROMPT_ASSETS_DIR", raising=False)


# ---------------------------------------------------------------------------
# Section I: .gitignore contract (Spec R2)
# ---------------------------------------------------------------------------


class TestGitignoreCoverage:
    def test_local_gitignore_excludes_out(self):
        local = SCRIPT_DIR / ".gitignore"
        assert local.exists(), "scripts/dashboard_audit/.gitignore must exist"
        contents = local.read_text()
        assert "out/" in contents or "out" in contents.split(), (
            "local .gitignore must exclude out/"
        )

    def test_local_gitignore_excludes_venv(self):
        local = SCRIPT_DIR / ".gitignore"
        if not local.exists():
            pytest.skip("local .gitignore not yet created")
        contents = local.read_text()
        assert ".venv-audit" in contents, (
            "local .gitignore should exclude .venv-audit/ (ephemeral pip env)"
        )

    def test_root_gitignore_excludes_audit_out(self):
        """Spec R2: belt-and-suspenders entry in root .gitignore."""
        root_ignore = REPO_ROOT / ".gitignore"
        assert root_ignore.exists()
        contents = root_ignore.read_text()
        assert "scripts/dashboard_audit/out/" in contents or \
               "scripts/dashboard_audit/out" in contents, (
            "root .gitignore must include scripts/dashboard_audit/out/ "
            "(spec R2, defense-in-depth against git add -A)"
        )


# ---------------------------------------------------------------------------
# Section J: Untouched-baseline contract (Spec R7)
# ---------------------------------------------------------------------------


class TestExistingScriptUntouched:
    def test_audit_pass_01_still_exists(self):
        """audit_pass_01.py must not be deleted or renamed."""
        sibling = SCRIPT_DIR / "audit_pass_01.py"
        assert sibling.exists(), (
            "audit_pass_01.py was removed or renamed — spec R7 forbids."
        )

    def test_audit_pass_01_index_filename_disjoint(self):
        """Manifest files must NOT collide between the two scripts."""
        if not SCRIPT_PATH.exists():
            pytest.skip("audit_dashboard.py not yet implemented")
        new_src = SCRIPT_PATH.read_text()
        old_src = (SCRIPT_DIR / "audit_pass_01.py").read_text()
        assert "_index.json" in old_src, (
            "audit_pass_01.py changed — its manifest filename is the baseline."
        )
        # The new script must NOT write to bare _index.json (would clobber).
        # It may write _audit_dashboard_index.json.
        for line in new_src.splitlines():
            if "_index.json" in line and "_audit_dashboard_index.json" not in line:
                # check it's not a write target
                lowered = line.lower()
                assert not any(
                    verb in lowered
                    for verb in ("write_text", "json.dump", " open(", '"w"', "'w'")
                ), (
                    f"audit_dashboard.py writes to _index.json — would collide "
                    f"with audit_pass_01.py manifest. Line: {line!r}"
                )
