---
title: No-`bridge` naming guard reference
description: >
  Reference for tests/no_bridge_naming.rs — the executable, shell-grep-shaped
  anti-regression guard that enforces the operator rule "nothing we control may
  be named bridge" across the Simard source tree. Specifies the three test
  functions (CamelCase absence and on-disk module renames from #2636, plus the
  lowercase-word absence added by #2951), the exact component-boundary matching
  rule that flags a standalone `bridge` while ignoring `abridged`/`Cambridge`, the
  documented allowlist (the guard's own fixtures excluded by basename, plus the
  small set of frozen wire / persisted / CLI string values), the planted
  self-tests, how to run it, and how CI enforces it. Extends #2636; tracks #2951.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: proposed
related:
  - ../concepts/bridge-terminology-elimination.md
  - ../howto/fix-a-no-bridge-naming-guard-failure.md
  - ../architecture/rpc-pattern.md
  - ../reference/rpc-wire-protocol.md
  - ../../tests/no_bridge_naming.rs
---

# No-`bridge` naming guard reference

> **Status: proposed (design spec).** This page is the target contract for
> [`tests/no_bridge_naming.rs`](../../tests/no_bridge_naming.rs), the guard that
> makes the operator rule *"nothing we control may be named `bridge`"* executable.
> The CamelCase and module-path checks already ship (issue #2636); the
> **lowercase-word check and the frozen-string allowlist described here are the
> #2951 addition** and are not yet implemented. The guard is an integration test
> auto-discovered by Cargo (no `[[test]]` entry needed) and runs under
> `cargo test` and in CI. Narrative and rationale live in
> [Eliminating "bridge" terminology](../concepts/bridge-terminology-elimination.md).

## Contents

- [What it enforces](#what-it-enforces)
- [Test surface](#test-surface)
- [The component-boundary matching rule](#the-component-boundary-matching-rule)
- [The allowlist](#the-allowlist)
- [Self-tests (planted fixtures)](#self-tests-planted-fixtures)
- [Scope](#scope)
- [Running it](#running-it)
- [Failure output](#failure-output)
- [Changing the allowlist](#changing-the-allowlist)

## What it enforces

The guard scans `src/**/*.rs` and fails if it finds the word `bridge` — as a
standalone component of a **name or operator-facing string**, in either case — in
identifiers, comments, or string literals, *except* the small documented
allowlist of frozen wire / persisted / CLI values. It also runs a structural
check that the misleadingly-named modules were renamed on disk. It is deliberately
*shell-grep-shaped*: an operator running the equivalent `git grep` gets the same
answer the test does.

## Test surface

The guard is three `#[test]` functions:

| Test | Kind | Origin | Fails when |
|------|------|--------|------------|
| `no_camelcase_bridge_naming_in_src` | content scan | #2636 (shipped) | any case-sensitive `Bridge` substring remains in `src/` (types/traits/variants, e.g. `RpcBridge`, `EnrichmentBridges`), outside the excluded files |
| `misnamed_bridge_modules_are_renamed_on_disk` | path scan | #2636 (shipped) | a `*bridge*` module path still exists, or its accurate replacement path is missing |
| `no_lowercase_bridge_word_in_src` | content scan | **#2951 (proposed)** | any lowercase/standalone `bridge` remains at a component boundary in `src/` — string literals, comments, snake_case identifiers — outside the allowlist |

`no_lowercase_bridge_word_in_src` is the check added for #2951. The other two are
already implemented and unchanged.

### On-disk rename map (`misnamed_bridge_modules_are_renamed_on_disk`)

Structural check on paths only (never file content), so it is immune to any
preserved wire/on-disk string literal:

| Old path (must NOT exist) | New path (must exist) |
|---------------------------|-----------------------|
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

## The component-boundary matching rule

The lowercase check does **not** use a naive `contains("bridge")` — that would
false-flag `abridged`, `unabridged`, and `Cambridge`, which legitimately appear in
the source (e.g. in `src/overseer/pr_verify.rs`). Instead it flags the stem
`bridge` only when it begins a **name component**.

For each line, for each case-insensitive occurrence of the 6-char stem `bridge`
starting at index `i`:

> **Flag the occurrence unless the character immediately before it (`i-1`) is an
> ASCII letter `[A-Za-z]`.**

The trailing side is unrestricted. Consequences:

| Text | Flagged? | Why |
|------|:--------:|-----|
| `bridge` (start of line/token) | ✅ | preceding char is not a letter (or none) |
| `memory_bridge` | ✅ | preceded by `_` |
| `"reader bridge"` | ✅ | preceded by space (inside a string/comment) |
| `bridges`, `bridge_name`, `bridge.foo`, `bridge'{x}'` | ✅ | boundary on the left; right side is free |
| `--terminal-bridge-json` | ✅ boundary, but **allowlisted** | preceded by `-`; exempt as a frozen CLI value |
| `EnrichmentBridges` | ✅ (via the CamelCase test) | the `B` is preceded by a letter, so the *lowercase* test skips it; the case-sensitive test catches it |
| `abridged`, `unabridged` | ❌ | `bridge` preceded by a letter (`a`, `a`) |
| `Cambridge` | ❌ | `bridge` preceded by a letter (`m`) |

Matching is case-insensitive, so a boundary `Bridge` (after `_`, `-`, `"`,
whitespace, `/`, `:`, or start of line) is also flagged by the lowercase test —
the two content tests overlap on purpose so neither can be the sole gate.

## The allowlist

The guard's allowlist is **minimal and fully documented**. It has two parts: files
excluded by basename (the guard's own detection machinery) and a small set of
token-scoped frozen values (external contracts).

### Excluded files (the guard's own machinery)

These files *are* the no-`bridge`/`Bridge` linter and its fixtures; they must
retain the literal as their detection substring or they could no longer detect
anything. They are excluded by basename from the content scans:

| File | Why it is excluded |
|------|--------------------|
| `src/overseer/pr_verify.rs` | the Overseer no-`Bridge` linter + its CamelCase fixtures (`PaymentBridge`, `HttpBridge`) and the words `abridged`/`Cambridge` used to prove the boundary rule |
| `src/overseer/merge_ops.rs` | unit tests for that linter |
| `src/operator_commands_dashboard/index_html/tests_tab_meta.rs` | asserts no dashboard tab is named `Bridge`; needs the literal as its detection substring |

> The guard file itself, `tests/no_bridge_naming.rs`, is **not** an allowlist
> entry — it lives in `tests/`, which the guard does not scan (see
> [Scope](#scope)). It names the thing it forbids only in its own source, outside
> the scanned tree.

### Frozen-value allowance (token-scoped)

These are lowercase `bridge` string **values** other systems, on-disk formats, or
CLI callers depend on. They are *not* renameable without a compatibility break, so
they are exempted by token — a line is exempt only for the listed occurrence; any
*other* `bridge` on that same line still flags.

| Allowed token | Where | Why unavoidable |
|---------------|-------|-----------------|
| `bridge.health` | `HEALTH_METHOD` const in `src/rpc.rs` + wire test fixtures | frozen **JSON-RPC method name** to the external `amplihack-memory-lib` server |
| `bridge_timeout` | `PartialReason::as_wire_str()` in `src/meeting_backend/close_guard.rs` | serialized **wire value** for a meeting-close reason |
| `load-bridge-context` | `SessionPhase` token in `src/engineer_loop/…` | **persisted / parsed** phase name; changing it breaks stored + in-flight sessions |
| `--terminal-bridge-json` | CLI flag in `src/bin/simard_engineer_step.rs` | **command-line interface** other tooling invokes |
| `cognitive-bridge` | runtime-port name + log tag in `src/memory_store_adapter/store.rs` | registered `runtime-port:…:cognitive-bridge` identifier |
| `bridge::native::{}` / `bridge:native:{}` | telemetry descriptor in `src/rpc_transport/native.rs` | trace label consumed by external observability |

For `bridge.health`, centralizing behind `HEALTH_METHOD` keeps production
references to a single line; the value may still appear in test fixtures that
match the wire method, which the same token-scoped allowance covers. See
[RPC Wire Protocol](../reference/rpc-wire-protocol.md) for why wire/persisted
values are frozen while Rust names are not.

## Self-tests (planted fixtures)

To prove the scanner is real (and not vacuously passing), the guard includes
self-tests that run against in-test fixture strings, not the tree:

- `boundary_rule_flags_standalone_bridge` — planted lines containing a lowercase
  standalone `bridge` (and variants `bridges`, `memory_bridge`, `"reader bridge"`)
  are flagged.
- `boundary_rule_ignores_bridge_inside_words` — `abridged`, `unabridged`,
  `Cambridge`, and `abridge` are **not** flagged.
- `frozen_tokens_are_allowlisted` — a line whose only `bridge` is a frozen value
  (e.g. `bridge.health`, `bridge_timeout`) is exempt, while the same line with an
  *extra* `bridge` still flags.

These fixtures live inside the test binary (temp strings / a temp dir), never in
`src/`, so they require no allowlist entry of their own.

## Scope

- **Scanned:** `src/**/*.rs`, including in-module `#[cfg(test)]` code.
- **Not scanned:** `tests/`, `docs/`, `Specs/`, `scripts/`, Python fixtures. The
  guard is a *source-tree* gate; the compiler + the rest of the suite keep the
  `tests/` tree consistent because renamed public symbols would not compile. This
  is why the guard file itself needs no allowlist entry.
- **Read-only:** the guard only reads files under the repo `src/` tree; it never
  writes and never follows symlinks outside the tree.

## Running it

```bash
# Just the guard
cargo test --test no_bridge_naming

# The equivalent operator grep (prints only the documented frozen-string allowlist)
git grep -niE '\bbridge\b' -- 'src/**/*.rs'
```

`cargo test` (no filter) runs it alongside the full suite; CI runs `cargo test`
and fails the job on any violation. There is no bypass — never `--no-verify` or
`--admin`.

## Failure output

Illustrative of the intended output: each content test lists every straggler as
`path:line:text` so the fix is mechanical:

```
Rename incomplete: 3 lowercase `bridge` word reference(s) remain in `src/`.
The operator rule is absolute — nothing we control may be named `bridge`. Rename
to the accurate RPC / client / reader / server / handoff vocabulary (see #2951).
Frozen wire/persisted/CLI values are allowlisted, not renamed.
Stragglers:
src/error/display.rs:225:                write!(f, "bridge '{bridge}' transport error: {reason}")
src/error/mod.rs:205:        bridge: String,
src/base_type_turn.rs:297:pub fn launch_enrichment_bridges(
```

The path test lists each unrenamed/missing module path:

```
Rename incomplete: 2 module-path issue(s).
still present (must be renamed away): src/memory_bridge
missing (rename target not created):  src/memory_client   (was src/memory_bridge)
```

## Changing the allowlist

Adding an allowlist entry is a **red flag** and requires review. Only two
categories are ever acceptable, and each must be commented at the entry:

1. **The guard's own detection machinery** (a new linter/fixture file that must
   contain the literal to do its job).
2. **A genuinely-unavoidable external value** you do not control — a wire method,
   a serialized/persisted token, or a published CLI flag (like the
   [frozen values](#frozen-value-allowance-token-scoped) above) — and even then,
   prefer centralizing it behind a single named constant so it appears once.

Everything else is a rename, not an allowance. Picking a "connector"/"link"
synonym to dodge the guard defeats the rule — see the
[how-to](../howto/fix-a-no-bridge-naming-guard-failure.md).
