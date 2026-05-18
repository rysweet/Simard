---
title: "How to carry meeting decisions into engineer sessions"
description: Persist concise meeting records under a shared state root and verify that later engineer sessions carry them forward through the canonical `simard` CLI.
last_updated: 2026-03-30
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../index.md
  - ../reference/simard-cli.md
  - ../reference/runtime-contracts.md
  - ./configure-bootstrap-and-inspect-reflection.md
  - ../tutorials/run-your-first-local-session.md
---

# How to carry meeting decisions into engineer sessions

Use this guide when you want to prove one specific product seam: a meeting run captures durable meeting memory, and a later engineer run against the same state root carries that planning context forward without pretending code changed during the meeting.

## Prerequisites

- [ ] You are in the repository root
- [ ] `cargo run --quiet -- ...` works locally
- [ ] You want a local file-backed handoff, not a network service or remote orchestrator

## 1. Pick one explicit state root for both commands

The handoff only works when both runs point at the same durable state root.

```bash
STATE_ROOT="$(mktemp -d /tmp/simard-meeting-handoff.XXXXXX)"
```

Keep that shell variable for the rest of this guide.

## 2. Capture a structured meeting record

Run `meeting run` with a real structured objective. A carried meeting record is persisted when the objective includes any persistable structured output such as `update:`, `decision:`, `risk:`, `next-step:`, `open-question:`, or structured `goal:` lines. This example uses all of them.

```bash
MEETING_OBJECTIVE="$(cat <<'EOF'
agenda: align the next Simard workstream
decision: preserve meeting-to-engineer continuity
risk: workflow routing is still unreliable
next-step: keep durable priorities visible
open-question: how aggressively should Simard reprioritize?
goal: Preserve meeting handoff | priority=1 | status=active | rationale=meeting decisions must shape later work
goal: Keep outside-in verification strong | priority=2 | status=active | rationale=operator confidence depends on real product exercise
EOF
)"

cargo run --quiet --   meeting run local-harness single-process   "$MEETING_OBJECTIVE"   "$STATE_ROOT"
```

Look for output like this:

```text
Identity: simard-meeting
Decision records: 1
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
```

This run writes one concise meeting record and goal state under `STATE_ROOT`. It does not run code or mutate the repository.

## 3. Reuse the same state root in a later engineer run

Now point the engineer loop at the same repository and the same `STATE_ROOT`.

```bash
ENGINEER_OBJECTIVE=$'inspect the repository state
run one safe local engineering action
verify the outcome explicitly
persist truthful local evidence and memory'

cargo run --quiet --   engineer run single-process "$PWD" "$ENGINEER_OBJECTIVE" "$STATE_ROOT"
```

Look for output like this:

```text
Repo root: /path/to/repo
Active goals count: 2
Active goal 1: p1 [active] Preserve meeting handoff
Active goal 2: p2 [active] Keep outside-in verification strong
Carried meeting decisions: 1
Verification status: verified
```

The important contract is additive:

- `Active goals count` and `Active goal N` still describe the durable goal register
- `Carried meeting decisions` describes separate meeting-decision memory
- the engineer mode currently surfaces at most the three most recent persisted meeting records from that state root
- the engineer loop keeps that decision memory visible while choosing one bounded local action
- `Verification status: verified` still proves the engineer loop performed its normal inspect -> act -> verify cycle

## 4. Configuration rules that matter

For predictable handoff behavior, keep these rules in mind:

- pass the same explicit `state-root` argument to both `meeting run` and `engineer run`
- keep `meeting run` on a supported facilitator pairing such as `local-harness single-process`
- keep `engineer run` pointed at a real repository path for `workspace-root`
- expect the engineer loop to surface at most the three most recent carried meeting records, not an unbounded history dump
- treat carried meeting decisions as advisory context only; they do not auto-edit code or silently rewrite goals

## 5. Troubleshoot the common failure shapes

### `Carried meeting decisions: 0`

Usually one of these is true:

- the prior meeting run used a different `STATE_ROOT`
- the meeting objective only contained agenda text and no persistable structured output such as `update:`, `decision:`, `risk:`, `next-step:`, `open-question:`, or structured `goal:` lines
- you are looking at a clean state root with no earlier meeting data

### Goals show up, but no carried decision does

That usually means the goal state came from `goal-curation` or another earlier flow rather than the shared `meeting` run, or the engineer run is reading a different state root. A meeting run that persisted structured output writes both the goal updates and a concise meeting record.

### The engineer loop ran, but nothing was verified

That is outside this feature's contract. The handoff is only complete when the later engineer run still reports `Verification status: verified`.

## 6. Persisted memory bound

The engineer loop keeps **persisted** meeting memory bounded so that a long
chain of meeting → engineer handoffs cannot grow the on-disk memory store
without limit. The bound is enforced at the end of every engineer run that
persists artifacts.

### What the bound is

- The cap is `MAX_PERSISTED_MEETING_MEMORY = 32` records per scope.
- The cap is applied **independently per scope**: the `Decision` scope and
  the `SessionSummary` scope each retain at most 32 records on disk.
- As of this contract, only `Decision` and `SessionSummary` are pruned.
  Other scopes (for example `SessionScratch`) are **not** touched by this
  bound. Records written before the engineer-loop persist step (such as
  the early `SessionScratch` write) are out of scope.
- The cap is a build-time constant in `src/engineer_loop/mod.rs`. Operators
  cannot change it at runtime; changing it requires a code change and a
  rebuild.

### When pruning runs

Pruning runs at the end of `persist_engineer_loop_artifacts`, immediately
after the engineer loop has written the run's `SessionSummary` and
`Decision` records and before the engineer-loop session is advanced to
the next phase via `session.advance(...)`.

The order is deterministic:

1. `SessionSummary` is pruned first.
2. `Decision` is pruned second.

Each prune call is independent: when a scope is over the cap it performs
one atomic write; when it is at-or-under the cap it performs no I/O. The
two scopes are **not** pruned in a single transaction, so if the process
crashes between the two writes, one scope may already be at the cap while
the other is still over. The next successful engineer-loop persist will
re-converge both scopes.

### Eviction policy

When a scope is over the cap, the store evicts oldest-first using a stable,
total ordering on `(created_at, key)` ascending:

- Records with `created_at = None` sort **before** records with
  `created_at = Some(_)`. None is treated as "oldest known".
- Among records with the same `created_at`, the lexicographically smaller
  `key` is evicted first.
- Eviction is **permanent and destructive**. There is no archive, no tomb-
  stone, and no undo. Once a record is pruned it is gone from disk.
- Exactly `count - cap` records are evicted, where `count` is the number of
  in-scope records present at the moment the prune call took the store
  lock. The most recent `cap` records (by the same total ordering) are
  retained.

### Atomicity and noop semantics

- When the in-scope count is `<= cap` the prune call is a strict noop: it
  returns `Ok(0)` and performs **no** disk I/O. The on-disk file is
  byte-identical before and after.
- When pruning does write to disk it reuses the same checksummed write-
  then-rename path as `put`, so a concurrent crash leaves either the
  pre-prune file or the post-prune file intact, never a partial file.
- The in-memory record set is swapped to the pruned set **only** if the
  disk write returned `Ok`. On a write error, in-memory state is unchanged
  and the on-disk file is unchanged.
- The store lock is held for the entire critical section (count → sort →
  persist → swap). There is no read-modify-write race against other
  callers in the same process.

### Concurrency assumption

The bound assumes a **single-writer** model: exactly one engineer-loop
process is persisting into a given state root at a time. The lock prevents
intra-process races, but two simultaneous engineer-loop processes pointed
at the same state root can still race at the filesystem level and produce
last-writer-wins behavior. Operators who run multiple engineer loops in
parallel should give each one its own state root.

### Inspecting the bound

To confirm the bound is in effect for a given state root, run the engineer
loop enough times to exceed the cap and then count records on disk
directly. The file-backed memory store writes a single JSON file at
`<state_root>/memory_records.json` with the envelope
`{"crc32": <u32>, "records": [...]}`, so `jq` against that file is the
authoritative way to count retained records per scope.

A minimal check looks like this (replace `STATE_ROOT` with your state
root):

```bash
# After more than 32 persist cycles per scope, both scopes hold exactly 32:
jq '[.records[] | select(.scope == "Decision")]       | length' \
  "$STATE_ROOT/memory_records.json"   # => 32
jq '[.records[] | select(.scope == "SessionSummary")] | length' \
  "$STATE_ROOT/memory_records.json"   # => 32
```

The retained records are always the most recent 32 by
`(created_at, key)` ascending order; the oldest entries have been
permanently evicted.

For a higher-level view of meeting content (agendas, decisions text), see
[Inspect meeting records](./inspect-meeting-records.md). That guide uses
`meeting read`, which surfaces meeting payloads but does not expose the
per-scope on-disk record counts that this bound is concerned with.

### What the bound does **not** do

- It does not affect the in-session "carried meeting decisions" surface,
  which is governed independently by `MAX_CARRIED_MEETING_DECISIONS` and
  caps how many records are *shown to the engineer loop in a single run*
  (currently the three most recent).
- It does not modify, reorder, or otherwise rewrite the `value` payload of
  retained records. Pruning only removes records; retained records are
  byte-identical to what was last persisted.
- It does not page or archive evicted records anywhere. If you need long-
  term meeting history, capture it via the meeting markdown export flow
  documented in [Export meeting markdown](./export-meeting-markdown.md)
  before it ages out.

## Related reading

- For the exact command tree and compatibility surfaces, see [Simard CLI reference](../reference/simard-cli.md).
- For the broader runtime contract, see [Runtime contracts reference](../reference/runtime-contracts.md).
- For a longer end-to-end walk through the current operator flows, see [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md).
