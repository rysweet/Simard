---
title: Overseer activity feed reference
description: The bounded, durable "recent Overseer activity" feed — its data model (OverseerActivity / OverseerActivityRecord / OverseerThreadStatus), the cross-process activity.json file contract the daemon writes each tick and the status/dashboard/TUI read, the StatusSnapshot.overseer section, the auth-gated GET /api/overseer dashboard endpoint, the dashboard Overseer tab and TUI Overseer pane, and the honest disabled/observing/absent states.
last_updated: 2026-07-05
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./status-snapshot-api.md
  - ./telemetry-metrics.md
  - ./stewardship-api.md
  - ./overseer-goal-board-health-api.md
  - ./cognitive-thread-scheduling.md
  - ../howto/watch-overseer-activity.md
  - ../howto/simard-status.md
  - ../dashboard.md
  - ./simard-tui.md
  - ./operator-read-state-root-contract.md
---

# Overseer activity feed reference

Simard's cognition runs on two clocks. The **engineer** side runs the OODA loop
that picks up goals, writes code, and opens PRs — and it is already visible on
the dashboard (goals, live engineers) and in the TUI. The **steward** side —
the acting **Overseer** meta-loop — runs *alongside* it on its own cadence,
watching the whole system and quietly intervening: filing stewardship issues,
launching fix workstreams, verifying and merging green PRs, running guarded
deploys, escalating to the operator, or *holding* when a gate says "not yet".

Until now that steward activity was invisible outside the daemon's log. The
**Overseer activity feed** makes it a first-class, queryable surface: the last
`N` Overseer ticks and their outcomes, plus per-thread status, readable from the
**dashboard** (`localhost:8080`), the **TUI**, and `simard status`.

> **This feed only *reads and surfaces* Overseer activity.** It does not change
> what the Overseer decides or does. The producer touches the Overseer only at
> the tick boundary, to *record* what already happened.

> **Modules:** producer + model `src/overseer/activity.rs` (types + store),
> `src/overseer/wiring.rs` (`OverseerTickReport`), the daemon tick boundary
> (`src/operator_commands_ooda/daemon/mod.rs`); reader `src/status/provider.rs`
> (`assemble_overseer`), `src/status/render.rs` (`OVERSEER` section); surfaces
> `src/operator_commands_dashboard/overseer.rs` (`GET /api/overseer`) +
> `.../index_html/` (Overseer tab), `src/bin/simard_tui/tabs/overseer.rs`
> (Overseer pane).

## At a glance

| You want to… | Use |
|---|---|
| Watch Overseer activity in a browser | Dashboard **Overseer** tab (open `http://localhost:8080/`, click **Overseer**) |
| Watch it in the terminal UI | TUI **Overseer** tab (press **`Alt+8`**) |
| See it inline with the rest of status | `simard status` → **OVERSEER** section (also the Status tab / `/api/status/snapshot`) |
| Read it as JSON / script against it | `GET /api/overseer` (auth-gated) |
| Read the raw durable file | `~/.simard/overseer/activity.json` |

All surfaces render the **same** data because they all read the one durable
feed the daemon writes each tick.

## Why a durable file (not an in-RAM ring)

The daemon (which *runs* the Overseer), the dashboard server (`simard dashboard
serve`), and the TUI are **separate processes**. A pure in-memory ring buffer
in the daemon would be unreachable by the other two. So the feed keeps a
bounded in-memory `VecDeque` (cap `N = 100`) as the write surface **and**
persists the whole capped feed atomically to disk each tick. That file is the
cross-process seam every reader consumes — the same pattern the
[telemetry snapshot](./telemetry-metrics.md) and
[status provider](./status-snapshot-api.md) already use.

This mirrors the [operator read / state-root contract](./operator-read-state-root-contract.md):
the daemon is the single writer; every reader degrades to an honest "no data"
state rather than fabricating one.

## Data model

Defined in `src/overseer/activity.rs`. Every struct derives
`Serialize`/`Deserialize` + `Default` and is `#[serde(default)]`, so the schema
grows additively and an older reader tolerates a newer file (and vice-versa).

```rust
/// Bumped only on an INCOMPATIBLE shape change. Additive fields do not bump it.
pub const SCHEMA_VERSION: u32 = 1;

/// The whole feed: top-level Overseer status + per-thread status + recent ticks.
pub struct OverseerActivity {
    pub schema_version: u32,
    pub enabled: bool,                       // overseer_acting_enabled() at last write
    pub cadence_secs: u64,                   // overseer_interval_secs()
    pub author_login: String,                // the Overseer's distinct git identity
    pub last_tick_at: Option<String>,        // RFC3339 of the most recent tick, or None
    pub totals: OverseerTotals,              // summed over the records currently retained
    pub threads: Vec<OverseerThreadStatus>,  // from Mind::health() (#2531)
    pub recent: VecDeque<OverseerActivityRecord>, // newest-first, capped at N=100
}

/// Cumulative outcome counts across the retained records (not since boot).
pub struct OverseerTotals {
    pub problems: u64,
    pub issues_filed: u64,
    pub recipes_launched: u64,
    pub prs_merged: u64,
    pub deploys: u64,
    pub escalations: u64,
    pub held: u64,
    pub errors: u64,
}

/// One Overseer tick: the outcome report plus time + gate context.
pub struct OverseerActivityRecord {
    pub timestamp: String,        // RFC3339, tick completion
    pub enabled: bool,            // gate state at this tick
    pub report: OverseerTickReport,
}

/// Per cognitive thread, derived from ThreadHealth (epochs → RFC3339).
pub struct OverseerThreadStatus {
    pub id: String,               // stable thread id, e.g. "overseer"
    pub enabled: bool,
    pub last_run: Option<String>, // from last_run_epoch, or None
    pub next_due: Option<String>, // from next_run_epoch, or None
    pub last_success: Option<bool>,
    pub consecutive_errors: u32,
    pub backoff_until: Option<String>,
    pub health: String,           // derived label — see below
}
```

`OverseerTickReport` is the existing outcome tally emitted by the Overseer
meta-loop (`src/overseer/wiring.rs`), now `Serialize`/`Deserialize` so it can be
recorded. Its fields are recorded **verbatim** — no interpretation:

| Field | Meaning |
|---|---|
| `problems` | Problems the Overseer *observed* this tick (after de-dup against in-flight work). |
| `issues_filed` | Stewardship issues filed (deduplicated). |
| `recipes_launched` | Fix workstreams launched. |
| `prs_merged` | Green, merge-ready PRs verified **and** merged (normal merge — never admin). |
| `deploys` | Guarded deploys performed through the canary / self-deploy gates. |
| `escalations` | Interventions handed off to the operator. |
| `held` | Interventions a gate held back (autonomy / budget / conflict). |
| `errors` | Capability errors encountered while acting (isolated, never fatal). |
| `panicked` | The tick itself panicked and was isolated. Recorded, not swallowed. |
| `duration_ms` | Wall-clock duration of the tick. |

### `health` derivation

`OverseerThreadStatus.health` is a pure, testable label derived from the raw
fields (first match wins):

| Condition | `health` |
|---|---|
| `!enabled` | `"disabled"` |
| `backoff_until.is_some()` | `"backoff"` |
| `consecutive_errors > 0` | `"erroring"` |
| `last_run.is_none()` | `"idle"` |
| otherwise | `"ok"` |

### Bounded by construction

`recent` is capped at **`N = 100`** ticks, evicting oldest-first, and the cap is
enforced **on write** (before serialize) so the file can never grow unbounded.
`totals` is recomputed from the **retained** records only — it is a rolling
window, not an all-time counter. On read, an 8 MiB size guard and
degrade-to-`None`-on-corrupt parse keep readers safe regardless of the file.

## Durable file contract

- **Path:** `~/.simard/overseer/activity.json` (`<state_root>/overseer/activity.json`).
- **Writer:** the daemon, once per Overseer tick, at the tick boundary — the
  **single** writer.
- **Atomic write:** serialize the whole `OverseerActivity`, write
  `activity.json.tmp` with mode `0o600` under a `0o700` parent, `fsync`, then
  `rename` over the target. Readers never see a torn or briefly world-readable
  file. (Mirrors `telemetry::snapshot::write_atomic`.)
- **Read:** `is_file` check → 8 MiB size guard → `serde` parse; any of missing /
  oversized / corrupt / permission-denied / unknown-higher-`schema_version`
  degrades to `None` (never a panic, never a fabricated value).
- **Write failure is non-fatal.** If the write fails, the daemon logs a
  structured `tracing::warn!(target: "overseer.activity", …)` and the tick
  continues untouched. The feed simply reads `stale` next time — surfaced
  honestly, never silently.

### Freshness rule

A reader computes freshness from the last tick time versus the cadence:

| Condition | Freshness |
|---|---|
| `now − last_tick_at ≤ 2 × cadence_secs` | `live` |
| `now − last_tick_at > 2 × cadence_secs` | `stale` (with `as_of = last_tick_at`) |
| no file / `last_tick_at` absent | `absent` |

> **Why not the shared `SNAPSHOT_FRESHNESS_SECS`?** The telemetry snapshot uses a
> *fixed* 300 s stale threshold, but the Overseer ticks on its own (default
> 15-minute) cadence. Reusing the fixed 300 s constant here would mark a
> perfectly healthy feed `stale` for two-thirds of every cadence window. This
> section therefore uses a **cadence-relative** window (`2 × cadence_secs`); the
> implementation must **not** reuse `SNAPSHOT_FRESHNESS_SECS` for it.

## `StatusSnapshot.overseer` section

The feed is exposed as a normal section on the one typed
[`StatusSnapshot`](./status-snapshot-api.md), so it lights up **every** status
surface (CLI, Status tab, TUI Status tab) for free — not just the dedicated tab:

```rust
pub struct StatusSnapshot {
    // … existing sections …
    #[serde(default)]
    pub overseer: SectionEnvelope<OverseerActivity>,
}
```

`status::provider::assemble_overseer(state_root)` reads the durable file and
returns the section, using `overseer_acting_enabled()` to distinguish the honest
states below. Like every section it is assembled in **isolation** and never
panics. `status::render` adds an `OVERSEER` header to `SECTION_HEADERS` and a
`render_overseer()` that prints the honest header line, per-thread rows, and a
newest-first timeline.

## Honest states

The feed never shows an empty or misleading panel. Four states are rendered
plainly and are covered by tests:

| Real situation | availability / freshness | `data.enabled` | Rendered as |
|---|---|---|---|
| Acting Overseer **disabled** (`SIMARD_OVERSEER_ENABLED` falsey) | `ok` / `live` | `false` | `Overseer: disabled` |
| Enabled but **never ticked** (no file yet) | `unavailable` / `absent` | — | `Overseer: no ticks recorded yet` |
| Enabled, ticked, **zero interventions** | `ok` / `live` (or `stale`) | `true` | `Overseer: enabled, observing, 0 interventions` |
| Feed file **unreadable / corrupt** | `unavailable` / `absent` | — | `Overseer activity feed unavailable` |

**Disabled is a *present* state**, distinct from *absent*: the UI can say
"disabled" without pretending there is simply no data. "Zero interventions" is
stated explicitly rather than shown as a blank list — observing-and-holding is a
real, truthful outcome.

## Dashboard endpoint — `GET /api/overseer`

A new handler in `src/operator_commands_dashboard/overseer.rs`, registered in
`routes.rs` **behind** the existing `require_auth` layer — it is **not** a new
auth path. It accepts the **same** credentials as every other `/api/*` route:
the `simard_session` cookie set by the dashboard login, or an
`Authorization: Bearer <SIMARD_DASHBOARD_TOKEN>` header for scripted reads.
Anything else returns **401**.

It reuses the one `status::assemble` provider on a blocking thread
(`spawn_blocking`) and returns only the `overseer` section, so the dedicated tab
and the Status tab can never diverge. It **degrades to an error object at HTTP
200, never a 500** — a join/serialize failure returns `{"error": "<detail>"}`,
which the SPA shows as a soft banner while keeping the last good render.

- Method: `GET` · Path: `/api/overseer` · No path params, query params, or body.
- Auth: inherited `require_auth`. 401 without a valid session/token.
- Network exposure: inherits the dashboard bind address; restrict the network
  path (loopback / firewall / SSH tunnel) as for any other dashboard route.

### Response (HTTP 200)

```jsonc
{
  "schema_version": 1,                       // overseer::activity::SCHEMA_VERSION
  "generated_at": "2026-07-05T15:31:39Z",    // RFC3339, snapshot assembly time
  "section": {                               // serialized SectionEnvelope<OverseerActivity>
    "availability": "ok",                    // "ok" | "unavailable" | "error"
    "freshness": "live",                     // "live" | "stale" | "absent"
    "as_of": "2026-07-05T15:30:00Z",         // last tick time (omitted if none)
    "note": null,                            // honest-state text (omitted if none)
    "data": {
      "schema_version": 1,
      "enabled": true,
      "cadence_secs": 900,
      "author_login": "simard-overseer[bot]",
      "last_tick_at": "2026-07-05T15:30:00Z",
      "totals": {
        "problems": 12, "issues_filed": 3, "recipes_launched": 2,
        "prs_merged": 1, "deploys": 0, "escalations": 1, "held": 4, "errors": 0
      },
      "threads": [
        {
          "id": "overseer", "enabled": true,
          "last_run": "2026-07-05T15:30:00Z",
          "next_due": "2026-07-05T15:45:00Z",
          "last_success": true, "consecutive_errors": 0,
          "backoff_until": null, "health": "ok"
        }
      ],
      "recent": [
        {
          "timestamp": "2026-07-05T15:30:00Z",
          "enabled": true,
          "report": {
            "problems": 2, "issues_filed": 1, "recipes_launched": 1,
            "prs_merged": 0, "deploys": 0, "escalations": 0,
            "held": 1, "errors": 0, "panicked": false, "duration_ms": 843
          }
        }
      ]
    }
  }
}
```

Consumers should treat the section as optional: check `availability` /
`freshness` before reading `data`. `recent` is newest-first. Unknown fields are
ignored and missing fields default, so the schema can grow additively.

### Honest-state response example (disabled)

```jsonc
{
  "schema_version": 1,
  "generated_at": "2026-07-05T15:31:39Z",
  "section": {
    "availability": "ok",
    "freshness": "live",
    "note": "Overseer: disabled",
    "data": { "enabled": false, "cadence_secs": 900, "totals": { }, "threads": [], "recent": [] }
  }
}
```

## Dashboard **Overseer** tab

A new tab (slug `overseer`, label **Overseer**), declared once in the
[tab-identity table](../dashboard.md#tab-identity-contract) (`tab_meta.rs`) so
its nav label, browser title, `<h1>`, and plain-English lede stay in lockstep.
The lede is **jargon-free** (no "OODA", "meta-loop", "cognitive memory" — it is
cross-checked against `BANNED_JARGON`). It reads roughly:

> **Overseer** — What Simard's steward has been doing on its own: what it
> noticed, what it changed, and — when it chose to wait — why. Refreshes
> automatically.

The panel shows, most-recent-first and auto-refreshing (~30 s, like the other
tabs):

1. **Overseer status line** — enabled/disabled, cadence (e.g. "every 15 min"),
   the identity it acts under, and last-tick time. Honest states from the table
   above render here verbatim.
2. **Operator threads** — a row per cognitive thread: name, enabled, cadence,
   last run, next due, and a plain health word (`ok` / `idle` / `erroring` /
   `backoff` / `disabled`).
3. **Recent activity timeline** — one line per tick in plain language: what it
   **saw** (problems observed) and what it **did** (issues filed, workstreams
   launched, PRs merged, deploys, escalations) — or, when nothing needed doing,
   **why it held** ("observed 2 problems, held 1 — waiting on a gate", or
   "observing, 0 interventions").

Every interpolated value is escaped (`esc()` / `escAttr()`) before it reaches
`innerHTML`, so a value that ever contained markup renders inert — covered by an
XSS regression test.

## TUI **Overseer** pane

The TUI gains `Tab::Overseer` (`src/bin/simard_tui/tabs/overseer.rs`), reachable
by pressing **`Alt+8`** (the footer lists `Alt+1–8`; bare digits are ignored so
they never steal input on the Meeting tab). It assembles the same
`StatusSnapshot`, reads `snapshot.overseer`, and renders a bordered pane with the
identical content as the dashboard tab — the Overseer status line, the operator-
thread rows, and the newest-first activity timeline — in the TUI's existing
style. The disabled / observing / absent states render as the same honest
one-liners. See [`simard-tui`](./simard-tui.md).

## Configuration

The feed has **no configuration of its own** — it records whatever the Overseer
does. Its content is governed entirely by the existing Overseer settings
(`src/overseer/config.rs`):

| Env var | Effect on the feed | Default |
|---|---|---|
| `SIMARD_OVERSEER_ENABLED` | A falsey value (`0`/`false`/`no`/`off`) **disables** the acting Overseer: no new ticks are recorded and every surface shows `Overseer: disabled`. Unset or truthy → enabled (opt-**out**). | enabled |
| `SIMARD_OVERSEER_INTERVAL_SECS` | The Overseer cadence, which also drives the feed's `cadence_secs` and the `live`/`stale` window (`2 × cadence`). Clamped to a 60 s floor. | `900` (15 min) |
| `SIMARD_OVERSEER_AUTHOR_LOGIN` | The distinct git identity shown as `author_login`. | `simard-overseer[bot]` |

The feed file location follows the standard
[state-root resolution](./state-root-resolution.md): `~/.simard/overseer/`
(honoring `SIMARD_STATE_ROOT`). No new file needs to be created by an operator;
the daemon creates `overseer/activity.json` on its first tick.

## Guarantees

- **Read-only surfacing.** No Overseer decision or intervention logic is
  changed. The only producer-side touch is recording a report at the tick
  boundary (and the additive `Serialize`/`Deserialize` derive on
  `OverseerTickReport`).
- **Never panics.** Writers are size-capped and non-fatal; readers degrade
  missing/corrupt input to `None`.
- **Bounded.** `N = 100` ticks, enforced on write; 8 MiB read guard.
- **Honest.** Disabled, observing-with-zero-interventions, and absent are
  distinct, explicit states — never a blank or a fabricated `0`.
- **Process-agnostic.** Identical data from the dashboard, the TUI, and
  `simard status`, because they all read the one durable file.
- **Auth-inherited.** `/api/overseer` sits behind the same `require_auth` as
  every other `/api/*` route; no new auth surface.

## Activation

The daemon must be **redeployed** after this feature merges before it starts
writing `overseer/activity.json`. Until then the surfaces show
`Overseer: no ticks recorded yet` — which is the honest, correct state.

## See also

- [How to watch Overseer activity](../howto/watch-overseer-activity.md) — the
  operator walkthrough with rendered output.
- [StatusSnapshot API reference](./status-snapshot-api.md) — the section model
  and provider this feed plugs into.
- [Cognitive thread scheduling](./cognitive-thread-scheduling.md) — where the
  per-thread `ThreadHealth` comes from.
- [Stewardship API](./stewardship-api.md) — the issue-filing path the Overseer
  uses.
- [Operator read / state-root contract](./operator-read-state-root-contract.md)
  — the single-writer / many-readers discipline this feed follows.
