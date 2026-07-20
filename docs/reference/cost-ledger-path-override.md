---
title: Cost-ledger path override (SIMARD_COST_LEDGER_PATH)
description: The highest-precedence, test-only environment override that pins the LLM cost-tracking JSON-lines ledger to an explicit file, so tests can isolate cost entries without racing on process-global HOME.
last_updated: 2026-07-20
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./state-root-resolution.md
  - ./daily-budget-display-guard.md
  - ./status-snapshot-api.md
  - ./telemetry-metrics.md
---

# Cost-ledger path override (`SIMARD_COST_LEDGER_PATH`)

`simard` records estimated token usage and cost for every session turn into a
JSON-lines ledger. By default that ledger lives at
`~/.simard/costs/ledger.jsonl`, derived from the process `HOME`. The
`SIMARD_COST_LEDGER_PATH` environment variable is a **highest-precedence,
narrow override** that pins the ledger to an explicit file path, bypassing the
`HOME`-based default entirely.

> The override exists to make cost-tracking tests hermetic. Before it existed,
> tests that needed to read back a just-written cost entry mutated the
> process-global `HOME` and raced other `#[serial]` tests doing the same,
> producing flaky lookups (the entry landed under one temp `HOME` while the
> read resolved against another). Pinning `SIMARD_COST_LEDGER_PATH` to a
> per-test temp file removes that race. The variable is **inert in
> production** — when unset, the default `~/.simard/costs/ledger.jsonl` path is
> used unchanged.

---

## Resolution order

`cost_tracking::ledger_path()` resolves the ledger file by the **first match**:

1. `$SIMARD_COST_LEDGER_PATH`, if set to a **non-empty** value — used
   **verbatim** as the ledger file path (not a directory; the full path
   including filename). An empty or whitespace-only value is treated as unset.
2. `$HOME/.simard/costs/ledger.jsonl` — the default. When `HOME` is unset the
   helper falls back to a hardcoded home base (`/home/azureuser`).

The override wins over the `HOME`-derived default. It does **not** interact
with `SIMARD_STATE_ROOT`: the cost ledger has always resolved from `HOME`, not
from the shared state root, and that is unchanged. See
[State-root resolution](./state-root-resolution.md) for the separate state
tree.

| Variable | What it overrides | Scope |
|---|---|---|
| `SIMARD_COST_LEDGER_PATH` | The full path to `ledger.jsonl` (default `~/.simard/costs/ledger.jsonl`) | Test isolation; operator override |

### Implementation

The override lives entirely inside `cost_tracking::ledger_path()`. That single
private helper gains an `env::var("SIMARD_COST_LEDGER_PATH")` check ahead of its
existing `HOME`-based join; every other caller is unchanged. Because
`write_entry` and `summarize_filtered` (and thus `daily_summary` /
`weekly_summary`) all already funnel through `ledger_path()`, redirecting that
one function redirects the whole write/read path in lockstep. The summary
readers stream the ledger line-by-line through `summarize_filtered` rather than
materializing the full entry list, so the override redirects that streaming
read path too. The behavior is additive: when the
variable is absent, `ledger_path()` returns exactly what it returns today, so
production is byte-for-byte unchanged.

---

## Semantics

- **Value is a file path, not a directory.** Set it to the complete path
  including the `.jsonl` filename (e.g. `/tmp/xyz/ledger.jsonl`), not to the
  containing directory.
- **Used verbatim.** The value is treated as a trusted `PathBuf`. No expansion
  of `~`, environment interpolation, or path normalization is performed. It is
  never passed to a shell or a command, so there is no injection surface.
- **Parent directories are created on first write.** `record_cost` creates the
  parent directory of the resolved path if it does not exist, exactly as it
  does for the default location.
- **Callers of `ledger_path()` honor it.** The writer (`record_cost` /
  `write_entry`) and the summary readers (`daily_summary`, `weekly_summary`,
  and therefore the dashboard `/api/costs` endpoint, which calls those
  summaries in `operator_commands_dashboard::monitoring::costs`) all resolve
  through the same helper, so the override redirects that whole read/write path
  consistently.
- **Surfaces with independent path resolution are NOT redirected.** Two cost
  surfaces deliberately do *not* go through `ledger_path()` and are therefore
  unaffected by `SIMARD_COST_LEDGER_PATH`:
  - `simard status` (`status::provider::assemble_llm`) reads the ledger from
    `state_root/costs/ledger.jsonl` — a **state-root**-relative path, not the
    `HOME`-based `ledger_path()`. It follows `SIMARD_STATE_ROOT`, not this
    override. See [State-root resolution](./state-root-resolution.md).
  - The dashboard activity/logs preview (`operator_commands_dashboard::logs`)
    tails a directly-constructed `$HOME/.simard/costs/ledger.jsonl` path.

  This divergence is **pre-existing and out of scope** for the override; the
  override intentionally targets only the `ledger_path()` write/read path that
  the failing test exercises. Do not assume `simard status` reflects a pinned
  ledger.
- **Unset ⇒ production default.** When the variable is not present, behavior is
  byte-for-byte identical to before the override existed.

---

## Usage

### Tests (primary use case)

Pin the ledger to a per-test temp file so cost entries are deterministic and
independent of parallel `HOME` mutation. Use a scoped RAII guard that restores
or unsets the variable on drop, and keep the test in its existing serial group:

```rust
#[test]
#[serial(cognitive_memory)]
fn meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let ledger = tmp.path().join("ledger.jsonl");

    // Scoped guard: sets SIMARD_COST_LEDGER_PATH for the duration of the test
    // and restores the previous value (or unsets) on drop.
    let _ledger_env = ScopedEnv::set("SIMARD_COST_LEDGER_PATH", &ledger);

    // ... exercise the code path that calls cost_tracking::record_cost ...

    // Read back deterministically from the pinned ledger.
    let contents = std::fs::read_to_string(&ledger).expect("ledger written");
    assert!(contents.contains("\"prompt_tokens_est\""));
}
```

> Do **not** import `test_support::hermetic::EnvBinding` for this — that guard
> is intentionally module-private (issue #2360). Use a local scoped guard (or
> `std::env::set_var` / `remove_var` under the existing `#[serial]` SAFETY
> contract) instead.

### Operators (optional)

An operator may point the writer at an explicit ledger file — for example, to
place it on a dedicated volume — before launching a session:

```bash
SIMARD_COST_LEDGER_PATH=/srv/simard/costs/ledger.jsonl simard meeting repl daily
```

For the duration of that process, every code path that resolves through
`cost_tracking::ledger_path()` — the writer and the `daily_summary` /
`weekly_summary` readers (including the dashboard `/api/costs` endpoint) —
uses that file. Note that `simard status` resolves the ledger from the state
root instead (see [Surfaces with independent path resolution](#semantics)
above), so it will **not** reflect this override.

---

## Worked examples

### Default (unset)

The writer appends to the `HOME`-based default, and the summary readers resolve
to the same file:

```rust
// SIMARD_COST_LEDGER_PATH unset
cost_tracking::record_cost("sess-1", "gpt-4", 4_000, 800, "turn");
// -> appended to ~/.simard/costs/ledger.jsonl
let daily = cost_tracking::daily_summary().unwrap();
// daily reads back ~/.simard/costs/ledger.jsonl
```

### Override pins an explicit file (writer + summaries together)

```rust
std::env::set_var("SIMARD_COST_LEDGER_PATH", "/tmp/run-42/ledger.jsonl");

cost_tracking::record_cost("sess-2", "gpt-4", 1_200, 350, "turn");
// -> appended to /tmp/run-42/ledger.jsonl

let daily = cost_tracking::daily_summary().unwrap();
// daily reads back /tmp/run-42/ledger.jsonl (same override)
assert_eq!(daily.entry_count, 1);
```

### Dashboard `/api/costs` follows the override

`operator_commands_dashboard::monitoring::costs` builds its JSON from
`daily_summary` / `weekly_summary`, both of which resolve through
`ledger_path()`. With `SIMARD_COST_LEDGER_PATH` set on the dashboard process,
`GET /api/costs` reports totals from the pinned file. (`simard status` and the
activity/logs preview do **not** — see [Semantics](#semantics).)

---

## Interaction with telemetry

The override changes only the **JSONL ledger** file location. Token throughput
is still mirrored into the unified telemetry facade
(`telemetry::counter_add(LLM_TOKENS, ...)`, issue #2528) independent of the
ledger path — those counters are not redirected by
`SIMARD_COST_LEDGER_PATH`. Dollar cost remains ledger-sourced for the honest
`$/token/credit` reconciliation `simard status` performs. See
[Telemetry Metrics](./telemetry-metrics.md).

---

## Security notes

- **Trusted operator/test config, not remote input.** The value is used
  verbatim as a `PathBuf` and never enters a shell or command context, so
  there is no path-traversal or injection surface beyond what the operator
  already has on their own filesystem.
- **Production-inert.** When unset, the ledger resolves to the existing
  `HOME`-based default; no production caller sets the variable, so the override
  never broadens exposure of ledger contents (session IDs plus token/cost
  metadata).
- **Restrictive temp locations in tests.** Per-test ledgers live under a
  `tempfile` directory with default restrictive permissions and are removed
  when the temp dir is dropped at test end — no writes to shared or
  world-readable locations.
- **Diagnostics via structured tracing only.** When the override is active,
  `ledger_path()` emits a single `tracing::debug!` line noting that the override
  is in effect — it does **not** log the override path itself, so no
  home/username is leaked. Diagnostics never use `print!`/`println!`.

---

## See also

- [State-root resolution](./state-root-resolution.md) — the separate
  `SIMARD_STATE_ROOT` tree for meetings, handoffs, and the goal board (the cost
  ledger is **not** part of this tree).
- [Daily-Budget Display Guard](./daily-budget-display-guard.md) — how ledger
  totals surface in the budget display.
- [StatusSnapshot API](./status-snapshot-api.md) — the `simard status` surface.
  Note it reads the ledger via **state-root** resolution
  (`state_root/costs/ledger.jsonl`), so it is governed by `SIMARD_STATE_ROOT`,
  not by this override.
- [Telemetry Metrics](./telemetry-metrics.md) — the parallel token counters
  that are not affected by this override.
