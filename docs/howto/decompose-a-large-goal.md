---
title: Decompose a large goal into linked sub-goals
description: Operator runbook for breaking one large goal into 2–6 bounded sub-goals with simard goal decompose, verifying the parent↔child edges round-trip in the cognitive-memory graph, and reading parent progress as a roll-up of its children (issue #2405).
last_updated: 2026-06-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/goal-decomposition.md
  - ../reference/goal-board-api.md
  - ../reference/simard-cli.md
  - ./unblock-stuck-ooda-goals.md
  - ./inspect-durable-goal-register.md
  - ../concepts/ooda-loop-self-detection.md
---

# Decompose a large goal into linked sub-goals

## When to use this

You have one large, unbounded goal on the board — the kind that never reaches
`Completed` because it is really an umbrella over many smaller pieces of work.
Instead of letting the OODA loop spin on it (or waiting for the
[auto-decompose trigger](../reference/goal-decomposition.md#ooda-integration)),
you want to break it into **2–6 bounded, independently-verifiable sub-goals**
and record the parent↔child structure as real, queryable edges in the
cognitive-memory graph.

This runbook uses the operator command. For the full data/edge model see the
[goal decomposition reference](../reference/goal-decomposition.md).

## Prerequisites

- A reachable state root (the same one the daemon uses). Pass `--state-root`
  explicitly or rely on the resolved default — see
  [state-root resolution](../reference/state-root-resolution.md).
- The goal id you want to decompose. List the board first — `simard goal list`
  prints the flat board as TSV (`ID / PRIORITY / STATUS / ASSIGNED /
  DESCRIPTION`):

  ```console
  $ simard goal list
  active goals: 2 / 7
  ID	PRIORITY	STATUS	ASSIGNED	DESCRIPTION
  goal-7a1c	p1	in-progress(35%)	-	Give Simard a first-class goal-decomposition capability
  goal-3b40	p2	in-progress(10%)	-	Harden the meeting-close lifecycle
  backlog: 0 item(s)
  ```

## Step 1 — Preview the decomposition (dry run)

Always preview first. `--dry-run` calls the decomposer and prints the proposed
sub-goals and edges **without** writing anything to the graph or the board:

```console
$ simard goal decompose goal-7a1c --dry-run
[simard] goal decompose --dry-run: 'goal-7a1c' would produce 4 sub-goal(s) (clamped to 2-6 on apply); nothing written:
  1. Add parent_goal_id + GoalNode data model (done: serde back-compat test green)
  2. Implement typed-edge relationship facts (done: edge round-trips via search_facts)
  3. Add decompose_goal driver + prompt asset (done: 2-6 children, content-pin test green)
  4. Wire simard goal decompose CLI verb (done: verb routes through writer bridge)
```

If the slices look wrong, the wording lives in
`prompt_assets/simard/goal_decomposition.md` and **hot-reloads** — edit it and
re-run the dry run; no redeploy needed.

## Step 2 — Decompose for real

Drop `--dry-run` to persist. `--max-children` caps the fan-out — the value is
clamped into `[2, 6]`, and omitting the flag requests the default of **6**
(so `simard goal decompose goal-7a1c` with no flag is a valid full run):

```console
$ simard goal decompose goal-7a1c --max-children 4
[simard] goal decompose: 'goal-7a1c' -> 4 child goal(s) [Board]: goal-7a1c-c1, goal-7a1c-c2, goal-7a1c-c3, goal-7a1c-c4
```

Child ids are minted deterministically as `<parent_id>-c<n>`, so re-running
the command dedups its edges instead of forking new ones.

The write routes through the same cognitive-memory **writer bridge** as
`goal add` / `goal remove`, so it is serialized through the daemon IPC socket
when the daemon is running, or takes the local writer lock directly when it is
not. A failure to acquire a writer exits non-zero — it never silently degrades
to a read-only handle.

> **Cap awareness.** `MAX_ACTIVE_GOALS` is **20**. If promoting all children
> would exceed the cap, the overflow children land in the **backlog** (and the
> parent stays active as the anchor instead of being demoted). They stay linked
> to the parent through their `decomposes_into` edges and are promoted later by
> the normal backlog-scoring path. The edges and `GoalNode`s are written
> regardless of placement, so a backlog child is still a queryable child of its
> parent.

## Step 3 — Confirm the children landed on the board

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

The four children are now active goals and the parent has been demoted to the
backlog tracking node. `simard goal list` renders the **flat** board, so it
does **not** print a `parent=` column — each child still carries its
`parent_goal_id`, but the parent↔child structure lives in the graph edges.
Verify that structure in the next step.

## Step 4 — Verify the edges round-trip in the graph

The point of this capability is that the parent↔child relationships are **real
graph edges**, not just a board field. Read the `goal-edge:*` facts back out of
cognitive memory with the read-only introspection command and filter for the
`goal-edge:` concept keys (add `--json` for machine-readable output):

```console
$ simard memory dump --type=facts --limit=200 | grep 'goal-edge:decomposes_into'
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c1","edge_type":"decomposes_into"}
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c2","edge_type":"decomposes_into"}
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c3","edge_type":"decomposes_into"}
  facts:        goal-edge:decomposes_into: {"from":"goal-7a1c","to":"goal-7a1c-c4","edge_type":"decomposes_into"}
```

Four `decomposes_into` facts, each with `from = goal-7a1c` and a distinct
child in `to` — the edges Simard wrote in Step 2, read straight back. `simard
memory dump` is read-only and safe to run while the daemon holds the store;
raise `--limit` if you have many facts so the goal edges are not truncated out
of the sample. Each edge fact also carries `from:` / `to:` / `edge_type` as
discrete tags, so the in-code `search_facts` path can fetch a single edge by
parent id, child id, or edge type — see
[Querying an edge back](../reference/goal-decomposition.md#querying-an-edge-back-round-trip).

## Step 5 — Watch parent progress roll up

As the children advance, the parent's progress is computed as a **roll-up** of
its children rather than a stale standalone percentage:

- each child's status maps to a percent (`Completed → 100`,
  `InProgress { percent } → percent`, otherwise `0`);
- the parent percent is the rounded **mean** of its children;
- the parent is `Completed` only when **every** child is `Completed`;
- any `Blocked` child surfaces the block on the parent so the umbrella is
  visibly gated, not silently `InProgress`.

You observe the inputs directly: as the children move, their `STATUS` advances
in `simard goal list`.

```console
# after goal-7a1c-c1 completes and goal-7a1c-c2 reaches 50%:
$ simard goal list
active goals: 4 / 7
ID	PRIORITY	STATUS	ASSIGNED	DESCRIPTION
goal-7a1c-c1	p1	completed	-	Add parent_goal_id + GoalNode data model
goal-7a1c-c2	p1	in-progress(50%)	-	Implement typed-edge relationship facts
goal-7a1c-c3	p1	not-started	-	Add decompose_goal driver + prompt asset
goal-7a1c-c4	p1	not-started	-	Wire simard goal decompose CLI verb
```

The roll-up rule then yields a parent progress of
`(100 + 50 + 0 + 0) / 4 = 37.5 → 38%`. That rolled-up value becomes the
parent's `GoalProgress` on its tracking node, so the OODA brain and any
consumer that reads the parent see real movement instead of a stuck
percentage; it is not surfaced as a separate `goal list` column.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Exits non-zero with `invalid goal id '<id>': …` and nothing is written | `<goal_id>` failed `validate_goal_id` (empty, too long, leading `-`/`.`, or a disallowed character) | Re-check the id with `simard goal list`; ids are charset-validated before any work begins. |
| Exits non-zero with `goal '<id>' not found on active board` | The id is not an active goal | Decomposition operates on an active goal; promote or re-check the id first. |
| Exits non-zero with `decomposition failed: …` and the goal is left intact | The decomposer returned an unusable shape (fewer than 2 sub-goals after clamping, a malformed child id, or the decomposer itself errored) | This is the **deterministic-fallback** safeguard refusing to write garbage — the board and graph are untouched. Adjust the goal wording or the prompt and re-run. (A fan-out larger than 6 is **clamped** to 6, not rejected.) |
| Children landed in the backlog instead of the board | Promoting all children would exceed `MAX_ACTIVE_GOALS = 20` | Expected. They stay linked to the parent through their `decomposes_into` edges and are promoted later by backlog scoring. |
| Re-running `decompose` did not create duplicate edges | The edge caller key (`goal-edge:{type}:{from}->{to}`) makes writes idempotent | Expected — a re-run supersedes the prior edge fact instead of appending. |

## Related

- [Goal decomposition & the goal graph](../reference/goal-decomposition.md) — the data model, edge model, roll-up rule, and full CLI contract
- [Goal board API reference](../reference/goal-board-api.md)
- [Simard CLI reference](../reference/simard-cli.md) — the `simard goal` verb tree
- [How to unblock stuck OODA goals](./unblock-stuck-ooda-goals.md)
- [How to inspect the durable goal register](./inspect-durable-goal-register.md)
- [OODA loop self-detection](../concepts/ooda-loop-self-detection.md) — when Simard decides to decompose autonomously
