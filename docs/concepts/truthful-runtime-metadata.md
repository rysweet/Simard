---
title: "Concept: truthful runtime metadata"
description: Why Simard keeps runtime metadata truthful and how the current contract expresses that truth.
last_updated: 2026-03-27
review_schedule: as-needed
owner: simard
doc_type: explanation
related:
  - ../index.md
  - ../reference/runtime-contracts.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
---

# Concept: truthful runtime metadata

Truthful runtime metadata means reflection must describe the runtime that is actually running, not a convenient label, thin wrapper, or degradation story.

## Contents

- [What the contract guarantees](#what-the-contract-guarantees)
- [Why explicit defaults are still acceptable](#why-explicit-defaults-are-still-acceptable)
- [Why session identity needs a harder boundary](#why-session-identity-needs-a-harder-boundary)
- [Why stop remains a lifecycle contract](#why-stop-remains-a-lifecycle-contract)
- [Why metadata truth should live in one place](#why-metadata-truth-should-live-in-one-place)

## What the contract guarantees

The current repo guarantees:

- bootstrap defaults are opt-in rather than silent degradation
- bootstrap fails if UTF-8 config decoding fails instead of pretending the value is missing
- prompt loading failures stay failures
- `ManifestContract` carries `entrypoint`, `provenance`, and `freshness`
- freshness acquisition fails explicitly instead of fabricating an epoch timestamp
- reflection reports `agent_program_backend`, `handoff_backend`, `adapter_backend`, `topology_backend`, `transport_backend`, `supervisor_backend`, `memory_backend`, and `evidence_backend` from live wiring
- reflection reports `runtime_node` and `mailbox_address` from the injected topology and transport services
- memory and evidence stores already report truthful backend descriptors
- `simard::bootstrap::assemble_local_runtime` is the assembly boundary reflected by the CLI path
- `stop()` moves the runtime into `Stopped`
- `snapshot()` still works after `stop()`
- post-stop calls surface `RuntimeStopped`
- session IDs are canonicalized to `session-<uuid>` and validated at parsing boundaries

## Why explicit defaults are still acceptable

Simard allows explicit defaults at bootstrap time.

That is not a contradiction.

The rule is:

- startup may use a documented default when the caller chose that mode intentionally
- runtime execution may not recover from failure by silently changing behavior

That is why `SIMARD_BOOTSTRAP_MODE=builtin-defaults` is acceptable today, while missing prompt assets after startup still fail.

## Why session identity needs a harder boundary

Process-local uniqueness is not enough for Simard.

Session IDs appear in:

- memory records
- evidence records
- base-type session requests
- reflection output

The runtime addresses that boundary in two ways:

- `UuidSessionIdGenerator` emits UUID v7 values
- `SessionId::parse(...)` accepts only UUID-based values and canonicalizes them to `session-<uuid>`
- the local UUID strategy is selected explicitly at bootstrap instead of being hidden inside runtime composition defaults

## Why stop remains a lifecycle contract

`stop()` means something real in the current runtime.

After shutdown:

- the runtime state is `Stopped`
- `snapshot()` remains available for inspection
- `run()` fails with `RuntimeStopped`
- `start()` fails with `RuntimeStopped`
- repeated `stop()` fails with `RuntimeStopped`

That makes shutdown observable without forcing callers to interpret generic transition pairs.

The same rule applies to failed runs:

- the runtime state becomes `Failed`
- the last session remains inspectable with `SessionPhase::Failed`
- `start()` and `run()` fail with `RuntimeFailed`
- `stop()` remains the explicit boundary that closes the failed runtime

That keeps failure visible instead of silently resetting the runtime or collapsing back to a generic transition error.

## Why metadata truth should live in one place

`ManifestContract` answers three separate questions in one object:

- what assembled this runtime
- where that metadata came from
- whether the metadata is still current

Keeping those answers together makes reflection harder to fake accidentally and removes duplicated top-level fields from `ReflectionSnapshot`.

## What this means for implementers

- keep the current explicit-default bootstrap behavior
- keep composition behind the bootstrap assembly boundary
- validate session IDs at parsing and injection boundaries
- preserve truthful store descriptors and extend the same rule to handoff stores, base-type backends, topology drivers, transports, and supervisors
- keep session ID allocation explicit at composition boundaries
- surface stopped runtimes with a dedicated lifecycle error

## See also

- [Runtime contracts reference](../reference/runtime-contracts.md)
- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md)
- [Tutorial: Run your first local session](../tutorials/run-your-first-local-session.md)

See the [documentation index](../index.md) for the full set of Simard docs.
