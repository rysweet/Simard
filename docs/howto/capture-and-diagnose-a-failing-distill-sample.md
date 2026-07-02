---
title: Capture and diagnose a failing distill sample
description: Step-by-step how-to for harvesting a real, currently-failing distillation recipe output with the env-gated raw-capture diagnostic, classifying whether the residual parse failure is an extractor-chokepoint gap or a prompt/retry-side gap, and turning the harvested bytes into a regression test — the SIMARD_DISTILL_RAW_CAPTURE toggle, reading distill_parse_success_rate from metrics.jsonl, and wiring the sample into the recipe_output and distillation test suites.
last_updated: 2026-07-02
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/distill-raw-capture-on-parse-failure.md
  - ../reference/distill-recipe-output-capture.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ../howto/diagnose-decide-orient-parse-failures.md
  - ../architecture/episode-distillation.md
---

# Capture and diagnose a failing distill sample

Use this how-to when distillation is producing few or no facts and you suspect a
**residual parse failure** — a distill pass that exits `0` but whose output the
extractor cannot turn into a `{ "facts": [...] }` object, even after the
#2504 / #2512 launch-banner chokepoint fixes.

The goal is to stop guessing: harvest the **exact bytes** that fail, classify
the real root cause, and lock it down with a regression test built from a real
sample rather than a hand-written approximation.

For the diagnostic's full contract (config, security, API), see
[Distill raw-capture on parse failure](../reference/distill-raw-capture-on-parse-failure.md).

> **Note:** the raw-capture diagnostic is **implemented (Wave 1)** — the
> `SIMARD_DISTILL_RAW_CAPTURE` toggle and `~/.simard/distill-captures/` output
> ship in the Wave 1 build. It is **off by default**: nothing is written unless
> you set the toggle. Step 1 (measuring `distill_parse_success_rate`) works on
> any build; Steps 2–4 require a build that includes Wave 1.

## Before you start

- A running (or runnable) OODA daemon that performs distillation passes.
- Write access to `~/.simard/` (state home).
- The Simard source checkout, to add the regression test once you have a sample.

## Step 1 — Confirm there really is a residual failure

Raw-capture only exists to explain a *residual* failure, so first prove one is
happening. The `distill_parse_success_rate` metric is emitted once per pass that
reached output parsing; its mean is the parse-success rate.

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | tail -n 50
```

- If recent values are mostly `1.0`, the launch-banner fix already recovered the
  path — **there is no residual failure to chase.** Record that evidence and
  stop; do not re-harden a working extractor.
- If recent values are frequently `0.0`, note the `failure_class`, `attempt`,
  and `fact_count` fields, then continue.

## Step 2 — Enable raw-capture and restart the daemon

Capture is default-off. Turn it on and restart so the running process picks up
the environment:

```bash
SIMARD_DISTILL_RAW_CAPTURE=1 simard ooda run
```

If you run under systemd-user, add the variable to the unit environment and
restart:

```bash
systemctl --user set-environment SIMARD_DISTILL_RAW_CAPTURE=1
systemctl --user restart simard-ooda
```

Only a `parse-failure` that survives the bounded in-cycle retry writes a
sample, so nothing appears until a genuine residual failure recurs.

## Step 3 — Harvest a sample

Watch the capture directory and read the first sample that lands:

```bash
ls -lt ~/.simard/distill-captures/
head -20 ~/.simard/distill-captures/distill-parsefail-*.txt
```

The header block tells you the failure context; everything below the
`# ---- raw recipe-runner stdout (verbatim) ----` fence is the raw runner stdout
that failed to parse:

```text
# distill parse-failure raw capture
# See docs/reference/distill-raw-capture-on-parse-failure.md
# failure_class: parse-failure
# recipe_exited_ok: true
# attempt: 2
# recovered_after_retry: false
# input_count: 34
# fact_count: 0
# captured_at: 2026-07-02T20:31:14+00:00
# raw_bytes: 4127
# ---- raw recipe-runner stdout (verbatim) ----
<the exact bytes that failed to parse>
```

Copy one representative sample somewhere safe before it rotates out of the ring.

## Step 4 — Classify the root cause

Look at the payload below the fence and decide which layer is actually broken:

| What the payload shows | Likely root cause | Fix layer |
| --- | --- | --- |
| A valid `{ "facts": [...] }` object buried under ANSI / log / banner / launch-preamble noise | **Extractor gap** — a noise shape the chokepoint does not strip yet | `src/recipe_output/extract.rs` |
| No JSON object at all — the agent answered in prose, or emitted `{...}` without a `facts` key | **Prompt/retry gap** — the strict-JSON retry still misses on `attempt=2` | distill recipe prompt + retry path in `src/memory_consolidation/distillation.rs` |
| Truncated / malformed JSON (unbalanced braces, cut mid-string) | **Upstream truncation** — runner output limit or agent cutoff | runner invocation / output cap |

`recipe_exited_ok: true` with `fact_count: 0` and a payload that clearly
contains a facts object points at the extractor; the same fields with a payload
that contains **no** facts object point at the prompt/retry side. This
distinction is the whole reason to harvest a real sample — the two fixes are in
different files, and hardening the wrong one leaves the failure intact.

## Step 5 — Turn the sample into a regression test

Add the harvested bytes verbatim as a fixture and assert the *current* behavior
you want:

- **Extractor gap** → add a test in the `#[cfg(test)]` module of
  `src/recipe_output/extract.rs` asserting `extract_json_payload(SAMPLE)` returns
  the embedded facts object. This mirrors the existing
  `extract_json_payload_recovers_*` tests.
- **Prompt/retry gap** → add a test in `src/memory_consolidation/distillation.rs`
  driving the runner with the captured output and asserting the pass recovers
  (or classifies correctly) rather than silently deferring.

Keep the sample byte-for-byte — the point of a real capture is that it
reproduces the production failure exactly. Then run the suite:

```bash
cargo test --quiet recipe_output
cargo test --quiet distill
```

## Step 6 — Verify recovery, then turn capture back off

After the fix lands and the daemon redeploys, confirm the metric recovers:

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | tail -n 50
```

Values should return toward `1.0`. Once you have your sample and the fix is
verified, disable capture again so the daemon stops writing diagnostic files:

```bash
systemctl --user unset-environment SIMARD_DISTILL_RAW_CAPTURE
systemctl --user restart simard-ooda
# or simply relaunch without the variable set
```

## Troubleshooting

- **No files appear.** Check that the failure is actually a `parse-failure`
  (Step 1) — a `spawn-failure`, `copilot-terminal-failure`,
  `recipe-reported-failure`, or `serialize-failure` never reaches parsing and is
  never captured. Confirm `SIMARD_DISTILL_RAW_CAPTURE` is truthy in the daemon's
  environment.
- **Samples rotate away too fast.** Raise `SIMARD_DISTILL_RAW_CAPTURE_KEEP`
  (default `20`).
- **Samples are truncated.** Raise `SIMARD_DISTILL_RAW_CAPTURE_MAX_BYTES`
  (default `65536`, max `4194304`).
- **Metric never changes.** A stale daemon that never redeployed the fix will
  keep failing even though `main` is fixed — a merged fix does not reach the
  running process until the daemon relaunches into the new binary. Confirm the
  deployed `~/.simard/bin/simard` post-dates the fix commit before treating a
  persistent failure as a code bug.

## Related

- [Distill raw-capture on parse failure](../reference/distill-raw-capture-on-parse-failure.md)
  — full reference for the diagnostic.
- [Distill recipe output capture](../reference/distill-recipe-output-capture.md)
  — the envelope parse contract.
- [Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md)
  — the shared chokepoint your extractor test targets.
- [Diagnose decide/orient parse failures](./diagnose-decide-orient-parse-failures.md)
  — the sibling brain-phase parse diagnostic.
