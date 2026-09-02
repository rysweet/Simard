---
title: Dashboard — memory growth rate uses a bounded 24 h window
description: Reference for the Memory Growth panel's "long-term mem/hr" rate on the dashboard, which is now measured over a bounded trailing 24 h window ending at the newest snapshot instead of being averaged across the entire retained snapshot ring buffer. GET /api/memory/history computes rate_per_hour by diffing the newest snapshot against the oldest snapshot still inside the 24 h window (edge inclusive) and reports 0.0 when no prior sample lies inside the window. The response also carries rate_window_secs so consumers can see the window width. Regression unit tests and an outside-in gadugi scenario pin the window semantics so the rate cannot silently regress to a diluted whole-history figure.
last_updated: 2026-07-15
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ../memory.md
  - ./dashboard-memory-tab.md
  - ./dashboard-memory-recent-last-hour-count.md
---

# Dashboard — memory growth rate uses a bounded 24 h window

The Memory Growth panel shows a large **"long-term mem/hr (24 h)"** rate
(`#mem-growth-rate`, `index_html/part_03.rs`) and derives the growth-trend badge
from it. That rate is the backend field `GET /api/memory/history` →
`rate_per_hour.long_term_total`. It now measures **recent** memory formation over
a bounded trailing **24 hour** window, not the whole retained history
([#4107](https://github.com/rysweet/Simard/issues/4107)).

> **What changed.** The card looks the same; the caption gains a `(24 h)`
> qualifier. The backend field that feeds it is now computed over a bounded
> trailing window instead of the entire multi-week snapshot buffer. This is a
> **data fix to an existing card**, not a new surface.

## Root cause (what was wrong)

`rate_per_hour()` in `operator_commands_dashboard/memory.rs` used
`snapshots[0]` (the **oldest retained** sample) and `snapshots[last]` (newest)
and divided the delta by the **full** elapsed span. The retained ring buffer
holds up to `HISTORY_MAX_SNAPSHOTS = 500` samples spanning weeks — including
multi-day daemon-down gaps — so the advertised "per hour" figure was diluted to
a near-zero, meaningless value. The docstring even claimed it used entries
"within the window", but no window was applied.

Observed live: `rate_per_hour.long_term_total = -0.366/hr`, computed as
`(1926 − 2275) / (39 days in hours)`, while an active hour of memory formation
was producing tens of long-term memories.

## The fix

`rate_per_hour()` now:

1. anchors the window at the **newest** snapshot's `epoch_secs`;
2. selects the **oldest snapshot still inside** the trailing
   `GROWTH_RATE_WINDOW_SECS = 86 400` (24 h) window
   (`epoch_secs >= newest − window`, **edge inclusive**), excluding the newest
   itself;
3. computes `(newest − baseline) / hours_between` per rail; and
4. reports **`0.0`** ("insufficient recent data") when no prior sample lies
   inside the window (a fresh daemon after a long gap) or the in-window pair has
   a zero-length span.

Samples older than the window can no longer dilute the denominator. This mirrors
the bounded-window discipline of `select_last_hour_baseline`
([#2679](https://github.com/rysweet/Simard/issues/2679)).

The `GET /api/memory/history` response additionally carries
`rate_window_secs: 86400` so a consumer can see the exact window width the rate
was measured over.

## Regression coverage

- **Unit** (`operator_commands_dashboard::memory::tests_memory_history`):
  an ancient baseline outside the window is ignored; only-newest-in-window
  reports `0.0`; the window edge is inclusive; the oldest in-window sample is the
  chosen baseline.
- **Outside-in** (`tests/qa-scenarios/dashboard-memory-growth-rate-window.yaml`
  → `scripts/qa-dashboard-memory-growth-rate-window.sh`): against a real
  standalone dashboard seeded with an ancient baseline plus recent in-window
  samples, the served rate equals an independent 24 h-windowed recomputation
  from `/api/memory/history` and **differs** from the naive whole-history rate.
  Validated with `gadugi-test validate` and run with `gadugi-test run`.
