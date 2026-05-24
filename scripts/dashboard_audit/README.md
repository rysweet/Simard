# Dashboard usability audit — re-runnable Playwright sweep

This directory holds the diagnostic audit harness for the Simard self-serve
dashboard at `http://localhost:8080`. It is the follow-up tooling for issue
[#1662](https://github.com/rysweet/Simard/issues/1662) ("dashboard: first-pass
usability audit"). Each pass produces a fresh set of full-page screenshots and
visible-text dumps for every top-level route the dashboard surfaces, so we can
file *concrete* sub-issues (with screenshots and quoted text) rather than vague
"the dashboard is confusing" complaints.

## What the script does

`audit_pass_01.py` is a one-shot Playwright run that:

1. Reads the dashboard login code from `~/.simard/.dashkey`.
2. POSTs `/api/login` to obtain a `simard_session` cookie.
3. Loads `http://localhost:8080/` once and **dynamically enumerates** every
   top-level nav target — `<nav>` anchors, `[role=tab]`, `[data-tab]`, header
   anchors. Nothing is hard-coded, so a future tab added to the SPA is picked
   up automatically.
4. For each target it changes the SPA hash in place (no full reload, so the
   SPA's own click handlers fire), waits for network-idle + a settle delay,
   then writes:
   - `out/NN-<slug>.png` — full-page screenshot
   - `out/NN-<slug>.txt` — `document.body.innerText`
5. Writes `out/_index.json` summarising every capture (label, href, byte
   sizes, etc.).

The `out/` directory is gitignored — captures are intentionally ephemeral and
should be regenerated per audit pass against the live daemon.

## How to re-run an audit pass

Pre-requisites (one-time):

- Simard daemon running locally on `:8080` (`simard ooda run`)
- `~/.simard/.dashkey` populated (created automatically by the daemon)
- A Python virtualenv with Playwright + Chromium. The recommended layout is a
  worktree-local `.venv-audit/`:

```bash
python3 -m venv .venv-audit
.venv-audit/bin/pip install playwright==1.59.0
# Chromium is already installed under ~/.cache/ms-playwright/chromium-* on
# this host; if not, run:  .venv-audit/bin/python -m playwright install chromium
```

Run a pass:

```bash
.venv-audit/bin/python scripts/dashboard_audit/audit_pass_01.py
```

Typical wall time is ~50-60s. Output goes to `scripts/dashboard_audit/out/`.

## How to file findings

After the pass completes, read the `.txt` dumps (they are far easier to grep
than the PNGs) and look for:

- **Jargon** — terms a non-Simard-developer would not understand (`OODA`,
  `stewardship`, `subordinate`, `handoff`, `reflection snapshot`, `cognitive
  memory`, raw enum names like `continue_skipping`, raw struct field names,
  bare UUIDs).
- **Missing panels** — operator questions ("what is the daemon doing right
  now?", "which goals are active and why?") that cannot be answered from any
  panel.
- **Human-friendliness regressions** — ISO timestamps without a relative "X
  minutes ago", JSON blobs displayed raw, no empty-state copy, no per-panel
  explanation of purpose, data-source mismatches between two panels claiming
  to show the same thing.

For each *distinct* finding, file a focused sub-issue against `rysweet/Simard`:

```bash
gh issue create \
  --label dashboard,audit-1662-followup \
  --title "dashboard: <one-line>" \
  --body "<finding, screenshot path, proposed fix in plain prose, parent: #1662>"
```

Aim for 5-10 small sub-issues per pass — never one mega-issue.

## Future passes

The script name (`audit_pass_01.py`) is intentionally numbered. Subsequent
passes should land alongside as `audit_pass_02.py`, `audit_pass_03.py`, etc.,
optionally extending the harness (e.g. probing every documented `/api/*`
endpoint, exercising forms, simulating slow networks). Re-using the same
script per pass is fine if the harness needs no changes — only the captures
and findings change between passes.

---

# `audit_dashboard.py` — structured first-pass audit + REPORT.md generator

`audit_dashboard.py` is a sibling tool to `audit_pass_01.py` purpose-built for
the **self-serve-dashboard-improvement** initiative (parent issue
[#1990](https://github.com/rysweet/Simard/issues/1990), epic to be filed). Where
`audit_pass_01.py` produces raw screenshots + text dumps for human review,
`audit_dashboard.py` additionally:

- Captures per-page **HTTP errors** (responses with status ≥ 400) and
  **browser console errors** (`console.error(...)`) into structured JSON.
- Runs a **jargon scan** across every captured DOM dump and produces an
  inverted index (term → list of pages where it appears).
- Emits a single consolidated **`out/REPORT.md`** — the document that gets
  embedded into the parent epic issue and decomposed into 3–5 child issues.

It is intentionally small (**target ≤150 LOC** — a soft design budget, not a
hard runtime cap; treat as a discipline that flags scope creep when exceeded),
stdlib + `playwright.sync_api` only, zero new project dependencies, so it can
be re-read at a glance and re-run deterministically.

## When to use which script

| You want…                                          | Use                  |
| -------------------------------------------------- | -------------------- |
| Raw PNG + TXT dumps to grep, no opinion            | `audit_pass_01.py`   |
| A structured REPORT.md ready to paste into an epic | `audit_dashboard.py` |
| Per-page HTTP + console error inventory            | `audit_dashboard.py` |
| Numbered, ordered captures for diffing across runs | `audit_pass_01.py`   |
| Jargon inverted index                              | `audit_dashboard.py` |

Both scripts are safe to keep co-resident; they read the same dashkey, hit the
same daemon, and write to the same `out/` directory under **disjoint filename
conventions** that cannot collide:

- `audit_pass_01.py` writes `NN-<slug>.{png,txt}` and `out/_index.json`.
- `audit_dashboard.py` writes `<slug>.{png,txt,errors.json}`, `out/REPORT.md`,
  and `out/_audit_dashboard_index.json` (deliberately distinct from
  `_index.json` so the two scripts never overwrite each other's manifest).

## What `audit_dashboard.py` does

Six functions, one execution path:

1. **`load_key()`** — reads `~/.simard/.dashkey`, strips whitespace, validates
   it is non-empty and within a sane envelope (`1 ≤ len ≤ 256` characters; the
   daemon's exact format is not pinned here, so the check is a sanity gate
   rather than a strict equality). The key is never logged, never written to
   `out/`, never echoed in error messages — login failures surface only the
   HTTP status code.
2. **`authenticate(context, key)`** — POSTs `/api/login` with JSON body
   `{"code": "<key>"}` via `context.request.post(...)` (the context-level
   APIRequestContext, not `page.request`, so the call can run before any page
   is created). On success the `simard_session` cookie returned in
   `Set-Cookie` is added to the browser context with `context.add_cookies([...])`;
   subsequent navigations from any page opened on that context are
   authenticated. On failure the script exits non-zero with the HTTP status
   only (no body, no key).
3. **`discover_routes(page)`** — loads `/` post-auth and harvests candidate
   routes from the live DOM: `<a href>`, `[role=tab][data-route]`, and
   `[data-tab]` attributes. The result is unioned with a documented
   **fallback tab list**, derived from the nav set observed in
   `audit_pass_01.py`'s prior runs against the live dashboard:
   ```
   overview, goals, traces, logs, processes, memory,
   costs, chat, whiteboard, thinking, terminal
   ```
   Routes are filtered to same-origin (`http://localhost:8080`), schemes
   `javascript:`, `data:`, `mailto:`, `file:` are rejected, hash routes
   (`#/memory`) are normalized to their slug form, and the final list is
   deduplicated and capped at `MAX_ROUTES = 50`. Fallback-tab routes that turn
   out not to exist (HTTP 404 on first navigation, or zero-length body) are
   recorded in `errors.json` but **filtered out** of REPORT.md's "Pages found"
   and "What each page conveys" sections, so unknown tabs don't pollute the
   epic body.
4. **`capture_page(page, route)`** — for each route:
   - Attaches a fresh `page.on("response", ...)` listener capturing every
     response with `status >= 400` as `{"url": ..., "status": ...}`.
   - Attaches a fresh `page.on("console", ...)` listener capturing every
     message where `msg.type == "error"` as `{"text": ..., "location": ...}`.
   - **Listener lifecycle:** both listeners are removed in a `try/finally`
     via `page.remove_listener("response", handler)` /
     `page.remove_listener("console", handler)` before the function returns,
     so listeners do not accumulate across the N route captures (avoids the
     O(N²) buffering bug). Per-route error buffers are local to the function
     scope and discarded after the JSON write.
   - **Navigation:** the dashboard is a hash-routed SPA. The script does **not**
     re-navigate with `page.goto(...)` per route (which would race the
     SPA's own `hashchange` handler when the path doesn't change); instead it
     mutates the hash in place via
     `page.evaluate("location.hash = arguments[0]", route_hash)`, then
     `page.wait_for_load_state("networkidle", timeout=TIMEOUT_MS)` plus a
     small settle delay — the same strategy `audit_pass_01.py` already
     validated against the live SPA. The first route in the run uses a full
     `page.goto(BASE_URL + "/")` to land on the SPA shell.
   - Writes three artefacts:
     - `out/<slug>.png` — full-page screenshot (`full_page=True`).
     - `out/<slug>.txt` — `document.body.innerText`.
     - `out/<slug>.errors.json` — `{"http": [...], "console": [...]}`.
5. **`scan_jargon(text_dumps)`** — substring-matches (case-insensitive) every
   captured DOM dump against the seeded jargon vocabulary:
   ```
   OODA, ooda loop, cognitive memory, handoff bundle, facilitator,
   recipe runner, consolidation, episodic, semantic memory,
   procedural memory, LadybugDB, spawn_engineer, workboard, whiteboard
   ```
   The vocabulary deliberately omits weak signals (`trace` is generic English;
   `self-serve` is the product name and would trigger on every page header).
   Output is an inverted index: `{term: [slug, slug, ...]}`. Terms can be
   added to `JARGON_TERMS` (module-top constant) between passes without
   changing any function signature.
6. **`write_report(...)`** — composes `out/REPORT.md` with five sections (see
   "REPORT.md anatomy" below). The Markdown body is built from a single
   f-string template constant to keep LOC down and the rendered shape
   inspectable at a glance.

The script exits **0 on success**, **non-zero on any uncaught exception or any
of the documented preconditions**:

- `~/.simard/.dashkey` missing, empty, or outside the sanity envelope
- `/api/login` returns non-2xx (printed as the status code, never the body)
- Zero routes discovered even after unioning the fallback tab list
- Any write attempted under a forbidden path (see Hard safety constraints)
- `out/` not writable, or `playwright` / Chromium not installed (the
  underlying `playwright.sync_api` import or browser-launch exception
  propagates with its original message)
- Daemon unreachable on `:8080` (the underlying `ConnectionRefusedError` /
  Playwright timeout propagates)

## Hard safety constraints (enforced in code)

- **Forbidden write paths.** A module-top `assert` refuses to run if
  `OUT_DIR.resolve()` falls under either of:
  - `~/.simard/prompt_assets/`
  - `$SIMARD_PROMPT_ASSETS_DIR` (if set)
- **Slug whitelist.** Output filenames are derived through the regex
  `[a-z0-9_-]+`, capped at 64 chars, with collision suffixes `_2`, `_3`, … and
  a fallback `route_<n>` if a route's path yields an empty slug. Path
  traversal via `../` or absolute paths is structurally impossible.
- **Same-origin only.** Routes pointing outside `http://localhost:8080` are
  rejected during discovery.
- **Route cap.** `MAX_ROUTES = 50` bounds the work even if the DOM injects
  pathological route lists.
- **No dashkey in logs.** Login error paths surface only `r.status` — never
  the request body, never the response body, never the key itself.
- **Default Chromium sandbox.** No `--no-sandbox` flag.
- **No external network.** Only `http://localhost:8080` is contacted.

## Configuration

All knobs are module-level constants at the top of `audit_dashboard.py`:

| Constant       | Default                                  | Purpose                                |
| -------------- | ---------------------------------------- | -------------------------------------- |
| `BASE_URL`     | `http://localhost:8080`                  | Dashboard origin                       |
| `DASHKEY_PATH` | `~/.simard/.dashkey`                     | Login code source                      |
| `OUT_DIR`      | `scripts/dashboard_audit/out`            | Where artefacts are written            |
| `MAX_ROUTES`   | `50`                                     | Hard upper bound on routes audited     |
| `TIMEOUT_MS`   | `15000`                                  | Per-page navigation timeout            |
| `FALLBACK_TABS`| `[overview, goals, traces, …]`           | Routes union'd if DOM discovery is thin|
| `JARGON_TERMS` | seed list (see above)                    | Terms scanned per page                 |

There are no CLI flags by design — the script is invoked the same way every
time, so output is diff-able across runs.

## Usage

Pre-requisites identical to `audit_pass_01.py` (live dashboard on `:8080`,
populated `~/.simard/.dashkey`, ephemeral `.venv-audit/` with
`playwright==1.59.0`):

```bash
# from the repo root
python3 -m venv scripts/dashboard_audit/.venv-audit
scripts/dashboard_audit/.venv-audit/bin/pip install playwright==1.59.0
scripts/dashboard_audit/.venv-audit/bin/python -m playwright install chromium

# run the audit
scripts/dashboard_audit/.venv-audit/bin/python \
  scripts/dashboard_audit/audit_dashboard.py
```

Typical wall time is ~45-75s depending on route count. On success the script
prints a one-line summary to stdout:

```
audit_dashboard: 11 routes captured, 0 http errors, 2 console errors,
                 7 jargon terms found, REPORT.md written.
```

## Output layout

```
scripts/dashboard_audit/out/
├── REPORT.md                         ← consolidated, embedded into epic issue
├── overview.png
├── overview.txt
├── overview.errors.json
├── goals.png
├── goals.txt
├── goals.errors.json
├── …
└── _audit_dashboard_index.json       ← machine-readable run manifest
                                        (named so it cannot collide with
                                        audit_pass_01.py's _index.json)
```

`out/` is gitignored both by the root `.gitignore` and a local
`scripts/dashboard_audit/.gitignore`, so a careless `git add -A` cannot leak
session data or screenshots.

## REPORT.md anatomy

`out/REPORT.md` is a deterministic Markdown render with exactly five sections,
in this order:

1. **Pages found** — a table with columns:
   `slug | url | <title> | text-length | http-errors | console-errors`.
2. **What each page conveys** — per slug, either the first `<h1>` text or the
   first ~120 characters of `innerText` (whichever exists).
3. **Jargon inventory** — the inverted index from `scan_jargon()`, rendered as
   `term → page1, page2, …`. Terms with zero hits are omitted.
4. **Missing context** — heuristic flags per page:
   - `text-length < 200` (likely empty state with no explanatory copy)
   - no `<h1>` detected
   - any HTTP or console errors observed
5. **Top-5 highest-impact usability fixes** — a **heuristic-ranked**
   candidate list, scored on the following signals per page:
   - jargon density (terms-per-100-chars)
   - empty-state indicator (text length, missing H1)
   - error counts (HTTP + console)
   - missing nav label
   
   The header of this section is explicit that the ranking is heuristic and
   the engineer hand-curates final wording before pasting into the epic.

REPORT.md is restricted to **aggregate counts, page titles, jargon terms, and
short page excerpts** — never full DOM dumps, never `errors.json` payloads
(which can contain URLs with query strings), never screenshots. Specifically,
Section 2 ("What each page conveys") emits at most the first `<h1>` text or,
if no `<h1>` is present, a **≤120-character** truncation of `innerText`. That
short excerpt *can* in principle include operator-visible labels (e.g. a
recent goal title that happens to fall in the first 120 chars of a page
body), so REPORT.md is treated as **human-review-required** before any
`gh issue create` runs — see the worked example below. If a 120-char excerpt
looks sensitive on a given page, the recommended fix is to hand-edit
REPORT.md (it's plain Markdown) before pasting, or to drop the fallback for
that route entirely.

## Worked example: end-to-end audit + issue filing

```bash
# 1. Run the audit
scripts/dashboard_audit/.venv-audit/bin/python \
  scripts/dashboard_audit/audit_dashboard.py

# 2. Eyeball the report (do NOT commit out/)
less scripts/dashboard_audit/out/REPORT.md

# 3. Commit only the script + .gitignore edits (NEVER -A)
git add scripts/dashboard_audit/audit_dashboard.py \
        scripts/dashboard_audit/.gitignore \
        .gitignore
git diff --cached      # human-review before commit
git commit -m "dashboard: first-pass Playwright audit + jargon inventory"
git push -u origin feat/issue-1990-conduct-first-pass-dashboard-audit-with-playwright
# (branch name is system-generated by the worktree harness and references
#  parent #1990; the exact string above is illustrative — use whatever
#  `git rev-parse --abbrev-ref HEAD` reports in your worktree.)

# 4. Open the draft PR
gh pr create --draft \
  --title "dashboard: first-pass Playwright audit + jargon inventory" \
  --body "$(cat <<'EOF'
Self-serve goal: self-serve-dashboard-improvement.
First-pass audit run via scripts/dashboard_audit/audit_dashboard.py.
See linked epic for REPORT.md and child issues.
Cross-links: #1662 (baseline first-pass audit).
EOF
)"

# 5. File the epic with REPORT.md embedded
gh issue create \
  --title "epic: self-serve-dashboard-improvement — make the dashboard understandable to humans" \
  --label epic,dashboard,usability \
  --body-file scripts/dashboard_audit/out/REPORT.md

# 6. For each of the top 3–5 fixes, file a child issue with plain-English
#    acceptance criteria of the form:
#    "A human visiting /<page> can <outcome> without knowing the term <jargon>."
gh issue create \
  --title "dashboard: /memory should explain what Simard remembered recently" \
  --label dashboard,usability \
  --body "Parent: #<epic-number>

Acceptance criteria (plain-English user outcome):
A human visiting /memory can tell what Simard remembered in the last hour
without knowing the term 'cognitive memory'.

Technical notes:
- Replace the heading 'Cognitive Memory' with 'What Simard remembered'.
- Show items as a reverse-chronological list with relative timestamps
  ('3 minutes ago') rather than raw ISO strings.
- Add an empty-state line: 'No new memories in the last hour.'"
```

## Plain-English acceptance criteria — the house style

Every child issue derived from `REPORT.md` MUST express its acceptance criteria
in the form:

> A human visiting `<page>` can `<observable outcome>` without knowing the term
> `<jargon>`.

This pattern keeps the test of done in the user's reality, not the
implementer's. It also makes it obvious to a reviewer whether the proposed fix
actually addresses the jargon flagged by the audit, or whether it just renames
one piece of jargon to another.

## Safety & privacy posture

- The dashkey is read once, held in memory, and used exactly once (the
  `/api/login` POST). It is never written to disk, never logged, never
  echoed in tracebacks.
- `out/` artefacts can contain operator-visible session data (goal names,
  recent prompts, console error messages with stack traces). They are
  **gitignored** and the workflow above commits only the script + ignore
  files via explicit `git add <files>` — never `git add -A`.
- `REPORT.md` (the only artefact that ever leaves the local machine, via
  the epic issue body) is restricted by construction to counts, titles,
  jargon terms, and ≤120-char page excerpts. Because excerpts can in
  principle surface short operator-visible labels, **re-read REPORT.md
  end-to-end before `gh issue create`** and hand-edit any line that looks
  sensitive — it is a plain Markdown file.

## Relationship to issue #1662

`audit_pass_01.py` was the original first-pass tooling filed under
[#1662](https://github.com/rysweet/Simard/issues/1662). `audit_dashboard.py`
is its formalised successor: same auth model, same discovery strategy, plus
structured error capture, jargon scanning, and a publishable REPORT.md. The
epic issue created from `audit_dashboard.py`'s output cross-links #1662 as the
baseline first-pass and positions itself as the concrete decomposition of that
work into actionable child issues — not as a duplicate.
