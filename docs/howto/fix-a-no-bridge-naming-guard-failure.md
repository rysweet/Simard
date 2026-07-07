---
title: How to fix a no-`bridge` naming guard failure
description: >
  Step-by-step guide for a contributor whose PR failed tests/no_bridge_naming.rs
  because they introduced (or left behind) the word "bridge" in src/. Explains how
  to read the failure, pick a meaningful replacement from the RPC / client /
  reader / endpoint / handoff vocabulary (never a synonym that still hides meaning),
  apply the rename as a behavior-preserving change, recognize the small set of
  frozen wire / persisted / CLI values that are allowlisted rather than renamed
  (starting with bridge.health), and re-run the guard — without ever using
  --no-verify or --admin. Extends #2636; tracks #2951.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: how-to
status: proposed
related:
  - ../concepts/bridge-terminology-elimination.md
  - ../reference/no-bridge-naming-guard.md
  - ../architecture/rpc-pattern.md
  - ../reference/rpc-wire-protocol.md
---

# How to fix a no-`bridge` naming guard failure

> **Status: proposed (design spec).** This describes the target workflow once the
> #2951 lowercase-word check lands in
> [`tests/no_bridge_naming.rs`](../reference/no-bridge-naming-guard.md) (the
> CamelCase and module-path checks already ship under #2636). Use it when
> `cargo test` (locally or in CI) reports a failure from that guard. The rule it
> enforces: **nothing in `src/` that we control may be named "bridge"** — the word
> means nothing. Fixing it is a mechanical, behavior-preserving rename.
> Background: [Eliminating "bridge" terminology](../concepts/bridge-terminology-elimination.md).

## Prerequisites

- A local checkout that builds (`cargo build`).
- The failing test name and its straggler list (from the CI log or a local run).

## Step 1 — Reproduce and read the stragglers

Run the guard on its own; it prints every offender as `path:line:text`:

```bash
cargo test --test no_bridge_naming
```

You will hit one of three checks:

- **`no_lowercase_bridge_word_in_src`** — a lowercase/standalone `bridge` in a
  string, comment, or snake_case identifier.
- **`no_camelcase_bridge_naming_in_src`** — a CamelCase `*Bridge` type/variant.
- **`misnamed_bridge_modules_are_renamed_on_disk`** — a `*bridge*` file/dir path.

The equivalent operator grep shows the same lines (plus the allowlisted frozen
values, which are fine):

```bash
git grep -niE '\bbridge\b' -- 'src/**/*.rs'
```

## Step 2 — Decide: rename, or is it a frozen value?

First check whether the straggler is one of the small set of **frozen external
values** the guard allowlists — these are *not* renamed (see
[Step 4](#step-4-frozen-values-you-do-not-rename)). If it is a name or
operator-facing string **we control**, rename it.

## Step 3 — Pick a meaningful name

Replace "bridge" with the term that says **what the thing does**. Do **not** reach
for a synonym that still hides meaning ("connector", "link", "conduit",
"gateway") — that defeats the entire rule and reviewers will reject it.

| If the thing… | Name it… |
|---------------|----------|
| speaks the JSON-line RPC protocol to a server | `rpc` / `*RpcTransport` / `rpc_transport` |
| is the named remote peer an RPC error reports | `endpoint` (the error field) |
| reads recalled memory for enrichment | "memory recall reader" / `memory` (a memory-ipc client) |
| reads knowledge packs for enrichment | "knowledge-pack reader" / `knowledge` (a knowledge client) |
| talks to the cognitive-memory store | `memory_client` / "memory store" |
| carries terminal state between engineer runs | `handoff` / `engineer_handoff` |
| talks to the gym eval engine | `gym_client` |

See the full
[replacement vocabulary and rename map](../concepts/bridge-terminology-elimination.md#the-replacement-vocabulary).

### Apply the rename (behavior-preserving)

Rename the identifier/string/comment; do **not** change any logic, wire shape,
timeout, or on-disk format. Let the compiler find every call site.

**Identifiers and functions** — rename and rebuild:

```rust
// before
pub fn launch_enrichment_bridges(state_root: &Path) -> (Option<..>, Option<..>) { .. }
// after
pub fn launch_enrichment_clients(state_root: &Path) -> (Option<..>, Option<..>) { .. }
```

**Operator log / error strings** — rename the printed word too. The `SimardError`
RPC variants carry an `endpoint` field (renamed from `bridge`) and print "rpc
endpoint" — the remote peer the client failed to reach:

```rust
// before
Self::RpcTransportError { bridge, reason } =>
    write!(f, "bridge '{bridge}' transport error: {reason}"),
// after
Self::RpcTransportError { endpoint, reason } =>
    write!(f, "rpc endpoint '{endpoint}' transport error: {reason}"),
```

**Comments / test assertions** — reword them ("reader bridge" → "reader client",
"no memory bridge" → "no memory client").

**Module / file paths** (already done under #2636) — if you add a new one, use
`git mv` and update `mod`/`use` and any `src/lib.rs` re-exports:

```bash
git mv src/memory_bridge src/memory_client
```

Rebuild after each cluster; renames are compiler-checked:

```bash
cargo build --all-targets
```

## Step 4 — Frozen values you do **not** rename

A small set of lowercase `bridge` strings are **values other systems depend on**.
They are allowlisted, not renamed — changing them is a compatibility break. If
your straggler is one of these, leave the value alone:

| Frozen value | Why |
|--------------|-----|
| `bridge.health` | JSON-RPC method name on the wire to the external `amplihack-memory-lib` server |
| `bridge_timeout` | stable wire value emitted to operator logs / scrapers (`PartialReason::as_wire_str()`) |
| `--terminal-bridge-json` | published CLI flag other tooling invokes |

Internal identities you produce *and* consume — descriptor / runtime-port labels
like `cognitive-bridge` and `bridge:subprocess:…`, or the persisted
`load-bridge-context` phase name — are **not** frozen: rename them consistently on
both sides (that preserves behavior) rather than allowlisting them.

For `bridge.health`, prefer referencing it through a single named constant so the
literal lives in one place (optional, but tidy):

```rust
// src/rpc.rs
pub const HEALTH_METHOD: &str = "bridge.health"; // frozen external wire method — allowlisted

// call sites
let req = RpcRequest::new(HEALTH_METHOD, json!({}));
```

These frozen values are already on the guard's allowlist. Adding *any new*
allowlist entry needs explicit review — see
[Changing the allowlist](../reference/no-bridge-naming-guard.md#changing-the-allowlist).

## Step 5 — Confirm you did not over-match

The guard intentionally leaves real words alone. If your change touched
`abridged`, `unabridged`, or `Cambridge`, revert that part — those are correct and
the [component-boundary rule](../reference/no-bridge-naming-guard.md#the-component-boundary-matching-rule)
ignores them.

## Step 6 — Re-run and verify

```bash
cargo test --test no_bridge_naming     # guard is green
cargo test                             # full suite still green (rename is behavior-preserving)
cargo clippy --all-targets             # no new warnings
git grep -niE '\bbridge\b' -- 'src/**/*.rs'   # prints only the frozen-value allowlist
```

Also confirm you introduced **no new `println!`/`eprintln!`** — the rename changes
words, not the log surface:

```bash
git diff --stat
git diff | grep -nE '^\+.*\b(println|eprintln)!' || echo "no new stdout/stderr sinks"
```

## Step 7 — Commit and push (no bypass)

Commit normally and let the hooks and CI run. **Never** use `--no-verify` or
`--admin` to get past the guard — the guard failing means the word is still there,
and hiding that only defers the operator's confusion.

```bash
git commit   # hooks run; do NOT pass --no-verify
git push     # CI runs cargo test; the guard must be green
```

## Troubleshooting

- **"It still fails on a line I already fixed."** You likely renamed the
  identifier but left the word in a nearby comment or string on another line.
  Re-read the full straggler list — the guard reports every occurrence.
- **"I need to keep a persisted `bridge` string for format compatibility."** Check
  it against the [frozen-value list](#step-4-frozen-values-you-do-not-rename). If
  it is genuinely a wire/persisted/CLI value owned by another system, it is
  already allowlisted — leave it. Internal wire tokens (produced and consumed only
  within this repo) are renamed in lockstep with their producers/consumers; they
  are *not* allowlist candidates.
- **"The path test fails but I renamed the file."** Make sure you used `git mv`
  (so the old path is gone) *and* that the new path matches the
  [rename map](../reference/no-bridge-naming-guard.md#on-disk-rename-map-misnamed_bridge_modules_are_renamed_on_disk)
  exactly.
