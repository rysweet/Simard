# Simard dashboard audit — cycle 1 (issue #1880)

- **Captured:** `2026-05-19T08-18-24-156Z` (UTC)
- **Tool:** Playwright + Chromium (headless, viewport 1440x900)
- **Endpoint:** `http://localhost:8080`
- **Auth source:** `~/.simard/.dashkey` (8 bytes)
- **Auth winner:** `form-post-discovered` (endpoint: `/api/login`, field: `code`)
- **Auth attempts tried (in order):**
  - `form-post-discovered` → status `200`, ok=`true` (`/api/login`)
- **Artifacts dir:** `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/`

## Cross-reference: issue #1944

Issue [#1944](https://github.com/rysweet/Simard/issues/1944) proposes refining the canonical description of the `improve-simard-dashboard` goal so it explicitly names Simard's self-introspection needs (goal-board, OODA, engineers, memory growth, merge-judge, per-PR readiness, brain-failure). Cycle 1 confirms why that refinement matters: **the current dashboard primarily serves a human operator's diagnostic needs (live tabs, screenshots, log tails) and only weakly serves Simard's self-introspection needs.** Three of the seven mandated dimensions (merge-judge decisions, per-PR readiness, brain-failure surfacing) have neither API nor panel today; two more (OODA cycle health, cognitive memory growth) expose point-in-time data only — no time-series, no per-cycle delta, no rate. A dashboard built for Simard-as-reader needs to render *change* over *snapshot*. Cycle 2 should prioritise the three MISSING dimensions and add timestamped history columns to the PARTIAL ones, in service of #1944's intent.

## Seven-dimension coverage matrix

| # | Dimension | Coverage | Citing evidence |
|---|---|---|---|
| 1 | goal-board state | **PRESENT** | API /api/goals → 200 (keys: active, active_count, backlog, backlog_count)<br>Tab .tab[data-tab="goals"] (#tab-goals) screenshot: 02-goals.png<br>Cross-reference API /api/workboard → 200 also surfaces "goals" key |
| 2 | OODA cycle health | **PARTIAL** | API /api/status surfaces daemon_health.cycle_number, daemon_health.cycle_phase, daemon_health.actions_taken, daemon_health.cycle_duration_secs (keys: active_processes, daemon_health, disk_usage_pct, git_hash, ooda_daemon, timestamp, version)<br>API /api/ooda-cycles → 404 (no per-cycle history endpoint)<br>Tab #tab-overview screenshot: 01-overview.png<br>Tab #tab-thinking present — but classified separately from OODA cycle metrics: 10-thinking.png |
| 3 | engineer subprocesses | **PRESENT** | API /api/processes → 200 (keys: count, processes, root_pid, timestamp)<br>Tab .tab[data-tab="processes"] (#tab-processes) screenshot: 05-processes.png |
| 4 | cognitive memory growth | **PARTIAL** | API /api/memory → 200 (keys: evidence_records, goal_records, handoff, last_consolidation, memory_records, native_memory, native_memory_db_exists, native_memory_db_path, native_memory_error, state_root, timestamp, total_facts); includes native_memory.episodic/semantic counts<br>API /api/workboard → 200 also surfaces cognitive_statistics block (point-in-time snapshot only)<br>Tab .tab[data-tab="memory"] (#tab-memory) screenshot: 06-memory.png |
| 5 | merge-judge decisions | **MISSING** | API probes: /api/judges→404, /api/judge→404, /api/merge-judge→404<br>no panel header mentions merge-judge |
| 6 | per-PR readiness for #1880, #1893, #1894 | **MISSING** | API probes: /api/prs→404, /api/pulls→404, /api/pr/1880→404<br>no panel header mentions #1880 / #1893 / #1894 |
| 7 | brain-failure surfacing tied to #1890 _(issue #1890 closed)_ | **MISSING** | API probes: /api/brain-failures→404, /api/failures→404<br>no panel header mentions brain-failure / EMPTY_RESPONSE_SENTINEL / #1890<br>Cross-ref: issue #1890 is CLOSED; this dimension scores on dashboard surface, not on issue state. |

### Per-dimension notes

- **OODA cycle health** — Cycle number, phase, last summary, and timestamp are exposed via /api/status.daemon_health, but there is no dedicated OODA tab showing phase transitions over time, no /api/ooda-cycles endpoint, and no inline visualisation of the OODA loop.
- **cognitive memory growth** — Current totals (episodic / semantic / procedural / prospective / sensory / working) are exposed, but there is no time-series of GROWTH — no per-cycle delta, no rate, no chart. Dashboard answers "how much memory is there NOW" but not "is it growing".
- **merge-judge decisions** — No dashboard surface for merge-judge: no dedicated tab, no API endpoint, no panel cites it. This is one of the dimensions that PR #1880 was scoped to address; cycle 1 confirms it is unaddressed in the live build.
- **per-PR readiness for #1880, #1893, #1894** — No per-PR readiness surface in cycle 1 build: no /api/prs* endpoints respond 200, no panel header references the target PR numbers. Issue #1944 proposes refining the goal description to include this dimension; cycle 1 confirms the dashboard does not yet serve it.
- **brain-failure surfacing tied to #1890** — Issue #1890 is closed, but the surfacing it was meant to enable is not on the dashboard today: no panel, no API endpoint. Cycle 1 confirms closure ≠ delivery for self-introspection.

## Routes captured

| Slug | Label | Panels | Latest timestamp seen | Console errors Δ | Screenshot |
|---|---|---:|---|---:|---|
| `landing` | Landing (post-login) | 9 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/00-landing.png` |
| `overview` | Overview | 10 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/01-overview.png` |
| `goals` | Goals | 2 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/02-goals.png` |
| `traces` | Traces | 2 | 2026-05-19T08:18:25 | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/03-traces.png` |
| `logs` | Logs | 40 | 2026-05-19T08:18:25.913174572Z | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/04-logs.png` |
| `processes` | Processes | 2 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/05-processes.png` |
| `memory` | Memory | 9 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/06-memory.png` |
| `costs` | Costs | 3 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/07-costs.png` |
| `chat` | Chat | 1 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/08-chat.png` |
| `workboard` | Whiteboard | 10 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/09-workboard.png` |
| `thinking` | 🧠 Thinking | 1 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/10-thinking.png` |
| `terminal` | Terminal | 3 | — | 0 | `scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/11-terminal.png` |

## API probe summary

| Endpoint | Status | Bytes | Keys / preview | Linked dimensions |
|---|---:|---:|---|---|
| `/api/status` | 200 | 405 | active_processes, daemon_health, disk_usage_pct, git_hash, ooda_daemon, timestamp, version | ooda, memory |
| `/api/goals` | 200 | 5601 | active, active_count, backlog, backlog_count | goal-board |
| `/api/processes` | 200 | 17568 | count, processes, root_pid, timestamp | engineers |
| `/api/memory` | 200 | 791 | evidence_records, goal_records, handoff, last_consolidation, memory_records, native_memory, native_memory_db_exists, native_memory_db_path | memory |
| `/api/workboard` | 200 | 24908 | cognitive_statistics, cycle, goals, next_cycle_eta_seconds, recent_actions, spawned_engineers, task_memory, timestamp | goal-board, ooda, memory |
| `/api/traces` | 200 | 99289 | otel_enabled, otel_endpoint, span_count, spans, timestamp | ooda |
| `/api/logs` | 200 | 2156327 | cost_log_lines, cycle_reports, daemon_log_lines, ooda_transcripts, terminal_transcripts, timestamp | — |
| `/api/costs` | 200 | 315 | daily, weekly | — |
| `/api/judges` | 404 | 0 | — | merge-judge |
| `/api/judge` | 404 | 0 | — | merge-judge |
| `/api/merge-judge` | 404 | 0 | — | merge-judge |
| `/api/prs` | 404 | 0 | — | per-pr |
| `/api/pulls` | 404 | 0 | — | per-pr |
| `/api/pr/1880` | 404 | 0 | — | per-pr |
| `/api/brain-failures` | 404 | 0 | — | brain-failure |
| `/api/failures` | 404 | 0 | — | brain-failure |
| `/api/ooda-cycles` | 404 | 0 | — | ooda |

## Prioritised follow-up queue (cycle 2 candidates — NOT filed in cycle 1)

- **P2** — _OODA cycle health_ — dashboard: deepen OODA cycle health (currently point-in-time only / no history / no labels)
    - Cycle number, phase, last summary, and timestamp are exposed via /api/status.daemon_health, but there is no dedicated OODA tab showing phase transitions over time, no /api/ooda-cycles endpoint, and no inline visualisation of the OODA loop.
- **P2** — _cognitive memory growth_ — dashboard: deepen cognitive memory growth (currently point-in-time only / no history / no labels)
    - Current totals (episodic / semantic / procedural / prospective / sensory / working) are exposed, but there is no time-series of GROWTH — no per-cycle delta, no rate, no chart. Dashboard answers "how much memory is there NOW" but not "is it growing".
- **P1** — _merge-judge decisions_ — dashboard: add merge-judge decisions surface
    - No dashboard surface for merge-judge: no dedicated tab, no API endpoint, no panel cites it. This is one of the dimensions that PR #1880 was scoped to address; cycle 1 confirms it is unaddressed in the live build.
- **P1** — _per-PR readiness for #1880, #1893, #1894_ — dashboard: add per-PR readiness for #1880, #1893, #1894 surface
    - No per-PR readiness surface in cycle 1 build: no /api/prs* endpoints respond 200, no panel header references the target PR numbers. Issue #1944 proposes refining the goal description to include this dimension; cycle 1 confirms the dashboard does not yet serve it.
- **P1** — _brain-failure surfacing tied to #1890_ — dashboard: add brain-failure surfacing tied to #1890 surface
    - Issue #1890 is closed, but the surfacing it was meant to enable is not on the dashboard today: no panel, no API endpoint. Cycle 1 confirms closure ≠ delivery for self-introspection.
- **P3** — _polish / formatting_ — dashboard: replace raw ISO timestamps + bare UUIDs with friendly relative-time + labelled identifiers
    - Earlier audit (Pass 2, scripts/dashboard_audit/) flagged raw ISO and unlabelled UUIDs across tabs; carry-over.

## Screenshots (repo-relative)

- ![Landing (post-login)](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/00-landing.png)
- ![Overview](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/01-overview.png)
- ![Goals](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/02-goals.png)
- ![Traces](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/03-traces.png)
- ![Logs](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/04-logs.png)
- ![Processes](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/05-processes.png)
- ![Memory](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/06-memory.png)
- ![Costs](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/07-costs.png)
- ![Chat](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/08-chat.png)
- ![Whiteboard](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/09-workboard.png)
- ![🧠 Thinking](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/10-thinking.png)
- ![Terminal](scripts/dashboard-audit/out/2026-05-19T08-18-24-156Z/11-terminal.png)

## Self-introspection vs. human-operator verdict

The cycle 1 dashboard is **primarily a human-operator console**, not a self-introspection surface. Evidence:
- All seven dimensions are framed in terms a human watching a service would want; only goal-board + processes are first-class today.
- Memory and OODA tabs show *current state*, not *trajectory*. Simard reading her own dashboard cannot answer "am I learning?" or "did my last OODA cycle improve things?" from these surfaces.
- Three dimensions (merge-judge, per-PR readiness, brain-failure) have no surface at all — those are the highest priority for cycle 2 if the goal text in #1944 is adopted.
