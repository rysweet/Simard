"""Pytest fixtures for the dashboard tab-identity smoke test.

The fixtures here log in once per test session by reading the dashboard
login code from ``~/.simard/.dashkey`` (or the ``SIMARD_DASHKEY``
environment variable) and POSTing it to ``/api/login`` as a JSON body —
matching the actual ``operator_commands_dashboard::auth::login`` route
handler, which deserialises ``code`` from JSON.

The resulting ``simard_session`` cookie is added to the Playwright browser
context so every test runs authenticated. No credentials are echoed to
stdout or to the evidence table.
"""

from __future__ import annotations

import os
import pathlib
from urllib.parse import urlparse

import pytest
import requests
from playwright.sync_api import BrowserContext, Page, Playwright


DEFAULT_URL = os.environ.get("SIMARD_DASHBOARD_URL", "http://localhost:8080")


def _read_dashkey() -> str:
    """Read the dashboard login code.

    Resolution order:
      1. ``SIMARD_DASHKEY`` environment variable (used in CI).
      2. ``~/.simard/.dashkey`` file (matches the path used by
         ``operator_commands_dashboard::auth::dashkey_path``).
    """
    env = os.environ.get("SIMARD_DASHKEY")
    if env:
        return env.strip()
    home = pathlib.Path(os.environ.get("HOME", "~")).expanduser()
    keyfile = home / ".simard" / ".dashkey"
    if not keyfile.is_file():
        raise RuntimeError(
            f"dashkey not found: set SIMARD_DASHKEY or create {keyfile}"
        )
    return keyfile.read_text().strip()


@pytest.fixture(scope="session")
def dashboard_url() -> str:
    return DEFAULT_URL.rstrip("/")


@pytest.fixture(scope="session")
def session_cookie(dashboard_url: str) -> dict[str, str]:
    """Authenticate against ``/api/login`` and return the session cookie."""
    code = _read_dashkey()
    resp = requests.post(
        f"{dashboard_url}/api/login",
        json={"code": code},
        timeout=10,
    )
    if resp.status_code != 200:
        raise RuntimeError(
            f"dashboard login failed: HTTP {resp.status_code} — "
            f"{resp.text[:200]}"
        )
    cookie_value = resp.cookies.get("simard_session")
    if not cookie_value:
        raise RuntimeError(
            "login response did not set the simard_session cookie"
        )
    parsed = urlparse(dashboard_url)
    return {
        "name": "simard_session",
        "value": cookie_value,
        "domain": parsed.hostname or "localhost",
        "path": "/",
    }


@pytest.fixture
def authed_context(
    playwright: Playwright,
    session_cookie: dict[str, str],
) -> BrowserContext:
    """Browser context with the dashboard session cookie already set."""
    browser = playwright.chromium.launch(headless=True)
    context = browser.new_context()
    context.add_cookies([session_cookie])
    yield context
    context.close()
    browser.close()


@pytest.fixture
def page(authed_context: BrowserContext) -> Page:
    page = authed_context.new_page()
    yield page
    page.close()
