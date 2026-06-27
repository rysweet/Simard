---
title: Goal decomposition & the goal graph (parent → child edges)
description: How Simard breaks a large goal into 2–6 bounded sub-goals and records the parent↔child structure as typed edges in the cognitive-memory graph, so the OODA brain and the operator can reason over goal structure, gate sub-goals on each other, and roll parent progress up from children. Documents the data model (parent_goal_id + GoalNode), the typed relationship-fact edge model (decomposes_into / depends_on), the decompose_goal driver and its prompt/recipe assets, the OODA loop-awareness trigger, the simard goal decompose CLI, the roll-up rule, and how to query an edge back.
last_updated: 2026-06-27
owner: simard
doc_type: reference
status: shipped (first increment) — issue #2405
related:
  - ../concepts/goal-board-persistence.md
  - ../concepts/ooda-loop-self-detection.md
  - ./goal-board-api.md
  - ./cognitive-memory-provenance.md
  - ./goal-target-repo-routing.md
  - ./maximum-safe-parallelism.md
  - ./goal-coverage-allocation.md
  - ./simard-cli.md
  - ../howto/decompose-a-large-goal.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# Goal decomposition & the goal graph (parent → child edges)

> **Status: shipped — first increment (issue
> [#2405](https://github.com/rysweet/Simard/issues/2405)).**
>
> This reference describes Simard's first-class goal-decomposition
> capability: the data model, the typed-edge model, the `decompose_goal`
> driver, the OODA trigger, and the `simard goal decompose` operator command.
> The **first increment** ships the durable core — `parent_goal_id` on
> `ActiveGoal`, the `GoalNode` graph projection, the `decompose_goal` driver,
> the queryable `goal-edge:*` relationship facts, parent-progress roll-up, the
> `goal_decomposition` prompt asset, and the `simard goal decompose` verb. A
> small set of explicitly-scoped behaviours are tracked as
> [follow-ups](#guarantees-and-non-guarantees). It is the companion of the
> [goal-board persistence](../concepts/goal-board-persistence.md) and
> [goal-board API](./goal-board-api.md) docs. The parent↔child edges are
> **real and queryable** — a round-tripped edge is the acceptance bar, not a
> stub.

## The problem this solves

Before this capability the goal board was a **flat** list. An
[`ActiveGoal`](./goal-board-api.md) (`src/goal_curation/types.rs`) carried
`id`, `description`, `priority`, `status`, `assigned_to`,
`current_activity`, and `wip_refs` — but **no parent, no children, and no
edges**. The whole [`GoalBoard`](../concepts/goal-board-persistence.md) was
persisted as a single `goal-board:snapshot` fact in cognitive memory.

Simard already *described* decomposition in prose. The loop-awareness
prompts ([`goal_session_objective.md`](../concepts/ooda-loop-self-detection.md),
`ooda_decide.md`, `goal_curator_system.md`) told the brain to break an
unbounded goal into "concrete, completable sub-goals" when it had looped
without progress. But that decomposition was **prompt-only**: it emitted
*sibling* goals onto the same flat board with **no recorded relationship**.
Once the parent was replaced by its slices, nothing could answer:

- Which goals are children of this large goal?
- Which sub-goal unblocks which (ordering / dependencies)?
- What is the parent's progress as a roll-up of its children?

This is the same gap the
[provenance work (`DERIVES_FROM` edges)](./cognitive-memory-provenance.md)
closed for distilled facts: a free-text `source_id` is a string, not a
traversable edge. Goal decomposition closes it for **goals** — it turns the
flat goal board into a connected **goal graph** the OODA brain and the
operator can reason over.

## What the capability does

`goal_curation::decompose_goal` takes **one** large goal and emits **2–6
bounded, independently-verifiable sub-goals**, each with its own
done-criterion, then writes them as **child nodes with typed edges back to
the parent**. It is invoked two ways:

- **Autonomously**, by the OODA brain, when it decides a goal is too big or
  has looped without progress (see [OODA integration](#ooda-integration)).
- **Manually**, by the operator, via
  [`simard goal decompose <goal_id>`](#operator-cli).

Both paths write through the same cognitive-memory writer bridge that backs
`goal add` / `goal remove`, so decomposition is serialized by the daemon
when one is running (see [goal-board persistence](../concepts/goal-board-persistence.md)).

## Data model

Goals gain **graph identity** through two additions. Both preserve serde
back-compat — new fields are `#[serde(default)]` and (where `Option`)
`skip_serializing_if = "Option::is_none"`, so pre-#2405 snapshots and
`goal_records.json` files deserialize unchanged and re-serialize
byte-identically when the new fields are unset.

### Parent linkage on `ActiveGoal`

```rust
pub struct ActiveGoal {
    // … existing fields (id, description, priority, status, …) …

    /// Id of the goal this sub-goal decomposes from. `None` for a
    /// top-level goal that was never produced by decomposition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_goal_id: Option<String>,
}
```

`parent_goal_id` is the cheap, always-present back-reference. It lets any
consumer that already holds the board (the dashboard, the engineer loop,
`goal list`) group children under a parent **without** a graph query.

A builder keeps construction ergonomic and back-compat:

```rust
let child = ActiveGoal::new(id, description, priority)
    .with_parent(Some(parent_id));
```

`with_parent(Option<String>)` sets `parent_goal_id` (pass `None` to clear it);
goals constructed without it stay top-level (`parent_goal_id == None`).

### `GoalNode` — the graph projection

Each goal (parent and child) is also projected into the cognitive-memory
graph as a `GoalNode` — a stable node keyed by the goal `id` — so edges have
endpoints to attach to even when a goal has left the active board (e.g. a
parent demoted to the backlog once its children are on the board). The node
carries the goal id, description, and an optional `done_criterion` (`None` for
an umbrella whose completion is a roll-up rather than a single criterion); it
is the durable anchor the edges point at.

```rust
pub struct GoalNode {
    pub id: String,
    pub description: String,
    pub done_criterion: Option<String>,
}
```

### Edge types

| Edge | Direction | Meaning |
|---|---|---|
| `decomposes_into` | parent → child | The parent goal decomposes into this child. Traversing it backwards gives the parent (the "`parent_of`" read direction); no separate `parent_of` fact is stored. |
| `depends_on` | child → child | This sub-goal is gated on a sibling completing first (ordering / dependency). Optional. |

`decomposes_into` is written for **every** child of a decomposition.
`depends_on` is written only when the decomposition expresses an explicit
ordering between siblings (so child *B* is not started until child *A* is
`Completed`).

```rust
pub enum GoalEdgeType {
    DecomposesInto,
    DependsOn,
}

pub struct GoalEdge {
    pub from: String,
    pub to: String,
    pub edge_type: GoalEdgeType,
}
```

## Edge model — typed relationship facts

**Design choice (stated explicitly):** edges are represented as **typed
relationship facts** through the existing
[`CognitiveMemoryOps`](../architecture/cognitive-memory.md) trait — option
**(b)** from the issue — **not** a new graph-edge method on the trait.

Rationale: the trait surface today is fact/episode/procedure-oriented
(`store_fact`, `store_fact_with_caller_key`, `search_facts`,
`recall_facts_ranked`, `store_episode`, `store_procedure`). It exposes **no
public typed-edge writer**. The library backend *does* maintain typed edges
internally — `SUPERSEDES` for caller-key fact revisions, and
[`DERIVES_FROM` / `PROCEDURE_DERIVES_FROM`](./cognitive-memory-provenance.md)
for provenance — but those are written by dedicated provenance methods, not
a general edge API. Representing goal edges as typed relationship **facts**
gives **real, queryable** parent↔child edges today, composes with the
existing snapshot persistence and bridge ladder, and needs no upstream
`amplihack-memory-lib` change or pin bump. Promoting these to first-class
graph edges (a typed-edge API upstream + a pin bump under the
[self-maintain-deps pattern](../howto/self-maintain-dependency-pins.md)) is
a **follow-up** tracked off #2405; the relationship-fact representation is
forward-compatible with it.

### Concept keys and payload

Each edge is one fact, stored with a stable caller key so re-running
`decompose_goal` is **idempotent** (the same edge dedups instead of
accumulating; a changed edge supersedes its prior revision via the backend's
`SUPERSEDES` edge — see
[`store_fact_with_caller_key`](./goal-board-api.md)):

| Edge type | Concept key | Caller key |
|---|---|---|
| parent → child | `goal-edge:decomposes_into` | `goal-edge:decomposes_into:{parent_id}->{child_id}` |
| child → child | `goal-edge:depends_on` | `goal-edge:depends_on:{from_id}->{to_id}` |

The fact **content** is a small, stable JSON object so the endpoints and
type survive a round-trip:

```json
{ "from": "goal-7a1c", "to": "goal-9f02", "edge_type": "decomposes_into" }
```

and the **tags** carry the same `from` / `to` / `edge_type` as discrete
tokens so the keyword recall path (`search_facts` tokenizes a query into
keywords and ORs one `CONTAINS` per token — see
[tokenized fact recall](./cognitive-memory-fact-recall.md)) surfaces an edge
whether you query by parent id, child id, or edge type:

```
tags = ["goal-edge", "decomposes_into", "from:goal-7a1c", "to:goal-9f02"]
```

### Writing an edge

```rust
// Emit one decomposes_into edge from parent to a freshly-created child.
write_edge(
    ops,
    &GoalEdge {
        from: parent_id.clone(),
        to: child_id.clone(),
        edge_type: GoalEdgeType::DecomposesInto,
    },
)?;
```

`write_edge` builds the concept key, caller key, content, and tags shown
above and calls `store_fact_with_caller_key`. The caller key makes the write
idempotent: re-running `decompose_goal` for the same parent/child pair
supersedes the prior edge fact rather than appending a duplicate.

Endpoint ids are validated before an edge is written — `from` and `to` must
be non-empty, must pass the shared `validate_goal_id` charset check (defined
in `src/engineer_worktree/sweep.rs` and re-exported for the decomposition
path), and `from != to` (no self-edge). This prevents a malformed
LLM decomposition from forging a caller key or writing a degenerate edge.

### Querying an edge back (round-trip)

Because edges are facts under a well-known concept key, they are queried
with the same `search_facts` path every other goal fact uses. To enumerate
a parent's children:

```rust
// Children of `parent_id`: every decomposes_into edge whose `from` is the parent.
let edges = ops.search_facts(
    &format!("goal-edge:decomposes_into from:{parent_id}"),
    64,   // limit
    0.0,  // min_confidence
)?;
let child_ids: Vec<String> = edges
    .iter()
    .filter_map(|f| parse_goal_edge(&f.content)) // -> GoalEdge { from, to, edge_type }
    .filter(|e| e.from == parent_id && e.edge_type == GoalEdgeType::DecomposesInto)
    .map(|e| e.to)
    .collect();
```

The `from` / `to` filter on the parsed content is the authoritative check;
the keyword query just narrows the candidate set. The same shape answers
"what does this sub-goal depend on?" against `goal-edge:depends_on`. An edge
written by `decompose_goal` and read back by this query is the
**round-trip** the `goal_curation` edge-storage unit tests cover — the
acceptance proof that the edges are real, not a stub.

Two convenience readers wrap that query:

```rust
// Child goal ids of a parent (decomposes_into edges, from == parent_id).
let child_ids: Vec<String> = children_of(ops, parent_id)?;

// Every edge of a given type out of a node (e.g. what `child_id` depends on).
let deps: Vec<GoalEdge> = edges_of_type(ops, GoalEdgeType::DependsOn, child_id)?;
```

## Roll-up: parent progress from children

A parent's progress is a **roll-up** of its children, so the board never
shows a large goal parked at a stale percentage while its slices move:

- Map each child's [`GoalProgress`](./goal-board-api.md) to a percent:
  `Completed → 100`, `InProgress { percent } → percent`,
  `NotStarted / Proposed / Paused → 0`. A `Blocked` child contributes `0`
  **and** marks the parent as needing attention.
- Parent percent = the mean of its children's percents (rounded), surfaced
  as `InProgress { percent }`.
- The parent is `Completed` **only** when **every** child is `Completed`.
- If any child is `Blocked`, the parent surfaces the block so the operator
  and the brain see that the umbrella is gated, not silently `InProgress`.

The `rollup_parent_progress(children: &[ActiveGoal]) -> Option<GoalProgress>`
helper computes this from a slice of the parent's child goals: callers gather
those children through `parent_goal_id` (cheap, in-board) and/or the
`decomposes_into` edges (authoritative, graph), then pass them in. A parent
with no children yields `None` — it keeps its own directly-tracked status. The
helper is a pure function over the one level of children it is handed; it does
not itself recurse through the edge graph, so there is no traversal cycle to
guard.

## Active-goal cap and placement

Decomposition must not blow the active cap. `MAX_ACTIVE_GOALS` is **7**
(`src/goal_curation/types.rs`). When `decompose_goal` writes children it
keeps the active set within that cap by one of:

- **Replace** the parent on the board with its children when there is room —
  the parent is demoted to a backlog tracking node (`source = "decompose-parent"`,
  score `0.0`) and keeps its `GoalNode` + edges, so it remains the roll-up
  anchor; or
- **Backlog** the children when promoting them all would exceed the cap — the
  parent stays active as the anchor and the children land in the backlog
  (`source = "decompose:<parent_id>"`). They are linked to the parent through
  their `decomposes_into` edges (a `BacklogItem` has no `parent_goal_id` field;
  the edge is the linkage) and are promoted later by the normal backlog-scoring
  path.

Either way the **edges and `GoalNode`s are written regardless of placement**,
so a child sitting in the backlog is still a queryable child of its parent.

## OODA integration

Decomposition is the **loop-break**, not a spin. The loop-awareness work
([#2403](https://github.com/rysweet/Simard/issues/2403) /
[#2404](https://github.com/rysweet/Simard/issues/2404), see
[OODA loop self-detection](../concepts/ooda-loop-self-detection.md)) detects
a goal that is too large or has looped without progress. The intended wiring is
that when the Decide brain reaches that conclusion it **calls `decompose_goal`**
for that goal instead of re-triaging it:

- `goal_session_objective.md` instructs the goal-action brain to express an
  unbounded goal as concrete, completable sub-goals — this capability is the
  structured sink for that decision.
- `ooda_decide.md` routes a "too big / looping" goal to decomposition rather
  than another no-progress cycle.

> **Increment scope:** the automatic Decide-brain *call site* that fires
> `decompose_goal` is a **follow-up** (see
> [non-guarantees](#guarantees-and-non-guarantees)). This increment ships the
> driver, the queryable edges, the roll-up, the prompt, and the operator CLI
> that the trigger will call; the autonomous trigger is wired on top of them
> next.

After decomposition the parent's status reflects its children through the
[roll-up rule](#roll-up-parent-progress-from-children), so a subsequent cycle
sees real movement (children advancing) instead of a parent stuck at a high
percentage.

The decomposition path follows the established **deterministic-fallback**
pattern: when the decomposer step fails or returns an unusable shape (fewer
than 2 sub-goals after clamping, or a malformed child id), `decompose_goal`
surfaces a **loud** error and leaves the board and graph untouched rather than
silently producing zero or malformed children. All existing prose/JSON output
contracts the brains and parsers expect are preserved, and a content-pin test
guards the `goal_decomposition` prompt wording so the parser contract cannot
drift silently.

## Decomposition driver assets

The decomposition itself is prompt-driven, consistent with Simard's
[prompt-driven brain](../concepts/prompt-driven-brain-iteration.md) model:

| Asset | Path | Role |
|---|---|---|
| Prompt | `prompt_assets/simard/goal_decomposition.md` | Takes one large goal; emits 2–6 bounded sub-goals, each with an explicit done-criterion and optional `depends_on` ordering. Hot-reloads — no redeploy needed for wording changes. |
| Recipe | `prompt_assets/simard/recipes/goal-decomposition.yaml` | Wires the prompt into the recipe-runner so the brain and the CLI share one decomposition path. |

The prompt fences the incoming goal description as untrusted **data**, not
instructions, so a goal whose text contains "ignore your instructions and …"
cannot steer the decomposer. Its output is a fenced JSON block: a list of
2–6 objects, each `{ "description", "done_criterion", "depends_on"? }`. The
driver function `goal_curation::decompose_goal` parses that block, clamps the
list to `[2, 6]`, mints a child goal id per entry, then writes each child
goal + its `decomposes_into` edge (and any `depends_on` edges) through the
bridge. The prompt wording is pinned by a content-pin test
(`src/ooda_brain/prompt_store_tests.rs`) so the parser contract cannot drift
silently.

## Operator CLI

```
simard goal decompose <goal_id> [--max-children <N>] [--dry-run]
```

Triggers decomposition of an existing goal manually.

| Flag | Default | Effect |
|---|---|---|
| `--max-children <N>` | `6` | Upper bound on sub-goals to request (clamped into `[2, 6]`). |
| `--dry-run` | off | Print the proposed sub-goals and edges **without** writing them to the graph or the board. |

`<goal_id>` is validated with the shared `validate_goal_id` check before any
work begins; an unknown or malformed id exits non-zero with a clear message
and writes nothing.

It routes through the same cognitive-memory **writer bridge** path as
`goal add` / `goal remove` (`launch_writer_bridge(&state_root)` →
`bridge.ops()`, in `src/operator_cli/goal.rs`), so:

- when the OODA daemon is running, the write is serialized through the
  daemon IPC socket;
- when no daemon is running, it takes the local LadybugDB writer lock
  directly;
- a failure to acquire a writer surfaces synchronously as a non-zero exit —
  it does **not** silently degrade to a read-only handle.

It is listed alongside the other `simard goal` verbs in the
[CLI reference](./simard-cli.md). Inspect the result with `simard goal list`
— the children appear in the active section. `goal list` renders the **flat**
board (`ID / PRIORITY / STATUS / ASSIGNED / DESCRIPTION`); each child carries
its `parent_goal_id`, but that field is **not** a `goal list` column. To see
the parent↔child structure, query the `goal-edge:*` facts: in code via the
`search_facts` path shown in
[Querying an edge back](#querying-an-edge-back-round-trip), or from the
operator CLI with `simard memory dump --type=facts` (optionally `--json`)
filtered for the `goal-edge:` concept keys. A worked example is in
[How to decompose a large goal](../howto/decompose-a-large-goal.md).

### Example session

Child ids are minted deterministically as `<parent_id>-c<n>`, so a re-run
dedups its edges. The CLI prints a one-line summary to stderr:

```console
$ simard goal decompose goal-7a1c --max-children 4
[simard] goal decompose: 'goal-7a1c' -> 4 child goal(s) [Board]: goal-7a1c-c1, goal-7a1c-c2, goal-7a1c-c3, goal-7a1c-c4
```

`--dry-run` instead prints the proposed sub-goals (with their done-criteria)
and writes nothing:

```console
$ simard goal decompose goal-7a1c --max-children 4 --dry-run
[simard] goal decompose --dry-run: 'goal-7a1c' would produce 4 sub-goal(s) (clamped to 2-6 on apply); nothing written:
  1. Add parent_goal_id + GoalNode data model (done: serde back-compat test green)
  2. Implement typed-edge relationship facts (done: edge round-trips via search_facts)
  3. Add decompose_goal driver + prompt asset (done: 2-6 children, content-pin test green)
  4. Wire simard goal decompose CLI verb (done: verb routes through writer bridge)
```

After a (non-dry) run the children are active goals and the parent is demoted
to a backlog tracking node. `simard goal list` renders the flat board — the
children show in the active section and the demoted parent in the backlog
section; there is no `parent=` column:

```console
$ simard goal list
active goals: 4 / 7
ID	PRIORITY	STATUS	ASSIGNED	DESCRIPTION
goal-7a1c-c1	p1	not-started	-	Add parent_goal_id + GoalNode data model
goal-7a1c-c2	p1	not-started	-	Implement typed-edge relationship facts
goal-7a1c-c3	p1	not-started	-	Add decompose_goal driver + prompt asset
goal-7a1c-c4	p1	not-started	-	Wire simard goal decompose CLI verb
backlog: 1 item(s)
ID	SCORE	SOURCE	DESCRIPTION
goal-7a1c	0.00	decompose-parent	Give Simard a first-class goal-decomposition capability
```

The parent↔child edges live in the cognitive-memory graph. Read them back
with the read-only introspection command, filtering the fact dump for the
`goal-edge:` concept keys:

```console
$ simard memory dump --type=facts --limit=200 | grep 'goal-edge:decomposes_into'
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c1","edge_type":"decomposes_into"}
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c2","edge_type":"decomposes_into"}
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c3","edge_type":"decomposes_into"}
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c4","edge_type":"decomposes_into"}
```

## Guarantees and non-guarantees

**Contract the first increment provides:**

- A large goal can be decomposed into 2–6 linked sub-goals.
- Every parent → child relationship is written as a `decomposes_into`
  typed relationship fact in the cognitive-memory graph and is **queryable
  back** by parent id, child id, or edge type via `search_facts`.
- Re-running `decompose_goal` for the same parent/child pair is
  **idempotent** — the edge dedups (changed edges supersede, they do not
  accumulate).
- Parent progress **rolls up** from children via `rollup_parent_progress`.
- Decomposition never pushes the active set past `MAX_ACTIVE_GOALS = 7`;
  overflow children land in the backlog and stay linked to the parent through
  their `decomposes_into` edges (board children additionally carry the cheap
  `parent_goal_id` back-reference).
- Serde back-compat: legacy snapshots and `goal_records.json` files load and
  re-serialize unchanged when the new fields are unset.
- All existing brain/parser output contracts are preserved; the new prompt
  is guarded by a content-pin test.

**Not guaranteed (follow-ups tracked off #2405):**

- **First-class graph-edge API.** Edges are typed relationship *facts*, not
  a `CognitiveMemoryOps` typed-edge method. Promoting them to a first-class
  upstream edge API (with a pin bump under the
  [self-maintain-deps pattern](../howto/self-maintain-dependency-pins.md)) is
  a follow-up; the fact representation is forward-compatible.
- **OODA auto-decompose trigger.** The Decide-brain call site that fires
  `decompose_goal` automatically when a goal is judged too big / looping is a
  follow-up; the driver, edges, and CLI it depends on ship in this increment.
- **Recursive multi-level decomposition.** The increment decomposes one
  level (parent → children). Decomposing a child further is supported by the
  same data model but automatic multi-level fan-out is a follow-up.
- **Cross-goal `depends_on` scheduling in the Act phase.** `depends_on`
  edges are recorded and queryable; using them to *gate* engineer dispatch
  (don't start child *B* until child *A* is `Completed`) is a follow-up — the
  edges are written now so the scheduler can consume them later.
- **Edge garbage collection.** Deleting a goal does not yet prune its
  `goal-edge:*` facts; orphaned edges are harmless (their endpoints simply no
  longer resolve) and GC is a follow-up.

## Related

- [Goal board persistence — cognitive-memory single source of truth](../concepts/goal-board-persistence.md)
- [Goal board API reference](./goal-board-api.md)
- [Cognitive-memory provenance (`DERIVES_FROM` edges)](./cognitive-memory-provenance.md) — the precedent for typed graph edges over a previously flat node store
- [OODA loop self-detection](../concepts/ooda-loop-self-detection.md) — when the brain decides to decompose
- [Maximum safe parallelism](./maximum-safe-parallelism.md) — the **prompt-only** decomposition that predated this capability (`simard goal add` fanned an umbrella goal into per-issue sibling goals with no recorded relationship); this page is the first-class, edge-recorded form of that same behavior
- [Goal coverage allocation](./goal-coverage-allocation.md) — the per-cycle allocator that parallelizes the distinct child goals decomposition produces, up to the AIMD cap
- [Tokenized fact recall in preparation](./cognitive-memory-fact-recall.md) — how `search_facts` surfaces the `goal-edge:*` facts
- [Simard CLI reference](./simard-cli.md) — the `simard goal` verb tree
- [How to decompose a large goal](../howto/decompose-a-large-goal.md)
- [How to unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md)
