#!/usr/bin/env node
/**
 * Per-route H1, document.title, and lede regression check (issues #1993, #1994).
 *
 * What this asserts, for every SPA route discovered on the live dashboard:
 *   1. document.title is non-empty AND unique across routes.
 *   2. The active view contains EXACTLY ONE visible <h1> element with non-empty text.
 *   3. A non-empty lede element exists directly under the H1, length ≤ 140 chars.
 *
 * Auth flow mirrors audit.mjs: reads ~/.simard/.dashkey, attempts the discovered
 * form POST first, falls back to Bearer / cookie / query string.
 *
 * Run:    cd scripts/dashboard-audit && node regression-h1-title-lede.mjs
 * Exit:   0 = PASS · non-zero = FAIL (with per-route diagnostics)
 */

import { chromium, request as pwRequest } from "playwright";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";

const BASE_URL = process.env.SIMARD_DASHBOARD_URL || "http://localhost:8080";
const TOKEN_PATH = path.join(os.homedir(), ".simard", ".dashkey");
const MAX_LEDE_CHARS = 140;

const log = (...args) => console.log("[regression]", ...args);

function readToken() {
  if (!fs.existsSync(TOKEN_PATH)) throw new Error(`token file not found at ${TOKEN_PATH}`);
  return fs.readFileSync(TOKEN_PATH, "utf8").trim();
}

// ─── auth (matches audit.mjs winner cascade) ──────────────────────────────────

function discoverLoginSubmit(html) {
  const fetchMatch = html.match(/fetch\(['"]([^'"]+)['"]\s*,\s*\{[^}]*method:\s*['"]POST['"]/i);
  const inputMatch = html.match(/<input[^>]*id=["']([a-zA-Z0-9_-]+)["']/i)
    || html.match(/<input[^>]*name=["']([a-zA-Z0-9_-]+)["']/i);
  return {
    endpoint: fetchMatch ? fetchMatch[1] : "/api/login",
    field: inputMatch ? inputMatch[1] : "code",
  };
}

async function authenticate(token) {
  const api = await pwRequest.newContext({ baseURL: BASE_URL, ignoreHTTPSErrors: true });
  const r = await api.get(`${BASE_URL}/login`);
  if (r.status() >= 500) {
    await api.dispose();
    throw new Error(`/login returned HTTP ${r.status()} — dashboard unreachable`);
  }
  const discovered = discoverLoginSubmit(await r.text());
  const body = { [discovered.field]: token };
  const post = await api.post(discovered.endpoint, {
    headers: { "content-type": "application/json" },
    data: body,
  });
  const cookies = (await api.storageState()).cookies;
  if (post.status() >= 200 && post.status() < 400 && cookies.length > 0) {
    return { api, cookies };
  }
  await api.dispose();
  throw new Error(`form-post auth failed: status=${post.status()} cookies=${cookies.length}`);
}

// ─── route walk ──────────────────────────────────────────────────────────────

async function gatherRouteFacts(browser, cookies) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  if (cookies && cookies.length) {
    await ctx.addCookies(cookies.map(c => ({
      name: c.name, value: c.value,
      domain: c.domain || "localhost", path: c.path || "/",
      httpOnly: !!c.httpOnly, secure: !!c.secure, sameSite: "Strict",
    })));
  }
  const page = await ctx.newPage();
  const rootResp = await page.goto(`${BASE_URL}/`, { waitUntil: "domcontentloaded", timeout: 15_000 });
  if (rootResp && rootResp.url().endsWith("/login")) {
    log("cookie not honoured by browser — falling back to in-page login");
    await page.fill('#code, input[type="text"]', readToken());
    await page.click('button[type="submit"]');
    await page.waitForURL(`${BASE_URL}/`, { timeout: 10_000 });
  }
  await page.waitForSelector(".tab[data-tab]", { timeout: 10_000 });

  // Discover routes from the live SPA
  const slugs = await page.evaluate(() =>
    Array.from(document.querySelectorAll(".tab[data-tab]")).map(el => el.getAttribute("data-tab"))
  );
  if (slugs.length === 0) throw new Error("no .tab[data-tab] elements found — SPA structure changed?");
  log(`discovered ${slugs.length} routes: ${slugs.join(", ")}`);

  const facts = [];
  for (const slug of slugs) {
    await page.click(`.tab[data-tab="${slug}"]`, { timeout: 5_000 });
    // Wait for the tab-content to activate AND for the dynamic h1 to update.
    try { await page.waitForSelector(`#tab-${slug}.active`, { timeout: 5_000 }); } catch {}
    // Tiny settle for the title/h1/lede JS to run.
    await page.waitForTimeout(150);

    const fact = await page.evaluate(() => {
      // Visible h1s only: rules out hidden elements in inactive tab-contents
      // (defence in depth — the new design uses a single global h1).
      const allH1s = Array.from(document.querySelectorAll("h1"));
      const visibleH1s = allH1s.filter(el => {
        const r = el.getBoundingClientRect();
        const cs = getComputedStyle(el);
        return r.width > 0 && r.height > 0 && cs.display !== "none" && cs.visibility !== "hidden";
      });
      const h1 = visibleH1s[0] ? visibleH1s[0].textContent.trim() : "";
      const lede = document.getElementById("page-lede");
      const ledeText = lede ? (lede.textContent || "").trim() : "";
      return {
        title: document.title,
        h1Count: visibleH1s.length,
        h1AllCount: allH1s.length,
        h1Text: h1,
        ledeText,
        ledeLen: ledeText.length,
      };
    });
    facts.push({ slug, ...fact });
  }
  await ctx.close();
  return facts;
}

// ─── assertions ──────────────────────────────────────────────────────────────

function fmt(s, n) { return String(s).padEnd(n); }

function checkFacts(facts) {
  const failures = [];

  // (1) document.title uniqueness + non-empty
  const titles = facts.map(f => f.title);
  const titleSet = new Set(titles);
  if (titleSet.size !== titles.length) {
    const counts = {};
    titles.forEach(t => { counts[t] = (counts[t] || 0) + 1; });
    const dups = Object.entries(counts).filter(([, n]) => n > 1).map(([t, n]) => `"${t}" × ${n}`);
    failures.push(`document.title is NOT unique across routes — duplicates: ${dups.join("; ")}`);
  }
  facts.forEach(f => {
    if (!f.title || !f.title.trim()) failures.push(`route "${f.slug}" has an empty <title>`);
  });

  // (2) exactly one visible <h1>, non-empty
  facts.forEach(f => {
    if (f.h1Count !== 1) {
      failures.push(`route "${f.slug}" has ${f.h1Count} visible <h1> elements (expected exactly 1)`);
    }
    if (!f.h1Text) {
      failures.push(`route "${f.slug}" has an empty <h1>`);
    }
  });

  // (3) lede exists, non-empty, ≤140 chars
  facts.forEach(f => {
    if (!f.ledeText) {
      failures.push(`route "${f.slug}" is missing a lede (no text under #page-lede)`);
    } else if (f.ledeLen > MAX_LEDE_CHARS) {
      failures.push(`route "${f.slug}" lede is ${f.ledeLen} chars (> ${MAX_LEDE_CHARS} budget): "${f.ledeText}"`);
    }
  });

  return failures;
}

function renderTable(facts) {
  const W = { slug: 10, h1: 14, len: 5, title: 34, lede: 100 };
  const head = `${fmt("slug", W.slug)} | ${fmt("h1", W.h1)} | ${fmt("h1s", 3)} | ${fmt("titlelen", W.title)} | ${fmt("ledelen", W.len)} | lede (truncated)`;
  const sep = "-".repeat(head.length);
  const rows = facts.map(f =>
    `${fmt(f.slug, W.slug)} | ${fmt(f.h1Text.slice(0, W.h1), W.h1)} | ${fmt(f.h1Count, 3)} | ${fmt(f.title.slice(0, W.title), W.title)} | ${fmt(f.ledeLen, W.len)} | ${f.ledeText.slice(0, W.lede)}`
  );
  return [head, sep, ...rows].join("\n");
}

// ─── main ────────────────────────────────────────────────────────────────────

async function main() {
  let token;
  try { token = readToken(); } catch (e) { console.error("FATAL:", e.message); process.exit(2); }
  log(`token: ${token.length} bytes; base: ${BASE_URL}`);

  let auth;
  try { auth = await authenticate(token); }
  catch (e) { console.error("FATAL (auth):", e.message); process.exit(2); }
  log("authenticated");

  log("launching chromium…");
  const browser = await chromium.launch({ headless: true });
  let facts;
  try {
    facts = await gatherRouteFacts(browser, auth.cookies);
  } finally {
    await browser.close();
    await auth.api.dispose();
  }

  console.log("");
  console.log("Per-route H1 / title / lede facts (issues #1993, #1994):");
  console.log(renderTable(facts));
  console.log("");

  const failures = checkFacts(facts);
  if (failures.length === 0) {
    console.log(`✅ PASS — ${facts.length} routes; every route has a unique non-empty <title>, exactly one <h1>, and a lede ≤ ${MAX_LEDE_CHARS} chars.`);
    process.exit(0);
  }
  console.log(`❌ FAIL — ${failures.length} regression(s):`);
  for (const f of failures) console.log(`  - ${f}`);
  process.exit(1);
}

main().catch(e => {
  console.error("UNCAUGHT", e);
  process.exit(1);
});
