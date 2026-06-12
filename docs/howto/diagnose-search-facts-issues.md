---
title: Diagnose search_facts issues
description: How to use diagnostic logging in search_facts() to verify cognitive memory queries are returning expected results, and troubleshoot empty-result scenarios.
last_updated: 2026-06-12
owner: simard
doc_type: howto
related:
  - ../concepts/preparation-compound-objective-search.md
  - ../concepts/goal-fact-dedup-in-preparation.md
  - ../reference/cognitive-memory-bridge-helpers.md
  - ../architecture/cognitive-memory.md
---

# Diagnose search_facts issues

This guide explains how to use the diagnostic logging added to
`search_facts()` (issue
[#2270](https://github.com/rysweet/Simard/issues/2270)) to verify that
cognitive memory queries return the expected results.

---

## Enable diagnostic logging

Set the `RUST_LOG` environment variable to include debug-level output
for the cognitive memory module:

```bash
export RUST_LOG=simard::cognitive_memory=debug
```

For more targeted output, combine with other module filters:

```bash
export RUST_LOG=simard::cognitive_memory=debug,simard::memory_consolidation=debug
```

Then start (or restart) the OODA daemon or run the engineer session.

---

## What the logs show

`search_facts()` emits two `tracing::debug!` messages per invocation:

### Entry log

```
search_facts: query_len=47, is_wildcard=false
```

| Field | Meaning |
|-------|---------|
| `query_len` | Length in bytes of the query string passed to `search_facts()` |
| `is_wildcard` | `true` if the query is `"*"` (fetch-all), `false` otherwise |

### Result log

```
search_facts: returned 8 rows
```

| Field | Meaning |
|-------|---------|
| `returned N rows` | Number of `CognitiveFact` records returned by the underlying Cypher query |

---

## Common diagnostic scenarios

### Preparation returns zero facts for multi-goal sessions

**Symptom:** Engineer sessions for multi-goal objectives have empty
`relevant_facts` in `PreparedContext`.

**What to check:**

1. Look for multiple `search_facts` entry logs per preparation cycle —
   one per goal in the objective. If you see only one entry with a
   large `query_len`, the objective is not being split correctly.

2. Verify each fragment returns > 0 rows. If all fragments return 0,
   the graph may not contain matching facts (normal for a new session
   with no prior episodes).

**Expected pattern (3-goal objective):**

```
search_facts: query_len=22, is_wildcard=false
search_facts: returned 3 rows
search_facts: query_len=18, is_wildcard=false
search_facts: returned 5 rows
search_facts: query_len=14, is_wildcard=false
search_facts: returned 2 rows
```

### Goal-store list returning fewer goals than expected

**Symptom:** `GoalStore::list()` returns fewer goals than expected.

**What to check:** Look for a `search_facts` call with
`is_wildcard=false` and a query matching `goal-store:record`. The
`returned N rows` value is the raw (pre-dedup) count. If N is close to
the limit (256), historical revisions may be crowding out current
records — see [Goal fact dedup](../concepts/goal-fact-dedup-in-preparation.md).

### Wildcard queries

**Symptom:** A `search_facts` call with `is_wildcard=true` appears in
logs.

**What it means:** A caller is fetching all facts (`"*"` query). This
is used by goal-store listing and some diagnostic tools. It is normal
but may be slow on large graphs.

---

## Verifying the compound-objective fix

After deploying the fix from issue #2270, verify it is working:

1. Set `RUST_LOG=simard::cognitive_memory=debug,simard::memory_consolidation=debug`

2. Trigger an engineer dispatch for a multi-goal objective (or wait for
   the OODA daemon to dispatch one)

3. Check logs for multiple `search_facts` entries during the
   preparation phase — one per goal description

4. Verify that at least some fragments return > 0 rows (assuming the
   graph has prior facts)

If you see a single `search_facts` entry with `query_len` equal to the
full joined objective length, the split is not working. Check that the
deployed binary includes the issue #2270 changes.

---

## Privacy note

The diagnostic logs emit **only** the query length (an integer) and
whether the query is a wildcard (a boolean). The raw query content is
**never logged** to avoid leaking goal descriptions or user-authored
content into debug logs.

---

## Related

- [Compound objective splitting](../concepts/preparation-compound-objective-search.md)
  — the design that these diagnostics support.
- [Goal fact dedup in preparation](../concepts/goal-fact-dedup-in-preparation.md)
  — the dedup layer that runs after the objective search.
- [Cognitive memory bridge helpers](../reference/cognitive-memory-bridge-helpers.md)
  — how `search_facts` reaches the graph store.
- [Cognitive Memory Architecture](../architecture/cognitive-memory.md)
  — full schema and query model.
