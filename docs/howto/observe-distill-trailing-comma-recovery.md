---
title: Observe distill trailing-comma recovery
description: Step-by-step how-to for confirming the string-aware trailing-comma recovery pass is rescuing distillation facts documents and driving the parse-fail rate back toward zero — reading the parse_recovery discriminator from metrics.jsonl, distinguishing recovered from strict-ok / deferred / zero-facts, spotting a high recovered share that signals a recurring agent bug behind the auto-repair, confirming the overseer distill parse-fail anomaly self-heals, and verifying the P0 fix with the extract.rs and distillation regression tests.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/distill-trailing-comma-recovery.md
  - ../reference/distill-recipe-output-capture.md
  - ../reference/telemetry-metrics.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../howto/capture-and-diagnose-a-failing-distill-sample.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# Observe distill trailing-comma recovery

Use this how-to when distillation was reporting a **100% parse-fail rate** (the
`overseer-obs:anomaly:distill parse-fail rate 100%` signature) and you want to
confirm the [trailing-comma recovery](../reference/distill-trailing-comma-recovery.md)
pass is now rescuing those documents, that the parse-fail rate has dropped, and
that the recovery is not silently masking a *different* recurring agent bug.

The recovery pass adds a `parse_recovery` discriminator to every distill metrics
record, so you never have to guess whether a pass was clean, repaired, deferred,
or filtered to empty — you read it directly from
`~/.simard/metrics/metrics.jsonl`.

> **Note:** the recovery pass and the `parse_recovery` key are **specified for
> issue #2669** and land with the implementing PR. Once that build is running,
> every step below works as written. On any build without the fix the
> `parse_recovery` key is simply absent from the context object; upgrade to see
> it.

## Before you start

- A running (or recently run) OODA daemon that performs distillation passes.
- Read access to `~/.simard/metrics/metrics.jsonl` (the state home;
  see [State-root resolution](../reference/state-root-resolution.md) if you
  override `SIMARD_STATE_ROOT`).
- `jq` is convenient for the queries below but not required.

## Step 1 — Read the parse outcome for recent passes

Each distill pass that reached parsing emits a `distill_parse_success_rate`
record; every pass that ran the recipe emits `distill_success_rate`. The
`parse_recovery` label lives inside the stringified `context` field.

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | tail -n 50 \
  | jq -r '.context | fromjson | .parse_recovery'
```

Expected labels and what each means:

| `parse_recovery` | Meaning | Action |
| --- | --- | --- |
| `strict-ok` | The document parsed cleanly on the first strict attempt — no repair. | None. The healthy steady state. |
| `recovered` | Strict parse failed, then succeeded after trailing-comma stripping. The #2669 fix path. | None per-pass, but track the **share** (Step 3). |
| `deferred` | No candidate parsed under either view; the pass deferred for a safe retry. | Investigate — this is a *non-trailing-comma* failure (Step 4). |
| `zero-facts` | The document parsed but every fact was dropped by the category filter. | Not a parse failure; see Step 5. |

If you previously saw a run of `0.0` values for
`distill_parse_success_rate`, they should now be `1.0` with
`parse_recovery = recovered` for the documents that carried a trailing comma —
that is the signature clearing.

## Step 2 — Confirm the parse-fail rate has dropped

The mean of `distill_parse_success_rate` over recent passes **is** the
parse-success rate (it is emitted once per pass that reached parsing). Confirm it
has recovered from `~0.0`:

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | tail -n 100 \
  | jq -s 'map(.value) | add / length'
```

- A value at or near `1.0` means the trailing-comma inputs that pinned the rate
  at 100% failure are now recovered.
- A value still near `0.0` means the residual failures are **not** trailing
  commas — go to Step 4 and harvest a sample.

## Step 3 — Watch the `recovered` share (regression detector)

`parse_recovery = recovered` is a **detection control**, not just a success
marker. A *persistently high* recovered share means the distill agent keeps
emitting the same malformed structure and the auto-repair is quietly absorbing
it — exactly the kind of recurring bug the recovery pass is designed to make
visible, not hide.

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl \
  | tail -n 200 \
  | jq -r '.context | fromjson | .parse_recovery' \
  | sort | uniq -c | sort -rn
```

Interpretation:

- Mostly `strict-ok`, a small tail of `recovered` → healthy. Agents occasionally
  slip a trailing comma; the pass repairs it.
- A large or growing `recovered` share → the agent prompt is producing
  malformed JSON systematically. Fix the **prompt** (or the agent), not just the
  parser — the repair is buying you resilience, but the underlying output is
  still wrong. This is the intended signal.

## Step 4 — If passes still `deferred`, harvest the real bytes

A `deferred` label means the failure is something the trailing-comma repair
deliberately does **not** touch (truncated object, missing quotes, comments,
json5, etc.). Don't guess — capture the exact failing bytes with the env-gated
diagnostic and turn them into a regression test:

```bash
SIMARD_DISTILL_RAW_CAPTURE=1 simard ooda run
# …wait for a deferred pass, then:
head -20 ~/.simard/distill-captures/distill-parsefail-*.txt
```

Full procedure: [Capture and diagnose a failing distill sample](./capture-and-diagnose-a-failing-distill-sample.md).
The recovery pass and the raw-capture diagnostic are complementary — recovery
clears the *trailing-comma* class automatically; raw-capture explains whatever
class remains.

## Step 5 — Disambiguate "zero facts" from a parse failure

If you see `parse_recovery = zero-facts`, the document parsed correctly but every
fact was dropped by the `pr-pattern | bug-pattern | lesson-learned` category
filter. This is **not** a parse failure and must not be chased as one. Confirm it
via the distinct warn the pass emits:

```bash
journalctl -u simard 2>/dev/null | grep 'simard::distill' \
  | grep 'all facts were dropped by the category filter' | tail -n 20
```

The warn carries only `input_facts` and `kept_facts=0` counts — no content. A
non-zero `input_facts` with `kept_facts=0` means the agent produced facts that
were all out-of-category; tune the prompt to emit in-category facts, not the
parser.

## Step 6 — Confirm the overseer anomaly self-heals

The overseer taxonomy that emits the recurring signature
(`Signal::Anomaly` / `process:distill_fail`, deduped) is already in the base and
watches the observed `distill_fail_pct`. Once Steps 1–2 show the parse-fail rate
below the anomaly threshold, the Observe pass stops re-emitting the signature —
no code needs to land. The anomaly is **not** persisted to a `signals*.json`
file; it surfaces in the rolling `TELEMETRY / UNEXPECTED SIGNALS` section of
`simard status`. Once the fix holds you should see `parse-fix holding  yes
(distill parse-fail 0%)` and **no** `distill parse-fail rate` anomaly line:

```bash
simard status | sed -n '/TELEMETRY \/ UNEXPECTED SIGNALS/,/^$/p'
```

For the workstream-gap / blocked-goal signals bundled in the original
signature (which are correlational, not caused by the distill fix), see
[Review overseer workstream gaps](./review-overseer-workstream-gaps.md) and
[Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md) — they are handled on
their own tracks.

## Step 7 — Verify the fix in the test suite

The recovery pass ships with unit tests for the repair helper and regression
tests for the parser. Run them to confirm the fix locally:

```bash
# Brick A — the string-aware stripper (adversarial inputs incl.
# comma-in-string, escaped-quote-then-comma, emoji, empty, whitespace-only):
cargo test -p simard recipe_output::extract

# Bricks B/C — trailing-comma recovery + zero-facts warn end-to-end:
cargo test -p simard distillation
```

Key assertions:

- A bare and an enveloped trailing-comma document each recover **≥1 fact**
  (`parse_facts_document` returns `Ok`).
- A comma **inside** a string literal is left untouched (content integrity).
- Genuinely malformed input still returns `Err` (never a hollow `Ok`).

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| `parse_recovery` key absent from `context` | Build predates #2669 | Upgrade to a build that includes the recovery pass. |
| Rate still `~0.0`, labels all `deferred` | Residual failure is **not** a trailing comma | Harvest with raw-capture (Step 4); the fix targets a different class. |
| `recovered` share climbing over time | Agent systematically emits malformed JSON | Fix the distill prompt/agent; the repair is masking a real defect (Step 3). |
| Many `zero-facts` warns | Agent emits out-of-category facts | Tune the prompt; parser is fine (Step 5). |

## Related

- [Distill trailing-comma recovery](../reference/distill-trailing-comma-recovery.md)
  — the full API, data contract, and security model.
- [Distill recipe output capture](../reference/distill-recipe-output-capture.md)
  — the envelope parse contract.
- [Telemetry metrics](../reference/telemetry-metrics.md) — the `metrics.jsonl`
  reader surface.
- [Capture and diagnose a failing distill sample](./capture-and-diagnose-a-failing-distill-sample.md)
  — harvest a still-failing (non-trailing-comma) sample.
