---
title: Memory write CLI (`simard memory remember`)
description: Operator and agent reference for `simard memory remember` and `simard memory remember-procedure` — the per-record cognitive-memory WRITE commands the distiller agent calls (one process = one fact) so distilled knowledge reaches memory as tool calls instead of a scraped JSON envelope (issue #2679).
last_updated: 2026-07-06
owner: simard
doc_type: reference
related:
  - ./simard-memory-cli.md
  - ./distill-write-boundary-gate.md
  - ./cognitive-memory-provenance.md
  - ./state-root-resolution.md
  - ../architecture/distillation-semantic-handoff.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ../memory.md
---

# Memory write CLI (`simard memory remember`)

> Shipped in issue [#2679](https://github.com/rysweet/Simard/issues/2679).
> This is the **write** companion to the read-only
> [`simard memory stats` / `dump`](./simard-memory-cli.md) commands and the
> guarded [`simard memory import`](./simard-memory-cli.md#simard-memory-import)
> restore path.

`simard memory remember` writes **one** semantic fact into Simard's cognitive
memory. `simard memory remember-procedure` writes **one** procedure. They are
the agent-facing tool the distiller calls during the
[distillation semantic handoff](../architecture/distillation-semantic-handoff.md):
instead of printing a `{ "facts": [...] }` envelope for Simard to scrape and
deserialize, the distiller agent calls `remember` once per fact. The write **is**
the output — there is no return document to parse, so a trailing comma or a
noisy launcher banner can no longer discard a batch.

Both commands are **single-record by design**: one process = one fact (or one
procedure). There is deliberately no batch/array form and no JSON-body form —
that would reintroduce the parse this feature removed. Facts are passed as
**scalar flags** only.

---

## Why single-record, scalar-flag

The whole point of #2679 is that **no Simard-side document is ever
deserialized**. A `remember --concept ... --content ...` invocation carries its
fields as argv scalars that the CLI packs straight into a typed IPC request; the
daemon reads typed fields, never free text it must re-parse. Emitting N facts is
N calls. This keeps the failure surface at zero: there is no envelope, no array,
no trailing comma to get wrong.

---

## `simard memory remember`

Write a single semantic fact.

```text
Usage: simard memory remember
         --concept <pr-pattern|bug-pattern|lesson-learned|...>
         --content <TEXT>
         [--source-episode-id <ID> ...]
         [--confidence <0..1>]
         [--tags <a,b,c>]
         [--pass-id <OPAQUE>]
         [state-root]
```

| Flag | Required | Meaning |
|------|----------|---------|
| `--concept <LABEL>` | yes | Concept label for the fact. The distiller uses `pr-pattern`, `bug-pattern`, or `lesson-learned`; other callers may use any non-empty label. |
| `--content <TEXT>` | yes | The fact body — a short, declarative sentence. Stored verbatim. Length-capped at the IPC handler (see [write-boundary gate](./distill-write-boundary-gate.md#input-validation-framing)). |
| `--source-episode-id <ID>` | no (repeatable) | `node_id` of a source episode this fact derives from; repeat for several. The daemon writes **one `DERIVES_FROM` edge per id**, grounds the fact if **at least one** id resolves to a real episode node in the store, and sets the scalar `source_id` to `distill:{first id}`. |
| `--confidence <0..1>` | no | A confidence **hint** only. The server IGNORES it and re-derives the stored confidence from the write-boundary gate; it is accepted (and range-parsed) purely so a caller may pass one without erroring. |
| `--tags <a,b,c>` | no | Comma-separated tags stored with the fact. Empty entries are dropped. |
| `--pass-id <OPAQUE>` | no | Opaque pass identifier used to attribute this write to a distillation pass in the daemon's write ledger, so the pass can count how many facts the gate accepted. Injected by the runner; irrelevant for manual use. |
| `state-root` (positional) | no | Explicit state root; the socket resolves to `<state_root>/memory.sock`. Omit to fall back to `$SIMARD_STATE_ROOT`, then `$HOME/.simard` (see [State-root resolution](./state-root-resolution.md)). Independently, a **non-empty `SIMARD_MEMORY_SOCKET` overrides the socket path verbatim**, bypassing state-root joining entirely. |

`--confidence` is a hint the caller cannot use to self-rate: confidence is
computed **server-side** by the [write-boundary gate](./distill-write-boundary-gate.md)
from provenance grounding, content, and concept — the client is not trusted.

### Behaviour

1. Parses the scalar flags. A missing `--concept`/`--content` or an unparseable
   flag is a **usage error (exit 2)** — nothing is sent.
2. Resolves the memory socket via `socket_path_for(state_root)` — a non-empty
   `SIMARD_MEMORY_SOCKET` is returned **verbatim** as the socket path, otherwise
   the socket is `<state_root>/memory.sock`.
3. Connects to the running daemon over the memory IPC socket and sends one
   `StoreFactGated` request. There is deliberately **no** direct-open fallback: a
   direct open would bypass the server-side gate, so if no daemon is reachable the
   tool exits **3** rather than writing un-gated.
4. The daemon applies the write-boundary gate (ground → score → quarantine →
   dedup → persist) and reports its disposition. A **stored** fact exits `0`; a
   **quarantined** fact exits `4` (a normal, expected gate decision, surfaced so a
   mis-grounding agent is diagnosable, not an error the pass should retry).
5. Prints a one-line human result and exits.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Fact **stored** — it cleared the gate and was persisted with provenance. |
| `2` | Usage error — missing/empty `--concept` or `--content`, a non-numeric `--confidence`, or an unknown flag. Nothing was sent. |
| `3` | No gated write path — the daemon socket is absent / the connection failed, **or** the gated write itself errored. The fact was **not** stored; the caller (or scheduler) treats the pass as skipped. There is no un-gated fallback. |
| `4` | The daemon **quarantined** the fact (ungrounded, empty content, or below the reliability threshold). Nothing was stored. |

Exit codes are stable so the recipe/agent and any wrapping tooling can branch on
outcome without scraping the message text.

### Examples

Write one lesson-learned fact with provenance:

```bash
simard memory remember \
  --concept lesson-learned \
  --content "CARGO_TARGET_DIR must point off the OOM-prone tmpfs for CI linker steps" \
  --source-episode-id ep_01H9Z...
```

Write a fact derived from two episodes, tagged to a distillation pass:

```bash
simard memory remember \
  --concept bug-pattern \
  --content "recipe-runner-rs E2BIG when a 50-episode batch is inlined on argv" \
  --source-episode-id ep_A --source-episode-id ep_B \
  --pass-id distill-2026-07-06T12:00:00Z-7f3a
```

Human output (fact stored — exit 0):

```text
[simard] memory remember: stored concept=bug-pattern confidence=0.90 node_id=sem_42
```

Human output (fact quarantined by the gate — exit 4):

```text
[simard] memory remember: quarantined concept=bug-pattern confidence=0.40 (below gate)
```

---

## `simard memory remember-procedure`

Write a single recurring procedure.

```text
Usage: simard memory remember-procedure
         --name <PROCEDURE_NAME>
         --step <TEXT> [--step <TEXT> ...]
         [--prerequisite <TEXT> ...]
         [--source-episode-id <ID> ...]
         [--pass-id <OPAQUE>]
         [state-root]
```

| Flag | Required | Meaning |
|------|----------|---------|
| `--name <NAME>` | yes | Stable procedure handle (e.g. `ci-fix:auto`). Upsert-by-name — reuse the same name across passes to reinforce rather than duplicate. |
| `--step <TEXT>` | yes (repeatable, ≥1) | One ordered step. Repeat the flag to add steps in order. |
| `--prerequisite <TEXT>` | no (repeatable) | A prerequisite for the procedure. |
| `--source-episode-id <ID>` | no (repeatable) | Episode `node_id`s the procedure was distilled from; the daemon writes **one `PROCEDURE_DERIVES_FROM` edge per id**. |
| `--pass-id <OPAQUE>` | no | Pass attribution, as above. |
| `state-root` (positional) | no | Explicit state root; same resolution as `remember`. |

Exit codes: `0` success (idempotent upsert-by-name), `2` usage error (missing
`--name` or no `--step`), `3` no reachable daemon **or** the write errored.
There is no `4` — a procedure is not gate-quarantined. Procedure writes are
**idempotent by name**, so a retried pass reinforces the existing procedure
instead of creating a duplicate.

### Example

```bash
simard memory remember-procedure \
  --name ci-fix:auto \
  --step "reproduce the failing job locally with the same runner image" \
  --step "bisect the offending step" \
  --step "apply the minimal fix and re-run the job" \
  --source-episode-id ep_C --source-episode-id ep_D
```

---

## Relationship to the daemon

`remember` / `remember-procedure` are **write** commands and therefore require a
running OODA daemon that holds the store and serves the memory socket — the same
socket the read-only `stats` / `dump` commands opportunistically use. Unlike
`import`, they are **not** a stopped-daemon operation: they are designed to be
called *by an agentic step the daemon itself scheduled*, while the daemon is up
and listening.

If the socket is absent (exit `3`), the caller does not fall back to writing the
on-disk store directly and does not fall back to printing an envelope — the
distillation scheduler simply treats the pass as skipped and retries next cycle.
This preserves the D4 invariant: facts are committed **only** through the live
write endpoint, never marshalled through a channel that must be parsed.

---

## Security notes

- The write-boundary **gate is server-side and authoritative**. This CLI cannot
  set a confidence the server honors, bypass quarantine, or supply a pre-scored
  fact: `--confidence` is accepted only as a hint the server **ignores** (it
  re-derives confidence from grounding + content + concept), and there is no flag
  to skip the gate.
- Fact content is treated as **opaque data**: it is never interpolated into a
  shell, a path, or a query. The `--pass-id` is opaque and never logged with the
  fact body.
- Only the **local** Unix socket is used; there is no network exposure. The
  socket is created `0600` inside a `0700` directory, and the IPC handler
  length-caps every field and enforces a maximum frame size. See
  [write-boundary gate → input validation](./distill-write-boundary-gate.md#input-validation-framing).
