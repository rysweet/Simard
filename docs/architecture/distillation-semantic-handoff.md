---
title: Distillation semantic handoff
description: How the distiller became a true agentic step that writes facts DIRECTLY into cognitive memory through the memory write tool, removing the JSON-scrape-and-deserialize gate from the distillation result path so a trailing comma or a noisy launcher banner can no longer discard a whole batch (issue #2679 / #2658).
last_updated: 2026-07-06
owner: simard
doc_type: concept
related:
  - ./episode-distillation.md
  - ./cognitive-memory.md
  - ./rpc-pattern.md
  - ../reference/simard-memory-remember-cli.md
  - ../reference/distill-write-boundary-gate.md
  - ../reference/automatic-distillation-scheduler.md
  - ../reference/telemetry-metrics.md
  - ../howto/verify-the-distillation-semantic-handoff.md
  - ../memory.md
---

# Distillation semantic handoff

> Shipped in issue [#2679](https://github.com/rysweet/Simard/issues/2679)
> (root cause shared with [#2658](https://github.com/rysweet/Simard/issues/2658)).
> This is the successor to the JSON-file result path documented in
> [Episode distillation](./episode-distillation.md): the batch of episodes,
> the concept labels, the threshold gate, and the mark-everything rule are all
> unchanged — **only the way distilled facts reach memory changed.**

Episode distillation is a **many-to-few** pass: it reads a batch of recent
episodes and produces a small number of durable semantic facts. Before #2679
the distiller agent was told to *print* (later, *write to a file*) a
`{ "facts": [...] }` envelope, and Simard then **scraped that text back out and
hand-deserialized it** with `serde_json`. That deserialize was the single
failure surface: one malformed token — a trailing comma, a stray log line, an
ANSI escape from the copilot launcher banner — made the strict parse fail and
**discarded the entire batch**. The parse-fail rate had climbed from 91% to
100%.

The fix does **not** make the parser more lenient. It **removes the parse
entirely.** The distiller is now a real agentic step whose *writes are its
output*: it calls the cognitive-memory write tool once per fact, and those
writes land in the same store Simard reads from. There is no return document
for Simard to deserialize, so the trailing-comma / noisy-stdout failure mode is
**structurally impossible** — there is nothing left to fail.

---

## What changed in one picture

**Before (#2281 → #2622, removed):**

```
distiller agent  ──writes──▶  facts.json  ──Simard reads & serde_json::from_str──▶  DistillOutput
                                              │
                                              └─ trailing comma / noisy bytes → PARSE FAIL → whole batch discarded
```

**After (#2679):**

```
distiller agent ──`simard memory remember` (once per fact)──▶ memory IPC socket ──▶ write-boundary gate ──▶ cognitive memory
                                                                                        (ground · score · quarantine · dedup)
        (no return payload — the agent's writes ARE the output; nothing for Simard to parse)
```

Simard's role shrinks to: **prepare the batch, guarantee a live write endpoint,
run the agent, and count how many facts it successfully committed.** No text is
extracted. No JSON is deserialized. `DistilledFact`, `DistillOutput`, the
balanced-object scanner, and every `serde_json::from_str` on agent output are
gone from the result path.

---

## The semantic handoff, step by step

1. **Batch selection (unchanged).** `distill_recent_episodes_with_runner`
   pulls up to `DISTILL_BATCH_SIZE` (50) undistilled episodes. If fewer than
   `DISTILL_MIN_EPISODES` (20) are available, the pass is skipped entirely — no
   agent, no writes, no markers. See
   [Episode distillation → threshold gate](./episode-distillation.md#threshold-gate).

2. **Live-endpoint guarantee (new — D4 invariant).** Before the agentic step
   runs, the scheduler ensures a reachable cognitive-memory **write** endpoint:
   the OODA daemon's memory IPC socket (`<state_root>/memory.sock`, resolved by
   `socket_path_for`). The subprocess runner threads that socket path to the
   agent through the `SIMARD_MEMORY_SOCKET` environment variable, plus an
   opaque `distill_pass_id` used only to tag and count this pass's writes. If no
   endpoint can be guaranteed, the pass is **skipped gracefully** (`Ok(None)`)
   exactly like an unavailable recipe runner — distillation never blocks the
   OODA cycle, and it never falls back to stdout scraping.

3. **Agentic write (new).** The `distill-episodes` recipe no longer asks the
   agent to emit a JSON envelope. It instructs the agent to call
   [`simard memory remember`](../reference/simard-memory-remember-cli.md) **once
   per fact** (and `simard memory remember-procedure` once per recurring
   procedure). Each call is one process, one fact — scalar flags only, no
   document. The CLI routes through the memory IPC client to the daemon. The
   agent's file-reading tool still reads the episode batch from `episodes_path`;
   only the *output* channel changed.

4. **Write-boundary gate (moved — D2).** Every fact passes through **one
   authoritative gate on the server side** of the IPC boundary: grounding by
   confirming the fact's supplied `source_episode_ids` resolve to real episode
   nodes **in the store** (a store-existence check — the server holds no
   in-memory batch, and a fact is grounded if **at least one** supplied id
   resolves), ISAO-style reliability scoring, confidence clamp + minimum
   threshold, low-reliability **quarantine**, and identity dedup, before
   `store_fact_with_provenance` persists it with one `DERIVES_FROM` edge per
   supplied source episode. The gate now lives at the write boundary because
   direct agent writes bypass the old post-parse location. The client is never
   trusted to pre-score; the server is the sole decision point. See
   [Distill write-boundary gate](../reference/distill-write-boundary-gate.md).

5. **Marking (unchanged).** On a clean agent run, every input episode is marked
   distilled — including those the agent chose to skip — so the batch is never
   re-fed. If the agentic step fails as a whole (non-zero exit), no markers are
   set and the batch retries next pass, preserving the original retry-safety
   invariant.

6. **Reporting (new source).** Success is "the agentic commit completed." The
   fact count in `DistillReport` and the `simard.distill.facts` metric come from
   the **write ledger** — the number of facts the gate actually accepted for
   this `distill_pass_id` — not from the length of a parsed array. Quarantined
   candidates are counted separately, exactly as before.

---

## Why removal, not tolerance

The facts are **only ever consumed by another agent** — the cognitive-memory
brain that reads them back during preparation. Marshalling them through a strict
JSON envelope that Simard hand-parses was pure ceremony sitting between two
agents. It added no value and was the entire failure surface. A lenient parser
would have kept the ceremony and merely hidden some failures; representing each
fact as a **tool call** removes the document, and with it the class of failure.

This is a **semantic** agent-to-agent handoff: the meaning (a fact, a concept, a
provenance link) is transmitted as a memory operation, not as a serialized blob
that must survive a lossy stdout channel intact.

---

## The `parse_fail` failure class is gone

`DistillFailureClass::ParseFailure`, the `ATTR_RESULT="parse_fail"` telemetry
attribute, the `distill_parse_success_rate` denominator, and the env-gated
raw-capture-on-parse-failure diagnostic are all **removed**. No code path can
emit `parse_fail` because there is no parse. The surviving failure classes are
the structural ones that never involved parsing (spawn failure, recipe-reported
terminal failure). Success telemetry (`simard.distill.runs` `result=ok`,
`simard.distill.facts`, `simard.distill.procedures`,
`simard.distill.episodes_marked`) is preserved. See
[Telemetry metrics](../reference/telemetry-metrics.md#distillation-simarddistill).

> **Migration note for dashboards & alerts.** Any panel or alert keyed on
> `simard.distill.runs{result="parse_fail"}` or `distill_parse_success_rate`
> will read zero / empty after #2679 — those series no longer exist. Track
> `simard.distill.runs{result="ok"}` and `simard.distill.facts` instead.

---

## Back-compatibility for test & stub runners

The `DistillRecipeRunner` trait change is **additive with defaults**. Stub and
test runners that implement the in-process `run` / `run_all` path still compile
and pass unmodified: they perform their (mock) memory writes in process rather
than shelling out, and the result path counts the ledger the same way. Only
`RecipeRunnerSubprocess` overrides the new behaviour to inject
`SIMARD_MEMORY_SOCKET` + `distill_pass_id` and treat exit-0 as success with no
stdout consumption.

---

## Boundaries — what this change does *not* touch

- **The batch, labels, threshold, and marking rules** are unchanged; see
  [Episode distillation](./episode-distillation.md).
- **The shared `recipe_output` extract helpers** (`extract_json_payload`,
  `balanced_objects`, `extract_verdict`) are **not deleted** — other,
  out-of-scope agent→agent stdout-scrape sites (merge-readiness judge, progress
  reviewer, recipe-brain) still use them. Those are filed as **follow-on
  candidates** for the same semantic-handoff treatment
  ([#2715](https://github.com/rysweet/Simard/issues/2715) — OODA brain
  decide/orient verdict; [#2716](https://github.com/rysweet/Simard/issues/2716) —
  merge-readiness judge verdict); migrating them is out of scope here.
- **No lenient/tolerant JSON parsing** is introduced anywhere as a substitute.

See also: [Distill write-boundary gate](../reference/distill-write-boundary-gate.md)
for the gate + IPC schema, the
[`simard memory remember` CLI reference](../reference/simard-memory-remember-cli.md)
for the write tool, and the how-to
[Verify the distillation semantic handoff](../howto/verify-the-distillation-semantic-handoff.md).
