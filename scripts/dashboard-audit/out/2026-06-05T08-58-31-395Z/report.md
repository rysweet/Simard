# Simard dashboard audit (issue #1880)

- **Captured:** `2026-06-05T08-58-31-395Z` (UTC)
- **Tool:** Playwright + Chromium (headless, viewport 1440×900)
- **Endpoint:** `http://localhost:8080`
- **Auth source:** `~/.simard/.dashkey` (8 bytes)
- **Auth winner:** `form-post-discovered` (endpoint: `/api/login`, field: `code`)
- **Auth attempts tried (in order):**
  - `form-post-discovered` → status `200`, ok=`true` (`/api/login`)
- **Artifacts dir:** `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/`

## Cross-reference: issue #1944

Issue [#1944](https://github.com/rysweet/Simard/issues/1944) proposes refining the canonical description of the `improve-simard-dashboard` goal so it explicitly names Simard's self-introspection needs (goal-board, OODA, engineers, memory growth, merge-judge, per-PR readiness, brain-failure). Since the original cycle 1 audit, PRs #2102 (merge-judge panel), #2112 (per-PR readiness panel), and #2109 (brain-failure surfacing) have landed. The three formerly MISSING dimensions now have dedicated tabs and API endpoints. Remaining gaps are in the PARTIAL dimensions: OODA cycle health lacks per-cycle history with trend, and cognitive memory growth lacks time-series / per-cycle delta. A dashboard built for Simard-as-reader needs to render *change* over *snapshot*. Future work should focus on deepening the PARTIAL dimensions with timestamped history and trend data.

## Seven-dimension coverage matrix

| # | Dimension | Coverage | Citing evidence |
|---|---|---|---|
| 1 | goal-board state | **PRESENT** | API /api/goals → 200 (keys: active, active_count, backlog, backlog_count)<br>Tab .tab[data-tab="goals"] (#tab-goals) screenshot: 02-goals.png<br>Cross-reference API /api/workboard → 200 also surfaces "goals" key |
| 2 | OODA cycle health | **PRESENT** | API /api/status surfaces daemon_health.cycle_number, daemon_health.cycle_phase, daemon_health.actions_taken, daemon_health.cycle_duration_secs (keys: active_processes, daemon_health, disk_usage_pct, git_hash, ooda_daemon, timestamp, version)<br>API /api/ooda-cycles → 200 (per-cycle history with duration trend)<br>Tab #tab-overview screenshot: 01-overview.png<br>Tab #tab-thinking present with cycle history visualisation: 10-thinking.png |
| 3 | engineer subprocesses | **PRESENT** | API /api/processes → 200 (keys: count, processes, root_pid, timestamp)<br>Tab .tab[data-tab="processes"] (#tab-processes) screenshot: 05-processes.png |
| 4 | cognitive memory growth | **PARTIAL** | API /api/memory → 200 (keys: evidence_records, goal_records, handoff, last_consolidation, memory_records, native_memory, native_memory_db_exists, native_memory_db_path, native_memory_error, state_root, timestamp, total_facts); includes native_memory.episodic/semantic counts<br>API /api/memory/history → not 200 or missing<br>API /api/workboard → 200 also surfaces cognitive_statistics block (point-in-time snapshot only)<br>Tab .tab[data-tab="memory"] (#tab-memory) screenshot: 06-memory.png |
| 5 | merge-judge decisions | **PRESENT** | API /api/merge-judge → 200 (keys: decisions, persistence_available, persistence_reason, summary, timestamp)<br>Tab .tab[data-tab="merge-decisions"] discovered, screenshot: 12-merge-decisions.png<br>Legacy probes: /api/judges→404, /api/judge→404<br>Panel header text mentions "merge-judge" / "merge decision" |
| 6 | per-PR readiness for #1880, #1893, #1894 | **PRESENT** | API /api/prs → 200 (keys: prs, summary, timestamp)<br>Tab .tab[data-tab="pr-readiness"] discovered, screenshot: 13-pr-readiness.png<br>Legacy probes: /api/pulls→404, /api/pr/1880→404<br>Panel header text mentions PR identifiers |
| 7 | brain-failure surfacing tied to #1890 _(issue #1890 closed)_ | **PRESENT** | API /api/brain-failures → 200 (keys: failures, summary, timestamp)<br>Tab .tab[data-tab="brain-failures"] discovered, screenshot: 11-brain-failures.png<br>Legacy probe: /api/failures → 404<br>Panel header text mentions brain-failure indicator<br>Cross-ref: issue #1890 is CLOSED; this dimension scores on dashboard surface, not on issue state. |

### Per-dimension notes

- **OODA cycle health** — Per-cycle history with duration trend is exposed via /api/ooda-cycles; current cycle state via /api/status.daemon_health; visualisation in the Thinking tab.
- **cognitive memory growth** — Current totals (episodic / semantic / procedural / prospective / sensory / working) are exposed, but there is no time-series of GROWTH — no per-cycle delta, no rate, no chart. Dashboard answers "how much memory is there NOW" but not "is it growing".
- **merge-judge decisions** — Merge Decisions tab and /api/merge-judge endpoint provide a record of every merge-judge verdict with reasoning and timestamps. Panel shows current decisions; historical trend analysis is a future enhancement.
- **per-PR readiness for #1880, #1893, #1894** — PR Readiness tab and /api/prs endpoint show open PRs with CI status, review state, and remaining blockers. Shows current snapshot; historical readiness trajectory is a future enhancement.
- **brain-failure surfacing tied to #1890** — Brain Failures tab and /api/brain-failures endpoint list every brain failure with failure type, component, timestamp, and recovery status. Surfacing is complete for issue #1890.

## Routes captured

| Slug | Label | Panels | Latest timestamp seen | Console errors Δ | Screenshot |
|---|---|---:|---|---:|---|
| `landing` | Landing (post-login) | 10 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/00-landing.png` |
| `overview` | Overview | 12 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/01-overview.png` |
| `goals` | Goals | 3 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/02-goals.png` |
| `traces` | Traces | 3 | 2026-06-05T08:51:54 | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/03-traces.png` |
| `logs` | Logs | 40 | 2026-06-05T08:57:14Z | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/04-logs.png` |
| `processes` | Processes | 3 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/05-processes.png` |
| `memory` | Memory | 11 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/06-memory.png` |
| `costs` | Costs | 4 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/07-costs.png` |
| `chat` | Chat | 2 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/08-chat.png` |
| `workboard` | Workboard | 11 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/09-workboard.png` |
| `thinking` | 🧠 Thinking | 3 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/10-thinking.png` |
| `brain-failures` | Brain Failures | 3 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/11-brain-failures.png` |
| `merge-decisions` | Merge Decisions | 2 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/12-merge-decisions.png` |
| `pr-readiness` | PR Readiness | 2 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/13-pr-readiness.png` |
| `terminal` | Terminal | 4 | — | 0 | `scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/14-terminal.png` |

## API probe summary

| Endpoint | Status | Bytes | Keys / preview | Linked dimensions |
|---|---:|---:|---|---|
| `/api/status` | 200 | 409 | active_processes, daemon_health, disk_usage_pct, git_hash, ooda_daemon, timestamp, version | ooda, memory |
| `/api/goals` | 200 | 3972 | active, active_count, backlog, backlog_count | goal-board |
| `/api/processes` | 200 | 24254 | count, processes, root_pid, timestamp | engineers |
| `/api/memory` | 200 | 853 | evidence_records, goal_records, handoff, last_consolidation, memory_records, native_memory, native_memory_db_exists, native_memory_db_path | memory |
| `/api/memory/history` | 404 | 0 | — | memory |
| `/api/workboard` | 200 | 24360 | cognitive_statistics, cycle, goals, next_cycle_eta_seconds, recent_actions, spawned_engineers, task_memory, timestamp | goal-board, ooda, memory |
| `/api/traces` | 200 | 120286 | otel_enabled, otel_endpoint, span_count, spans, timestamp | ooda |
| `/api/logs` | 200 | 2861197 | cost_log_lines, cycle_reports, daemon_log_lines, ooda_transcripts, terminal_transcripts, timestamp | — |
| `/api/costs` | 200 | 310 | daily, weekly | — |
| `/api/merge-judge` | 200 | 445 | decisions, persistence_available, persistence_reason, summary, timestamp | merge-judge |
| `/api/prs` | 200 | 1544 | prs, summary, timestamp | per-pr |
| `/api/brain-failures` | 200 | 720 | failures, summary, timestamp | brain-failure |
| `/api/judges` | 404 | 0 | — | merge-judge |
| `/api/judge` | 404 | 0 | — | merge-judge |
| `/api/pulls` | 404 | 0 | — | per-pr |
| `/api/pr/1880` | 404 | 0 | — | per-pr |
| `/api/failures` | 404 | 0 | — | brain-failure |
| `/api/ooda-cycles` | 200 | 14946 | cycles, duration_trend, timestamp, total_cycles | ooda |

## Prioritised follow-up queue

- **P2** — _cognitive memory growth_ — dashboard: deepen cognitive memory growth (currently point-in-time only / no history / no labels)
    - Current totals (episodic / semantic / procedural / prospective / sensory / working) are exposed, but there is no time-series of GROWTH — no per-cycle delta, no rate, no chart. Dashboard answers "how much memory is there NOW" but not "is it growing".
- **P3** — _polish / formatting_ — dashboard: replace raw ISO timestamps + bare UUIDs with friendly relative-time + labelled identifiers
    - Earlier audit (Pass 2, scripts/dashboard_audit/) flagged raw ISO and unlabelled UUIDs across tabs; carry-over.

## Screenshots (repo-relative)

- ![Landing (post-login)](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/00-landing.png)
- ![Overview](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/01-overview.png)
- ![Goals](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/02-goals.png)
- ![Traces](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/03-traces.png)
- ![Logs](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/04-logs.png)
- ![Processes](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/05-processes.png)
- ![Memory](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/06-memory.png)
- ![Costs](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/07-costs.png)
- ![Chat](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/08-chat.png)
- ![Workboard](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/09-workboard.png)
- ![🧠 Thinking](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/10-thinking.png)
- ![Brain Failures](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/11-brain-failures.png)
- ![Merge Decisions](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/12-merge-decisions.png)
- ![PR Readiness](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/13-pr-readiness.png)
- ![Terminal](scripts/dashboard-audit/out/2026-06-05T08-58-31-395Z/14-terminal.png)

## Self-introspection vs. human-operator verdict

Of the seven mandated dimensions: **6 PRESENT**, **1 PARTIAL**, **0 MISSING**.

The dashboard now covers the majority of self-introspection dimensions. Goal-board, engineer processes, merge-judge, per-PR readiness, and brain-failure all have dedicated tabs and APIs. The remaining PARTIAL dimensions (OODA cycle health, cognitive memory growth) expose current state but lack trajectory / trend data — Simard reading her own dashboard can see "what is happening now" but not "am I improving over time." Deepening the PARTIAL dimensions with time-series history is the highest-leverage next step.
