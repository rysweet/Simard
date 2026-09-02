---
title: The RecipeInvoker seam — one brick, ten thin threads
description: >
  Rust API reference for `src/cognitive_threads/recipe_rail.rs`, the single
  synchronous "run one recipe → strict-JSON stdout" seam shared by the ten new
  cognitive threads (issue #5). Documents the `RecipeInvoker` trait, the
  `InvokeResult` classification (Json / SemanticMiss / InfraFailure), the
  production `RecipeRunnerInvoker` (a faithful extraction of the existing
  progress-checker subprocess logic), the offline `FakeRecipeInvoker` test
  double, and the security contract the brick enforces on behalf of every
  thread (argv discipline, control-char stripping, out-of-band file transport for
  unbounded memory payloads, output-size cap, secret scrub, hot-vs-in-tree path
  resolution). This is the one place the SR-4/6/7/8/9/11/13
  requirements converge.
last_updated: 2026-07-08
review_schedule: as-needed
owner: simard
doc_type: reference
status: reference — issue #5 (shared brick); threads default-ON since issue #4845
related:
  - ./cognitive-threads-catalog.md
  - ./cognitive-thread-scheduling.md
  - ./recipe-context-var-sanitization.md
  - ./recipe-brain-api.md
  - ../concepts/salience-and-decide.md
  - ../howto/add-a-new-cognitive-thread.md
---

# The RecipeInvoker seam — one brick, ten thin threads

Module: `simard::cognitive_threads::recipe_rail`.

`RecipeInvoker` is the **one shared brick** behind the ten new cognitive threads
(see the [catalog](./cognitive-threads-catalog.md)). It is a synchronous
"run one recipe, get its strict-JSON stdout" seam: given a recipe name and a set
of context variables, it resolves and spawns `recipe-runner-rs`, reads stdout,
and returns a **classified** result. Every recipe-backed thread depends on this
one trait, so every thread's acceptance test can run **offline and
credential-free** through the fake.

!!! note "Status — shared brick (issue #5); threads default-ON since issue #4845"
    This brick lands in **Phase 0** of the build (it blocks the eight
    recipe-backed threads). It is a *refactor-by-extraction* of subprocess logic
    that already exists inline in `recipe_progress_checker.rs` and the goal
    decomposer — not new behaviour — plus the security contract below. Each
    thread it enables is registered behind its double env gate, now **default-ON
    opt-out** (issue #4845): a thread runs unless a gate is set to an explicit
    falsy token.

## Why a seam exists at all

Before this brick, there was **no synchronous "run recipe → strict-JSON" seam**:

- `overseer::launch::RecipeRunner` is *workstream-shaped*
  (`spawn` → `probe` → PR) — the wrong shape for a thread that needs one blocking
  recipe call returning JSON.
- `recipe_progress_checker.rs` has the right subprocess logic but **hard-codes**
  it inline with no injectable seam, so it cannot be tested without a real agent
  binary and credentials.

Extracting that logic once into a trait with a production impl and a fake is
strictly simpler than eight inline copies, and it is what makes every thread's
acceptance test offline. One brick, ten thin consumers — no premature "thread
framework".

## The trait and its result type

```rust
/// Synchronous "run one recipe, get its strict-JSON stdout" seam.
pub trait RecipeInvoker: Send {
    /// Resolve + spawn `<recipe_name>.yaml`, pass each (k, v) as a distinct
    /// `-c k=v` argv pair, and return the classified stdout. NEVER silently
    /// degrades: INFRA and SEMANTIC misses are distinct typed results, both
    /// non-success.
    fn invoke(&self, recipe_name: &str, ctx_vars: &[(&str, String)]) -> InvokeResult;
}

pub enum InvokeResult {
    /// exit 0, non-empty stdout, parsed as strict JSON.
    Json(serde_json::Value),
    /// exit 0, non-empty stdout, but unparseable / typeless envelope.
    SemanticMiss { raw: String },
    /// spawn error | non-zero exit | empty stdout.
    InfraFailure { detail: String },
}
```

The three-way classification is the crux of the **no-silent-degradation**
contract (invariant I4 / SR-9). A thread maps **both** `SemanticMiss` and
`InfraFailure` to `ThreadOutcome::failed()` and performs **zero writes** —
write-through is reached only on `Json`. This is deliberately the *opposite* of
the progress-checker's historical accept-on-infra posture, and each thread
asserts the asymmetry in its own test.

## Production impl — `RecipeRunnerInvoker`

`RecipeRunnerInvoker` mirrors the existing `recipe-runner-rs` subprocess seams,
adding the security contract:

1. **Resolve the recipe path** — check the hot-reload dir
   `~/.simard/prompt_assets/simard/recipes/<name>.yaml` **first**, then the
   in-tree `<repo_root>/prompt_assets/simard/recipes/<name>.yaml`; **log which
   one was used** (hot vs. in-tree). If the hot dir is **group- or
   world-writable**, reject it, fall back to in-tree, and warn (SR-4). The
   residual risk of a trusted, correctly-permissioned hot dir is accepted in
   writing.
2. **Verify the binary** — `AMPLIHACK_AGENT_BINARY` is set via
   `RuntimeConfig::load` before the spawn.
3. **Build the `-c` context args by transport class** (`build_context_args`):
   - a **fenced untrusted-memory** payload (`is_fenced_payload`, i.e. the output
     of `fence_untrusted`) is *unbounded*, so it is written verbatim to a private
     per-invocation `0700` temp file via `ContextFile`
     (`src/recipe_context_file.rs`) and only
     `-c <key>_path=<abs>` rides on argv — the payload never touches argv, so
     `execve` can never fail with `E2BIG`/"Argument list too long" (SR-13,
     issues #2640/#2692), and the recipe reads `{{<key>_path}}` to see the full
     recall byte-for-byte (no truncation → no silent degradation);
   - a **small scalar** (`state_root`, `repo_path`, counts) is passed inline as
     `-c <key>=<sanitize_value>`, control-char stripped so it can never smuggle a
     second `-c` pair or a newline into prompt context (SR-7, SR-8).
4. **Spawn with distinct argv pairs** — one `.arg("-c").arg(value)` per variable,
   **no shell** (SR-8). The `ContextFile` guards outlive the spawn so the payload
   files exist while the recipe reads them.
5. **Read stdout with a cap** — enforce `MAX_OUTPUT_BYTES` on the parsed output
   so a runaway recipe cannot exhaust memory or flood a durable sink (SR-11).
6. **Classify** — into `Json | SemanticMiss | InfraFailure` (SR-9). A
   context-file write failure (e.g. `ENOSPC`) is itself an `InfraFailure`, never
   swallowed.

```
RecipeInvoker::invoke(recipe_name, &[(k, v), …]):
  1. resolve_recipe_path: log hot|in-tree; reject writable hot dir     (SR-4)
  2. build_context_args:
       fenced payload  -> ContextFile; argv carries only k_path=<abs>  (SR-13)
       small scalar    -> inline -c k=sanitize_value(v)                (SR-7/8)
  3. spawn recipe-runner-rs, distinct .arg("-c").arg(value), no shell  (SR-8)
     (ContextFile guards held alive until output())
  4. read stdout; enforce MAX_OUTPUT_BYTES cap                         (SR-11)
  5. classify -> Json | SemanticMiss | InfraFailure                   (SR-9)
```

## Offline test double — `FakeRecipeInvoker`

`FakeRecipeInvoker` returns a canned `InvokeResult` per recipe name, modelled on
the existing `FakeRunner` and `FakeIdeaSource` doubles. It lets each thread's
unit test drive the full rail — assemble → invoke → parse → write — with no
subprocess, no network, no credentials, and no sleeps (an injected clock
supplies `now_epoch`). Every thread in the [catalog](./cognitive-threads-catalog.md)
has an offline test built on this double, plus a gated live-smoke check.

```rust
// Illustrative usage inside a thread's offline test.
let fake = FakeRecipeInvoker::returning(
    "metacognition-appraise",
    InvokeResult::Json(json!({
        "calibration_error": 0.3,
        "decision_quality": 0.7,
        "patterns": [{ "name": "over_optimism", "evidence": "…" }],
        "recalibration_goal": null
    })),
);
// inject clock, run one tick, assert the metric + fact were written, and that a
// second identical tick writes no duplicate (dedup by pattern name).
```

## Shared helpers the brick exports

Alongside the trait, the brick exports three small helpers that every rail uses
before a durable write, so the security posture is centralized rather than
re-implemented ten times:

| Helper | Purpose | Requirement |
|--------|---------|-------------|
| `sanitize_value(&str) -> String` | strip control chars / newlines from a scalar value bound for argv or prompt context | SR-7, SR-8 |
| `fence_untrusted(&str) -> String` | wrap memory-sourced text in the `<<UNTRUSTED_MEMORY>>…<<END_UNTRUSTED>>` data region | SR-2 |
| `is_fenced_payload(&str) -> bool` | recognize a fenced payload so the invoker routes it off argv through a `ContextFile` (E2BIG-safe) | SR-13 |
| `secret_scrub(&str) -> String` | redact token-shaped substrings before writing to a fact, metric line, or issue body | SR-6 |

## The security contract in one place

This brick is the single point at which seven security requirements converge; it
is tested directly (context-transport unit tests, path-permission fixture,
size-cap fixture) so per-thread regressions are impossible to introduce silently:

| SR | What the brick guarantees | Test |
|----|---------------------------|------|
| SR-4 | resolved recipe path is logged (hot vs. in-tree); a group/world-writable hot dir is rejected with fallback + warning | writable-hot-dir fixture ⇒ rejected, falls back, warns; path logged |
| SR-6 | `secret_scrub` available and applied by rails before durable writes | seeded fake token not echoed into a stored fact / issue / metrics line |
| SR-7 | control-char / separator sanitization of scalar values and LLM-derived keys | over-long / control-char / `..` key rejected |
| SR-8 | distinct argv `-c k=v` pairs, no shell; no second-pair or newline smuggling | value `foo\n-c evil=1` ⇒ exactly one `-c` pair, sanitized |
| SR-9 | `SemanticMiss` / `InfraFailure` are non-success; caller writes nothing | non-JSON envelope ⇒ zero writes, `failed()` surfaced |
| SR-11 | parsed output size-bounded via `MAX_OUTPUT_BYTES` | recipe returning 10k items ⇒ ≤ cap facts/issues written |
| SR-13 | an unbounded fenced payload is delivered out-of-band via a private `0700` `ContextFile`; only `<key>_path=<abs>` rides on argv, so `execve` never fails with `E2BIG` and the recall is byte-for-byte (no truncation, no silent degradation) — issues #2640/#2692 | fenced value ⇒ argv carries only `k_path=<abs>`; file holds the payload verbatim; scalar stays inline |

For the salience-specific fence (numeric-only Decide projection) and the
overseer-vs-values separation of powers, see
[Salience and the OODA Decide handoff](../concepts/salience-and-decide.md). For
the standard recipe-rail template and how a new thread wires into this brick, see
[Add a new cognitive thread](../howto/add-a-new-cognitive-thread.md).

## See also

- [Cognitive-threads catalog](./cognitive-threads-catalog.md) — the ten consumers of this brick.
- [Recipe context-var sanitization](./recipe-context-var-sanitization.md) — the broader context-var safety model this brick aligns with.
- [Cognitive-thread scheduling](./cognitive-thread-scheduling.md) — invariants I1–I8 and the `Mind` contract.
