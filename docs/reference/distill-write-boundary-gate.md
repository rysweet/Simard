---
title: Distill write-boundary gate & memory-IPC gated write
description: Reference for the single authoritative write-boundary gate that scores, quarantines, and dedups every distilled fact server-side (issue #2679 / #2433), the additive memory-IPC StoreFactGated / StoreProcedureProvenance requests and FactWrite response, the shared fact_reliability scorer, the removal of the parse_fail telemetry class, and the daemon socket hardening (0600/0700 + MAX_FRAME).
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ../architecture/distillation-semantic-handoff.md
  - ./simard-memory-remember-cli.md
  - ./rpc-wire-protocol.md
  - ./cognitive-memory-provenance.md
  - ./telemetry-metrics.md
  - ./trustworthy-confidence-api.md
  - ../architecture/rpc-pattern.md
  - ../memory.md
---

# Distill write-boundary gate & memory-IPC gated write

> Shipped in issue [#2679](https://github.com/rysweet/Simard/issues/2679).
> Homes the ISAO reliability gate first landed in
> [#2433](https://github.com/rysweet/Simard/issues/2433) at the **write
> boundary** so it still runs when facts arrive as direct agent writes instead
> of a parsed batch.

When distillation stopped returning a parseable document
([distillation semantic handoff](../architecture/distillation-semantic-handoff.md)),
the reliability/quarantine/dedup logic that used to run in Simard *after the
parse* had nowhere to sit. This reference documents where it moved, the IPC
schema that carries a gated write, the shared scorer, and the telemetry and
socket-hardening consequences.

---

## The single authoritative gate

Every distilled fact is decided in **exactly one place**: the memory IPC
server's handler for a gated-write request. There is no client-side gate and no
un-gated write variant reachable by the distiller. The handler applies, in
order:

1. **Input validation** — every field is opaque data; a required field that is
   empty (`concept`, `content`) or a field over its length cap is rejected
   (quarantined, nothing stored) rather than truncated silently. `MAX_FRAME`
   already bounds the whole request.
2. **Grounding (store-existence check)** — confirm the fact's supplied
   `source_episode_ids` resolve to real episode nodes **in the store**. The
   server holds no in-memory batch, so grounding is an existence lookup, not a
   batch scan: the fact is grounded if **at least one** supplied id resolves.
   An ungrounded fact (no id resolves) scores low and is quarantined — never
   stored blind. The write records provenance to each resolved id. Each cited id
   is first passed through `fact_reliability::normalize_source_episode_id`
   (trim surrounding whitespace, drop empties), and that **normalized** set is
   used for *both* the existence check *and* the persisted `DERIVES_FROM` edges.
   Episode node ids (UUID-v7 / ULID) never carry whitespace, so this is a no-op
   for a well-formed id; it only rescues an id an LLM re-emitted with a stray
   leading/trailing newline or space, which would otherwise fail the exact
   grounding match — silently quarantining a genuinely grounded fact (lost
   fact-yield) or, if grounded, dangling its `DERIVES_FROM` edge on the padded
   key. The in-process stub seam normalizes its cited id identically before its
   batch-membership lookup, so both seams ground and thread provenance the same
   way (interior whitespace is preserved — a genuinely different id stays
   ungrounded rather than silently folding).
3. **Reliability scoring** — `fact_reliability::score_fact_reliability` computes a
   confidence in `[0.0, 1.0]` (already clamped into range by the scorer) from the
   fact and its resolved grounding. The client cannot supply this; any
   client-provided `confidence` hint is ignored. There is **no** confidence floor
   — an ungrounded or empty fact keeps its genuinely low score so the threshold
   can quarantine it. (Fail-closed: an unscoreable input scores low, never stored
   blind.) The content-quality component scores **distinct informative words** —
   alphanumeric-bearing tokens, case/punctuation-normalized and de-duplicated —
   rather than raw whitespace tokens, so content that carries no information
   (empty, whitespace-only, or punctuation/symbol-only such as `"... ... ..."`)
   is hard-gated to `0.0`, and degenerate repetition (`"the the the"`) earns only
   the partial short-content weight instead of full credit.
4. **Threshold quarantine** — if the score is below
   `DISTILL_RELIABILITY_THRESHOLD`, the fact is **quarantined** (counted, not
   stored) so a low-reliability candidate can never corrupt past experience.
5. **Identity dedup** — a weaker-or-equal new fact never clobbers a
   higher-confidence existing fact of the **same identity** (concept + content).
   Distinct lessons that merely share a label still accumulate. A known concept
   the agent re-emits in a variant surface form (`PR-Pattern`, `pr_pattern`,
   `bug pattern`) is first folded to its canonical `KNOWN_CONCEPTS` label via
   `fact_reliability::canonical_concept`, so the dedup key AND the stored concept
   are one identity: a variant-labelled restatement dedups instead of storing a
   redundant near-duplicate, and concept-consistent recall for the canonical
   label surfaces every fact of that concept instead of missing the ones stored
   under a variant form (the concept-axis analog of the content-whitespace and
   episode-id normalizations). A genuinely off-spec concept is preserved verbatim.
6. **Persist** — surviving facts are written with
   `store_fact_with_provenance` (computed confidence, **one `DERIVES_FROM` edge
   per supplied `source_episode_id`**, and a scalar `source_id` of
   `distill:{ids[0]}` — the first/primary id, with the full provenance set
   carried by the edges). When exactly one id is supplied this reduces to the
   familiar `distill:{source_episode_id}` form.
7. **Pass ledger** — the disposition (stored / quarantined) is recorded against
   the request's opaque `pass_id` so the scheduler can report accurate counts
   without parsing anything.

Because the gate is server-side, the same guarantees hold whether a fact arrives
from the distiller, a manual `simard memory remember`, or any future agentic
writer.

---

## `fact_reliability` — the shared scorer

`src/fact_reliability.rs` is a small, mostly-pure module holding the scorer, the
threshold constant, the concept-label helpers, and — since the #2679 refactor —
the shared **write-boundary gate orchestration** itself
(`commit_gated_fact`: score → threshold → identity-dedup → persist). It exists so
the **stub in-process loop** (used by test runners) and the **IPC handler** reach
the **same disposition** for the same fact — one implementation, two call sites.
Each seam resolves only what genuinely differs per boundary (grounding —
batch-membership vs. store-existence) and then defers the score/threshold/dedup/
persist decision to the one shared function, so the two seams can never drift.
Consolidating the gate here also shrinks the `src/memory_consolidation` fork,
keeping engine-shaped logic consolidated per the G2 architecture rule.

The scorer is a **pure function of one fact plus its resolved grounding** — it
takes **no batch argument**. Grounding is the dominant, *necessary* signal (a
grounded fact already clears the threshold; an ungrounded one never can), so
the store/quarantine **decision is fully determined per-fact**. The legacy
in-process scorer additionally awarded a small same-concept **corroboration**
bonus computed over the whole batch; that term is **deliberately excluded** from
the shared scorer because (a) a per-fact IPC write has no sibling batch to
inspect, and (b) it is **disposition-neutral** — corroboration was awarded only
to already-grounded facts and merely nudged an already-storable confidence
upward (e.g. `0.9 → 1.0`), never flipping `store ↔ quarantine`. Dropping it is
exactly what lets the stub and the IPC handler agree on every decision.

| Symbol | Meaning |
|--------|---------|
| `fact_reliability::score_fact_reliability(concept: &str, content: &str, grounded: bool) -> f64` | Confidence in `[0.0, 1.0]` from the fact's concept/content and whether its provenance resolved. Deterministic; fail-closed (ungrounded / empty → low → quarantined). No batch argument. |
| `fact_reliability::fact_passes_gate(concept, content, grounded) -> bool` | Thin predicate: `score >= RELIABILITY_THRESHOLD`. The shared store/quarantine decision. |
| `fact_reliability::commit_gated_fact(memory, concept, content, grounded, source_id, tags, source_episode_ids) -> SimardResult<FactGateDecision>` | The shared gate orchestration both seams call: score → threshold → identity-dedup → `store_fact_with_provenance`. Grounding is resolved by the caller; the returned `FactGateDecision` is `Stored { confidence, node_id }` or `Quarantined { confidence }` (a `confidence >= RELIABILITY_THRESHOLD` quarantine is a dedup skip, below is a low-reliability block). |
| `fact_reliability::RELIABILITY_THRESHOLD` (re-exported as `distillation::DISTILL_RELIABILITY_THRESHOLD`) | Minimum confidence to store rather than quarantine (`0.5`). |
| `fact_reliability::canonical_concept(label) -> Option<&'static str>` | Canonicalises / validates a concept label. Applied by `commit_gated_fact` before the dedup key and stored concept are derived, so a known label's surface variants (`PR-Pattern`, `pr_pattern`, `bug pattern`) fold onto one identity; an off-spec label (returns `None`) is stored verbatim. |
| `fact_reliability::normalize_source_episode_id(raw: &str) -> &str` | Canonical grounding / provenance key for a cited episode id: trim surrounding whitespace (no-op for a well-formed id; interior whitespace preserved). Both seams normalize with this so a padded id grounds and threads provenance identically. |

> **G2 / D3 note.** #2679 homes the gate at the IPC handler that fronts
> `amplihack-memory-lib`; it does **not** add a new memory-engine primitive, so
> **no `amplihack-memory` pin bump is required** for this change. If a future
> revision pushes the gate *into* the library as a first-class gated-write API,
> that is where the pin bump would happen — not in `src/memory_consolidation`.

---

## Memory-IPC schema (additive)

The write is carried by **new, additive** request variants on `MemoryRequest`
and one new response variant on `MemoryResponse`. Every pre-existing variant
(`StoreFact`, `StoreProcedure`, `GetStatistics`, …) is untouched, so existing
clients and the `CognitiveMemoryOps` trait stay byte-compatible.

### Request variants

```jsonc
// Gated single-fact write (backs `simard memory remember`).
{ "op": "store_fact_gated",
  "concept": "bug-pattern",
  "content": "recipe-runner-rs E2BIG when a 50-episode batch is inlined on argv",
  "source_episode_ids": ["ep_A", "ep_B"],
  "pass_id": "distill-2026-07-06T12:00:00Z-7f3a" }

// Procedure write with provenance (backs `simard memory remember-procedure`).
{ "op": "store_procedure_provenance",
  "name": "ci-fix:auto",
  "steps": ["reproduce locally", "bisect", "apply minimal fix"],
  "prerequisites": [],
  "source_episode_ids": ["ep_C", "ep_D"],
  "pass_id": "distill-2026-07-06T12:00:00Z-7f3a" }
```

Note there is **no `confidence` field** on the request — scoring is the server's
job.

### Response variant

```jsonc
// Result of a gated write. `disposition` is "stored" or "quarantined";
// both are successful outcomes.
{ "ok": "fact_write",
  "value": { "disposition": "stored",
             "confidence": 0.90,
             "node_id": "fact_01H...",   // present when stored
             "derives_from": 2 } }
```

The existing `{ "ok": "error", "value": "<msg>" }` variant still carries backend
failures (which map to CLI exit `4`).

### Client wrappers

`RemoteCognitiveMemory` gains **inherent** (non-trait) methods
`remember_fact_gated(..)` and `remember_procedure_provenance(..)` that build the
new requests and interpret the `FactWrite` response. They are inherent, not
`CognitiveMemoryOps` trait methods, so the trait — and every existing impl,
including `SharedMemory` and the read-only clients — stays exactly as it was.

---

## Input validation & framing

The IPC handler treats every field as **opaque data** and validates at the
boundary:

- **Length caps** on `concept`, `content`, `name`, each `step`, and each
  `source_episode_id`. Over-long fields are rejected, not truncated silently.
- **Empty-field rejection** for required fields (`concept`, `content`, `name`,
  ≥1 `step`).
- **`MAX_FRAME` cap** in `read_frame` (introduced by #2679 — `read_frame` was
  previously uncapped): a length prefix above the cap is rejected before
  allocation, bounding a single request's memory (DOS-1). This applies to *all*
  IPC traffic, not just gated writes.
- Fact bodies are **never logged**; only counts, concepts, and dispositions
  appear in logs/metrics.

---

## Daemon socket hardening (AUTHZ-3 / DOS-1)

The memory socket is now created with restrictive permissions:

- The socket file is `0600` (owner read/write only).
- Its parent directory is `0700`.
- `read_frame` gains a `MAX_FRAME` ceiling (new in #2679; it was previously
  uncapped) so a malformed or hostile length prefix cannot force a giant
  allocation.

These apply to the whole memory-IPC surface; the gated-write path inherits them.

---

## Telemetry changes

The parse-based failure telemetry is **removed** because the parse is gone:

| Removed | Replacement |
|---------|-------------|
| `simard.distill.runs{result="parse_fail"}` | — (unreachable; only `result="ok"` and structural failure classes remain) |
| `distill_parse_success_rate` | — (no parse to succeed/fail) |
| raw-capture-on-parse-failure diagnostic | — (nothing to capture) |

Preserved / added:

| Metric | Meaning |
|--------|---------|
| `simard.distill.runs{result="ok"}` | A distillation pass whose agentic commit completed. |
| `simard.distill.facts` | Facts **accepted by the gate** for the pass — sourced from the write ledger keyed by `pass_id`, not a parsed array length. |
| `simard.distill.procedures` | Procedures written for the pass. |
| `simard.distill.episodes_marked` | Episodes marked distilled after the pass. |
| distill write ledger (per `pass_id`) | Integer counts of stored vs quarantined writes; the authoritative source for the report. |

See [Telemetry metrics → distillation](./telemetry-metrics.md#distillation-simarddistill).

---

## Failure classes after #2679

| Class | Reachable? | Notes |
|-------|-----------|-------|
| `ParseFailure` / `parse_fail` | **No — removed** | There is no parse. |
| spawn failure | Yes | recipe-runner-rs could not be launched. |
| recipe terminal failure | Yes | non-zero exit from the agentic step. |
| no write endpoint | Yes (skips pass) | socket absent → `Ok(None)`, retried next cycle. |

Structural failures leave the batch **unmarked** and retry next pass, exactly as
before.

---

## Follow-on candidates (filed, out of scope here)

The same "agents marshal meaning through scraped stdout" anti-pattern exists at
other sites. #2679 files follow-on issues to migrate them to a semantic handoff,
but does **not** change them:

- `extract_verdict` callers — merge-readiness / recipe-merge judge verdict
  ([#2716](https://github.com/rysweet/Simard/issues/2716)).
- Remaining `extract_json_payload` / `balanced_objects` callers — the OODA brain
  decide/orient verdict in `recipe-brain`
  ([#2715](https://github.com/rysweet/Simard/issues/2715)).
- A possible `amplihack-memory-lib` gated-write primitive (R2) to eventually
  home the gate inside the library rather than the Simard-side IPC handler.
