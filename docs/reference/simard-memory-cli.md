---
title: Memory introspection CLI
description: Operator reference for `simard memory stats` / `dump` — the read-only introspection commands that report per-type cognitive-memory counts and sample rows from the library backend, lock-safely while the daemon holds the store (issue #2308) — and `simard memory import`, the guarded snapshot-restore command (issue #2550).
last_updated: 2026-06-20
owner: simard
doc_type: reference
related:
  - ./simard-cli.md
  - ./goal-board-prospective-reconcile.md
  - ./prospective-trigger-firing.md
  - ./cognitive-memory-episodic-recall.md
  - ./state-root-resolution.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ../memory.md
---

# Memory introspection CLI

> Shipped in issue [#2308](https://github.com/rysweet/Simard/issues/2308)
> follow-up (introspection CLI + episode/trigger population on the library
> backend). Companion read-side reference to
> [Goal-board prospective reconcile](./goal-board-prospective-reconcile.md).

`simard memory` is the operator surface for **inspecting** Simard's
six-type cognitive memory directly from the terminal. Before #2308 the
only window into the persistent store was the dashboard Memory tab; there
was no command-line way to confirm that episodes, triggers, facts, and
procedures are actually populating the live store. `simard memory stats`
and `simard memory dump` fill that gap.

Both `stats` and `dump` are **read-only** and **safe to run while the OODA daemon
holds the store** — they route through the daemon's memory socket when it
is up, and fall back to a direct open of the on-disk store when it is
down. Neither command spawns an engineer, mutates memory, or fires
triggers.

A third command, [`simard memory import`](#simard-memory-import) (issue #2550),
is the one **write** path on this surface: a guarded snapshot-restore used to
recover a corrupted/reset store. Unlike `stats`/`dump` it must be run with the
daemon **stopped**. It is not a free-form mutation command — it only ingests a
`cognitive_snapshot.json` and deduplicates by content.

---

## Why this exists

The de-fork to [`amplihack-memory-lib`](../architecture/cognitive-memory-library-adapter.md)
as the sole backend (issue #2308) made the on-disk store opaque to
operators: the native fork's ad-hoc inspection helpers were deleted, and
the library exposes its counts only through the `CognitiveMemoryOps`
trait. During validation of episode and trigger population there was no
first-class way to answer "how many episodes are actually stored?" or
"did the active goals create prospects?" without reading dashboard JSON.

`simard memory stats` is the canonical answer. It is the verification
tool referenced throughout the episode/trigger work: the acceptance
criterion "the stored episode count must be demonstrably greater than
zero" is checked with `simard memory stats`, not by inference.

---

## `simard memory stats`

Print the per-type counts for the cognitive-memory store and a small
number of sample rows per populated type.

```text
Usage: simard memory stats [state-root] [--json]
```

| Argument | Meaning |
|----------|---------|
| `[state-root]` | Optional explicit state root. Omit to resolve `$SIMARD_STATE_ROOT`, then `$HOME/.simard` (see [State-root resolution](./state-root-resolution.md)). |
| `--json` | Emit a single JSON object instead of the human table. Stable keys for scripting. |

### Output (human)

```text
$ simard memory stats
cognitive memory @ /home/azureuser/.simard/cognitive  (via daemon socket)

  TYPE          COUNT
  sensory           4
  working           7
  episodic         18
  semantic          5     (facts)
  procedural        5     (procedures)
  prospective       5     (triggers)
  ---------------------
  total            44

edges / connections:
  (edges: run with daemon stopped for graph stats)

samples (best-effort):
  facts:        CARGO_TARGET_DIR points the test harness at /tmp/simard-target
  facts:        the dashboard serves on port 8080 by default
  episodes:     cycle 412 — ran cargo test; 0 failures
  procedures:   ooda:consolidate-memory | triggers: working-memory pressure
```

The **edges / connections** section (issue
[#2331](https://github.com/rysweet/Simard/issues/2331)) reports how the
per-type *nodes* are wired together. It is **gated on the access tier**: the
daemon socket exposes no graph reader, so over `via daemon socket` the section
prints the note above and the real counts require a `direct open` (daemon
stopped, or `stats` pointed at an idle state root):

```text
$ simard memory stats        # daemon stopped → direct on-disk open

  ... per-type table ...

edges / connections:
  DERIVES_FROM                 12     (fact -> episode)
  PROCEDURE_DERIVES_FROM        3     (procedure -> episode)
  SIMILAR_TO                    0     (fact <-> fact)
  SUPERSEDES                    0     (deduped snapshot)
  facts with provenance:  4 / 5
  snapshot dedup:         1 distinct caller keys / 6 snapshot facts
```

| Line | Meaning |
|------|---------|
| `DERIVES_FROM` | `DERIVES_FROM` provenance edges (fact → source episode), summed over all facts (the read side of `store_fact_with_provenance`). |
| `PROCEDURE_DERIVES_FROM` | Provenance edges (procedure → source episode), summed over all procedures. |
| `SIMILAR_TO` | Fact↔fact similarity edges. Always `0` on the library backend — the pinned `amplihack-memory` rev exposes no public per-type edge reader. |
| `SUPERSEDES` | Caller-key dedup edges (new snapshot → archived prior). Always `0` for the same reason; the `snapshot dedup` line is the computed proxy. |
| `facts with provenance: X / Y` | `X` of `Y` facts carry at least one `DERIVES_FROM` edge. |
| `snapshot dedup: D / T` | Scoped to the `goal-board:snapshot` concept only: `T` snapshot facts (live + superseded revisions) collapsed onto `D` distinct caller keys. `T` well above `D` is the visible dedup signal. The per-goal `goal-store:record:{slug}` dedup family is **not** counted here. |

The section is read-only and **never fails the report**: if the direct-open
graph read errors, the table still prints and the section shows
`(edges: graph stats unavailable)`.

The **counts** are authoritative — they come from a single
`get_statistics()` call (see [Type → field mapping](#type-field-mapping)).
The **samples** are best-effort: they are fetched with the enumeration
methods the active access tier happens to support, and a populated type
can legitimately print no sample rows (the probe used to enumerate is a
keyword/CONTAINS query that needs a real term — see
[Sampling is best-effort](#sampling-is-best-effort)). Never infer a count
from the number of sample rows; read the count column.

> **No trigger sample rows.** The store has no read-only prospective
> enumerator: the only way to list prospects (`check_triggers`) *mutates*
> matches to `"triggered"` (fire-once). `stats` therefore reports the
> `prospective` (triggers) **count only** and never samples trigger rows —
> sampling them would consume live goal triggers. Row-level trigger
> inspection is [out of scope](#out-of-scope). To confirm triggers are
> *firing*, watch the OODA cycle log's `N triggers` figure (see
> [Triggers — confirming goal population](#triggers-confirming-goal-population)).

The banner names the resolved store path (`<state_root>/cognitive`) and
the **access tier** that served the read: `via daemon socket`,
`via in-process writer`, or `via direct open`. The tier is diagnostic — a
`direct open` tier while you expected the daemon to be running tells you
the daemon is down or rooted at a different state root.

### Output (`--json`)

```json
{
  "state_root": "/home/azureuser/.simard",
  "store_path": "/home/azureuser/.simard/cognitive",
  "access_tier": "daemon-socket",
  "counts": {
    "sensory": 4,
    "working": 7,
    "episodic": 18,
    "semantic": 5,
    "procedural": 5,
    "prospective": 5,
    "total": 44
  },
  "edges": {
    "derives_from": 0,
    "procedure_derives_from": 0,
    "similar_to": 0,
    "supersedes": 0
  },
  "provenance": {
    "facts_with_provenance": 0,
    "facts_total": 0
  },
  "snapshot_dedup": {
    "distinct_caller_keys": 0,
    "snapshot_facts": 0
  },
  "edges_note": "(edges: run with daemon stopped for graph stats)"
}
```

> The example is the **daemon-socket** tier (`access_tier: "daemon-socket"`),
> so every edge object is zeroed and `edges_note` is present — the socket has
> no graph reader. On a **direct open** the edge objects carry the real counts
> (e.g. `edges.derives_from: 12`, `snapshot_dedup.snapshot_facts: 6`) and
> `edges_note` is omitted.

The JSON keys are the raw `CognitiveStatistics` field stems
(`sensory`, `working`, `episodic`, `semantic`, `procedural`,
`prospective`) plus `total`. They never carry the friendly aliases
(`facts`/`procedures`/`triggers`) so scripts bind to a stable schema.

The `edges`, `provenance`, and `snapshot_dedup` objects (issue
[#2331](https://github.com/rysweet/Simard/issues/2331)) mirror the human
**edges / connections** section. Their keys are **always present** (zeroed
when the counts could not be computed) so scripts bind to a stable shape.
When the read could not be computed for this access tier — e.g. over the
daemon socket, which has no graph reader — a top-level **`edges_note`**
string is added and the edge objects stay zeroed; on a successful direct
open `edges_note` is **absent**. The keys map onto the `GraphStats` struct
(`src/memory_cognitive.rs`):

| JSON path | `GraphStats` field |
|-----------|--------------------|
| `edges.derives_from` | `derives_from_edges` |
| `edges.procedure_derives_from` | `procedure_derives_from_edges` |
| `edges.similar_to` | `similar_to_edges` (always `0` — no public reader at the pinned rev) |
| `edges.supersedes` | `supersedes_edges` (always `0` — no public reader at the pinned rev) |
| `provenance.facts_with_provenance` | `facts_with_provenance` |
| `provenance.facts_total` | `facts_total` |
| `snapshot_dedup.distinct_caller_keys` | `distinct_snapshot_caller_keys` |
| `snapshot_dedup.snapshot_facts` | `snapshot_facts_total` |

### Type → field mapping

The counts come from a single authoritative source —
`CognitiveMemoryOps::get_statistics()`, which returns the six-field
`CognitiveStatistics` struct (`src/memory_cognitive.rs`). The CLI labels
map one-to-one onto those fields:

| CLI label (alias) | `CognitiveStatistics` field | Memory type |
|-------------------|-----------------------------|-------------|
| `sensory` | `sensory_count` | Sensory |
| `working` | `working_count` | Working |
| `episodic` (episodes) | `episodic_count` | Episodic |
| `semantic` (facts) | `semantic_count` | Semantic |
| `procedural` (procedures) | `procedural_count` | Procedural |
| `prospective` (triggers) | `prospective_count` | Prospective |
| `total` | `CognitiveStatistics::total()` | sum of all six |

> **Why `get_statistics()` and not `search_facts("*")`?**
> A single `get_statistics()` call covers all six types from one
> authoritative source and is supported over every access tier (including
> the daemon socket). Counting via per-type search queries
> (`search_facts("*")`, etc.) is unreliable — it depends on wildcard
> semantics, per-query limits, and is not uniformly available over IPC.
> `stats` therefore reports counts exclusively from `get_statistics()`.

---

## `simard memory dump`

Print the counts (as `stats`) **plus** a larger set of sample rows per
type, for eyeballing what is actually in the store.

```text
Usage: simard memory dump [state-root] [--type=TYPE] [--limit=N] [--json]
```

| Argument | Meaning |
|----------|---------|
| `[state-root]` | As for `stats`. |
| `--type=TYPE` | Restrict the dump to one type: `facts`, `episodes`, `procedures`, `working`, `sensory`. (`triggers` is count-only — see below.) Default: all. |
| `--limit=N` | Maximum sample rows per type. Default `10`. |
| `--json` | Emit JSON: `{ counts, samples }`. |

`dump` is the heavier, human-oriented sibling of `stats`. Use it to
confirm *content*, not just counts — e.g. that the episodes carry real
cycle observations rather than placeholder text. Like `stats`, its counts
are authoritative and its sample rows are best-effort.

### Sampling over IPC — the supported-method constraint

The sample rows are fetched with whatever **read-only** enumeration
methods the **active access tier** supports. This matters because the
daemon-socket tier (`RemoteCognitiveMemory`) implements only a subset of
`CognitiveMemoryOps`, and neutral enumerators (the library's
`get_episodes`, `search_episodes_starting_with`) are reachable **only**
over the direct-open tier:

| Type | Best sampling method | Over daemon socket | Over direct open (daemon stopped) |
|------|----------------------|--------------------|-----------------------------------|
| facts | `search_facts(concept)` (scoped probe) | best-effort (probe must hit a concept) | best-effort |
| episodes | `get_episodes(limit)` (neutral) / `search_episodes_by_keywords` (keyword) | keyword-only, best-effort | neutral rows |
| procedures | `recall_procedure(probe, limit)` | best-effort (probe must hit a name) | best-effort |
| triggers (prospective) | **none** — no read-only enumerator | count only | count only |
| working | none — no trait enumerator | count only | count only |
| sensory | none — no trait enumerator | count only | count only |

When a neutral sampler is **not** available on the active bridge (for
example, `get_episodes` over the socket, which `RemoteCognitiveMemory`
does not expose), `dump` prints the count and notes that full rows need a
direct open. It never crashes and never silently drops the type:

```text
  episodes        18     (keyword samples only over IPC — run with the daemon stopped for neutral rows)
```

Running `dump` with the daemon stopped routes through the direct-open
tier, where the neutral library enumerators (`get_episodes`, …) are
available and the richer rows are printed.

> **Triggers are never row-sampled.** The store exposes no read-only way
> to list prospective memories. The only listing method,
> `check_triggers`, *mutates* every keyword-overlap match to `"triggered"`
> (fire-once) and would consume live goal triggers — so `dump` reports the
> `triggers` (prospective) **count only**, on every tier. A read-only
> prospective enumerator is [out of scope](#out-of-scope) for this change;
> until one exists, confirm triggers via the OODA cycle log's `N triggers`
> figure, not by listing rows.

### Sampling is best-effort

Sample rows are an eyeballing aid, **never** a count. Facts and procedures
are enumerated with CONTAINS-style probes (`search_facts(concept)`,
`recall_procedure(probe)`) that need a real query term, so a broad probe
can return **zero rows even when the type is populated**. Episodes have a
neutral enumerator (`get_episodes`) only over the direct-open tier. The
authoritative signal for "is this type populated?" is always the
`get_statistics()` count, not the presence of sample rows. This is the
reason `stats` reports counts from `get_statistics()` exclusively and
treats every sample row as advisory.

---

## Lock-safe read path

Both commands open the store through `open_reader_bridge(state_root)`
(`src/memory_ipc/launcher.rs`), the canonical read-only consumer entry
point. Its resolution ladder is what makes the commands safe to run while
the daemon owns the store:

```
open_reader_bridge(state_root)
  0. in-process writer Arc      → same-process callers (not the CLI)
  1. daemon socket present?     → RemoteCognitiveMemory::connect(
                                    <state_root>/memory.sock)         ← no lock contention
  2. otherwise                  → LibraryCognitiveMemory::open(state_root)
                                    (direct; creates an empty store if none exists)
```

- **Daemon up (tier 1).** The read is serviced by the daemon's IPC
  server over `<state_root>/memory.sock`. The CLI never opens the
  LadybugDB file directly, so there is **no lock contention** with the
  daemon's writer. This is the common case and the reason `stats` is safe
  to run at any time.
- **Daemon down (tier 2).** A direct `LibraryCognitiveMemory::open`
  serves the read. The library has no read-only constructor, so this is a
  writer handle used only for reads — acceptable because tier 1 already
  covers the daemon-up case.

### State-root and socket agreement

The CLI resolves its state root with `simard_state_root()`, which is the
**same** helper the daemon uses (`memory_ipc::default_state_root()`
delegates to it). Both therefore compute the same socket path
`<state_root>/memory.sock`, so `simard memory stats` with no arguments
always targets the running daemon's store. Pass an explicit `[state-root]`
only to inspect a probe-isolated sandbox (a `TempDir` run, a backup
restored elsewhere).

### Caveat — `stats` on a never-initialised root

Because tier 2 *creates* an empty store when the DB has never been
opened, running `simard memory stats` against a state root that has never
hosted a daemon prints **all-zeros** rather than erroring:

```text
$ SIMARD_STATE_ROOT=/tmp/fresh simard memory stats
cognitive memory @ /tmp/fresh/cognitive  (via direct open)

  TYPE          COUNT
  sensory           0
  working           0
  episodic          0
  semantic          0
  procedural        0
  prospective       0
  ---------------------
  total             0
```

This is non-corrupting (an empty store is created, never overwritten) and
is the documented behaviour of the tier-2 direct open. If you see
all-zeros where you expected data, check that the state root matches the
daemon's (`echo $SIMARD_STATE_ROOT`) before concluding memory is empty.

---

## Episodes — stored vs recalled

`simard memory stats` reports the count of episodes **stored** in the
library (`episodic_count`). This is distinct from the
"`0 episodes recalled`" line the OODA daemon logs each cycle, which counts
episodes **recalled for the current objective**:

```
[simard] OODA cycle: prepared context (5 facts, 1 triggers, 5 procedures, 0 episodes)
```

The two numbers measure different things and a populated store can
legitimately show `0 episodes` in a given cycle:

| Number | Source | Meaning |
|--------|--------|---------|
| `episodic_count` (in `stats`) | `get_statistics()` | total episodes persisted in the store |
| `… N episodes` (in the cycle log) | `search_episodes_by_keywords(tokenize(objective), 5)` | episodes whose content shares a keyword with **this cycle's objective**, minus self-session noise |

Preparation recall (`src/memory_consolidation/mod.rs`) tokenises the
objective and asks the library for up to five episodes matching those
keywords, then drops `source_label`-`session-…` self-echo. So `0 recalled`
with a non-zero `episodic_count` simply means **no stored episode shares a
keyword with the current objective** — a relevance outcome, not a storage
or wiring failure.

The verification rule that follows from this:

> Use `simard memory stats` to prove episodes are **persisting**
> (`episodic_count > 0`, and that it grows across daemon restarts). Use the
> cycle log line to observe per-objective **relevance**. Do not treat a
> `0 episodes recalled` cycle line as evidence that episodes are not being
> stored — confirm with `stats` first.

For the recall mechanism itself see
[Episodic keyword recall](./cognitive-memory-episodic-recall.md).

---

## Triggers — confirming goal population

A store with active goals shows a non-zero `prospective` count in `stats`,
and that count **grows over cycles** — it is the operator-visible proof
that active goals are being mirrored into prospective memory, not a
fixed-cardinality gauge:

```text
  prospective       7     (triggers)     ← grows each cycle; not "one per goal"
```

> **`prospective_count` counts every prospective node, regardless of
> status.** The library's statistics do not filter by status, and a
> trigger that fires is marked `"triggered"` (fire-once) rather than
> deleted. The per-cycle reconcile stores a fresh `pending` prospect for
> each still-active goal, so the *total* prospective node count climbs
> cycle over cycle even with a stable set of goals. Treat
> `prospective_count > 0` (and growing) as "triggers are populating" — do
> **not** expect it to equal the active-goal count after the first cycle.

The meaningful **per-cycle** trigger signal is the OODA cycle log's
`N triggers` figure, which is `PreparedContext.triggered_prospectives.len()`
— the prospects that actually matched this cycle's objective:

```
[simard] OODA cycle: prepared context (5 facts, 1 triggers, 5 procedures, 0 episodes)
                                                ^^^^^^^^^^ this is the live "fired" count
```

So the two-part operator check for triggers is:

> Use `simard memory stats` to confirm prospects are **populating**
> (`prospective` count `> 0` and growing). Use the cycle-log `N triggers`
> figure to confirm they are **firing** for active goals. `stats` does not
> list individual trigger rows (no read-only enumerator — see
> [Sampling over IPC](#sampling-over-ipc-the-supported-method-constraint)).

How those prospects come to exist (the per-cycle board-sourced reconcile)
and how they fire is documented in
[Goal-board prospective reconcile](./goal-board-prospective-reconcile.md)
and [Prospective-trigger firing](./prospective-trigger-firing.md).

---

## `simard memory import`

Restore a cognitive-memory snapshot back into the store. This is the operator
recovery path introduced by issue #2550 after the 2026-07-04 corrupt-WAL
data-loss incident, where a periodic `cognitive_snapshot.json` existed but there
was **no command** to load it back.

```text
Usage: simard memory import <snapshot.json> [state-root] [--json]
```

| Argument | Meaning |
|----------|---------|
| `<snapshot.json>` | Path to a backup's `cognitive_snapshot.json` (or any snapshot in the same envelope format). Required. |
| `[state-root]` | As for `stats`. Resolves `$SIMARD_STATE_ROOT`, then `$HOME/.simard`. |
| `--json` | Emit a single JSON result object (`{ imported, new, deduplicated, store }`) instead of the human line. |

### Behaviour

- **Restore-only, not free-form mutation.** `import` ingests the snapshot's
  records (facts, procedures, and — for snapshots written post-#2550 — episodes
  and prospective/triggers). Provenance/similarity **edges** are not serialized:
  they are reconstructable and their endpoints change across a content-dedup
  restore, so only the durable memory *nodes* are carried. `import` never accepts
  ad-hoc content and has no `set`/`clear`/`forget` surface.
- **Idempotent — dedup by content.** Re-running `import`, or importing onto a
  store that already holds some of the records, deduplicates by content, so it is
  safe to run more than once and safe to run onto a partially-populated store.
  The output reports how many items were new versus deduplicated.
- **Run with the daemon stopped.** `import` writes to the store, so unlike
  `stats`/`dump` it does **not** route through the read socket. Stop
  `simard-ooda` first so nothing writes concurrently.

```text
$ sudo systemctl stop simard-ooda
$ simard memory import ~/.simard/backups/20260704_001500/cognitive_snapshot.json
imported 40488 items (40488 new, 0 deduplicated) -> /home/azureuser/.simard/cognitive
$ sudo systemctl start simard-ooda
```

The daemon also performs an **automatic** import on startup: if the live store is
empty and a newer non-empty snapshot exists, it restores from the newest good
snapshot and logs a `store was empty — auto-restored <n> memories from …
cognitive_snapshot.json` line, so a corruption-reset self-heals without operator
action. See the
[Cognitive-Memory WAL Recovery Runbook](../operations/cognitive-memory-wal-recovery-runbook.md)
for the full incident-driven procedure.

---

## Out of scope

Deliberately excluded from this change:

- **Free-form mutation commands.** `simard memory` has no `set`, `clear`,
  `forget`, or `compact` subcommand. `stats`/`dump` are read-only; the only
  write path is [`import`](#simard-memory-import), a guarded snapshot-restore
  that ingests a `cognitive_snapshot.json` and deduplicates by content. Ordinary
  memory is written only by the daemon's OODA loop and consolidation.
- **Sampling prospective (trigger) rows in `stats`/`dump`.** Issue #2550 added a
  read-only, side-effect-free prospective enumerator to the library
  (`get_all_prospective`, used by the verified backup to capture triggers), but
  wiring it into the `stats`/`dump` sample tables is out of scope here: those
  commands still report the prospective **count only** and never sample trigger
  rows. `check_triggers` remains unsuitable for sampling — it mutates status.
- **Working / sensory row sampling.** Neither type has a trait enumerator;
  both are count-only.
- **Dashboard parity.** The Memory tab's graph view and full-text search
  are unchanged; the CLI is a terminal-friendly complement, not a
  replacement.

---

## Exit codes

| Situation | Exit |
|-----------|------|
| Counts printed (including all-zeros on a fresh root) | `0` |
| `import` completes (items restored, including 0 on an empty snapshot) | `0` |
| Unparseable arguments (unknown flag, bad `--limit`, missing `import` path) | non-zero, usage to stderr |
| `import` snapshot file missing or not a valid snapshot envelope | non-zero, error to stderr |
| Bridge open fails on every tier | non-zero, error to stderr |

A successful read of an empty store is **not** an error — the all-zeros
table is valid output. Only an unparseable invocation or a genuine
bridge-open failure exits non-zero.

---

## Code location

| Item | File |
|------|------|
| `memory` subcommand dispatch | `src/operator_cli/memory.rs` |
| Subcommand registration | `src/operator_cli/mod.rs` |
| Snapshot import / restore (`import_memory_snapshot`, `restore_snapshot`) | `src/remote_transfer/mod.rs`, `src/memory_snapshot.rs` |
| Startup auto-restore (empty store + newer snapshot) | `src/operator_commands_ooda/daemon/` |
| Read bridge (`open_reader_bridge`) | `src/memory_ipc/launcher.rs` |
| `CognitiveStatistics` (`get_statistics`) | `src/memory_cognitive.rs` |
| `GraphStats` (edges / dedup, issue #2331) | `src/memory_cognitive.rs` |
| `graph_stats` computation (issue #2331) | `src/cognitive_memory/library_adapter.rs` |
| State-root resolution | `src/state_root.rs` |
| Library backend | `src/cognitive_memory/library_adapter.rs` |

---

## Testing

| Test | Coverage |
|------|----------|
| `stats` over direct open reports `get_statistics()` counts | Seed a `TempDir` store with one of each type; assert each printed count matches the struct field |
| `stats` is lock-safe while a writer holds the store | Open a writer, then run `stats` against the same root; assert it succeeds via the socket/in-process tier without erroring on a lock |
| `stats` on a never-initialised root prints all-zeros, exit 0 | Run against a fresh `TempDir`; assert all-zeros and zero exit (tier-2 create) |
| `stats` reports counts only and never lists trigger rows | Assert `stats` output contains the `prospective` count but no per-trigger row, and that running it does not change `prospective_count` (no `check_triggers` mutation) |
| `--json` emits the stable count schema | Parse the JSON; assert the six field stems plus `total` |
| human output carries the edges / connections section (issue #2331) | Assert the rendered table contains the `edges / connections`, `DERIVES_FROM`, and `facts with provenance:` labels |
| `--json` carries the `edges` / `provenance` / `snapshot_dedup` objects (issue #2331) | Parse the JSON; assert the edge keys, `provenance.facts_total`, and `snapshot_dedup.distinct_caller_keys` are present, and that direct-open output omits `edges_note` |
| direct open counts `DERIVES_FROM` / provenance edges (issue #2331) | Seed a fact with provenance; assert `edges.derives_from >= 1` and the human line `facts with provenance:  1 / …` |
| daemon socket notes edges are unavailable (issue #2331) | Force the daemon-socket tier; assert the human section prints the `(edges: run with daemon stopped …)` note and the JSON carries `edges_note` with edge keys still present (zeroed) |
| `dump` notes when neutral episode rows need direct open | Force the daemon-socket tier; assert the episode count prints with the "keyword samples only over IPC" note and no crash |
| `dump --type=triggers` is rejected or count-only | Assert triggers cannot be row-sampled (count-only), consistent with the no-mutation guarantee |
| Argument parsing rejects unknown flags / bad `--limit` | Assert non-zero exit and a usage message |
| `import` round-trips a snapshot | Export a snapshot from a seeded store, `import` it into a fresh `TempDir` store, and assert the counts match |
| `import` is idempotent (dedup by content) | Import the same snapshot twice; assert the second run reports items as deduplicated and the store count does not double |
| `import` rejects a missing / malformed snapshot file | Assert non-zero exit and an error message, with the store left unchanged |

---

## Related reading

- [Simard CLI reference](./simard-cli.md) — the full operator command
  tree, including the `memory` subcommand.
- [Goal-board prospective reconcile](./goal-board-prospective-reconcile.md)
  — how active goals become prospects so the `prospective` count is
  non-zero.
- [Prospective-trigger firing](./prospective-trigger-firing.md) — how the
  stored triggers fire during preparation.
- [Episodic keyword recall](./cognitive-memory-episodic-recall.md) — the
  recall mechanism behind the cycle-log `… episodes` number.
- [State-root resolution](./state-root-resolution.md) — how
  `[state-root]`, `$SIMARD_STATE_ROOT`, and `$HOME/.simard` are resolved.
- [Library-backed Cognitive Memory](../architecture/cognitive-memory-library-adapter.md)
  — the `amplihack-memory-lib` backend the commands read from.
- [Cognitive-Memory WAL Recovery Runbook](../operations/cognitive-memory-wal-recovery-runbook.md)
  — when and how to use `simard memory import` to recover a corrupted/reset store.
- [Verified Backups of the Live Cognitive Store](../operations/verified-backups.md)
  — where the `cognitive_snapshot.json` files `import` consumes come from.
- [Memory architecture](../memory.md) — the six memory types overview.
