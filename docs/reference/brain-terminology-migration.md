---
title: Terminology Migration — the unified Brain model
description: The authoritative old→new rename map for the behavior-preserving terminology cleanup that unifies Simard's cognition under one Brain. Covers the reasoner renames, the Mind→Brain executive rename, the total elimination of "Bridge", the frozen wire-value allow-list, log-string changes, doc renames, and the anti-drift CI gate. This page and the changelog are the only documents allowed to name the retired identifiers.
last_updated: 2026-07-03
owner: simard
doc_type: reference
related:
  - ../architecture/brain-model.md
  - ../architecture/adapter-pattern.md
  - ./brain-executive-api.md
  - ./ooda-brain-api.md
---

# Terminology Migration — the unified Brain model

This is the authoritative map for the pure, **behavior-preserving** terminology
cleanup that unifies Simard's cognition under one [Brain](../architecture/brain-model.md)
model. Every entry below is a mechanical rename or a frozen wire value — **no
runtime behavior changes**. Use this page as the specification when applying or
reviewing the change.

!!! info "Allow-listed to name retired identifiers"
    The terminology law forbids the strings "Bridge" (any case), a phase-level
    "brain", and the standalone scheduler name "Mind" in live code. This page
    and [What's Changed](../whats-changed.md) are the **only** documents
    permitted to spell the retired identifiers, because a migration map must. The
    [anti-drift CI gate](#anti-drift-ci-gate) allow-lists them here.

## The terminology law (target state)

1. **"Brain" = the whole cognition** — process (scheduler) + threads + memory.
2. **A single OODA phase is a "reasoner"** — never a "brain".
3. **Nothing is named "Bridge"** — memory is a `CognitiveMemoryAdapter`, peers
   are `*Client`, the JSON-line substrate is `ServerTransport`.
4. **Threads are cognitive processes of the one Brain** — `CognitiveThread`,
   `ThreadKind` kept.

## Rename 1 — OODA-phase "brains" → reasoners

Module `src/ooda_brain/` → `src/ooda_reasoners/`; crate path
`simard::ooda_brain` → `simard::ooda_reasoners`.

### Traits

| Old | New |
| --- | --- |
| `OodaOrientBrain` | `OrientReasoner` |
| `OodaDecideBrain` | `DecideReasoner` |
| `OodaBrain` (act / engineer-lifecycle) | `ActReasoner` |

### Implementations

| Old | New |
| --- | --- |
| `RustyClawdBrain` | `RustyClawdActReasoner` |
| `RustyClawdDecideBrain` | `RustyClawdDecideReasoner` |
| `RustyClawdOrientBrain` | `RustyClawdOrientReasoner` |
| `DeterministicFallbackBrain` / `DeterministicLifecycleBrain` (act floor) | `DeterministicFallbackActReasoner` |
| `DeterministicDecideBrain` / `DeterministicFallbackDecideBrain` | `DeterministicFallbackDecideReasoner` |
| `DeterministicOrientBrain` / `DeterministicFallbackOrientBrain` | `DeterministicFallbackOrientReasoner` |
| `RecipeBrain` | `RecipeReasoner` |

The three reasoners stay **separate** — this is a rename, not a merge.

### Fields, builders, daemon wire-up

| Old | New |
| --- | --- |
| field `brain` (act) | `act_reasoner` |
| field `decide_brain` | `decide_reasoner` |
| field `orient_brain` | `orient_reasoner` |
| `build_rustyclawd_brain` | `build_act_reasoner` |
| `build_rustyclawd_orient_brain` | `build_orient_reasoner` |
| `build_act_brain` | `build_act_reasoner` |
| `build_decide_brain` | `build_decide_reasoner` |
| `build_orient_brain` | `build_orient_reasoner` |
| `src/operator_commands_ooda/daemon/brains.rs` | `…/daemon/reasoners.rs` |
| `fallback_brain_count()` / `FALLBACK_BRAIN_COUNT` | `fallback_reasoner_count()` / `FALLBACK_REASONER_COUNT` |

The `record_fallback` **phase** argument values (`"act"`, `"decide"`,
`"orient"`) are unchanged — they name OODA phases, not brains.

### Errors and phase enum

| Old | New | Note |
| --- | --- | --- |
| `SimardError::BrainResponseUnparseable` | `ReasonerResponseUnparseable` | rename only |
| `BrainParseSource` | `ReasonerParseSource` | rename only |
| `BrainPhase` (enum) | `ReasonerPhase` | **value-safe** — serde values frozen (see [allow-list](#frozen-value-allow-list)) |

## Rename 2 — the top-level "Brain" (was `Mind`) + `OodaContext`

| Old | New | Note |
| --- | --- | --- |
| `pub struct Mind` | `pub struct Brain` | The whole cognition: scheduler + reasoners (via `OodaContext`) + memory handle. |
| `Mind::new` / `Mind::with_budget` / `Mind::run_due` … | `Brain::new` / `Brain::with_budget` / `Brain::run_due` … | method renames only |
| const `BUDGET_ENV` | `BRAIN_NONCRITICAL_BUDGET_ENV` | **value frozen** — still reads `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` |
| `CognitiveThread`, `ThreadKind` | **kept** | the Brain's threads/processes |
| `OodaBridges` (struct) | `OodaContext` | bundle of memory + peer clients + session + reasoners |
| field/param `bridges` | `ctx` | |
| `src/ooda_loop/bridge_factory.rs` | `…/context_factory.rs` | |
| `bridges_from_state_root` | `context_from_state_root` | |

`OodaContext` (not `OodaReasoners`): the bundle holds more than reasoners, so it
is a context; "reasoners" names only the three phase reasoners inside it.

## Rename 3 — eliminate "Bridge" entirely

No live type, module, field, file, or doc may contain "Bridge" (any case).

### Server-transport substrate (the JSON-line IPC)

| Old | New |
| --- | --- |
| `src/bridge.rs` | `src/server_transport.rs` |
| `src/bridge_circuit.rs` | `src/server_circuit.rs` |
| `src/bridge_launcher.rs` | `src/server_launcher.rs` |
| `src/bridge_subprocess/` | `src/server_subprocess/` |
| `BridgeTransport` (trait) | `ServerTransport` |
| `NativeBridgeTransport` | `NativeServerTransport` |
| `SubprocessBridgeTransport` | `SubprocessServerTransport` |
| `InMemoryBridgeTransport` | `InMemoryServerTransport` |
| `BridgeRequest` / `BridgeResponse` / `BridgeHealth` | `ServerRequest` / `ServerResponse` / `ServerHealth` |
| `BridgeServer` (Python base class) | `ServerBase` |
| const family `BRIDGE_ERROR_*` | `SERVER_ERROR_*` (**values frozen**) |

### Cognitive-memory client

| Old | New |
| --- | --- |
| `src/memory_bridge/` | `src/memory_adapter/` |
| `src/memory_bridge_adapter/` | `src/memory_store_adapter/` |
| `CognitiveMemoryBridge` | `CognitiveMemoryAdapter` |
| `CognitiveBridgeMemoryStore` | `CognitiveMemoryStoreAdapter` |

### Peer clients

| Old | New |
| --- | --- |
| `src/gym_bridge.rs` / `GymBridge` | `src/gym_client.rs` / `GymClient` |
| `src/gym_runner_bridge.rs` | `src/gym_runner_client.rs` |
| `src/knowledge_bridge.rs` / `KnowledgeBridge` | `src/knowledge_client.rs` / `KnowledgeClient` |
| `src/terminal_engineer_bridge/` / `TerminalBridgeContext` | `src/terminal_engineer/` / `TerminalEngineerContext` |

### `SimardError` variants (in `src/error/mod.rs`)

| Old | New |
| --- | --- |
| `BridgeSpawnFailed` | `ServerSpawnFailed` |
| `BridgeTransportError` | `ServerTransportError` |
| `BridgeProtocolError` | `ServerProtocolError` |
| `BridgeCallFailed` | `ServerCallFailed` |
| `BridgeCircuitOpen` | `ServerCircuitOpen` |

`Display` text is reworded to drop "bridge"; the error semantics are unchanged.

### Documentation renames

| Old | New |
| --- | --- |
| `docs/architecture/bridge-pattern.md` | `docs/architecture/adapter-pattern.md` ✅ |
| `docs/reference/bridge-wire-protocol.md` | `docs/reference/server-wire-protocol.md` |
| `docs/reference/cognitive-memory-bridge-helpers.md` | `docs/reference/cognitive-memory-adapter-helpers.md` |

Each renamed doc has its content de-Bridged, internal links fixed, and its
`mkdocs.yml` nav entry updated. Docs whose filenames contain the **legal** word
"brain" for the *whole* cognition (e.g. `brain-introspection.md`,
`brain-model.md`) are **kept**. Docs in the `ooda-brain-*` and `recipe-brain-*`
families keep their filenames for inbound-link stability (a deliberate
exception) while their **content** is reframed to "reasoner"; `ooda-brain-api.md`
is the worked example (retitled *OODA Reasoners API*).

## Frozen-value allow-list

These wire values keep their literal spelling to preserve external contracts —
the **identifier** is renamed, the **value** is frozen. They are the only places
a retired spelling legitimately survives, and the [CI gate](#anti-drift-ci-gate)
allow-lists each one.

| Frozen value | Referenced via | Contract |
| --- | --- | --- |
| `"bridge.health"` (method) | `HEALTH_METHOD` const | JSON-RPC method name on the wire. |
| `BRIDGE_ERROR_*` codes (e.g. `-32601`, `-32001`) | `SERVER_ERROR_*` consts | Numeric error codes. |
| serde key `"brain_judgments"` in `CycleReport` | `#[serde(rename = "brain_judgments")]` | Persisted JSON key must round-trip. |
| `BrainPhase` → `ReasonerPhase` serde values | `#[serde(rename_all = …)]` frozen | Persisted phase strings must round-trip. |
| env `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK` | `BRAIN_NONCRITICAL_BUDGET_ENV` const | Operator-set env literal. |
| prompt-asset filenames edited by operators (hot-reload) | code consts | Operator hot-reload paths. |

## Log-string changes

| Old | New |
| --- | --- |
| `OODA daemon: all 3 brains LLM-backed (no fallback in use)` | `OODA daemon: brain online — orient/decide/act reasoners LLM-backed (no fallback)` |
| `OODA daemon: brain = RustyClawdBrain (prompt-driven)` (per-component) | `OODA daemon: act_reasoner = RustyClawdActReasoner (prompt-driven)` |
| `OODA daemon: decide_brain = RecipeBrain …` | `OODA daemon: decide_reasoner = RecipeReasoner …` |
| `OODA daemon: orient_brain = …` | `OODA daemon: orient_reasoner = …` |
| `DEGRADED — {phase}_brain = Deterministic*FallbackBrain …` | `DEGRADED — {phase}_reasoner = DeterministicFallback*Reasoner …` |
| `DEGRADED MODE — {n}/3 brains fell back …` | `DEGRADED MODE — {n}/3 reasoners fell back …` |

The self-health probe wording (`self_deploy/health.rs`, `safe_update/mod.rs`,
`operator_cli/self_health.rs`) that referenced *"brains LLM-backed"* becomes
*"reasoners LLM-backed"*. Probe **semantics and metric names** are unchanged
(metric names on the frozen allow-list where they cross the wire).

## Anti-drift CI gate

A CI check (`scripts/ci/check-terminology-drift.sh`) enforces the end state. It
scans for **retired identifier tokens** — not English prose — so the gate is
precise:

- **Code (`src/`):** zero residual identifiers containing `Bridge`, a
  phase-level `*Brain` type, the standalone scheduler type `Mind`, or the
  `OodaBridges`/`decide_brain`/`orient_brain`/`bridges` names — outside the
  [frozen-value allow-list](#frozen-value-allow-list). Each frozen literal is
  annotated in code with a `// FROZEN WIRE VALUE:` comment the gate keys on.
- **Docs:** the same retired identifier tokens are forbidden, with three
  allow-list carve-outs that apply everywhere:
    1. **Frozen wire literals** (`bridge.health`, `SIMARD_MIND_MAX_NONCRITICAL_PER_TICK`,
       `brain_judgments`, `BRIDGE_ERROR_*` code names) — they are current values,
       not retired names.
    2. **Prose law statements** that quote a forbidden word to forbid it
       (e.g. "nothing is named *Bridge*") — English prose, not an identifier.
    3. **This page and `docs/whats-changed.md`** — the only documents permitted
       to spell retired identifiers, because a migration map and a changelog must.

The gate runs in the same job as `cargo build` + `cargo test` + `mkdocs build
--strict`, so a green pipeline proves: no dangling old names, no new warnings,
and every doc/link resolves.

## See Also

- [The Brain](../architecture/brain-model.md) — the model this migration realizes.
- [Adapter pattern](../architecture/adapter-pattern.md) — the transport/adapter/client substrate.
- [Brain executive API](./brain-executive-api.md) — `Brain` + `OodaContext`.
- [OODA reasoners API](./ooda-brain-api.md) — orient/decide/act reasoners.
