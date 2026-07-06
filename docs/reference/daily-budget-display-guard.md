---
title: Daily-budget display guard reference
description: Why every operator surface that prints the daily LLM budget — `simard status`, the dashboard monitoring JSON, and the TUI — resolves the ceiling through the single canonical `overseer::config` resolver, so the displayed ceiling always matches the Overseer `BudgetGate` it enforces (default 500.0) and the old, false "unset (no guard)" line can no longer appear (issue #6).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./status-snapshot-api.md
  - ./telemetry-metrics.md
  - ../howto/simard-status.md
  - ../design/overseer.md
  - ../dashboard.md
---

# Daily-budget display guard reference

The daily LLM-spend budget is **always** guarded. When
`SIMARD_DAILY_BUDGET_USD` is unset, empty, unparseable, or non-positive, Simard
falls back to a hard-coded default ceiling of **`500.0` USD**. That ceiling is
what the Overseer's `BudgetGate` enforces, and — as of issue #6 — it is also
exactly what every operator surface **displays**.

This page documents the display contract: one canonical resolver, one number,
no display surface that can disagree with the `BudgetGate` the daemon runs
under. (See [Scope and known boundary](#scope-and-known-boundary) for the one
enforcement path — the OODA loop's own config — that is *not yet* routed through
this resolver.)

> **Modules:** resolver `src/overseer/config.rs`
> (`resolve_daily_budget_usd` / `daily_budget_usd`); consumers
> `src/status/provider.rs` (`assemble_llm`), `src/status/render.rs`
> (`render_llm`), `src/operator_commands_dashboard/monitoring.rs`
> (`get_budget`). Guard: `src/overseer/guardrails.rs` (`BudgetGate`,
> single-sourced from the resolver). The OODA loop's own cycle enforcement
> (`src/ooda_loop/cycle.rs`) reads a separate `OodaConfig.daily_budget_usd`
> (`src/ooda_loop/types.rs`) — see
> [Scope and known boundary](#scope-and-known-boundary).

## The bug this closes

Before issue #6, the **guard** and the **display** read the budget from two
different places:

- The **guard** (the Overseer's `BudgetGate`) resolved the budget through
  `overseer::config`, which falls back to `500.0` when the env is absent — so a
  ceiling was **always** enforced. (The OODA loop's own cycle enforcement also
  defaults to `500.0` when the env is absent, so it was capping spend too.)
- The **display** paths (`simard status`, the dashboard monitoring JSON) read
  the **raw** `SIMARD_DAILY_BUDGET_USD` environment variable directly and
  treated "absent" as "no budget":

  ```text
  daily budget      unset (no guard)
  ```

Because `simard status` and the dashboard run **outside** the daemon's systemd
environment, they usually did **not** see `SIMARD_DAILY_BUDGET_USD`, so they
printed `unset (no guard)` even though the daemon was enforcing the `500.0`
ceiling the whole time. The line was **false** and actively misleading during
operator review: it implied spend was uncapped when it was not.

The fix routes every display path through the **same** canonical resolver the
guard uses, so the displayed ceiling can never diverge from the enforced one.

## Canonical resolver (single source of truth)

All budget-aware code — guard *and* display — resolves the daily ceiling
through one pair of functions in `src/overseer/config.rs`:

```rust
/// Env-injectable core. Falls back to DEFAULT_DAILY_BUDGET_USD (500.0) when the
/// value is unset, empty, unparseable, or non-positive. This is the single
/// source of truth the BudgetGate reads.
pub fn resolve_daily_budget_usd(lookup: impl Fn(&str) -> Option<String>) -> f64;

/// Production entry point: reads the real process environment.
pub fn daily_budget_usd() -> f64;
```

`daily_budget_usd()` is `resolve_daily_budget_usd(|k| std::env::var(k).ok())`.
Display code calls `daily_budget_usd()`; no display path calls
`std::env::var("SIMARD_DAILY_BUDGET_USD")` or parses the raw env itself.

### Resolution rules

`resolve_daily_budget_usd` trims the looked-up value and applies these rules, in
order:

| Env value (`SIMARD_DAILY_BUDGET_USD`) | Resolved ceiling |
|---|---|
| unset | `500.0` (default) |
| empty / whitespace-only | `500.0` (default) |
| `"250"`, `"250.0"` | `250.0` |
| `"25"` | `25.0` |
| non-positive (`"0"`, `"-10"`) | `500.0` (default) |
| non-finite (`NaN`, `inf`) | `500.0` (default) |
| unparseable (`"abc"`) | `500.0` (default) |

The resolver only accepts a **finite, strictly-positive** float; every other
case falls back to the default. This validation is **stricter** than the old
raw-env parse (which accepted `0`, negatives, `NaN`, and `inf`), so the
displayed ceiling is now always a real, enforceable number.

There is **no** explicit "disable the guard" sentinel: `0` and negatives do not
turn the guard off, they fall back to `500.0`. The budget is always on.

## `simard status` — LLM usage

`assemble_llm` in `src/status/provider.rs` populates
`LlmUsage.daily_budget_usd` from `overseer::config::daily_budget_usd()`. The
field stays `Option<f64>` for wire compatibility, but the provider now **always**
emits `Some(resolved)` — never `None` — because a ceiling always resolves.

`render_llm` in `src/status/render.rs` renders the guard as
`$<spent-today> / $<ceiling>`:

```text
LLM USAGE
  copilot turn      in 4,120  cached 1,900  out 880   ·  AI-credits 12
  ledger today      $1.87    in 412,000  out 88,000
  ledger 7d         $11.42   in 2,740,000  out 610,000
  ledger all-time   $208.91  in 51,300,000  out 9,900,000
  daily budget      $1.87 / $500.00
  reconciliation    ledger $1.87  vs  credits 940   ·  OK (within tolerance)
```

With `SIMARD_DAILY_BUDGET_USD` unset, the line now reads `$1.87 / $500.00`
(the real, enforced ceiling) instead of the old `unset (no guard)`.

The renderer's fallback label — previously the false `"unset (no guard)"` — is
now the neutral **`n/a`**. In production it is unreachable (the provider always
supplies `Some`); it is retained only for type completeness over the
`Option<f64>` and can never claim the guard is off.

## Dashboard monitoring — `get_budget`

`get_budget` in `src/operator_commands_dashboard/monitoring.rs` is **file-first**:
it reads `~/.simard/budget.json` and returns that JSON verbatim when it parses.
Only when the file is missing or invalid does it fall back to defaults — and the
**daily** default is now single-sourced through
`overseer::config::daily_budget_usd()` rather than a duplicated `500.0` literal:

```jsonc
// Fallback body when ~/.simard/budget.json is absent or unparseable,
// with SIMARD_DAILY_BUDGET_USD unset:
{
  "daily_budget_usd": 500.0,   // from overseer::config::daily_budget_usd()
  "weekly_budget_usd": 2500.0  // unchanged; no canonical weekly resolver yet
}
```

The weekly fallback (`2500.0`) and the file-first `budget.json` behaviour are
**unchanged** — only the daily fallback branch is single-sourced. With
`SIMARD_DAILY_BUDGET_USD=250`, the daily fallback reports `250.0`.

## `--json` / snapshot schema

`simard status --json` and `GET /api/status/snapshot` serialize the same
`StatusSnapshot`. `llm.data.daily_budget_usd` is now always a number:

```json
{
  "llm": {
    "availability": "ok",
    "freshness": "live",
    "data": {
      "ledger_today": { "cost_usd": 1.87, "tokens_in": 412000, "tokens_out": 88000 },
      "daily_budget_usd": 500.0,
      "reconciliation": { "ledger_usd": 1.87, "credits": 940, "delta_flag": "ok" }
    }
  }
}
```

The field type is unchanged (`Option<f64>`, `#[serde(default)]`), so older
consumers keep deserializing; the only observable change is that the value is a
number (the resolved ceiling) instead of `null` when the env is absent.

## Guarantees

- **Display = the `BudgetGate` ceiling.** Every display surface resolves the
  ceiling through the same `overseer::config` resolver the Overseer's
  `BudgetGate` reads, so the printed number is always the ceiling the
  `BudgetGate` enforces. For every normal configuration (unset, or any finite
  positive value) this is also exactly the OODA loop's own enforced ceiling; the
  one exception is documented in
  [Scope and known boundary](#scope-and-known-boundary).
- **Always guarded.** Unset/empty/invalid/non-positive → `500.0`. There is no
  configuration that renders the guard as absent.
- **No false "no guard".** The `unset (no guard)` line is retired; the retained
  fallback label is a neutral `n/a` that never claims spend is uncapped.
- **Single-sourced (display).** No display path reads or parses raw
  `SIMARD_DAILY_BUDGET_USD`; for display *and* the `BudgetGate`, the `500.0`
  default lives once, in `overseer::config` (`DEFAULT_DAILY_BUDGET_USD`). The
  OODA loop still carries its own `500.0` fallback — see
  [Scope and known boundary](#scope-and-known-boundary).

## Scope and known boundary

Issue #6 unifies every **display** path and the Overseer's `BudgetGate` onto the
canonical resolver. One **enforcement** path is intentionally out of scope for
this display fix and remains a tracked follow-up:

- The OODA loop builds its own `OodaConfig.daily_budget_usd`
  (`src/ooda_loop/types.rs`) with a local `env_f64("SIMARD_DAILY_BUDGET_USD",
  500.0)` helper, and `src/ooda_loop/cycle.rs` enforces spend against that value.
  `env_f64` does a plain `f64::parse`, so — unlike `resolve_daily_budget_usd` —
  it does **not** reject non-positive or non-finite input.

For every configuration an operator would actually set, the two agree:

| `SIMARD_DAILY_BUDGET_USD` | Display / `BudgetGate` | OODA-loop `cycle.rs` | Agree? |
|---|---|---|---|
| unset / empty / unparseable | `500.0` | `500.0` | yes |
| `"250"` (any finite `> 0`) | `250.0` | `250.0` | yes |
| `"0"` / `"-10"` | `500.0` | `0.0` / `-10.0` | **no** |
| `NaN` / `inf` | `500.0` | `NaN` / `inf` | **no** |

The divergence only occurs for pathological values no operator sets
deliberately, and even then it fails safe: a `0` or negative ceiling makes the
OODA loop's `daily.total_cost_usd >= config.daily_budget_usd` check refuse to run
almost immediately, so the loop is *more* conservative than the displayed
`500.0`, never less. It is nonetheless a second source of truth. The follow-up
is to route `OodaConfig` through `overseer::config::resolve_daily_budget_usd`,
after which "single source of truth" holds literally for enforcement as well as
display. Tracked as a follow-up to issue #6.

## Configuration

| Variable | Effect | Default |
|---|---|---|
| `SIMARD_DAILY_BUDGET_USD` | The daily LLM-spend ceiling. Single-sourced through `overseer::config::daily_budget_usd()`; the same value the Overseer's `BudgetGate` enforces and every operator surface displays. Non-positive / invalid values fall back to the default. | `500` (always guarded) |

To display the enforced ceiling on an out-of-daemon `simard status`, either set
`SIMARD_DAILY_BUDGET_USD` in the reading shell or rely on the `500.0`
default — both now render as `$<spent> / $<ceiling>`.

## Examples

```bash
# Unset → the enforced default ceiling is shown (no more "unset (no guard)"):
$ unset SIMARD_DAILY_BUDGET_USD
$ simard status | grep 'daily budget'
  daily budget      $1.87 / $500.00

# Explicit lower ceiling:
$ SIMARD_DAILY_BUDGET_USD=250 simard status | grep 'daily budget'
  daily budget      $1.87 / $250.00

# Invalid / non-positive → falls back to the default, still guarded:
$ SIMARD_DAILY_BUDGET_USD=0 simard status | grep 'daily budget'
  daily budget      $1.87 / $500.00

# Dashboard monitoring fallback (no ~/.simard/budget.json):
$ curl -fsS -H "Authorization: ******" \
    http://localhost:8080/api/budget | jq .daily_budget_usd
500
```

## See also

- [StatusSnapshot API reference](./status-snapshot-api.md) — the typed snapshot
  and the `LlmUsage.daily_budget_usd` field.
- [Telemetry metrics reference](./telemetry-metrics.md#configuration) — the
  `SIMARD_DAILY_BUDGET_USD` configuration knob.
- [How to read `simard status`](../howto/simard-status.md) — the operator
  walkthrough with the rendered LLM-usage block.
- [Overseer design](../design/overseer.md) — the `BudgetGate` whose canonical
  ceiling this display now mirrors.
