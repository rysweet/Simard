---
title: "Reference: Gym self-eval status wiring"
description: >
  The status-snapshot contract for the GYM section: assemble_gym now reports
  the real configured scenario count (benchmark_scenarios) and a non-idle
  self-eval state when the gym is enabled (SIMARD_SKIP_GYM unset), so
  `simard status` reflects a live self-evaluation quality signal instead of the
  previous hardcoded "0 configured / idle" stub. Purely additive status wiring —
  no change to gym execution behaviour.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./status-snapshot-api.md
  - ./coin-benchmark.md
  - ../howto/run-the-coin-gym-harness.md
  - ../howto/simard-status.md
  - ../../src/status/provider.rs
  - ../../src/status/mod.rs
  - ../../src/status/render.rs
  - ../../src/gym/scenarios/mod.rs
  - ../../src/gym_runner_client.rs
---

# Reference: Gym self-eval status wiring

> **Status: implemented (issue: gym self-eval inert, goal_hygiene).**
> Present-tense description of shipped behaviour. Primary source:
> [`assemble_gym`](https://github.com/rysweet/Simard/blob/main/src/status/provider.rs)
> in `src/status/provider.rs`, with the section type in
> [`src/status/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/status/mod.rs)
> and the terminal renderer in
> [`src/status/render.rs`](https://github.com/rysweet/Simard/blob/main/src/status/render.rs).
>
> Before this change the GYM status section was a stub: even with the gym
> enabled (`SIMARD_SKIP_GYM` unset) `simard status` reported
> *"scenarios absent configured"* and *"self-eval idle"*, so the self-evaluation
> loop looked inert and produced no visible quality signal. This change wires the
> section to the real scenario set. It is **purely additive** — it reports state
> only and does not alter gym execution.

---

## The GYM status section

`assemble_gym(skip_gym: bool)` builds the
[`Gym`](https://github.com/rysweet/Simard/blob/main/src/status/mod.rs) section
of the status snapshot:

```rust
pub struct Gym {
    pub skip_gym: bool,
    pub configured_scenarios: Option<u32>,
    pub self_eval_state: String,
}
```

Behaviour by mode:

| Condition | `configured_scenarios` | `self_eval_state` |
|---|---|---|
| Gym enabled (`skip_gym == false`) | `Some(N)` where `N` = number of built-in benchmark scenarios | non-idle (e.g. `"active"`) |
| Gym skipped (`skip_gym == true`, `SIMARD_SKIP_GYM=1`) | `None` | `"idle"` |

The configured count is the length of the canonical built-in scenario set
returned by
[`benchmark_scenarios()`](https://github.com/rysweet/Simard/blob/main/src/gym/scenarios/mod.rs)
(the `'static SCENARIOS` array in
[`src/gym/scenarios/data.rs`](https://github.com/rysweet/Simard/blob/main/src/gym/scenarios/data.rs),
currently 12 curated V1 scenarios) — the status layer reports that same count
rather than a hardcoded `0`/`None`. When the gym is enabled the self-eval state is reported as
non-idle so the section honestly reflects that the self-evaluation path is live.

> **Fast-path parity.** The `SIMARD_SKIP_GYM=1` fast path (see
> [`gym_runner_client`](https://github.com/rysweet/Simard/blob/main/src/gym_runner_client.rs))
> continues to report `None` / `"idle"`, so a deliberately-skipped gym still
> reads as skipped and idle — the wiring never fabricates a signal when the gym
> is off.

---

## Rendered output

The terminal renderer
([`render_gym`](https://github.com/rysweet/Simard/blob/main/src/status/render.rs))
prints:

```text
GYM
  SIMARD_SKIP_GYM   unset (gym enabled)
  scenarios         12 configured
  self-eval         active
```

versus the skipped case:

```text
GYM
  SIMARD_SKIP_GYM   set (gym skipped)
  scenarios         absent configured
  self-eval         idle
```

The JSON status API surfaces the same `Gym` fields
(`skip_gym`, `configured_scenarios`, `self_eval_state`) — see
[Status snapshot API](./status-snapshot-api.md).

---

## Scope and non-goals

- **Status-only.** This change reports the scenario count and self-eval state;
  it does **not** schedule, execute, or change gym scenarios. Runtime behaviour
  when scenarios were previously absent is unchanged except that the enabled-gym
  path is now honestly reported as configured/active.
- No new environment variables or configuration flags are introduced. The gym
  is enabled by default; set `SIMARD_SKIP_GYM=1` to skip it (unchanged).

---

## Tests

| Test surface | Guarantee |
|---|---|
| `provider.rs` — `assemble_gym` unit test | Enabled gym → `Some(N)` / non-idle; skipped gym → `None` / `"idle"`. |
| `tests/status_render_contract.rs` | The rendered GYM section shows the configured count and non-idle self-eval when enabled, and `absent` / `idle` when skipped. |
