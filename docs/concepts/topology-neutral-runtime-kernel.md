---
title: "Concept: topology-neutral runtime kernel"
description: Why Simard separates runtime requests from injected runtime services, and how that split enables truthful base-type selection, alternate topologies, and handoff restore.
last_updated: 2026-03-29
review_schedule: as-needed
owner: simard
doc_type: explanation
related:
  - ../index.md
  - ../reference/runtime-contracts.md
  - ../howto/export-and-restore-runtime-handoff.md
  - ../tutorials/run-rusty-clawd-on-loopback-mesh.md
  - ./truthful-runtime-metadata.md
---

# Concept: topology-neutral runtime kernel

Simard uses a topology-neutral runtime kernel so the runtime can tell the truth about what was requested, what was injected, and what actually ran.

## Contents

- [What the runtime kernel separates](#what-the-runtime-kernel-separates)
- [Why topology is injected instead of assumed](#why-topology-is-injected-instead-of-assumed)
- [Why selected base type and backend identity are both visible](#why-selected-base-type-and-backend-identity-are-both-visible)
- [Why handoff belongs inside the kernel](#why-handoff-belongs-inside-the-kernel)
- [Why handoff payloads are sensitive before handoff redaction lands](#why-handoff-payloads-are-sensitive-before-handoff-redaction-lands)
- [What stays out of scope](#what-stays-out-of-scope)

## What the runtime kernel separates

The runtime kernel splits one concern into two explicit objects:

- `RuntimeRequest` answers **what** should run: identity, selected base type, and topology
- `RuntimePorts` answers **how** it should run: prompt assets, memory, evidence, topology driver, mailbox transport, supervisor, agent program, handoff store, and session ID strategy

That separation matters because Simard needs to preserve both caller intent and concrete implementation truth.

Without that split, the runtime would be tempted to hide important facts such as:

- whether the caller asked for `copilot-sdk` or `local-harness`
- whether the runtime used the default in-process topology or an injected loopback mesh
- whether a handoff store was part of the runtime boundary or bolted on afterward

## Why topology is injected instead of assumed

The default CLI path is intentionally narrow:

- it reads bootstrap config from environment variables
- it assembles `RuntimePorts::new(...)`
- it runs a local `single-process` session

That path is useful, but it is not the whole runtime model.

Simard also supports in-process composition with caller-injected runtime services. That makes room for a second topology path built from:

- `LoopbackMeshTopologyDriver`
- `LoopbackMailboxTransport`
- explicit supervision
- a base type that supports the requested topology

Because topology is injected, Simard can fail honestly when a driver or backend does not support the requested combination. It never has to fake support by silently changing the topology behind the caller's back.

Today that practical path stops at `multi-process` for repository-provided end-to-end examples. The loopback mesh driver can advertise `distributed`, but callers still need to inject a compatible base type and the rest of the runtime services before that becomes a real run path.

## Why selected base type and backend identity are both visible

The runtime carries two related but different truths:

- `selected_base_type` is the contract choice the caller made
- `adapter_backend.identity` is the implementation identity that actually executed

That distinction matters today.

For example:

- `rusty-clawd` is a real backend and reflects as `rusty-clawd::session-backend`
- `copilot-sdk` is still an explicit alias and reflects as `local-harness`

If Simard collapsed those into one field, it would either lose the caller's choice or lie about the implementation. The current kernel does neither.

## Why handoff belongs inside the kernel

Handoff is not a side channel.

`RuntimeKernel::export_handoff()` exports the latest runtime boundary from the same runtime that owns:

- session identity
- selected base type
- runtime node and mailbox address
- memory records
- evidence records
- the injected handoff-store descriptor reported through reflection

`RuntimeKernel::compose_from_handoff(...)` restores that same boundary into fresh runtime ports.

Putting handoff inside the kernel gives Simard a single place to enforce restore rules:

- identity must match
- selected base type must match
- topology is preserved in the snapshot, but current restore does not yet enforce snapshot-topology equality
- records are rehydrated before the runtime starts
- restored runtimes stay in `Initializing` until the caller explicitly starts them

## Why handoff payloads are sensitive before handoff redaction lands

Live execution and persisted memory already narrow objective text down to `objective-metadata(...)`. Handoff export does not yet.

`RuntimeKernel::export_handoff()` currently clones `self.last_session` into `RuntimeHandoffSnapshot`, and `SessionRecord.objective` is stored as raw text.

So the current rule is:

- [PLANNED] handoff hardening should apply the same objective-metadata rule to exported session text
- until that change lands, exported payloads must be handled as fully sensitive runtime artifacts

A handoff snapshot still contains:

- session IDs
- execution phase
- selected base type and topology
- runtime-node and mailbox-address data
- memory summaries
- evidence records

That payload should be handled like a runtime artifact, not like public documentation or disposable log text.

## What stays out of scope

This feature deliberately does **not** define:

- an HTTP API
- a remote auth or token model
- TLS or transport-security rules for a public network protocol
- a database schema
- project-write behavior through memory policy

Those are separate contracts.

The runtime/base-type feature in this repository is a local CLI and in-process Rust contract. That narrower scope is a strength because it keeps the guarantees concrete and testable.

## See also

- [Runtime contracts reference](../reference/runtime-contracts.md)
- [How to export and restore runtime handoff](../howto/export-and-restore-runtime-handoff.md)
- [Tutorial: Run RustyClawd on the loopback mesh](../tutorials/run-rusty-clawd-on-loopback-mesh.md)
- [Concept: truthful runtime metadata](./truthful-runtime-metadata.md)
- [Documentation index](../index.md)
