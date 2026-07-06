---
title: Dashboard Feedback Widget — report a bug / request a feature
description: API reference for the dashboard feedback widget. Every dashboard tab carries a "Report bug / Request feature" control that captures the current page context and starts a new dev-orchestrator workstream through Simard's existing recipe-launch plumbing. Covers the widget, the authenticated REST endpoints, request/response schemas, context capture, task_description composition, the launcher reuse contract, validation, de-duplication, and the security model.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ../howto/report-a-bug-or-request-a-feature.md
  - ./recipe-context-var-sanitization.md
  - ./cross-repo-merge-authority.md
  - ./concurrent-engineer-dispatch.md
  - ./dashboard-chat.md
  - ../architecture/engineer-agent-orchestration.md
---

# Dashboard Feedback Widget

Every page of the [operator dashboard](../dashboard.md) carries a small
**Report bug / Request feature** control. An operator who notices a defect or
wants a change can file it **from the page they are looking at**, without
switching tools or hand-writing a goal. On submit, the widget captures the
current page context, bundles it with the operator's report, and starts a **new
`dev-orchestrator` workstream** — the same `smart-orchestrator` →
`default-workflow` recipe run that Simard's Overseer and manual operator
workstreams use.

The widget is **cross-cutting**: it lives in the shared dashboard `<header>`, so
it is present on the whole tab set — Overview, Goals, Traces, Logs, Processes,
Memory, Costs, Chat, Workboard, Thinking, and any others in the current
catalogue — with identical placement. There is no per-tab wiring.

!!! note "One launch path, reused — not re-implemented"
    The endpoint does **not** shell out to `amplihack` on its own. It composes a
    [`RecipeBrief`](#recipebrief) and calls the shipped
    [`RecipeLauncher`](#launcher-reuse) (`SmartOrchestratorLauncher`,
    `src/overseer/launch.rs`) — byte-for-byte the same `amplihack recipe run
    amplifier-bundle/recipes/smart-orchestrator.yaml -c task_description=… -c
    target_repo=…` invocation engineers and the Overseer already use. There is no
    second notion of "launch a workstream" in the codebase.

## At a glance

| Property | Value |
|----------|-------|
| Widget location | Shared dashboard `<header>` — present on every tab |
| Trigger | **Report bug / Request feature** button → modal form |
| Submit endpoint | `POST /api/feedback` |
| Status endpoint | `GET /api/feedback/status/{id}` |
| Auth | Behind the existing dashboard access-code gate (`require_auth`); session cookie required |
| Content type | `application/json` only (form-encoding is rejected) |
| Recipe launched | `smart-orchestrator` → `default-workflow` |
| Launcher | `crate::overseer::launch::SmartOrchestratorLauncher` (reused) |
| Durable artifact | The resulting GitHub **PR** (feedback state itself is ephemeral) |

## The widget

The control renders in the top-right of the dashboard header, next to the
existing **Glossary** / **Releases** actions:

```
🌲 Simard Dashboard        … ⟨/⟩ Source   📦 Releases   📖 Glossary   💬 Feedback   12:04
```

The button reads **💬 Feedback** and carries the tooltip / accessible label
**Report bug / Request feature**. Clicking it opens a modal form with three
inputs:

| Field | Control | Meaning |
|-------|---------|---------|
| **Type** | select — `Bug` / `Feature` | Whether this is a defect report or a feature request. Maps to `report.type` = `bug` \| `feature`. |
| **Title** | single-line text | Short summary. ≤ 200 characters. |
| **Description** | multi-line text | What happened / what you want. ≤ 5000 characters. |

On **Submit**, the client-side JS:

1. Reads the **active tab** and the key state it renders (see
   [Context capture](#context-capture)).
2. `POST`s `{report, context}` as JSON to `/api/feedback` with
   `credentials: "same-origin"` (so the session cookie is sent).
3. Shows the returned **workstream id** and then **polls**
   `/api/feedback/status/{id}` until a PR appears or the run ends
   (see [UI feedback](#ui-feedback)).

Server responses are rendered with DOM `textContent` wherever possible. The one
`innerHTML` use — the "PR ready" line — first validates the server-supplied PR
URL against `^https://github\.com/[^/]+/[^/]+/pull/\d+$` and HTML-escapes every
interpolated part; a URL that fails the allow-list falls back to a plain-text
line with no link.

## Context capture

When the operator submits, the widget snapshots **what they were looking at** so
the workstream starts with real context instead of a bare sentence. The captured
`context` object is:

| Field | Source | Notes |
|-------|--------|-------|
| `page` | The active tab's stable slug (`data-tab` id, e.g. `overview`, `goals`, `memory`) | Read from the DOM / tab metadata; decoupled from the tab-switch handlers so it survives tab restructuring. |
| `state` | The rendered content of the active panel (its visible text / the JSON it rendered) | Bounded — truncated to ≤ 16 KiB on a char boundary; control characters stripped. |
| `timestamp` | Client clock at submit | ISO-8601 (RFC-3339). |
| `identifiers` | Page identifiers visible at submit | An object carrying the active `page` slug, `url` (path), `hash`, and `doc_title`. Rendered into the task as compact JSON and truncated to ≤ 4 KiB. |

The captured `state` is deliberately **minimal and page-scoped**: it is the
section/JSON the page already renders to the operator. It must never include
auth material (the login code, the session cookie is `HttpOnly` and unreadable),
and the server enforces the size cap regardless of what the client sends.

## Endpoints

Both endpoints are registered **before** the `require_auth` layer in
`src/operator_commands_dashboard/routes.rs`, so they inherit the existing
dashboard access-code gate automatically. An unauthenticated request is
redirected/refused exactly like every other `/api/*` route.

### `POST /api/feedback`

Start a workstream from a report + captured context.

**Request** (`Content-Type: application/json`):

```json
{
  "report": {
    "type": "bug",
    "title": "Costs tab shows $0 after midnight rollover",
    "description": "After the daily ledger rolls over, the Costs tab renders $0.00 for every provider until the first new call lands. Expected: it should show the prior day's total until the new day has spend."
  },
  "context": {
    "page": "costs",
    "state": "{\"providers\":[{\"name\":\"anthropic\",\"model\":\"claude-opus\",\"usd\":0.0}]}",
    "timestamp": "2026-07-06T02:41:10Z",
    "identifiers": {
      "url": "https://localhost:8080/#costs",
      "session": null
    }
  }
}
```

**Response — `202 Accepted`** (the run is spawned, not awaited):

```json
{
  "ok": true,
  "state": "started",
  "workstream_id": "recipe-48213",
  "poll": "/api/feedback/status/recipe-48213"
}
```

The `workstream_id` is the [`WorkstreamHandle.id`](#workstreamhandle) returned by
the launcher. `poll` is the ready-made status URL for the client.

**Error responses** (all `application/json`, `{ "ok": false, "error": "…" }`):

| Status | `error` | Cause |
|--------|---------|-------|
| `400 Bad Request` | `invalid report` | The body is missing the `report` object (this also covers a missing, non-JSON, form-encoded, malformed, or oversized body: the `Option<Json<Value>>` extractor coerces any such request to an empty object, which then fails this check). |
| `400 Bad Request` | `invalid type` | `report.type` is missing or not exactly `bug` or `feature`. |
| `400 Bad Request` | `title required` / `title too long` | Empty title, or title > 200 chars. |
| `400 Bad Request` | `description required` / `description too long` | Empty description, or description > 5000 chars. |
| `429 Too Many Requests` | `duplicate` | An identical report (same `type` + `title` + `description`) was submitted within the de-dup window (~30 s). See [De-duplication & throttle](#de-duplication-throttle). |
| `429 Too Many Requests` | `busy` | The distinct-launch concurrency cap is saturated. |
| `500 Internal Server Error` | `failed to start workstream` | The launcher failed to spawn. The internal detail is logged server-side via `tracing::warn!` only; the response body carries a generic message (never internal paths). |

> **Malformed / non-JSON bodies never launch anything.** The submit handler
> extracts the body as `Option<Json<Value>>`: a missing, malformed, non-JSON,
> **form-encoded**, or oversized request deserialises to `None`, is coerced to an
> empty object, and is rejected as `400 invalid report`. Rejecting form-encoding
> this way is a CSRF-hardening measure — a cross-site `<form>` POST can never
> compose the JSON shape required to start a workstream.

### `GET /api/feedback/status/{id}`

Poll a launched workstream by its `workstream_id`. Same auth gate.

**Response — `200 OK`** while running:

```json
{ "ok": true, "state": "running", "workstream_id": "recipe-48213" }
```

**Response — `200 OK`** once a PR is produced:

```json
{
  "ok": true,
  "state": "pr",
  "workstream_id": "recipe-48213",
  "repo": "rysweet/Simard",
  "pr": 2637,
  "pr_url": "https://github.com/rysweet/Simard/pull/2637"
}
```

**Response — `200 OK`** on failure:

```json
{ "ok": true, "state": "failed", "workstream_id": "recipe-48213", "reason": "recipe finished but produced no PR" }
```

**Response — `404 Not Found`** for an unknown or unreadable id:

```json
{ "ok": false, "error": "unknown workstream" }
```

!!! note "Poll failures return 404, never leak"
    Every `RecipeLauncher::poll` failure — an unknown id (the real
    `AmplihackRecipeRunner::probe` returns
    `Capability { what: "recipe.probe", detail: "unknown workstream …" }`) or any
    other probe error — maps to a single generic `404 { "error": "unknown
    workstream" }`. The underlying detail is logged server-side via
    `tracing::warn!` only, so no internal path or reason ever reaches the client.
    A `200` is returned only when `poll` succeeds and yields a live
    [`WorkstreamStatus`](#workstreamstatus).

The three `state` values map directly from
[`WorkstreamStatus`](#workstreamstatus) via the pure `status_json` mapper; the
endpoint additionally echoes the polled `workstream_id`:

| `WorkstreamStatus` | `state` | Extra fields |
|--------------------|---------|--------------|
| `Running` | `running` | — |
| `ProducedPr { repo, pr }` | `pr` | `repo`, `pr`, `pr_url` |
| `Failed { reason }` | `failed` | `reason` |

## Launcher reuse

The handler is a thin adapter over the **existing** Overseer launch plumbing.
No new subprocess or shell-out code is introduced.

- **Trait:** `crate::overseer::capabilities::RecipeLauncher`
  ```rust
  pub trait RecipeLauncher {
      fn launch(&self, brief: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError>;
      fn poll(&self, handle: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError>;
  }
  ```
- **Production implementation:** `crate::overseer::launch::SmartOrchestratorLauncher`
  (built with `SmartOrchestratorLauncher::from_env()`), which runs
  `amplihack recipe run amplifier-bundle/recipes/smart-orchestrator.yaml` with
  `AMPLIHACK_AGENT_BINARY` preserved (Copilot/Claude parity), captures output,
  and extracts the resulting `github.com/<owner>/<repo>/pull/<n>` reference on
  completion.

The dashboard holds a single shared launcher (a `OnceLock<SmartOrchestratorLauncher>`)
so the `POST` and status handlers observe the same stateful runner — the one that
knows which subprocess each `workstream_id` maps to — across axum's worker threads.

!!! note "One required upstream change"
    Sharing the launcher across axum's multithreaded handlers requires it to be
    `Send + Sync`. That is the **single** production edit needed in the reused
    plumbing: add `Send + Sync` as a supertrait to the `RecipeRunner` trait
    (`pub trait RecipeRunner: Send + Sync`, `src/overseer/launch.rs`), which the
    `SmartOrchestratorLauncher`'s `Box<dyn RecipeRunner>` field then inherits.
    Both the real runner (`AmplihackRecipeRunner`, whose only state is a
    `Mutex<HashMap<..>>`) and every test `FakeRunner` already satisfy the bound,
    so no other impl is affected.

### `RecipeBrief`

The handler composes one brief per submit:

```rust
RecipeBrief {
    task_description: /* composed from report + context, see below */,
    target_repo: /* Simard's own slug — the dashboard is a Simard surface */,
    sequence_group: None,
}
```

### `WorkstreamHandle`

```rust
pub struct WorkstreamHandle { pub id: String }
```

### `WorkstreamStatus`

```rust
pub enum WorkstreamStatus {
    Running,
    ProducedPr { repo: String, pr: u32 },
    Failed { reason: String },
}
```

## `task_description` composition

The `task_description` is built by a **pure, unit-tested** function from the
validated report and captured context. It is assembled as plain data and passed
as a single `String` argument, so there is **no shell interpolation** anywhere on
the path (see [Security](#security-model)). The template is deterministic:

```
[BUG] Costs tab shows $0 after midnight rollover

After the daily ledger rolls over, the Costs tab renders $0.00 for every
provider until the first new call lands. Expected: it should show the prior
day's total until the new day has spend.

--- Operator feedback (untrusted input; captured from the dashboard) ---
Page/tab: costs
Timestamp: 2026-07-06T02:41:10Z
Identifiers: {"hash":"#costs","url":"/","doc_title":"Simard · Costs"}
Page state:
{"providers":[{"name":"anthropic","model":"claude-opus","usd":0.0}]}

Filed from the Simard dashboard feedback widget (issue #2629). Please triage
this operator report and, if actionable, address the bug or build the requested
feature following the default development workflow.
```

Notes:

- The first line is `[BUG]` or `[FEATURE]` followed by the title.
- The description follows verbatim (already length-capped and control-char
  stripped).
- The context block is clearly delimited and **labelled as untrusted input** so
  the downstream orchestrator treats operator text as data, not instructions.
- `Identifiers` is the captured identifiers object rendered as compact JSON.
- The rendered `state` is the ≤ 16 KiB truncated snapshot.
- A closing instruction directs the orchestrator to triage the report under the
  default development workflow.

## Validation

Server-side validation is authoritative (the client mirrors it only for UX):

| Rule | Limit | On violation |
|------|-------|--------------|
| `report.type` | exactly `bug` or `feature` | `400 invalid type` |
| `report.title` | non-empty, ≤ 200 chars (after trim) | `400 title required` / `title too long` |
| `report.description` | non-empty, ≤ 5000 chars (after trim) | `400 description required` / `description too long` |
| `context.state` | truncated to ≤ 16 KiB on a char boundary | silently truncated |
| `context.identifiers` | serialized JSON truncated to ≤ 4 KiB | silently truncated |
| all text | control characters stripped | silently sanitized |
| request body | missing / non-JSON / form-encoded / oversized | `400 invalid report` (coerced to empty by the `Option<Json<Value>>` extractor) |

## De-duplication & throttle

To keep an accidental double-click — or an operator retrying — from spawning
duplicate workstreams, the handler keeps ephemeral in-memory state (a
`OnceLock<FeedbackDedup>` wrapping a `Mutex` over a recent-report map keyed on
`hash(type | title | description)` plus a rolling list of accepted-launch
instants):

- A **duplicate** within ~30 s returns `429 { "error": "duplicate" }` and does
  **not** launch.
- A **distinct-launch concurrency cap** protects against a flood of *different*
  reports (which de-dup alone would not stop); over the cap returns
  `429 { "error": "busy" }`.

!!! warning "This path bypasses the Overseer's budget & per-cycle gates"
    When the Overseer launches a workstream it first passes `Overseer::gate`
    (the per-cycle launch cap **and** the daily-budget gate). The dashboard
    calls `SmartOrchestratorLauncher::launch()` **directly**, so *neither* of
    those ceilings applies here — the launcher itself
    (`AmplihackRecipeRunner`) spawns unconditionally. Each accepted submission
    is a full, cost-bearing `smart-orchestrator` run. On this surface the **only**
    cost-DoS controls are therefore the auth gate, the ~30 s de-dup window, and
    the handler's own distinct-launch concurrency cap. Size that cap
    conservatively, and treat it (not any Overseer ceiling) as the authoritative
    throttle. The [Concurrent Engineer Dispatch](./concurrent-engineer-dispatch.md)
    ceilings govern the Overseer loop, not this direct path.

All feedback state is **ephemeral** — there is no database, no migration, no
schema. The only durable output is the GitHub PR the workstream opens. (Note:
the launcher's in-memory run table retains each launched child until process
exit; a long-lived dashboard should evict terminal `ProducedPr`/`Failed`
entries on poll to keep that map bounded.)

## UI feedback

After a successful `POST`, the modal switches to a status line and begins
polling `poll`:

- **Running:** "Workstream `recipe-48213` started — waiting for a PR…"
- **PR ready:** the workstream id plus a **link to the PR**
  (`rysweet/Simard#2637`), rendered only after the URL passes the
  `github.com/…/pull/<n>` allow-list check.
- **Failed:** a short, generic failure line (the internal reason is not
  surfaced beyond `WorkstreamStatus::Failed.reason`).

## Security model

| Concern | Mitigation |
|---------|-----------|
| **AuthN/Z** | Both routes are registered before `.layer(require_auth)` → auto-gated, fail-closed. No new auth code; the client sends `credentials: "same-origin"`. |
| **CSRF** | The endpoint accepts **only** `application/json` (form-encoding is rejected) and the session cookie is `SameSite=Strict`. |
| **OS-command injection** | Impossible by construction: the launcher uses `Command::args(Vec<String>)` (no shell). The `task_description` is one argument. A regression test submits a `; rm -rf /` payload and asserts it appears only as inert text. |
| **Prompt injection** | The captured context and operator text are delimited and labelled *untrusted* in the `task_description`. The downstream workstream retains all existing guardrails: no `git --no-verify`, no `gh pr merge --admin`, CI must be green, and a human merges. |
| **Input DoS** | Title/description length caps, `state` truncation to 16 KiB, `identifiers` truncation to 4 KiB, and coercion of any missing/non-JSON/oversized body to `400` — plus the de-dup window and distinct-launch concurrency cap. |
| **Client XSS** | All responses render via `textContent`/DOM APIs; the PR URL is validated against `^https://github\.com/[^/]+/[^/]+/pull/\d+$` before assignment. |
| **Data protection** | Feedback is ephemeral (no DB). Logs use `tracing::warn!` with generic messages — never the full body, cookie, or login code. Captured `state` is page-scoped and size-bounded; keep it free of secrets. |

!!! info "Known limitation — cookie lacks `Secure`"
    The dashboard session cookie (`auth.rs`) is set `HttpOnly; SameSite=Strict`
    but **not** `Secure`. That is acceptable for the default loopback deployment
    and is unchanged by this feature. If the dashboard is ever served over TLS
    to a non-loopback origin, add `Secure` to the cookie so it is not exposed on
    a downgraded connection. This is a pre-existing, out-of-scope finding noted
    here for completeness.

## Testing

The feature ships with **hermetic** tests (no subprocess, no network) that
inject a fake `RecipeLauncher`:

- **Launch contract** — `handle_feedback` with a fake launcher asserts the
  composed `task_description` carries `[BUG]`/`[FEATURE]`, the title,
  description, and the page context; asserts the correct `target_repo`; and
  asserts no real `amplihack` process is spawned.
- **`compose_task_description`** — pure unit test pins the template.
- **Validation matrix** — bad `type`, empty/oversized title and description,
  and body-limit behaviour.
- **Injection safety** — a `; rm -rf /` payload is proven inert (argument, not
  shell).
- **De-dup** — a repeated identical report returns `429 duplicate`.
- **Concurrency throttle** — over-cap distinct reports return `429 busy`.
- **Status mapping** — `Running` / `ProducedPr` / `Failed` map to
  `running` / `pr` / `failed`; unknown id → `404`; launcher error → generic
  `500`.
- **Widget presence** — the rendered dashboard HTML
  (`index_html_string()`) contains the feedback button, the modal, and the
  `POST /api/feedback` client JS, so the control is present on every tab.

## See also

- [Dashboard](../dashboard.md) — the operator dashboard overview and tabs.
- [How to report a bug or request a feature from the dashboard](../howto/report-a-bug-or-request-a-feature.md) — task-oriented walkthrough.
- [Recipe Context-Var Sanitization](./recipe-context-var-sanitization.md) — how `-c key=value` context vars are kept injection-safe.
- [Concurrent Engineer Dispatch](./concurrent-engineer-dispatch.md) — the concurrency ceilings that govern the **Overseer** loop. These do **not** apply to this direct launch path; the handler's own cap is the throttle here (see [De-duplication & throttle](#de-duplication-throttle)).
- [Engineer-Loop Agent Orchestration](../architecture/engineer-agent-orchestration.md) — what a launched `dev-orchestrator` workstream does end-to-end.
