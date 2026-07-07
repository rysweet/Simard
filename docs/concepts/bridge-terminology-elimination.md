---
title: "Eliminating \"bridge\" terminology — one meaningful vocabulary, one guard"
description: >
  Why the word "bridge" is being purged from the names and operator-facing text
  of the Simard source tree — the CamelCase `*Bridge` type names and misnamed
  modules (already done under #2636), plus the lowercase `bridge` in snake_case
  identifiers, comments, and operator log / error strings (added by #2951) — and
  how an anti-regression guard (tests/no_bridge_naming.rs) keeps it from coming
  back. "Bridge" conveyed no meaning: renameable sites become the accurate RPC /
  client / reader / handoff vocabulary, so operator logs read
  `rpc server 'memory-ipc' transport error: …` instead of
  `bridge 'memory-ipc' transport error: …`. A small, explicit set of frozen wire /
  persisted / CLI string values (starting with the JSON-RPC method `bridge.health`)
  is NOT renamed — those are external contracts, and they are catalogued on the
  guard's documented allowlist. Extends #2636; tracks #2951.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: explanation
status: proposed
related:
  - ../architecture/rpc-pattern.md
  - ../reference/no-bridge-naming-guard.md
  - ../reference/rpc-wire-protocol.md
  - ../howto/fix-a-no-bridge-naming-guard-failure.md
  - ../../tests/no_bridge_naming.rs
---

# Eliminating "bridge" terminology

> **Status: proposed (design spec).** This page describes the target end-state of
> issue [#2951](https://github.com/rysweet/Simard/issues/2951) and the design of
> the guard that enforces it. It builds on
> [#2636](https://github.com/rysweet/Simard/issues/2636), which already removed
> the CamelCase `*Bridge` type names and renamed the misleadingly-named modules on
> disk. #2951 extends that work to the **lowercase `bridge`** that survives in
> snake_case identifiers, comments, and operator-facing log / error strings — the
> parts an operator actually reads. It is a **behavior-preserving rename of names
> and operator text we control**; a small, explicit set of frozen external string
> *values* is deliberately left unchanged (see
> [The frozen strings](#the-frozen-strings-not-renamed)).

## The operator rule

> **Nothing we control may be named "bridge."** The word conveys no meaning — a
> "bridge" between what and what, doing what? Every component that was called a
> "bridge" is really an **RPC client**, a **transport**, a **reader**, a
> **server**, or a **handoff**. Naming it accurately is the whole point.

The rule targets **names and operator-facing text** — the things we own and can
change without breaking anything outside this repository:

- **runtime log / error strings** an operator actually reads, e.g. the transport
  error that surfaced as
  `[simard] memory-ipc: connection error: bridge 'memory-ipc' transport error: write-len: Broken pipe`;
- **comments and doc comments** that called the memory + knowledge readers
  "bridges";
- **identifiers** — snake_case fields, variables, parameters, and functions
  (`launch_enrichment_bridges`, `bridge_name`, the error field `bridge: String`);
- **module and file names** (`bridge.rs`, `memory_bridge/`, `bridge_subprocess/`
  — already renamed under #2636).

The rule does **not** reach into string *values* that other systems, on-disk
formats, or operators' own scripts depend on. Those are a small, catalogued set
of **frozen strings** (below). The point is not "the six letters `bridge` appear
zero times"; it is "nothing *we name* is a meaningless 'bridge', and every place
the word remains is a documented external contract."

## Why "bridge" had to go as a *class*, not a site

Piecemeal renames leave the term alive in the corners operators see most — a log
line, a field, a comment. The word kept reappearing because there was no
executable definition of "done." Eliminating it as a class means three things
land together:

1. **A single replacement vocabulary** (below), chosen so each name states what
   the thing *does*.
2. **Every renameable site renamed** — strings, comments, identifiers, module
   paths — so a whole-tree grep comes back with nothing but the documented frozen
   strings.
3. **A guard that keeps it that way** —
   [`tests/no_bridge_naming.rs`](../reference/no-bridge-naming-guard.md) gains a
   *lowercase-word* check (the #2951 addition) alongside the existing CamelCase
   and module-path checks, so a regression cannot merge.

## The replacement vocabulary

The word "bridge" is replaced by the term that describes the actual role. Never
substitute a synonym that still hides meaning (no "connector", "link", "conduit").

| Role | Accurate term | Example |
|------|---------------|---------|
| Speaks the JSON-line RPC protocol to a server | **RPC / RPC client / transport** | `RpcTransport`, `NativeRpcTransport`, `rpc_transport/` |
| The named remote endpoint an RPC client talks to | **server** (as seen in errors) | `rpc server 'memory-ipc'` |
| Reads recalled memory for enrichment | **memory recall reader** (a memory-ipc client) | `EnrichmentClients.memory` |
| Reads knowledge packs for enrichment | **knowledge-pack reader** (a knowledge client) | `EnrichmentClients.knowledge` |
| Talks to the cognitive-memory store | **memory client / store** | `memory_client/`, `memory_store_adapter/` |
| Carries terminal state between engineer runs | **handoff** | `engineer_handoff/` |
| Talks to the gym eval engine | **gym client** | `gym_client.rs` |

Note the deliberate two-sided vocabulary: **our** components that call out are
*clients* (`memory_client`, `knowledge_client`, `gym_client`, `EnrichmentClients`,
`OodaClients`); the **remote peer** an error names is the *server*. An error means
"our client could not talk to the named server."

### Module & file renames (on disk — landed under #2636)

Enforced structurally by the guard's `misnamed_bridge_modules_are_renamed_on_disk`
test — the old path must be gone and the accurate path must exist.

| Was | Now |
|-----|-----|
| `src/bridge.rs` | `src/rpc.rs` |
| `src/bridge_circuit.rs` | `src/rpc_circuit_breaker.rs` |
| `src/bridge_launcher.rs` | `src/rpc_subprocess_launcher.rs` |
| `src/bridge_subprocess/` | `src/rpc_transport/` |
| `src/gym_bridge.rs` | `src/gym_client.rs` |
| `src/gym_runner_bridge.rs` | `src/gym_runner_client.rs` |
| `src/knowledge_bridge.rs` | `src/knowledge_client.rs` |
| `src/memory_bridge/` | `src/memory_client/` |
| `src/memory_bridge_adapter/` | `src/memory_store_adapter/` |
| `src/terminal_engineer_bridge/` | `src/engineer_handoff/` |

### Operator-facing strings (the #2951 work)

The change operators feel most is the error family in `src/error/display.rs`.
The `SimardError::Rpc*` variants carry a `server` field (renamed from `bridge`),
and every `Display` string names an **rpc server** — the remote peer the RPC
client failed to reach:

| Before (printed) | After (printed) |
|------------------|-----------------|
| `bridge '{b}' failed to spawn: {reason}` | `rpc server '{server}' failed to spawn: {reason}` |
| `bridge '{b}' transport error: {reason}` | `rpc server '{server}' transport error: {reason}` |
| `bridge '{b}' protocol error: {reason}` | `rpc server '{server}' protocol error: {reason}` |
| `bridge '{b}' call to '{m}' failed: {reason}` | `rpc server '{server}' call to '{m}' failed: {reason}` |
| `bridge '{b}' circuit is open — …until the bridge recovers` | `rpc server '{server}' circuit is open — …until the server recovers` |
| `bridge error: {msg}` | `rpc error: {msg}` |

`Display` output is operator-facing text, not a wire or on-disk contract, so
renaming it is safe and behavior-preserving. The log line the issue was filed
against now reads:

```
[simard] memory-ipc: connection error: rpc server 'memory-ipc' transport error: write-len: Broken pipe
```

Other renameable sites include snake_case identifiers such as
`launch_enrichment_bridges` → `launch_enrichment_clients`, the transport's
internal `bridge_name` field → `server_name`, and the many `bridges:` parameters
that carry an `OodaClients` / `EnrichmentClients` value.

## The frozen strings (NOT renamed)

Some lowercase `bridge` strings are **values other systems depend on**. Renaming
them is *not* behavior-preserving — it is a compatibility break — so #2951 does
**not** touch them. Each is documented and placed on the guard's allowlist. This
is the boundary between the rename (names, which mean nothing to other systems)
and the wire / on-disk / CLI (values, which other systems depend on).

| Frozen value | Where | Why it is frozen |
|--------------|-------|------------------|
| `bridge.health` | JSON-RPC method on the wire (`src/rpc.rs`, transports, clients) | protocol method name the external `amplihack-memory-lib` server understands; renaming breaks interop |
| `bridge_timeout` | `PartialReason::as_wire_str()` (`src/meeting_backend/close_guard.rs`) | serialized **wire value** for a meeting-close reason |
| `load-bridge-context` | `SessionPhase` token (`src/engineer_loop/…`) | **persisted / parsed** phase name; changing it breaks stored + in-flight sessions |
| `--terminal-bridge-json` | CLI flag (`src/bin/simard_engineer_step.rs`) | **command-line interface** other tooling invokes |
| `cognitive-bridge` | runtime-port name + log tag (`src/memory_store_adapter/store.rs`) | registered `runtime-port:…:cognitive-bridge` identifier |
| `bridge::native::{}` / `bridge:native:{}` | transport telemetry descriptor (`src/rpc_transport/native.rs`) | trace label consumed by external observability |

For `bridge.health` specifically, the design **centralizes the value behind a
single `HEALTH_METHOD` constant in `src/rpc.rs`** so production code references it
in one place; the literal may still appear in test fixtures that match the wire
method, and those are covered by the same token-scoped allowlist. The other frozen
values already live at a single producing site each.

If a future change needs to rename one of these, it must ship a real migration
story (versioned tokens, a CLI alias, a coordinated server release) — that is
explicitly out of scope for a terminology cleanup.

## What is explicitly *not* changed

- **No behavior.** Request/response shapes, on-disk formats, timeouts, retry
  logic, and circuit-breaker thresholds are untouched. Rename correctness is
  compiler-enforced across the referencing files.
- **No wire / persisted / CLI values.** The [frozen strings](#the-frozen-strings-not-renamed)
  above — and every JSON method name (`memory.store_fact`, `knowledge.query`,
  `gym.run_scenario`) — are unchanged. See
  [RPC Wire Protocol](../reference/rpc-wire-protocol.md).
- **No substrings.** The English words `abridged`, `unabridged`, and the place
  name `Cambridge` are *not* touched — the guard's component-boundary rule ignores
  `bridge` inside a larger word (see the guard reference).
- **No new sinks.** The rename adds no `println!`/`eprintln!`; the operator-log
  surface is the same size, only the words changed.

## How it stays gone

The guard runs under `cargo test` and in CI. It has three checks — CamelCase
absence and module-path renames (both from #2636), plus the new lowercase-word
absence (#2951) — with the frozen strings on a documented allowlist, and
self-tests that prove the scanner flags a planted lowercase `bridge`, ignores
`abridged`/`Cambridge`, and exempts the allowlisted frozen values. A regression
fails the build; there is no `--no-verify` / `--admin` path around it.

- **Reference (the contract):** [No-`bridge` naming guard](../reference/no-bridge-naming-guard.md)
- **How-to (you hit the guard):** [Fix a no-`bridge` naming guard failure](../howto/fix-a-no-bridge-naming-guard-failure.md)
- **Related architecture:** [RPC Transport Pattern](../architecture/rpc-pattern.md)
