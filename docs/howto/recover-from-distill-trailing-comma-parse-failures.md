---
title: Recover from distill trailing-comma parse failures
description: Runbook for the recurring overseer-obs:anomaly:distill parse-fail rate 100% signature — confirm the trailing-comma failure mode from distill_parse_success_rate and the distill logs, verify the string-aware strip_json_trailing_commas recovery is engaged, tell parse-fail apart from the zero-facts yield-loss warn, and lock a real failing sample down as a T1–T5 regression test.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/distill-trailing-comma-recovery.md
  - ../reference/distill-recipe-output-capture.md
  - ../reference/distill-raw-capture-on-parse-failure.md
  - ../howto/capture-and-diagnose-a-failing-distill-sample.md
  - ../architecture/episode-distillation.md
---

# Recover from distill trailing-comma parse failures

Use this how-to when the Overseer reports the recurring signature

```text
overseer-obs:anomaly:distill parse-fail rate 100%
```

together with any of the starved goals it drags down —
`process:distill_fail`, `quality:gym_skipped`, or a
`goal:blocked:*` parity goal (e.g. `advance-…-kgpacks-rs-to-full-parity`).
A **100%** distill parse-fail rate means the learning loop is dead: distillation
runs, exits `0`, but stores **zero facts** every pass, so nothing is promoted to
semantic/procedural memory and every downstream goal that depends on learning
starves.

The most common cause — and the one this runbook targets — is a **trailing
comma** in the distill agent's otherwise-valid JSON:

```json
{ "facts": [ {"concept":"bug-pattern","content":"…","source_episode_id":"t=42"}, ] }
```

Strict `serde_json` (JSON, not JSON5) rejects the `,]`. The shipped
**string-aware trailing-comma recovery** repairs exactly this case after strict
parsing has already missed. For the full contract, see
[Distill trailing-comma parse recovery](../reference/distill-trailing-comma-recovery.md).

## Before you start

- A running (or runnable) OODA daemon that performs distillation passes.
- Read access to `~/.simard/` (state home) and its `~/.simard/metrics/metrics.jsonl` sink.
- The Simard source checkout, to add a regression test once you have a sample.

---

## Step 1 — Confirm the failure rate

`distill_parse_success_rate` is emitted once per pass that reached output
parsing. A run of `0.0` values is the smoking gun behind the anomaly.

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl | tail -20
```

- A tail of `"value":0.0` events → parse is failing on **every** pass (the 100%
  anomaly). Continue to Step 2.
- Values recovering toward `1.0` → the fix is engaged; the anomaly should clear
  within the Overseer's observation window. Skip to Step 5 to lock it down.

## Step 2 — Confirm it is the trailing-comma mode (not yield-loss)

Two failure modes produce "distillation stored no facts," and they need
opposite fixes. Tell them apart from the logs:

```bash
grep 'simard::distill' ~/.simard/logs/*.log | tail -40
```

| What you see | Mode | Meaning |
|---|---|---|
| Tier-3 deferral / `Err`, **no** `kept_facts=0` warn | **parse-fail** | output never became a `RecipeEnvelope` — this runbook |
| `WARN … valid distill parse yielded zero allow-listed facts input_concepts=N kept_facts=0` | **yield-loss** | output parsed fine, but every concept was off the allow-list — a *prompt-side* problem, not a parser one |

If you see the `kept_facts=0` warn, the parser is working — the agent is
emitting concepts outside `pr-pattern` / `bug-pattern` / `lesson-learned`. Fix
the distill prompt, not the parser. Stop here.

If you see deferral with **no** such warn, you have a genuine parse-fail —
continue.

The `kept_facts=0` warn fires **at most once per pass**, and only on a pass that
ultimately stored **zero** facts — a pass that recovers and keeps ≥ 1 fact never
emits it. So a lone warn is never noise from an otherwise-successful pass; if it
is present, the pass really did yield nothing.

## Step 3 — Capture the exact failing bytes

Harvest the real output rather than guessing at it, using the env-gated
raw-capture diagnostic (see
[Capture and diagnose a failing distill sample](../howto/capture-and-diagnose-a-failing-distill-sample.md)):

```bash
SIMARD_DISTILL_RAW_CAPTURE=1 simard ooda run   # or restart the daemon with it set
ls -t ~/.simard/distill-captures/distill-parsefail-*.txt | head
```

Open the newest capture and look for the tell-tale `,}` or `,]` immediately
before a closing brace/bracket. If it is there, you have confirmed the
trailing-comma mode.

## Step 4 — Verify recovery is engaged

The recovery is delete-only and string-aware, and it runs **only after** strict
parsing misses and **only when** a structural trailing comma was actually
present. Confirm the code path exists in your build:

```bash
# The primitive and its re-export:
grep -rn 'strip_json_trailing_commas' src/recipe_output/
# The recover-only-on-Owned wiring in the distiller:
grep -n 'strip_json_trailing_commas' src/memory_consolidation/distillation.rs
```

You should see the `pub fn strip_json_trailing_commas` definition in
`src/recipe_output/extract.rs`, its re-export in `src/recipe_output/mod.rs`, and
the recovery branch inside `scan_cleaned_for_facts` in `distillation.rs`. If
those greps come back empty, your deployed binary predates the fix — rebuild and
redeploy from a source tree that includes it, then re-check Step 1.

## Step 5 — Lock the sample down as a regression test

Turn the harvested bytes into a permanent test so the exact failure can never
silently return. Add to the `#[cfg(test)]` module in
`src/memory_consolidation/distillation.rs`, using the existing
`runner_envelope()` / `step()` fixtures. Mirror the T1–T6 matrix from the
[reference](../reference/distill-trailing-comma-recovery.md#regression-tests):

```rust
#[test]
fn recovers_bare_trailing_comma_payload() {
    // T1: bare `{"facts":[ … ],}` → parses, yields ≥ 1 fact.
    let raw = r#"{"facts":[{"concept":"bug-pattern","content":"x","source_episode_id":"t=42"},]}"#;
    let out = scan_cleaned_for_facts(raw).expect("trailing comma recovered");
    assert_eq!(out.facts.len(), 1);
}

#[test]
fn recovery_preserves_comma_inside_string() {
    // T3: an object with BOTH a structural trailing comma (`},]}`, which
    // triggers recovery) AND a comma inside a string value (which must survive
    // verbatim). This exercises the wired recovery path together with its
    // string-awareness.
    let raw = r#"{"facts":[{"concept":"bug-pattern","content":"a, b,","source_episode_id":"t=7"},]}"#;
    let out = scan_cleaned_for_facts(raw).expect("trailing comma recovered");
    assert_eq!(out.facts[0].content, "a, b,");
}

#[test]
fn malformed_json_still_defers() {
    // T4: genuinely malformed JSON is NOT a trailing-comma defect — still None
    // (→ Err → deferred batch), never a hollow Ok.
    assert!(scan_cleaned_for_facts(r#"{"facts": [ {"concept": "#).is_none());
}
```

Also add the primitive-level units next to `strip_json_trailing_commas` in
`src/recipe_output/extract.rs`, asserting the `Cow::Borrowed` clean path
(byte-identical) and `Cow::Owned` on removal. Note that a *valid* payload with
no structural trailing comma (e.g. a comma only inside a string value) parses on
the fast path and never invokes recovery — the focused in-string guard for the
recovery primitive itself is the `strip_json_trailing_commas` unit test in
`extract.rs` (`"a, b,"` → `Cow::Borrowed`, untouched), while
`recovery_preserves_comma_inside_string` above proves the *wired* recovery path
preserves it.

Run the suites:

```bash
cargo test -p simard recipe_output
cargo test -p simard memory_consolidation::distillation
```

## Step 6 — Confirm recovery in production

Redeploy and watch the metric climb off the floor:

```bash
grep '"metric_name":"distill_parse_success_rate"' ~/.simard/metrics/metrics.jsonl | tail -20
```

You are done when:

1. New `distill_parse_success_rate` events read `1.0` for trailing-comma
   batches (was `0.0`).
2. The Overseer stops emitting `anomaly:distill parse-fail rate 100%`.
3. Facts are again promoted to semantic/procedural memory, unstarving
   `process:distill_fail` and any `goal:blocked:*` parity goal that was waiting
   on the learning loop.

---

## Worked example

**Symptom.** Overseer: `anomaly:distill parse-fail rate 100%`; two parity goals
`Blocked`; `quality:gym_skipped`.

**Step 1.** `metrics.jsonl` shows 12 consecutive `distill_parse_success_rate`
`0.0` events. Confirmed 100%.

**Step 2.** Logs show Tier-3 deferral every pass; **no** `kept_facts=0` warn →
parse-fail, not yield-loss.

**Step 3.** Capture `distill-captures/distill-parsefail-2026-07-06T115301Z-a1b2c3.txt`
ends with `…"source_episode_id":"t=9664"}, ]}` — a structural trailing comma.

**Step 4.** `grep` confirms `strip_json_trailing_commas` and the
`scan_cleaned_for_facts` recovery branch are present in the deployed source.

**Step 5.** Add the captured span as `recovers_bare_trailing_comma_payload`
(T1). `cargo test` green.

**Step 6.** After redeploy, `distill_parse_success_rate` events read `1.0`; the
anomaly clears; facts flow again; the parity goals leave `Blocked`.

---

## What this runbook does **not** cover

- **ANSI / launcher-banner** parse failures — see
  [Diagnose decide/orient parse failures](./diagnose-decide-orient-parse-failures.md)
  and [Text-parsing wire formats](../reference/text-parsing-wire-formats.md).
- **Yield-loss** (valid parse, zero allow-listed concepts, `kept_facts=0` warn)
  — a distill-prompt problem; widen or correct the concept labels the agent
  emits, not the parser.
- **External `agent-kgpacks-rs` parity work** (`#16`/`#17`) — those land in the
  `rysweet/agent-kgpacks-rs` repo. Restoring the distill learning loop here is
  what lets the OODA self-heal clear the stale block; it does not edit that
  repo's code.
