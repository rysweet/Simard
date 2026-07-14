---
title: File stewardship issues from orchestrator runs
description: Supply typed semantic identity and provenance to the guarded orchestrator-failure path.
last_updated: 2026-07-14
review_schedule: as-needed
owner: simard
doc_type: howto
---

# File stewardship issues from orchestrator runs

Use the semantic consolidation recipe before constructing
`OrchestratorRunSummary`. The recipe decides whether evidence is actionable and
provides a stable condition ID. Rust does not recover that decision from prose.

The trusted caller supplies:

- required run, recipe, step, source, failure-kind, and error fields;
- `IssueMutationIdentity` from the validated stable condition;
- a durable `CycleId`; and
- eligible `ArtifactProvenance`.

The crate-owned adapter calls:

```rust
process_orchestrator_run(&run, gh, &mut mutation_guard)?;
```

This function is crate-private because the GitHub mutation transport is not a
public write API.

Typed `ObservationOnly` values fail before repository routing or reservation.
Other eligible failures may create one guarded issue or replay a journaled
completion.

Created and replayed stewardship issues never enter `GoalBoard`.

Propagate all guard, persistence, provenance, and budget errors
to the owning cycle. Do not continue after failure.

See [File and consolidate stewardship issues safely](stewardship-filing-and-consolidation.md)
for the semantic producer contract.
