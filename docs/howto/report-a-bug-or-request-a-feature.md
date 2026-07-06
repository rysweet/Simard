---
title: How to report a bug or request a feature from the dashboard
description: Use the dashboard feedback widget to report a bug or request a feature directly from any tab. Simard captures the page context, starts a new dev-orchestrator workstream, and surfaces the resulting PR — all behind the existing dashboard access-code gate.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../dashboard.md
  - ../reference/dashboard-feedback-widget.md
  - ../howto/spawn-engineers-from-ooda-daemon.md
  - ../architecture/engineer-agent-orchestration.md
---

# How to report a bug or request a feature from the dashboard

Every dashboard tab has a **Report bug / Request feature** control in the top
header. Use it to file a defect or a change request **from the page you're
looking at**. Simard snapshots the page's context, starts a new
`dev-orchestrator` workstream (the same `default-workflow` engineers run), and
shows you the pull request when it opens — without you leaving the dashboard.

## Prerequisites

- The dashboard is running and you are **logged in** (the widget and its
  endpoints sit behind the existing access-code gate):

  ```bash
  simard dashboard serve --port=8080
  ```

  Open `http://localhost:8080/`, enter the login code
  (printed on first start, also stored in `~/.simard/.dashkey`), and you'll land
  on the dashboard with a session cookie set. See
  [Dashboard](../dashboard.md) for details.

## Steps

### 1. Open the feedback form

On any tab, click **💬 Feedback** in the top-right of the header (next to
**Glossary** and **Releases**; its tooltip reads *Report bug / Request
feature*). A modal opens.

The control is in the shared header, so it looks and behaves identically on
every tab — Overview, Goals, Traces, Logs, Processes, Memory, Costs, Chat,
Workboard, Thinking, and the rest.

### 2. Fill in the report

| Field | What to enter |
|-------|---------------|
| **Type** | `Bug` for something broken, `Feature` for something you want added. |
| **Title** | A short summary (≤ 200 characters). |
| **Description** | What happened and what you expected — or what you want built (≤ 5000 characters). |

You do **not** need to describe which page you're on or paste the data you're
looking at — Simard captures that automatically in the next step.

### 3. Submit

Click **Start workstream**. The widget:

1. Captures the **current page context** — the active tab, the key state/JSON
   the page is rendering, a timestamp, and page identifiers (the page slug, the
   URL path, the URL hash, and the document title).
2. Sends `{report, context}` to `POST /api/feedback` (authenticated with your
   session cookie).
3. Starts a new workstream and shows you a **workstream id**.

### 4. Watch for the PR

The modal switches to a status line and polls automatically:

- **Running** — "Workstream `recipe-…` started — waiting for a PR…"
- **PR ready** — a link to the pull request (for example
  `rysweet/Simard#2637`). Click through to review it.
- **Failed** — a short message if the run couldn't produce a PR.

That's it. The workstream runs the standard `default-workflow`: it branches off
`main`, implements the change, and opens a PR with **CI required to be green**
and a **human** to merge. Your report and the captured page context are written
into the workstream's task description so the engineer starts with real context.

## What gets captured (and what doesn't)

**Captured:** the active tab slug, the visible/rendered state of that panel
(size-bounded to 16 KiB), a timestamp, and page identifiers.

**Not captured:** your login code or session cookie (the cookie is `HttpOnly`
and unreadable to the widget). Even so, treat the description and any on-page
data as content that may appear in a **public PR** — don't paste secrets.

## Notes and limits

- **Each accepted report starts a real, cost-bearing run.** A submission that
  isn't a duplicate launches a full `dev-orchestrator` workstream — there is no
  budget gate on this path, so submit deliberately rather than in bursts.
- **One at a time per report.** Submitting the *same* report (same type, title,
  and description) twice within ~30 seconds is de-duplicated — you'll get a
  "duplicate" notice instead of a second workstream. This absorbs accidental
  double-clicks.
- **Busy signal.** If many *different* reports are launched at once and the
  concurrency cap is saturated, you'll get a short "busy" notice — try again in
  a moment.
- **JSON only.** The endpoint accepts JSON from the in-page widget; it does not
  accept HTML form posts (a CSRF-hardening measure).

## Troubleshooting

| Symptom | Cause / fix |
|---------|-------------|
| Clicking the button does nothing / redirects to login | Your session expired. Log in again, then reopen the form. |
| "duplicate" notice | You submitted the same report within ~30 s. Change the text or wait, then resubmit. |
| "busy" notice | The launch concurrency cap is saturated. Retry shortly. |
| "invalid" / "too long" | Type must be `bug`/`feature`; title ≤ 200 chars; description ≤ 5000 chars. |
| Status stays "running" for a long time | The workstream is still working. Follow the link once the PR appears, or check the [Processes](../dashboard.md) tab for the live subprocess. |

## See also

- [Dashboard Feedback Widget](../reference/dashboard-feedback-widget.md) — full
  API reference (endpoints, schemas, security model, tests).
- [Dashboard](../dashboard.md) — the operator dashboard and its tabs.
- [Engineer-Loop Agent Orchestration](../architecture/engineer-agent-orchestration.md)
  — what a launched `dev-orchestrator` workstream does end-to-end.
