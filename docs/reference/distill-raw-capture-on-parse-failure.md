---
title: Distill raw-capture on parse failure
description: Reference for the env-gated, default-off diagnostic that persists the raw recipe-runner-rs stdout of a failed distillation pass so a real currently-failing sample can be harvested and turned into a regression test — the SIMARD_DISTILL_RAW_CAPTURE toggle, the 0700 capture directory and 0600 sample files, the per-sample size cap and bounded rotation ring, the raw_capture module API, and the metrics-hygiene guarantee that metrics.jsonl carries only classification counters, never the raw payload.
last_updated: 2026-07-02
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./distill-recipe-output-capture.md
  - ../architecture/episode-distillation.md
  - ../concepts/copilot-launcher-preamble-stripping.md
  - ./text-parsing-wire-formats.md
  - ../howto/capture-and-diagnose-a-failing-distill-sample.md
  - ../../src/memory_consolidation/raw_capture.rs
  - ../../src/memory_consolidation/distillation.rs
  - ../../src/recipe_output/extract.rs
---

# Distill raw-capture on parse failure

> **Status: implemented (Wave 1, 2026-07-02 operator-review priority 1).** The
> env-gated raw-capture diagnostic lives in
> [`src/memory_consolidation/raw_capture.rs`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/raw_capture.rs)
> and is invoked from the surviving-parse-failure path in
> [`src/memory_consolidation/distillation.rs`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation.rs).
> It is **OFF by default** and writes nothing unless `SIMARD_DISTILL_RAW_CAPTURE`
> is set to a truthy value. It is the harvesting tool behind the residual distill
> parse failures that persist *in a stale deployment* after the #2504 / #2512 /
> #2513 chokepoint fixes — on current `main` the launch-banner class of failure
> is already recovered (see
> [`parser_extracts_facts_and_procedures_from_real_prose_prefixed_envelope`](https://github.com/rysweet/Simard/blob/main/src/memory_consolidation/distillation_tests.rs)),
> so any *new* residual has a different root cause (the agent emitted no
> `{ facts }` object, malformed JSON, or a strict-JSON-retry miss) that this
> diagnostic makes inspectable.

The distillation pass turns batches of episodes into semantic facts by shelling
out to `recipe-runner-rs` and parsing the distill agent's
`{ "facts": [...], "procedures": [...] }` JSON from the runner envelope. When a
pass classifies as a [`ParseFailure`](./distill-recipe-output-capture.md) — the
recipe exited `0` and a step ran, but its output carried no parseable facts
object — the failure is non-fatal: no markers are set and the batch retries.
That is correct for production resilience but **useless for debugging**, because
the exact bytes that failed to parse are gone by the next cycle.

Raw-capture closes that gap. When enabled, a `ParseFailure` (and only a
`ParseFailure`) writes the raw, pre-extraction recipe stdout to a rotating,
mode-`0600` file under a mode-`0700` directory. An operator can then harvest a
**real currently-failing sample**, add it verbatim as a regression fixture, and
confirm whether the residual failure is a chokepoint/extractor gap or a
prompt/retry-side gap — without guessing.

For the parse contract this diagnostic instruments, see
[Distill recipe output capture](./distill-recipe-output-capture.md). For the
shared noise-stripping chokepoint the samples are tested against, see
[Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md).

## Contents

- [Why](#why)
- [Enable it](#enable-it)
- [Where samples are written](#where-samples-are-written)
- [Configuration](#configuration)
- [Capture-file format](#capture-file-format)
- [Public API](#public-api)
- [Metrics hygiene](#metrics-hygiene)
- [Security model](#security-model)
- [Examples](#examples)
- [When capture does *not* fire](#when-capture-does-not-fire)

## Why

The #2504 and #2512 fixes routed every recipe-backed phase — decide, orient,
and the **distill** pass — through the single hardened extractor in
[`src/recipe_output/extract.rs`](https://github.com/rysweet/Simard/blob/main/src/recipe_output/extract.rs),
which strips ANSI colour codes, timestamped log lines, the runner's human
banner, and the GitHub Copilot CLI launch-log preamble
(`ℹ … NODE_OPTIONS=… (saved preference)`, `Run 'copilot update' …`,
`… launching copilot binary=…`). That eliminated the launch-banner class of
parse failure.

Any *residual* parse failure therefore has a different root cause — the agent
emitted no `{ facts }` object at all, emitted malformed JSON, or the strict-JSON
retry still missed on `attempt=2`. You cannot tell which from a counter alone.
Raw-capture makes the failing bytes inspectable so the fix targets the layer
that is actually broken, instead of re-hardening an extractor that already
works.

## Enable it

Capture is default-off. Turn it on for the running daemon (or a one-shot pass)
by setting the toggle:

```bash
SIMARD_DISTILL_RAW_CAPTURE=1 simard ooda run
```

The value is parsed leniently: `1`, `true`, `yes`, and `on`
(case-insensitive) enable capture; anything else — including unset, empty,
`0`, `false` — leaves it off. There is no CLI flag; capture is controlled
entirely by the environment so it can be toggled on a live daemon by editing
the service environment and restarting, without a rebuild.

## Where samples are written

Samples are written under the Simard state home, in a dedicated subdirectory:

```
~/.simard/distill-captures/
├── distill-parsefail-20260702T203114Z-a1b2c3.txt
├── distill-parsefail-20260702T204901Z-9f8e7d.txt
└── …
```

- The directory is created lazily on the first capture with mode `0700`.
- Each sample file is created with mode `0600`.
- Filenames are `distill-parsefail-<UTC-timestamp>-<6hex>.txt`, so they sort
  chronologically and never collide within a pass.

The base directory follows the same state-home resolution as the rest of
Simard (`SIMARD_STATE_ROOT` / `~/.simard`); see
[State-root resolution](./state-root-resolution.md).

## Configuration

All configuration is via environment variables, read once per pass. Invalid or
out-of-range values fall back to the documented default (never panic, never
disable a valid feature silently).

| Variable | Default | Purpose |
| --- | --- | --- |
| `SIMARD_DISTILL_RAW_CAPTURE` | `0` (off) | Master toggle. Truthy (`1`/`true`/`yes`/`on`) enables capture on `ParseFailure`. |
| `SIMARD_DISTILL_RAW_CAPTURE_MAX_BYTES` | `65536` | Per-sample byte cap (clamped to `[1024, 4194304]`). A larger payload is truncated on a UTF-8 char boundary at or below the cap; the header's `raw_bytes` line records the exact original byte count and a `(truncated to M of N bytes)` note. |
| `SIMARD_DISTILL_RAW_CAPTURE_KEEP` | `20` | Rotation ring size. After a write, the oldest `distill-parsefail-*.txt` files beyond this count are deleted. `0` disables pruning (unbounded — not recommended). |
| `SIMARD_DISTILL_RAW_CAPTURE_DIR` | `<state-home>/distill-captures` | Override the capture directory. Relative paths resolve under the state home; the mode-`0700` guarantee still applies. |

Clamping rules:

- `MAX_BYTES` is clamped to `[1024, 4_194_304]` (1 KiB … 4 MiB). A value of `0`,
  a non-numeric string, or a value above the ceiling is replaced by the default.
- `KEEP` is clamped to `[0, 10_000]`.

## Capture-file format

A sample is the raw runner stdout **exactly as the extractor received it** —
ANSI escapes, log lines, banner, and launch preamble all intact — because that
is precisely what a regression test must reproduce. A short, machine-readable
header precedes the payload so a harvested sample is self-describing:

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
<the exact bytes recipe-runner-rs wrote to stdout>
```

The header fields mirror the distill metrics context (see
[Metrics hygiene](#metrics-hygiene)) so a sample can be correlated to the
`metrics.jsonl` event it came from. Everything below the
`# ---- raw recipe-runner stdout (verbatim) ----` fence is byte-for-byte what
failed to parse (truncated at the byte cap, with the exact `raw_bytes` count
recorded in the header).

## Public API

The module exposes a small, panic-free surface. Every function returns without
propagating I/O errors into the distillation path — a capture failure is logged
via `tracing::warn!` and swallowed, because capturing a diagnostic must never
turn a recoverable distill miss into a hard error.

```rust
// src/memory_consolidation/raw_capture.rs

/// Resolved, validated capture settings. Built once per pass from the
/// environment; cheap to construct, `Copy`-free but small.
pub struct RawCaptureConfig {
    pub enabled: bool,
    pub dir: PathBuf,
    pub max_bytes: usize,
    pub keep: usize,
}

impl RawCaptureConfig {
    /// Read + validate all `SIMARD_DISTILL_RAW_CAPTURE*` vars, applying the
    /// documented defaults and clamps. Never panics.
    pub fn from_env() -> Self;
}

/// Metadata recorded in the sample header. Mirrors the distill metrics
/// context so a capture correlates 1:1 with its `metrics.jsonl` event.
pub struct CaptureMeta<'a> {
    pub failure_class: &'a str,
    pub recipe_exited_ok: bool,
    pub attempt: u32,
    pub recovered_after_retry: bool,
    pub input_count: u32,
    pub fact_count: u32,
}

/// Persist `raw` stdout for a parse failure. No-op when capture is disabled
/// or `meta.failure_class != "parse-failure"`. Best-effort: on any I/O error
/// it logs and returns `Ok(None)` rather than surfacing the error.
///
/// Returns `Ok(Some(path))` with the written sample path on success,
/// `Ok(None)` when nothing was written.
pub fn capture_parse_failure(
    cfg: &RawCaptureConfig,
    meta: &CaptureMeta<'_>,
    raw: &str,
) -> std::io::Result<Option<PathBuf>>;
```

`capture_parse_failure` is invoked from the distill failure branch in
`distillation.rs` immediately after `classify_distill_error` yields
`DistillFailureClass::ParseFailure` and after the bounded in-cycle retry
(`DISTILL_PARSE_RETRY_MAX`) has been exhausted, so a sample is only written when
a parse failure genuinely *survives* retry.

## Metrics hygiene

Raw-capture is deliberately the **only** place the raw payload is persisted. The
distill metrics path (`record_distill_success_metric` /
`build_distill_success_context` in `distillation.rs`) writes structured
classification counters to `~/.simard/metrics/metrics.jsonl` and **never** the
raw agent output. Guaranteed metric context fields:

| Field | Type | Meaning |
| --- | --- | --- |
| `outcome` | `"success"` \| `"failure"` | Pass outcome. |
| `recipe_exited_ok` | bool | Recipe process exited `0`. |
| `parse_attempted` | bool | A step ran and its output was parsed. |
| `parse_success` | bool | Parsing yielded a facts object. |
| `failure_class` | string \| null | One of `spawn-failure`, `copilot-terminal-failure`, `recipe-reported-failure`, `parse-failure`, `serialize-failure`, `other`. |
| `input_count` | u32 | Episodes fed to the pass. |
| `fact_count` | u32 | Facts extracted (`0` on failure). |
| `attempt` | u32 | 1-based runner invocation count for the pass. |
| `recovered_after_retry` | bool | Success followed at least one transient retry. |

The context is built with `serde_json::json!` and serialized, so no raw payload
substring, no ANSI bytes, and no un-escaped newline can leak into a metrics
line. This is a correctness-as-safety property: `metrics.jsonl` is line-oriented
and world-readable in some deployments, so it must not carry secrets or PII from
agent output, nor any bytes that could inject a spurious metrics line.

`distill_parse_success_rate` is emitted **only** for passes that reached parsing
(`parse_attempted == true`), so its plain mean over passes is exactly the
parse-success rate the priority-1 work drives back toward `1.0`. Measure it
before deciding whether a fix is even needed:

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | tail -n 50
```

The metric envelope is `MetricEntry` (`src/self_metrics/mod.rs`), serialized with
its Rust field names, so the JSON key is `metric_name` — not `metric` — and the
classification counters above live inside its stringified `context` field.

## Security model

- **Default-off.** No file is created and no directory is touched unless the
  operator explicitly opts in with `SIMARD_DISTILL_RAW_CAPTURE`.
- **Restrictive permissions.** The capture directory is `0700`; each sample is
  `0600`. Agent output can contain secrets or PII, so samples are readable only
  by the daemon's user.
- **Bounded on disk.** The per-sample cap (`MAX_BYTES`) and the rotation ring
  (`KEEP`) together bound total capture footprint, preventing a stuck failure
  loop from exhausting disk.
- **No metrics leakage.** See [Metrics hygiene](#metrics-hygiene) — the raw
  payload never reaches `metrics.jsonl`.
- **Panic-free on untrusted input.** The capture path operates on untrusted
  agent stdout and performs no unbounded allocation or indexing; a malformed or
  adversarial payload is truncated at the byte cap and written verbatim, never
  parsed.

## Examples

### Harvest one failing sample on a live daemon

```bash
# 1. Turn on capture and restart the daemon (or its service unit).
SIMARD_DISTILL_RAW_CAPTURE=1 simard ooda run

# 2. Wait for a distillation pass to fail parsing, then inspect.
ls -l ~/.simard/distill-captures/
head -20 ~/.simard/distill-captures/distill-parsefail-*.txt

# 3. Turn capture back off once a sample is harvested.
#    (unset the var / edit the service env, restart)
```

### Tighten the footprint on a busy host

```bash
SIMARD_DISTILL_RAW_CAPTURE=1 \
SIMARD_DISTILL_RAW_CAPTURE_MAX_BYTES=16384 \
SIMARD_DISTILL_RAW_CAPTURE_KEEP=5 \
simard ooda run
```

### Redirect captures to an inspection volume

```bash
SIMARD_DISTILL_RAW_CAPTURE=1 \
SIMARD_DISTILL_RAW_CAPTURE_DIR=/mnt/inspect/distill-captures \
simard ooda run
```

## When capture does *not* fire

Capture is intentionally narrow. Nothing is written when:

- `SIMARD_DISTILL_RAW_CAPTURE` is unset or falsy (the default).
- The failure class is anything other than `parse-failure` — a
  `spawn-failure`, `copilot-terminal-failure`, `recipe-reported-failure`, or
  `serialize-failure` never reached output parsing, so there is no meaningful
  payload to capture.
- The pass *succeeded* (including a success recovered by the in-cycle retry).
- The pass was below the `DISTILL_MIN_EPISODES` threshold and skipped the LLM
  call entirely.

This keeps the diagnostic focused on exactly the residual failure mode it
exists to explain.

## Related

- [Distill recipe output capture](./distill-recipe-output-capture.md) — the
  envelope parse contract this diagnostic instruments.
- [Copilot launch-log preamble stripping](../concepts/copilot-launcher-preamble-stripping.md)
  — the shared chokepoint samples are tested against.
- [Capture and diagnose a failing distill sample](../howto/capture-and-diagnose-a-failing-distill-sample.md)
  — the step-by-step harvesting how-to.
- [Episode distillation](../architecture/episode-distillation.md) — the
  surrounding pipeline.
