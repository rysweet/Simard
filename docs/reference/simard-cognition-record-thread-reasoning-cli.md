---
title: "Reference: simard cognition record-thread-reasoning (typed thread-reasoning tool)"
description: >
  The zero-privilege CLI tool every cognitive-thread recipe calls as its ACT step to
  record exactly one typed, validated ThreadReasoningRecord carrying a natural-language
  reasoning_summary plus a small closed set of per-thread domain fields. Covers the
  ThreadReasoningRecord schema, the ThreadName / ThreadDomain closed enums, the fail-CLOSED
  R1–R7 read matrix (read_verified_thread_reasoning), the freshness/anti-replay model,
  configuration, security, and worked examples. This tool replaces the boolean
  "{recipe}: ok" collapse: the thread's rail surfaces the record's reasoning_summary into
  the daemon log instead of a meaningless success string.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./ooda-record-decision-cli.md
  - ./ooda-record-orient-decide-cli.md
  - ./cognitive-threads-catalog.md
  - ./cognitive-thread-scheduling.md
  - ../architecture/metacognitive-model.md
  - ../architecture/reflective-cognitive-threads.md
  - ../howto/read-cognitive-thread-reasoning.md
  - ../concepts/agentic-recipes-first-principle.md
  - ../index.md
---

# Reference: `simard cognition record-thread-reasoning` (typed thread-reasoning tool)

CLI: `src/operator_cli/cognition.rs` (`dispatch_record_thread_reasoning`)
Record type + reader: `src/ooda_brain/thread_reasoning_record.rs`
(`ThreadReasoningRecord`, `ThreadName`, `ThreadDomain`, `THREAD_REASONING_SCHEMA`,
`MAX_AGE_SECS`, `sanitize_reasoning_summary`, `read_verified_thread_reasoning`)
Reader call site: `src/cognitive_threads/recipe_rail.rs` (`run_reflective_thread`)

`simard cognition record-thread-reasoning` is the **tool every cognitive-thread
recipe calls** to record its per-invocation reasoning. It is the ACT step that all
thirteen threads share. Each recipe reasons about its own domain (salience,
metacognition, reflection, prospection, operator-model, analogy, narrative,
values-deliberation, consolidation, creative_ideas, engineer_log_analysis,
interoception, maintenance) and then calls this tool **exactly once** to write a
single typed [`ThreadReasoningRecord`](#the-threadreasoningrecord-schema). The thin
Rust rail reads that record fail-closed and puts its natural-language
`reasoning_summary` into `ThreadOutcome.summary`.

!!! info "Normative contracts (this spec is the source of truth)"
    Three strings are fixed by this document and the implementation MUST match them
    exactly (they are not merely illustrative):

    1. **Record path** — `state_root/cognitive_threads/reasoning/<thread_name>.json`
       (one file per thread; `<thread_name>` is the [`ThreadName`](#the-threadname-closed-enum) variant).
    2. **Shared reflective domain tag** — the eight reflective threads without a
       specialized domain use the `ThreadDomain::notes` variant (tag `"notes"`).
    3. **Failure log format** — `cognitive-thread: <thread>: FAILED — R{n} <reason>`,
       with the per-check tails pinned in [Canonical failure log format](#canonical-failure-log-format).

!!! success "This is what killed the boolean `ok`"
    Before this tool existed, a recipe that exited `0` produced the log line
    `cognitive-thread: <recipe>: ok` — the recipe's actual reasoning was
    discarded. Now the daemon log carries the thread's real reasoning, e.g.
    `cognitive-thread: salience: prioritising goal #4970 over #4812 because a
    release-blocking regression outranks the docs polish`. See
    [The metacognitive model](../architecture/metacognitive-model.md).

## What the tool does (and does not do)

The tool holds **zero privilege**. Its sole side effect is writing one JSON record
file to the `--record-path` the rail supplied. It records **reasoning only**:

- It does **not** spawn engineers, mutate refs, propose goals, prune files, or
  write to memory. Those effects (where a thread has them) are performed by the
  thread's *other* tool calls inside the same recipe — this verb is purely the
  reasoning-handoff channel.
- It does **not** call Python, kuzu, the network, or a memory socket, and holds no
  tokens.
- It never scrapes stdout. The recipe's prose is irrelevant; only the record it
  writes is read back.

Separating *recording the reasoning* (this tool) from *reading it* (the rail) is
what lets Rust stay a thin rail while judgment lives entirely in the recipe.

## Usage

```text
simard cognition record-thread-reasoning \
  --thread <THREAD_NAME> \
  --reasoning-summary "<1–3 sentence domain reasoning>" \
  --domain <DOMAIN_TAG> \
  --record-path <ABSOLUTE_PATH> \
  --written-at-epoch <UNIX_SECONDS> \
  [ per-domain fields — see below ] \
  [--reasoning-summary-path <FILE>]
```

On success the tool validates every field, then writes the record **atomically**
(temp file + rename) with owner-only `0o600` permissions and prints nothing to
stdout. On **any** validation failure it writes **no file** (validate-all-then-write-once)
and exits non-zero with a diagnostic on stderr.

### Core arguments

| Flag | Required | Description |
|---|---|---|
| `--thread` | yes | One of the closed [`ThreadName`](#the-threadname-closed-enum) variants. Matched case-insensitively; anything else is rejected. Embedded in the record and re-verified by the reader (R6 identity). |
| `--reasoning-summary` | yes* | The natural-language domain reasoning, 1–3 sentences. Passed through the shared [`sanitize_reasoning_summary`](#reasoning_summary-validation-r5) chokepoint. |
| `--reasoning-summary-path` | no | Read `reasoning_summary` from a file instead of argv (for longer text). **Absolute**, no `..`, read under a 64 KiB cap. Mutually exclusive with `--reasoning-summary`. |
| `--domain` | yes | The domain tag selecting one [`ThreadDomain`](#the-threaddomain-closed-enum) variant. Must match the `--thread`'s expected domain; a mismatch or unknown tag fails R4. |
| `--record-path` | yes | **Absolute** path the rail supplied via the recipe's `-c record_path` context var. Must not contain `..`; hardened via `harden_path`. |
| `--written-at-epoch` | yes | Unix seconds the recipe stamps at write time. Defense-in-depth freshness check (R7). |

\* Exactly one of `--reasoning-summary` / `--reasoning-summary-path` must be
supplied and resolve to non-empty text after sanitize. Unknown or duplicate flags
are rejected (`reject_extra_args`); the tool never silently ignores an argument.

### Per-domain fields (closed set)

Each `--thread` maps to exactly one `--domain` variant. Domain fields are for
record consumers, tests, and audit — **only `reasoning_summary` reaches the daemon
log line**. List fields are bounded twice (a raw read cap, then a per-domain
element cap) and each element is re-sanitized and capped at 256 bytes.

| `--thread` | `--domain` | Extra flags | Bounds |
|---|---|---|---|
| `salience` | `salience` | `--top-signal <S>` (repeatable), `--priority <f32>` | ≤5 signals; `priority` finite, clamped `[0.0, 1.0]` |
| `interoception` | `interoception` | `--probe <S>` (repeatable), `--breach <bool>` | ≤8 probes |
| `maintenance` | `maintenance` | `--candidate <S>` (repeatable), `--freed-bytes <u64>` | ≤16 candidates; `freed_bytes` finite |
| `creative_ideas` | `creative_ideas` | `--ideas-considered <u32>`, `--kept-after-dedup <u32>` | `kept_after_dedup ≤ ideas_considered` |
| `engineer_log_analysis` | `engineer_log_analysis` | `--signature <S>` (repeatable), `--novel <bool>` | ≤16 signatures |
| `metacognition`, `reflection`, `prospection`, `operator_model`, `analogy`, `narrative`, `values_deliberation`, `consolidation` | `notes` | `--note <S>` (repeatable) | ≤5 notes |

Large list payloads ride files, not argv: any repeatable field also accepts a
`--<field>-path <FILE>` form (absolute, `..`-free, 64 KiB cap) that supplies one
element per line, mutually exclusive with the inline form. This keeps `execve` off
the `E2BIG` ceiling.

## The `ThreadReasoningRecord` schema

Serialized as JSON, owner-only `0o600`, one file per thread per invocation:

```jsonc
{
  "schema": "thread-reasoning/v1",   // THREAD_REASONING_SCHEMA pin (R3)
  "thread": "salience",              // closed ThreadName; R6 identity
  "reasoning_summary": "prioritising goal #4970 over #4812 because a release-blocking regression outranks docs polish",
  "written_at_epoch": 1793558400,    // recipe wall-clock stamp; R7 defense-in-depth
  "domain": {                        // internally-tagged closed ThreadDomain enum
    "kind": "salience",
    "top_signals": ["regression:#4970", "docs:#4812"],
    "priority": 0.92
  }
}
```

Field contract:

- **`schema`** — pinned `"thread-reasoning/v1"`. A mismatch fails R3.
- **`thread`** — the [`ThreadName`](#the-threadname-closed-enum) that wrote it. The
  reader fails closed unless it equals the invoking thread (R6).
- **`reasoning_summary`** — the required natural-language reasoning; the only field
  surfaced to `ThreadOutcome.summary` and the daemon log.
- **`written_at_epoch`** — Unix seconds; part of the freshness gate (R7).
- **`domain`** — an internally-tagged (`"kind"`) closed enum. An unknown tag or a
  tag that does not match the record's `thread` fails deserialization (R4).

### The `ThreadName` closed enum

`salience`, `metacognition`, `reflection`, `prospection`, `operator_model`,
`analogy`, `narrative`, `values_deliberation`, `consolidation`, `creative_ideas`,
`engineer_log_analysis`, `interoception`, `maintenance` — the full roster of
thirteen threads. Any other value is rejected by both the writer and the reader.

### The `ThreadDomain` closed enum

`salience`, `interoception`, `maintenance`, `creative_ideas`,
`engineer_log_analysis`, and the shared `notes` bucket (used by the eight
reflective threads without a specialized domain). Serde is
`#[serde(tag = "kind")]` internally-tagged; the writer and reader share the single
type so they cannot drift.

!!! warning "Unknown-key closure across `flatten`"
    `#[serde(deny_unknown_fields)]` does **not** propagate across a
    `#[serde(flatten)]` boundary. The reader therefore performs an explicit
    unknown-top-level-key check in addition to `deny_unknown_fields`, so a crafted
    extra field cannot slip past R4.

## `reasoning_summary` validation (R5)

The single shared chokepoint `sanitize_reasoning_summary` is called **identically**
by the writer (reject ⇒ no file written) and the reader (reject ⇒ R5 `Err`):

- Non-empty after sanitize; `≥ 8` graphemes; `≤ 600` bytes.
- C0/C1 control characters and ANSI escapes stripped (`sanitize_value`).
- Secrets scrubbed via `secret_scrub`; untrusted spans wrapped via
  `fence_untrusted` before any value reaches a log line.
- Empty-after-sanitize ⇒ treated as absent ⇒ fail-closed.

A single-line, control-stripped summary cannot forge a second record or fake an
`ok` in the daemon log.

## The fail-CLOSED read matrix (R1–R7)

`read_verified_thread_reasoning(path, expected_thread, invoke_start)` returns
`SimardResult<ThreadReasoningRecord>`. Every failure mode is an `Err`, which the
rail maps to a **failed tick** — never a silent success.

| # | Check | Fails closed when |
|---|---|---|
| R1 | File present & readable | The record is absent or unreadable at `path`. |
| R2 | Well-formed JSON | The bytes are not valid JSON (a torn/partial write is impossible — `persist_json` writes temp+rename). |
| R3 | Schema pin | `schema != "thread-reasoning/v1"`. |
| R4 | Closed-type parse | Unknown `ThreadName`, unknown/mismatched `ThreadDomain` tag, or any unknown top-level key. |
| R5 | Summary valid | `reasoning_summary` empty/too-short/too-long/control-only after `sanitize_reasoning_summary`. |
| R6 | Identity binding | `record.thread != expected_thread` (the thread the rail invoked). |
| R7 | Freshness / anti-replay | File `mtime < invoke_start`, **or** `now − mtime > MAX_AGE_SECS`, **or** `|now_epoch − written_at_epoch| > MAX_AGE_SECS`. |

### Freshness / anti-replay model

A cognitive-thread record has no `goal_id`/`cycle_number` (unlike the OODA
readers), so anti-replay is **time-based and path-based**, in three parts:

1. **Per-invocation unique path + pre-truncate.** The rail derives
   `state_root/cognitive_threads/reasoning/<thread_name>.json` and **deletes any
   pre-existing file at that path immediately before spawning the recipe**. A
   leftover record from a prior run can never be read as current. This is the
   primary anti-replay guarantee.
2. **R6 identity = `thread_name`.** The only stable identity a thread has.
3. **R7 freshness = mtime window + embedded epoch.** The rail captures
   `invoke_start: SystemTime` before spawn and requires `mtime ≥ invoke_start` and
   `now − mtime ≤ MAX_AGE_SECS`; the embedded `written_at_epoch` is checked as
   defense-in-depth against mtime spoofing.

`MAX_AGE_SECS = 300` (5 minutes) — a constant in
`src/ooda_brain/thread_reasoning_record.rs`. Threads are not latency-bound; 5
minutes tolerates recipe-runner spin-up while still rejecting any stale artifact.

## How the rail wires it in

`InvokeResult` stays a two-state `Ran`/`Failed` enum (it is **not** widened). The
record read happens in the rail **around** `into_outcome`, in `run_reflective_thread`:

1. **Before spawn:** compute `record_path`, delete any stale file, capture
   `invoke_start`, pass `record_path` to the recipe as `-c record_path=<abs>`.
2. **On `InvokeResult::Ran`:** call
   `read_verified_thread_reasoning(record_path, thread_name, invoke_start)`.
   - `Ok(rec)` ⇒ `ThreadOutcome::ok(rec.reasoning_summary, duration)`.
   - `Err(e)` ⇒ `ThreadOutcome::failed(...)`. **No `unwrap_or`, no stdout
     fallback.** A recipe that "ran" but wrote no valid record is a FAILURE, not a
     silent `"<recipe>: ok"`.
3. **On `InvokeResult::Failed`:** unchanged failure path.

The literal `ThreadOutcome::ok(format!("{recipe_name}: ok"), duration)` is deleted;
a definition-of-done grep gate keeps it deleted:

```console
$ grep -rn '"{recipe}: ok"\|{recipe_name}: ok' src/cognitive_threads/recipe_rail.rs
# (returns nothing)
```

### Canonical failure log format

When the read matrix returns `Err`, the rail logs a single deterministic line so an
operator can tell exactly which check failed. This is the **normative** format the
implementation emits (and the [how-to troubleshooting table](../howto/read-cognitive-thread-reasoning.md#troubleshooting)
mirrors it verbatim):

```text
cognitive-thread: <thread>: FAILED — R{n} <reason>
```

| Check | Canonical log tail |
|---|---|
| R1 | `R1 no record at expected path` |
| R2 | `R2 malformed JSON` |
| R3 | `R3 schema mismatch` |
| R4 | `R4 closed-type parse (unknown ThreadName/ThreadDomain or extra key)` |
| R5 | `R5 reasoning_summary invalid (empty/too-short/too-long after sanitize)` |
| R6 | `R6 identity mismatch (record.thread != invoked thread)` |
| R7 | `R7 freshness/anti-replay (stale or replayed record)` |

## Configuration

| Knob | Where | Effect |
|---|---|---|
| `MAX_AGE_SECS = 300` | `src/ooda_brain/thread_reasoning_record.rs` | Freshness window for R7. |
| `THREAD_REASONING_SCHEMA = "thread-reasoning/v1"` | same module | Schema pin for R3. Bump only with a matching reader change. |
| record dir | `state_root/cognitive_threads/reasoning/` | Created `0o700`; one `<thread>.json` file per thread, pre-truncated each invocation. Ephemeral (TTL ≤ 300 s); no history/rotation. |
| `SIMARD_COGNITIVE_THREADS_ENABLED` | env (master gate) | Default-ON; a falsy token disables all threads. |
| `SIMARD_THREAD_<NAME>_ENABLED` | env (per-thread) | Default-ON; a falsy token disables one thread. |

The gate stack, the security fence, and the scheduler wiring are unchanged by this
tool — it only adds the reasoning-handoff channel.

## Security summary

- **SR-TR-1 — zero privilege.** One write to the hardened `--record-path`; no env
  mutation, network, memory socket, or spawn.
- **SR-TR-2 — owner-only.** Records `0o600` (parents `0o700`) via `persist_json` +
  `harden_path`; never a hand-rolled `File::create`.
- **SR-TR-3 — no trust elevation from contents.** Identity (R6) and freshness (R7)
  are enforced out-of-band by the rail's own clock and chosen path;
  `written_at_epoch` is defense-in-depth only.
- **SR-TR-5 — single validation chokepoint.** One `sanitize_reasoning_summary`
  called identically by writer and reader; a parity test proves no drift.
- **SR-TR-7 — closed-type validation.** Schema pin, closed `ThreadName` (13
  variants), internally-tagged closed `ThreadDomain`, finite/clamped numerics, and
  an explicit unknown-top-level-key check that closes the `flatten` gap.
- **SR-TR-8 — path safety.** All paths absolute and `..`-free via `harden_path`;
  large payloads ride files under a 64 KiB cap (prevents traversal and `E2BIG`).
- **SR-TR-11 — secret scrub.** `secret_scrub` runs before persist and before any
  log line, so tokens or `AMPLIHACK_AGENT_BINARY` never leak into the record or the
  daemon log.
- **SR-TR-12 — ephemeral.** Single file per thread, pre-truncated each invocation,
  TTL ≤ 300 s; no archive.

## Examples

Salience thread recording a prioritisation:

```bash
simard cognition record-thread-reasoning \
  --thread salience \
  --domain salience \
  --reasoning-summary "prioritising goal #4970 over #4812 because a release-blocking regression outranks docs polish" \
  --top-signal "regression:#4970" \
  --top-signal "docs:#4812" \
  --priority 0.92 \
  --written-at-epoch 1793558400 \
  --record-path /home/simard/.simard/cognitive_threads/reasoning/salience.json
```

Interoception (deterministic sensor, still emits genuine NL reasoning):

```bash
simard cognition record-thread-reasoning \
  --thread interoception \
  --domain interoception \
  --reasoning-summary "disk at 91% breaches the 85% guard; filed a capacity health-goal" \
  --probe "disk_free_ratio=0.09" \
  --breach true \
  --written-at-epoch 1793558400 \
  --record-path /home/simard/.simard/cognitive_threads/reasoning/interoception.json
```

Reflection using the shared `notes` domain, with a long summary via file:

```bash
simard cognition record-thread-reasoning \
  --thread reflection \
  --domain notes \
  --reasoning-summary-path /run/simard/reflect-summary.txt \
  --note "engineer #4801 stalled on flaky CI, not code" \
  --note "add a retry to the coverage-comment step" \
  --written-at-epoch 1793558400 \
  --record-path /home/simard/.simard/cognitive_threads/reasoning/reflection.json
```

## See also

- [The metacognitive model](../architecture/metacognitive-model.md) — the whole
  cognitive architecture (diagrams + per-thread text) this record feeds.
- [Reflective cognitive threads act via tools](../architecture/reflective-cognitive-threads.md) —
  the tools-not-JSON exemplar this design extends.
- [`simard ooda record-decision` CLI](./ooda-record-decision-cli.md) — the sibling
  typed-record tool on the OODA decision path.
- [Cognitive-threads catalog](./cognitive-threads-catalog.md) — per-thread cadence,
  gates, and effects.
- [Read a thread's reasoning summary](../howto/read-cognitive-thread-reasoning.md) —
  how the summary reaches the daemon log and how to inspect a record.
