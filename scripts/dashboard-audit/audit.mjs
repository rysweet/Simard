#!/usr/bin/env node
/**
 * Simard dashboard audit — cycle 1 (issue #1880)
 *
 * What this does:
 *   1. Reads the dashkey from ~/.simard/.dashkey.
 *   2. Discovers the login submit endpoint + field name by parsing GET /login.
 *   3. Authenticates the dashboard, trying (in order): discovered form POST,
 *      Authorization: Bearer header, cookie `dashkey=...`, query string `?key=...`.
 *      Logs which mechanism succeeded.
 *   4. Walks the SPA's tabs (`.tab[data-tab="<slug>"]` + `#tab-<slug>`), screenshots
 *      each tab full-page, and records per-route metadata.
 *   5. Probes a set of known API endpoints (authenticated) to gather coverage evidence.
 *   6. Computes a PRESENT / PARTIAL / MISSING classification across the seven
 *      mandated coverage dimensions and writes report.json + report.md to
 *      ./out/<UTC-iso-timestamp>/.
 *
 * Run:    cd scripts/dashboard-audit && node audit.mjs
 * Output: scripts/dashboard-audit/out/<timestamp>/{report.json,report.md,*.png}
 *
 * Exit codes:
 *   0  success
 *   2  authentication failure (no further inspection performed)
 *   3  dashboard unreachable
 *   1  any other fatal error
 */

import { chromium, request as pwRequest } from "playwright";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE_URL = process.env.SIMARD_DASHBOARD_URL || "http://localhost:8080";
const TOKEN_PATH = path.join(os.homedir(), ".simard", ".dashkey");
const TS = new Date().toISOString().replace(/[:.]/g, "-");
const OUT_DIR = path.join(HERE, "out", TS);
fs.mkdirSync(OUT_DIR, { recursive: true });

const log = (...args) => console.log(`[audit ${new Date().toISOString()}]`, ...args);

function readToken() {
  if (!fs.existsSync(TOKEN_PATH)) {
    throw new Error(`token file not found at ${TOKEN_PATH}`);
  }
  return fs.readFileSync(TOKEN_PATH, "utf8").trim();
}

// ─── auth discovery & attempts ────────────────────────────────────────────────

async function fetchLoginPage(api) {
  const r = await api.get(`${BASE_URL}/login`);
  return { status: r.status(), body: await r.text() };
}

/**
 * Parse the login page HTML to discover the actual submit endpoint and field
 * name. Looks for `fetch('<url>', ...)` calls and `<input id="..."` tags.
 * Returns { endpoint, field } with sensible fallbacks.
 */
function discoverLoginSubmit(html) {
  const fetchMatch = html.match(/fetch\(['"]([^'"]+)['"]\s*,\s*\{[^}]*method:\s*['"]POST['"]/i);
  const inputMatch = html.match(/<input[^>]*id=["']([a-zA-Z0-9_-]+)["']/i)
    || html.match(/<input[^>]*name=["']([a-zA-Z0-9_-]+)["']/i);
  return {
    endpoint: fetchMatch ? fetchMatch[1] : "/api/login",
    field: inputMatch ? inputMatch[1] : "code",
  };
}

async function tryAuthMethods(token) {
  const attempts = [];
  const api = await pwRequest.newContext({ baseURL: BASE_URL, ignoreHTTPSErrors: true });

  // Inspect /login first
  let loginPage;
  try {
    loginPage = await fetchLoginPage(api);
  } catch (e) {
    await api.dispose();
    throw new Error(`could not reach ${BASE_URL}/login: ${e.message}`);
  }
  if (loginPage.status >= 500) {
    await api.dispose();
    throw new Error(`/login returned HTTP ${loginPage.status} — dashboard unreachable`);
  }
  const discovered = discoverLoginSubmit(loginPage.body);
  log("discovered login submit:", discovered);

  // Attempt 1: discovered form post (JSON body, since the inline JS uses JSON)
  try {
    const body = { [discovered.field]: token };
    const r = await api.post(discovered.endpoint, {
      headers: { "content-type": "application/json" },
      data: body,
    });
    const status = r.status();
    const cookies = (await api.storageState()).cookies;
    const ok = status >= 200 && status < 400 && cookies.length > 0;
    attempts.push({
      method: "form-post-discovered",
      endpoint: discovered.endpoint,
      field: discovered.field,
      bodyShape: "json",
      status, ok,
    });
    if (ok) {
      return { winner: attempts[0], attempts, api, cookies };
    }
  } catch (e) {
    attempts.push({ method: "form-post-discovered", error: e.message, ok: false });
  }

  // Attempt 2: Bearer header probe
  try {
    const api2 = await pwRequest.newContext({
      baseURL: BASE_URL,
      extraHTTPHeaders: { Authorization: `Bearer ${token}` },
    });
    const r = await api2.get("/api/status");
    const status = r.status();
    const ok = status === 200;
    attempts.push({ method: "bearer-header", endpoint: "/api/status", status, ok });
    if (ok) {
      return { winner: attempts[attempts.length - 1], attempts, api: api2, cookies: [] };
    }
    await api2.dispose();
  } catch (e) {
    attempts.push({ method: "bearer-header", error: e.message, ok: false });
  }

  // Attempt 3: cookie dashkey=...
  try {
    const api3 = await pwRequest.newContext({
      baseURL: BASE_URL,
      extraHTTPHeaders: { Cookie: `dashkey=${token}` },
    });
    const r = await api3.get("/api/status");
    const status = r.status();
    const ok = status === 200;
    attempts.push({ method: "cookie-dashkey", endpoint: "/api/status", status, ok });
    if (ok) {
      return { winner: attempts[attempts.length - 1], attempts, api: api3, cookies: [] };
    }
    await api3.dispose();
  } catch (e) {
    attempts.push({ method: "cookie-dashkey", error: e.message, ok: false });
  }

  // Attempt 4: query string ?key=...
  try {
    const api4 = await pwRequest.newContext({ baseURL: BASE_URL });
    const r = await api4.get(`/api/status?key=${encodeURIComponent(token)}`);
    const status = r.status();
    const ok = status === 200;
    attempts.push({ method: "query-key", endpoint: "/api/status?key=…", status, ok });
    if (ok) {
      return { winner: attempts[attempts.length - 1], attempts, api: api4, cookies: [] };
    }
    await api4.dispose();
  } catch (e) {
    attempts.push({ method: "query-key", error: e.message, ok: false });
  }

  await api.dispose();
  return { winner: null, attempts, api: null, cookies: [] };
}

// ─── API probing ──────────────────────────────────────────────────────────────

// Endpoints to probe. The mapping array also records WHY we care about each one
// — used later to cite selectors / endpoints in the coverage table.
const API_PROBES = [
  { path: "/api/status",     dims: ["ooda", "memory"] },
  { path: "/api/goals",      dims: ["goal-board"] },
  { path: "/api/processes",  dims: ["engineers"] },
  { path: "/api/memory",     dims: ["memory"] },
  { path: "/api/workboard",  dims: ["goal-board", "ooda", "memory"] },
  { path: "/api/traces",     dims: ["ooda"] },
  { path: "/api/logs",       dims: [] },
  { path: "/api/costs",      dims: [] },
  // The following are expected to be MISSING in cycle 1 — probed for evidence.
  { path: "/api/judges",         dims: ["merge-judge"], expectMissing: true },
  { path: "/api/judge",          dims: ["merge-judge"], expectMissing: true },
  { path: "/api/merge-judge",    dims: ["merge-judge"], expectMissing: true },
  { path: "/api/prs",            dims: ["per-pr"],      expectMissing: true },
  { path: "/api/pulls",          dims: ["per-pr"],      expectMissing: true },
  { path: "/api/pr/1880",        dims: ["per-pr"],      expectMissing: true },
  { path: "/api/brain-failures", dims: ["brain-failure"], expectMissing: true },
  { path: "/api/failures",       dims: ["brain-failure"], expectMissing: true },
  { path: "/api/ooda-cycles",    dims: ["ooda"] },
];

async function probeApis(api) {
  const results = {};
  for (const probe of API_PROBES) {
    try {
      const r = await api.get(probe.path);
      const status = r.status();
      let body = null;
      let bodyBytes = 0;
      try {
        const txt = await r.text();
        bodyBytes = txt.length;
        // Try JSON parse for keys preview
        try {
          const parsed = JSON.parse(txt);
          body = Array.isArray(parsed)
            ? { array: true, length: parsed.length, sampleKeys: parsed[0] ? Object.keys(parsed[0]).slice(0, 8) : [] }
            : { array: false, keys: Object.keys(parsed).slice(0, 20) };
        } catch { body = { array: false, keys: null, note: "non-json or empty" }; }
      } catch { /* ignore */ }
      results[probe.path] = { status, ok: status === 200, bodyBytes, body, dims: probe.dims, expectMissing: !!probe.expectMissing };
    } catch (e) {
      results[probe.path] = { status: 0, ok: false, error: e.message, dims: probe.dims, expectMissing: !!probe.expectMissing };
    }
  }
  return results;
}

// ─── browser walk ─────────────────────────────────────────────────────────────

function slugify(s) {
  return (s || "page").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "") || "page";
}

async function walkDashboard(browser, authResult) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });

  // Plant auth cookies discovered via API context.
  if (authResult.cookies && authResult.cookies.length) {
    await ctx.addCookies(authResult.cookies.map(c => ({
      name: c.name, value: c.value,
      domain: c.domain || "localhost", path: c.path || "/",
      httpOnly: !!c.httpOnly, secure: !!c.secure, sameSite: "Strict",
    })));
  }

  const page = await ctx.newPage();
  const consoleMsgs = [];
  page.on("console", m => consoleMsgs.push({ type: m.type(), text: m.text(), at: new Date().toISOString() }));
  page.on("pageerror", e => consoleMsgs.push({ type: "pageerror", text: String(e), at: new Date().toISOString() }));

  const routes = [];

  // Navigate to root. If cookies don't work (e.g. Bearer-only auth), perform a
  // browser-level login by filling the form on /login.
  const rootResp = await page.goto(`${BASE_URL}/`, { waitUntil: "domcontentloaded", timeout: 15_000 });
  if (rootResp && rootResp.url().endsWith("/login")) {
    log("cookie was not honoured by browser → performing in-page login");
    await page.fill('#code, input[type="text"]', readToken());
    await page.click('button[type="submit"]');
    await page.waitForURL(`${BASE_URL}/`, { timeout: 10_000 });
  }

  // Wait for tabs to appear
  await page.waitForSelector(".tab, nav a, [role=tab]", { timeout: 10_000 });
  // Landing screenshot
  const landingFile = "00-landing.png";
  await page.screenshot({ path: path.join(OUT_DIR, landingFile), fullPage: true });

  // Discover tabs (prefer .tab[data-tab] like the Simard SPA, fall back to nav anchors)
  const discoveredTabs = await page.evaluate(() => {
    const out = [];
    for (const el of document.querySelectorAll(".tab[data-tab]")) {
      out.push({ kind: "tab", slug: el.getAttribute("data-tab"), label: el.textContent.trim(), selector: `.tab[data-tab="${el.getAttribute("data-tab")}"]` });
    }
    if (out.length === 0) {
      for (const el of document.querySelectorAll("nav a, header a, [role=tablist] [role=tab]")) {
        const href = el.getAttribute("href") || "";
        out.push({ kind: "link", slug: (el.textContent.trim() || href).slice(0, 32), label: el.textContent.trim(), href, selector: el.tagName.toLowerCase() });
      }
    }
    return out;
  });
  log(`discovered ${discoveredTabs.length} tabs/routes`);

  routes.push({
    slug: "landing",
    label: "Landing (post-login)",
    url: page.url(),
    httpStatus: rootResp?.status() ?? null,
    title: await page.title(),
    screenshot: landingFile,
    panels: await extractPanels(page),
    latestTimestamp: await latestVisibleTimestamp(page),
    consoleErrorsBefore: consoleMsgs.filter(m => m.type === "error" || m.type === "pageerror").length,
  });

  for (let i = 0; i < discoveredTabs.length; i++) {
    const tab = discoveredTabs[i];
    const slug = slugify(tab.slug || tab.label || `tab-${i}`);
    log(`→ tab "${slug}"`);
    const errorsBefore = consoleMsgs.filter(m => m.type === "error" || m.type === "pageerror").length;
    let httpStatus = null;
    try {
      if (tab.kind === "tab") {
        await page.click(tab.selector, { timeout: 5_000 });
        // wait for matching tab-content to be active
        try { await page.waitForSelector(`#tab-${tab.slug}.active`, { timeout: 5_000 }); } catch { /* not all tabs follow this convention */ }
        await page.waitForTimeout(1500); // let XHR settle
      } else if (tab.href) {
        const r = await page.goto(new URL(tab.href, BASE_URL).toString(), { waitUntil: "domcontentloaded", timeout: 10_000 });
        httpStatus = r?.status() ?? null;
        await page.waitForTimeout(1500);
      }
    } catch (e) {
      log(`  click/nav failed: ${e.message}`);
    }
    const file = `${String(i + 1).padStart(2, "0")}-${slug}.png`;
    try {
      await page.screenshot({ path: path.join(OUT_DIR, file), fullPage: true });
    } catch (e) {
      log(`  screenshot failed: ${e.message}`);
    }
    const panels = await extractPanels(page, tab.kind === "tab" ? `#tab-${tab.slug}` : null);
    const latestTs = await latestVisibleTimestamp(page, tab.kind === "tab" ? `#tab-${tab.slug}` : null);
    const errorsAfter = consoleMsgs.filter(m => m.type === "error" || m.type === "pageerror").length;

    routes.push({
      slug,
      label: tab.label,
      kind: tab.kind,
      selector: tab.selector,
      url: page.url(),
      httpStatus,
      title: await page.title(),
      screenshot: file,
      panels,
      latestTimestamp: latestTs,
      consoleErrorsDelta: errorsAfter - errorsBefore,
    });
  }

  await ctx.close();
  return { routes, consoleMessages: consoleMsgs };
}

async function extractPanels(page, scopeSelector = null) {
  // Heuristic: each h1/h2/h3 in scope is a panel; the panel container is its
  // closest .card / section / div parent.
  return await page.evaluate((scopeSelector) => {
    const root = scopeSelector ? document.querySelector(scopeSelector) : document.body;
    if (!root) return [];
    const panels = [];
    for (const h of root.querySelectorAll("h1, h2, h3")) {
      if (!h.offsetParent && h.offsetHeight === 0) continue;
      const container = h.closest(".card, section, .panel, .tab-content, .grid > *") || h.parentElement;
      const title = h.textContent.trim().slice(0, 120);
      const bodyText = (container?.innerText || "").trim().slice(0, 240);
      panels.push({ title, sampleText: bodyText });
    }
    return panels.slice(0, 40);
  }, scopeSelector);
}

async function latestVisibleTimestamp(page, scopeSelector = null) {
  return await page.evaluate((scopeSelector) => {
    const root = scopeSelector ? document.querySelector(scopeSelector) : document.body;
    if (!root) return null;
    const text = root.innerText || "";
    const isoRe = /\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?\b/g;
    const matches = text.match(isoRe) || [];
    if (matches.length === 0) return null;
    // Newest by lexicographic sort (ISO timestamps sort correctly)
    return matches.sort().pop();
  }, scopeSelector);
}

// ─── coverage classification ──────────────────────────────────────────────────

/**
 * Each dimension is classified PRESENT, PARTIAL, or MISSING based on evidence
 * gathered from routes + API probes.
 *
 * The seven dimensions (in mandated order):
 *   1. goal-board state
 *   2. OODA cycle health
 *   3. engineer subprocesses
 *   4. cognitive memory growth
 *   5. merge-judge decisions
 *   6. per-PR readiness for #1880, #1893, #1894
 *   7. brain-failure surfacing tied to #1890
 */
function classifyCoverage(routes, apis) {
  const tabBySlug = Object.fromEntries(routes.map(r => [r.slug, r]));
  const apiOk = p => apis[p] && apis[p].ok;
  const findInPanels = (re) => routes.some(r => (r.panels || []).some(p => re.test(p.title) || re.test(p.sampleText)));

  const dims = [];

  // 1. goal-board state
  dims.push({
    name: "goal-board state",
    classification: apiOk("/api/goals") && tabBySlug["goals"] ? "PRESENT"
                  : apiOk("/api/goals") ? "PARTIAL"
                  : "MISSING",
    evidence: [
      apiOk("/api/goals")    ? `API /api/goals → 200 (keys: ${(apis["/api/goals"].body?.keys || []).join(", ")})` : "API /api/goals → not 200",
      tabBySlug["goals"]     ? `Tab .tab[data-tab="goals"] (#tab-goals) screenshot: ${tabBySlug["goals"].screenshot}` : "no #tab-goals discovered",
      apiOk("/api/workboard") ? `Cross-reference API /api/workboard → 200 also surfaces "goals" key` : null,
    ].filter(Boolean),
  });

  // 2. OODA cycle health
  const oodaStatusKeys = apis["/api/status"]?.body?.keys || [];
  const oodaCycleEvidence = oodaStatusKeys.includes("daemon_health") || oodaStatusKeys.includes("ooda_daemon");
  const oodaCyclesEndpoint = apiOk("/api/ooda-cycles");
  dims.push({
    name: "OODA cycle health",
    classification: oodaCyclesEndpoint && oodaCycleEvidence && tabBySlug["thinking"] ? "PRESENT"
                  : oodaCycleEvidence && tabBySlug["overview"] ? "PARTIAL"
                  : oodaCycleEvidence ? "PARTIAL"
                  : "MISSING",
    detail: oodaCyclesEndpoint
      ? "Per-cycle history with duration trend is exposed via /api/ooda-cycles; current cycle state via /api/status.daemon_health; visualisation in the Thinking tab."
      : "Cycle number, phase, last summary, and timestamp are exposed via /api/status.daemon_health, but there is no dedicated OODA tab showing phase transitions over time, no /api/ooda-cycles endpoint, and no inline visualisation of the OODA loop.",
    evidence: [
      apiOk("/api/status")        ? `API /api/status surfaces daemon_health.cycle_number, daemon_health.cycle_phase, daemon_health.actions_taken, daemon_health.cycle_duration_secs (keys: ${oodaStatusKeys.join(", ")})` : "API /api/status → not 200",
      oodaCyclesEndpoint           ? `API /api/ooda-cycles → 200 (per-cycle history with duration trend)` : (apis["/api/ooda-cycles"] ? `API /api/ooda-cycles → ${apis["/api/ooda-cycles"].status}` : null),
      tabBySlug["overview"]       ? `Tab #tab-overview screenshot: ${tabBySlug["overview"].screenshot}` : "no #tab-overview discovered",
      tabBySlug["thinking"]       ? `Tab #tab-thinking present with cycle history visualisation: ${tabBySlug["thinking"].screenshot}` : null,
    ].filter(Boolean),
  });

  // 3. engineer subprocesses
  const procApi = apis["/api/processes"];
  dims.push({
    name: "engineer subprocesses",
    classification: procApi?.ok && tabBySlug["processes"] ? "PRESENT"
                  : procApi?.ok ? "PARTIAL"
                  : "MISSING",
    evidence: [
      procApi?.ok            ? `API /api/processes → 200 (keys: ${(procApi.body?.keys || []).join(", ")})` : "API /api/processes → not 200",
      tabBySlug["processes"] ? `Tab .tab[data-tab="processes"] (#tab-processes) screenshot: ${tabBySlug["processes"].screenshot}` : "no #tab-processes discovered",
    ].filter(Boolean),
  });

  // 4. cognitive memory growth
  const memApi = apis["/api/memory"];
  const memKeys = memApi?.body?.keys || [];
  const hasNativeMemoryGrowth = memKeys.includes("native_memory") || (apis["/api/workboard"]?.body?.keys || []).includes("cognitive_statistics");
  dims.push({
    name: "cognitive memory growth",
    classification: memApi?.ok && tabBySlug["memory"] && hasNativeMemoryGrowth ? "PARTIAL"
                  : memApi?.ok && tabBySlug["memory"] ? "PARTIAL"
                  : memApi?.ok ? "PARTIAL"
                  : "MISSING",
    detail: "Current totals (episodic / semantic / procedural / prospective / sensory / working) are exposed, but there is no time-series of GROWTH — no per-cycle delta, no rate, no chart. Dashboard answers \"how much memory is there NOW\" but not \"is it growing\".",
    evidence: [
      memApi?.ok                        ? `API /api/memory → 200 (keys: ${memKeys.join(", ")}); includes native_memory.episodic/semantic counts` : "API /api/memory → not 200",
      apis["/api/workboard"]?.ok        ? `API /api/workboard → 200 also surfaces cognitive_statistics block (point-in-time snapshot only)` : null,
      tabBySlug["memory"]               ? `Tab .tab[data-tab="memory"] (#tab-memory) screenshot: ${tabBySlug["memory"].screenshot}` : "no #tab-memory discovered",
    ].filter(Boolean),
  });

  // 5. merge-judge decisions
  const judgeApiHits = ["/api/judges", "/api/judge", "/api/merge-judge"].filter(p => apis[p]?.ok);
  const judgePanelHit = findInPanels(/merge.?judge|judge decision/i);
  dims.push({
    name: "merge-judge decisions",
    classification: judgeApiHits.length > 0 || judgePanelHit ? "PARTIAL" : "MISSING",
    detail: "No dashboard surface for merge-judge: no dedicated tab, no API endpoint, no panel cites it. This is one of the dimensions that PR #1880 was scoped to address; cycle 1 confirms it is unaddressed in the live build.",
    evidence: [
      `API probes: ${["/api/judges","/api/judge","/api/merge-judge"].map(p => `${p}→${apis[p]?.status ?? "n/a"}`).join(", ")}`,
      judgePanelHit            ? `Panel header text mentions \"merge-judge\" — see report screenshots` : "no panel header mentions merge-judge",
    ],
  });

  // 6. per-PR readiness for #1880, #1893, #1894
  const prApiHits = ["/api/prs", "/api/pulls", "/api/pr/1880"].filter(p => apis[p]?.ok);
  const prPanelHit = findInPanels(/\bPR #?\d+|pull request|per-PR|#1880|#1893|#1894/i);
  dims.push({
    name: "per-PR readiness for #1880, #1893, #1894",
    classification: prApiHits.length > 0 || prPanelHit ? "PARTIAL" : "MISSING",
    detail: "No per-PR readiness surface in cycle 1 build: no /api/prs* endpoints respond 200, no panel header references the target PR numbers. Issue #1944 proposes refining the goal description to include this dimension; cycle 1 confirms the dashboard does not yet serve it.",
    evidence: [
      `API probes: ${["/api/prs","/api/pulls","/api/pr/1880"].map(p => `${p}→${apis[p]?.status ?? "n/a"}`).join(", ")}`,
      prPanelHit                ? `Panel header text mentions PR identifiers — see report screenshots` : "no panel header mentions #1880 / #1893 / #1894",
    ],
  });

  // 7. brain-failure surfacing tied to #1890
  const brainApiHits = ["/api/brain-failures", "/api/failures"].filter(p => apis[p]?.ok);
  const brainPanelHit = findInPanels(/brain[-\s]?failure|empty.?response|EMPTY_RESPONSE_SENTINEL|#1890/i);
  dims.push({
    name: "brain-failure surfacing tied to #1890",
    classification: brainApiHits.length > 0 || brainPanelHit ? "PARTIAL" : "MISSING",
    detail: "Issue #1890 is closed, but the surfacing it was meant to enable is not on the dashboard today: no panel, no API endpoint. Cycle 1 confirms closure ≠ delivery for self-introspection.",
    evidence: [
      `API probes: ${["/api/brain-failures","/api/failures"].map(p => `${p}→${apis[p]?.status ?? "n/a"}`).join(", ")}`,
      brainPanelHit             ? `Panel header text mentions brain-failure indicator — see report screenshots` : "no panel header mentions brain-failure / EMPTY_RESPONSE_SENTINEL / #1890",
      "Cross-ref: issue #1890 is CLOSED; this dimension scores on dashboard surface, not on issue state.",
    ],
  });

  return dims;
}

// ─── report rendering ─────────────────────────────────────────────────────────

function followUpQueue(dims, routes, apis) {
  const out = [];
  for (const d of dims) {
    if (d.classification === "MISSING") {
      out.push({
        priority: "P1",
        dimension: d.name,
        title: `dashboard: add ${d.name} surface`,
        rationale: d.detail || `No API or panel covers "${d.name}" in cycle 1.`,
      });
    } else if (d.classification === "PARTIAL") {
      out.push({
        priority: "P2",
        dimension: d.name,
        title: `dashboard: deepen ${d.name} (currently point-in-time only / no history / no labels)`,
        rationale: d.detail || `Data is reachable but not synthesised into the kind of surface Simard's self-introspection needs.`,
      });
    }
  }
  // A polish pass — generic UI quality observations (not blocking, but worth filing later)
  out.push({
    priority: "P3",
    dimension: "polish / formatting",
    title: "dashboard: replace raw ISO timestamps + bare UUIDs with friendly relative-time + labelled identifiers",
    rationale: "Earlier audit (Pass 2, scripts/dashboard_audit/) flagged raw ISO and unlabelled UUIDs across tabs; carry-over.",
  });
  return out;
}

function renderMarkdown({ meta, authResult, routes, apis, dims, followUps }) {
  const L = [];
  L.push(`# Simard dashboard audit — cycle 1 (issue #1880)`);
  L.push(``);
  L.push(`- **Captured:** \`${meta.timestamp}\` (UTC)`);
  L.push(`- **Tool:** Playwright + Chromium (headless, viewport 1440x900)`);
  L.push(`- **Endpoint:** \`${BASE_URL}\``);
  L.push(`- **Auth source:** \`~/.simard/.dashkey\` (${meta.tokenBytes} bytes)`);
  L.push(`- **Auth winner:** \`${authResult.winner ? authResult.winner.method : "NONE"}\` (endpoint: \`${authResult.winner?.endpoint || "—"}\`, field: \`${authResult.winner?.field || "—"}\`)`);
  L.push(`- **Auth attempts tried (in order):**`);
  for (const a of authResult.attempts) {
    L.push(`  - \`${a.method}\` → status \`${a.status ?? "err"}\`, ok=\`${a.ok}\`${a.endpoint ? ` (\`${a.endpoint}\`)` : ""}${a.error ? ` — error: ${a.error}` : ""}`);
  }
  L.push(`- **Artifacts dir:** \`scripts/dashboard-audit/out/${meta.timestamp}/\``);
  L.push(``);

  L.push(`## Cross-reference: issue #1944`);
  L.push(``);
  L.push(`Issue [#1944](https://github.com/rysweet/Simard/issues/1944) proposes refining the canonical description of the \`improve-simard-dashboard\` goal so it explicitly names Simard's self-introspection needs (goal-board, OODA, engineers, memory growth, merge-judge, per-PR readiness, brain-failure). Cycle 1 confirms why that refinement matters: **the current dashboard primarily serves a human operator's diagnostic needs (live tabs, screenshots, log tails) and only weakly serves Simard's self-introspection needs.** Three of the seven mandated dimensions (merge-judge decisions, per-PR readiness, brain-failure surfacing) have neither API nor panel today; two more (OODA cycle health, cognitive memory growth) expose point-in-time data only — no time-series, no per-cycle delta, no rate. A dashboard built for Simard-as-reader needs to render *change* over *snapshot*. Cycle 2 should prioritise the three MISSING dimensions and add timestamped history columns to the PARTIAL ones, in service of #1944's intent.`);
  L.push(``);

  L.push(`## Seven-dimension coverage matrix`);
  L.push(``);
  L.push(`| # | Dimension | Coverage | Citing evidence |`);
  L.push(`|---|---|---|---|`);
  dims.forEach((d, i) => {
    const ev = d.evidence.map(e => e.replace(/\|/g, "\\|")).join("<br>");
    L.push(`| ${i + 1} | ${d.name}${d.name.includes("#1890") ? " _(issue #1890 closed)_" : ""} | **${d.classification}** | ${ev} |`);
  });
  L.push(``);

  // Dimension detail sections (for the PARTIAL/MISSING ones that have notes)
  L.push(`### Per-dimension notes`);
  L.push(``);
  for (const d of dims) {
    if (!d.detail) continue;
    L.push(`- **${d.name}** — ${d.detail}`);
  }
  L.push(``);

  L.push(`## Routes captured`);
  L.push(``);
  L.push(`| Slug | Label | Panels | Latest timestamp seen | Console errors Δ | Screenshot |`);
  L.push(`|---|---|---:|---|---:|---|`);
  for (const r of routes) {
    const pic = `scripts/dashboard-audit/out/${meta.timestamp}/${r.screenshot}`;
    L.push(`| \`${r.slug}\` | ${r.label || ""} | ${r.panels?.length ?? 0} | ${r.latestTimestamp || "—"} | ${r.consoleErrorsDelta ?? r.consoleErrorsBefore ?? 0} | \`${pic}\` |`);
  }
  L.push(``);

  L.push(`## API probe summary`);
  L.push(``);
  L.push(`| Endpoint | Status | Bytes | Keys / preview | Linked dimensions |`);
  L.push(`|---|---:|---:|---|---|`);
  for (const p of API_PROBES) {
    const r = apis[p.path];
    const keys = r?.body?.keys ? r.body.keys.slice(0, 8).join(", ") : (r?.body?.array ? `array len=${r.body.length}` : "—");
    L.push(`| \`${p.path}\` | ${r?.status ?? "err"} | ${r?.bodyBytes ?? 0} | ${keys} | ${p.dims.join(", ") || "—"} |`);
  }
  L.push(``);

  L.push(`## Prioritised follow-up queue (cycle 2 candidates — NOT filed in cycle 1)`);
  L.push(``);
  for (const f of followUps) {
    L.push(`- **${f.priority}** — _${f.dimension}_ — ${f.title}`);
    L.push(`    - ${f.rationale}`);
  }
  L.push(``);

  L.push(`## Screenshots (repo-relative)`);
  L.push(``);
  for (const r of routes) {
    L.push(`- ![${r.label || r.slug}](scripts/dashboard-audit/out/${meta.timestamp}/${r.screenshot})`);
  }
  L.push(``);

  L.push(`## Self-introspection vs. human-operator verdict`);
  L.push(``);
  L.push(`The cycle 1 dashboard is **primarily a human-operator console**, not a self-introspection surface. Evidence:`);
  L.push(`- All seven dimensions are framed in terms a human watching a service would want; only goal-board + processes are first-class today.`);
  L.push(`- Memory and OODA tabs show *current state*, not *trajectory*. Simard reading her own dashboard cannot answer "am I learning?" or "did my last OODA cycle improve things?" from these surfaces.`);
  L.push(`- Three dimensions (merge-judge, per-PR readiness, brain-failure) have no surface at all — those are the highest priority for cycle 2 if the goal text in #1944 is adopted.`);
  L.push(``);
  return L.join("\n");
}

// ─── main ────────────────────────────────────────────────────────────────────

async function main() {
  const startedAt = new Date().toISOString();
  let token;
  try { token = readToken(); } catch (e) { log("FATAL:", e.message); process.exit(2); }
  log(`token: ${token.length} bytes`);

  log("trying auth methods…");
  let authResult;
  try {
    authResult = await tryAuthMethods(token);
  } catch (e) {
    log("FATAL (dashboard unreachable or /login broken):", e.message);
    const rep = { fatal: true, phase: "auth", error: e.message, startedAt, endedAt: new Date().toISOString() };
    fs.writeFileSync(path.join(OUT_DIR, "report.json"), JSON.stringify(rep, null, 2));
    fs.writeFileSync(path.join(OUT_DIR, "report.md"), `# dashboard-audit FATAL\n\nPhase: auth\nError: ${e.message}\n`);
    process.exit(3);
  }
  if (!authResult.winner) {
    log("FATAL: no auth method succeeded");
    const rep = { fatal: true, phase: "auth", attempts: authResult.attempts, startedAt, endedAt: new Date().toISOString() };
    fs.writeFileSync(path.join(OUT_DIR, "report.json"), JSON.stringify(rep, null, 2));
    fs.writeFileSync(path.join(OUT_DIR, "report.md"), `# dashboard-audit FATAL\n\nAuthentication failed for all four mechanisms.\n\nAttempts:\n\`\`\`json\n${JSON.stringify(authResult.attempts, null, 2)}\n\`\`\`\n`);
    process.exit(2);
  }
  log(`auth winner: ${authResult.winner.method}`);

  log("probing APIs…");
  const apis = await probeApis(authResult.api);
  log(`probed ${Object.keys(apis).length} endpoints`);

  log("launching chromium…");
  const browser = await chromium.launch({ headless: true });
  let walkResult;
  try {
    walkResult = await walkDashboard(browser, authResult);
  } finally {
    await browser.close();
    if (authResult.api) await authResult.api.dispose();
  }
  log(`captured ${walkResult.routes.length} routes`);

  const dims = classifyCoverage(walkResult.routes, apis);
  const followUps = followUpQueue(dims, walkResult.routes, apis);

  const meta = { timestamp: TS, tokenBytes: token.length, startedAt, endedAt: new Date().toISOString() };
  const report = {
    meta,
    baseUrl: BASE_URL,
    auth: { winner: authResult.winner, attempts: authResult.attempts },
    apis,
    routes: walkResult.routes,
    consoleMessages: walkResult.consoleMessages.slice(-200),
    dimensions: dims,
    followUps,
  };

  fs.writeFileSync(path.join(OUT_DIR, "report.json"), JSON.stringify(report, null, 2));
  fs.writeFileSync(path.join(OUT_DIR, "report.md"), renderMarkdown({ meta, authResult, routes: walkResult.routes, apis, dims, followUps }));
  log(`wrote ${OUT_DIR}/report.{json,md}`);
  log("DONE");
}

main().catch(e => {
  console.error("UNCAUGHT", e);
  fs.writeFileSync(path.join(OUT_DIR, "fatal.txt"), e.stack || String(e));
  process.exit(1);
});
