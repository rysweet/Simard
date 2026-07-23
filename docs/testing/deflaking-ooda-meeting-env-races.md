---
title: De-flaking the OODA-config and meeting cost-ledger env races
description: >
  How two parallel-`cargo test` flakes are made deterministic: the
  OODA-config default race (issue #4433) is closed by giving
  `ooda_config_default_values` the `cognitive_memory` serial key and clearing
  the concurrency env before it reads `OodaConfig::default()`, mirroring its
  already-correct twin, and by extending the `serial_guard` meta-test so the
  indirect concurrency-env read can no longer be reintroduced unkeyed. The
  meeting cost-ledger flake (issues #4359 / #4355 / #4354) is NOT yet fixed:
  the obvious HOME/serial hypothesis is retracted here because it is already
  implemented, so this page records a reproduce-before-fix contract, not a
  finished patch.
last_updated: 2026-07-23
review_schedule: when a new env-reading constructor is added to the OODA config path, when the meeting cost-ledger root cause is confirmed, or when serial_test is upgraded
owner: simard
doc_type: reference
related:
  - ./hermetic-tests.md
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-known-flaky-tests.md
  - ./ci-resilient-test-patterns.md
  - ./COVERAGE_BASELINE.md
---

# De-flaking the OODA-config and meeting cost-ledger env races

This page is the test-author and reviewer contract for two parallel-`cargo test`
flakes. It is a companion to
[serial(cognitive_memory) test isolation](./cognitive-memory-serial-isolation.md),
which owns the whole-binary env-serialization scheme this work plugs into, and
to [De-flaking the known flaky tests](./deflaking-known-flaky-tests.md), whose
structure it mirrors.

The two flakes are at very different stages, and this page deliberately keeps
them apart:

| Race | Flaky test | Status |
| ---- | ---------- | ------ |
| **A — OODA-config default** ([#4433](https://github.com/rysweet/Simard/issues/4433)) | `ooda_loop::tests_types::ooda_config_default_values` | **Root cause confirmed. Fix specified below and ready to build.** |
| **B — meeting cost-ledger** ([#4359](https://github.com/rysweet/Simard/issues/4359) · [#4355](https://github.com/rysweet/Simard/issues/4355) · [#4354](https://github.com/rysweet/Simard/issues/4354)) | `base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` | **Root cause UNCONFIRMED. No fix yet. Reproduce-before-fix contract only.** |

> **One canonical serial key.** Both races live inside the *same*
> process-global environment. A second, separate serial key would **not** help:
> the race is variable-agnostic — a glibc `setenv` on *any* name can
> `realloc(environ)` and free the array a concurrent `getenv` is mid-read (see
> [the cognitive_memory contract](./cognitive-memory-serial-isolation.md)). The
> only correct outcome is that *every* env mutator and every watched env reader
> in the lib-test binary funnels through the single existing `cognitive_memory`
> serial key. Anything that touches the process environment shares that one key
> or it races. This page adds nothing to the keying scheme; it only brings two
> stragglers into it (Race A) and refuses to guess at a third (Race B).

---

## Race A — OODA-config default (issue #4433): confirmed, ready to build

### The race

`ooda_loop::tests_types::ooda_config_default_values` asserts the shipped default
concurrency ceiling:

```rust
// src/ooda_loop/tests_types.rs — BEFORE (racy)
#[test]
fn ooda_config_default_values() {
    let config = OodaConfig::default();
    assert_eq!(config.max_concurrent_actions, 24); // issue #2935 default
    // ...
}
```

`OodaConfig::default()` reads three process-global variables —
`SIMARD_OODA_MAX_CONCURRENT`, `SIMARD_MAX_CONCURRENT_ACTIONS`, and
`SIMARD_SCALING` (`src/ooda_loop/types.rs`). This test carries **no serial key**
and does **not** clear those variables. A sibling suite in
`src/ooda_loop/types.rs` — `simard_ooda_max_concurrent_overrides_default`,
`max_concurrent_defaults_to_24_when_unset`, and the other `#2935` cases —
`set_var`/`remove_var`s exactly those variables. Those writers *do* carry
`#[serial(cognitive_memory)]`, but because `ooda_config_default_values` does
not, its read of `OodaConfig::default()` can run concurrently with a writer's
`set_var("SIMARD_OODA_MAX_CONCURRENT", "30")` and observe a torn or leaked
value — `max_concurrent_actions` comes back as `30` / `8` / `5` instead of the
default `24`, and the assertion fails intermittently.

The correct pattern already exists one file over, in the twin
`max_concurrent_defaults_to_24_when_unset`
(`src/ooda_loop/types.rs`): it holds the `cognitive_memory` serial key and
clears the three concurrency variables to a known-clean baseline before reading
`OodaConfig::default()`.

### The fix (finished shape)

Bring `ooda_config_default_values` up to the twin's pattern — the serial key
plus an explicit pre-read clear of the concurrency surface, so the read is
never concurrent with a writer and never observes leaked state:

```rust
// src/ooda_loop/tests_types.rs — AFTER (deterministic)
#[serial_test::serial(cognitive_memory)]
#[test]
fn ooda_config_default_values() {
    // Clear the concurrency-env surface OodaConfig::default() reads so the
    // assertion sees the shipped default, not a value leaked by a #2935
    // writer. Order-independent; no leakage in either direction.
    // SAFETY: serialised via #[serial(cognitive_memory)] — no concurrent env
    // mutation can tear this read/clear (see the cognitive_memory contract).
    unsafe {
        std::env::remove_var("SIMARD_OODA_MAX_CONCURRENT");
        std::env::remove_var("SIMARD_MAX_CONCURRENT_ACTIONS");
        std::env::remove_var("SIMARD_SCALING");
    }
    let config = OodaConfig::default();
    assert_eq!(config.max_concurrent_actions, 24); // issue #2935 default
    assert!((config.improvement_threshold - 0.02).abs() < f64::EPSILON);
    assert_eq!(config.gym_suite_id, "progressive");
}
```

This changes no production behaviour: `OodaConfig::default()` still resolves the
same variables in the same precedence. The suite is only prevented from racing
on them.

### Guardrail: extend the existing meta-test, do not add a new one

A regression guard for exactly this class of bug already ships:
`src/test_support/serial_guard.rs` (the `cognitive_memory` contract, issues
[#2360](https://github.com/rysweet/Simard/issues/2360) /
[#2375](https://github.com/rysweet/Simard/issues/2375)). It parses the source
tree with `syn` and fails the build when a `#[test]` touches the watched env
surface without the key. Its detection rule has two arms:

- **Mutation watch (`EnvWatch::AnyVar`, the shipped default):** any
  `set_var` / `remove_var` of *any* variable in a keyless test is an offender.
  This already covers every OODA concurrency *writer* — they carry the key, so
  they pass; a future keyless writer would be caught automatically. **No change
  needed for writers.**
- **Read watch (`READ_WATCHED_VARS` + `ENV_READING_HANDLERS`):** a keyless test
  is an offender if it directly `std::env::var`s a watched name
  (`SIMARD_STATE_ROOT`, `SIMARD_MEMORY_SOCKET`, `SIMARD_LLM_PROVIDER`,
  `SIMARD_MEETINGS_DIR`, `SIMARD_MEETINGS_ROOT`, `SIMARD_HANDOFF_DIR`) or calls
  a named env-reading handler.

The reader in Race A is the gap: `ooda_config_default_values` does not read the
concurrency variables *directly* — it reads them **indirectly** through the
`OodaConfig::default()` constructor. Neither the concurrency variable names nor
the constructor are in the read watch, so the guard cannot currently see this
reader. This is a *documented blind spot*, in the same family as the
false-negatives already recorded in the cognitive_memory contract.

The guardrail extension therefore adds the **OODA concurrency read surface** to
the existing guard, so a *future* keyless reader is flagged:

- Register `OodaConfig::default` as an env-reading trigger (analogous to the
  existing `ENV_READING_HANDLERS` entries such as `add_goal` /
  `write_auto_save`). Because the guard's call collector currently matches a
  single path segment (`default`, too generic to watch safely), the trigger
  must be recognised by its **fuller `OodaConfig::default` path**, and the
  collector extended minimally to record that two-segment call. This keeps the
  trigger precise and false-positive-free: only the concurrency-config
  constructor is watched, not every `::default()` in the tree.
- Detection rule, stated precisely: *a keyless `#[test]` that (a) mutates any
  process env var, (b) directly reads a `READ_WATCHED_VARS` name, (c) calls a
  registered env-reading handler, or (d) constructs `OodaConfig::default()`, is
  an offender.* Clauses (a)–(c) already exist; (d) is what #4433 adds.
- **Exemptions** use the guard's existing, machine-checked allowlist
  (`AuditOptions::allowlist`): a `(test_name, justification)` pair, where an
  empty justification is itself an audit failure. There is no new exemption
  syntax — genuinely env-free tests that merely *name* a watched symbol are
  allowlisted with a written reason, exactly as today.

The point of the arms above is that the fix is not "one more annotation on one
test"; it closes the *reader shape* so #4433 cannot silently return.

---

## Race B — meeting cost-ledger (issues #4359 / #4355 / #4354): UNCONFIRMED, no fix yet

> **This section describes work that has NOT been done, because the root cause
> is not yet known.** It intentionally contains no "finished fix" and no
> patched code block. Treat everything below as a contract for *how* to find and
> close the flake, not a description of a closed flake. Writing a fix before the
> reproduction exists is the exact blind-patch failure this page forbids.

### The retracted hypothesis (do not blind-patch this)

The obvious theory is: "the meeting cost ledger lives at
`$HOME/.simard/costs/ledger.jsonl`, so a concurrent meeting test on a shared
process-global `HOME` writes into the same ledger; redirect `HOME` to a temp
dir, hold the `cognitive_memory` serial key, and match the entry by session id."

**That hypothesis is retracted, because it is already fully implemented and the
flake persists.** In
`base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective`
today:

- `HOME` is already redirected to a per-test `tempfile::TempDir`.
- The test already carries `#[serial_test::serial(cognitive_memory)]`.
- The ledger entry is already matched by a **unique session id *and* model**
  (`session-…-000000004164`, `copilot-meeting`), so a concurrent meeting test
  sharing the temp `HOME` cannot substitute its own entry.
- `HOME` is restored with panic-safe teardown (`catch_unwind` +
  `resume_unwind`).

So HOME isolation, the serial key, and entry disambiguation are **not the
missing fix — they are already present.** Re-adding or re-emphasising them would
be a no-op dressed as a repair. Any PR that "fixes #4359 by isolating HOME /
adding the serial key" should be rejected on sight: read the test first.

### The actual contract: reproduce, then isolate at source

The true shared mutable state behind Race B is **not yet identified**. It is
some resource *other than* the already-isolated HOME ledger path — a candidate,
none confirmed, includes: a process-global `static` in the cost-tracking write
path (`crate::cost_tracking`), a shared on-disk path that does *not* derive from
`HOME`, an `ETXTBSY`/"Text file busy" race on the freshly-written `fake_copilot`
binary (the test already retries this, but the retry may be masking or
interacting with the real failure), or a torn env read of a variable the
meeting path consults that is outside the current watch set. **Pick none of
these by inspection.** The mandated order is:

1. **Reproduce first.** Stand up a deterministic reproduction *before* touching
   any production or test code. Run the single test under a stress loop with
   thread-count variation and no test caching, e.g.:

   ```bash
   for i in $(seq 1 200); do
     cargo test --locked --lib \
       base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective \
       -- --test-threads=8 --nocapture || { echo "FAILED on iter $i"; break; }
   done
   ```

   Vary `--test-threads` (1, 4, 8, 16) and run the *whole* `--lib` binary too
   (the race may only appear against concurrent unrelated tests). A fix is not
   allowed to proceed until a reproduction is captured.
2. **Identify the true shared resource** from the reproduction — the specific
   `static`, path, or env read that two concurrent tests contend on.
3. **Isolate it at source**, preferring the project's established pattern:
   thread an explicit, per-test root/handle through the exercised path so it
   never resolves the shared resource ambiently (the same shape that closed the
   goal-board state-root race in
   [De-flaking the known flaky tests](./deflaking-known-flaky-tests.md)). Fall
   back to the `cognitive_memory` serial key **only** if the contended resource
   is genuinely a process-global env read — and if so, extend
   `READ_WATCHED_VARS` / the guard so it is enforced, not just annotated once.
4. **Prove closure** by re-running the same stress loop from step 1 to a clean
   pass count (see the verification gate below).

Until steps 1–4 are done, the correct state of this page's Race B section is
"open, unconfirmed" — and it must stay that way rather than acquire a
speculative fix.

---

## Coverage follow-up (issue #4331): conditional scope

[Issue #4331](https://github.com/rysweet/Simard/issues/4331) (coverage) is
pulled into this work **only if** it shares a root cause with Race A or the
confirmed Race B cause — for example, if the same OODA concurrency-env reader or
the same cost-ledger resource is what a coverage gap left unguarded. If #4331 is
an independent coverage target, it is explicitly **out of scope here** and stays
on its own track. Do not expand this work to chase it speculatively.

## Constraints (apply to every change on this page)

- **Additive only.** No renames of existing tests, helpers, or public symbols;
  no signature churn on production APIs. Race A adds an attribute + a pre-read
  clear; the guard extension adds one trigger. Race B adds nothing until its
  cause is confirmed.
- **No `print!`/`println!` debugging left in tests** — use assertions and
  `--nocapture` transiently only.
- **Panic-safe teardown.** Any test that mutates process env or `HOME` must
  restore prior state through `catch_unwind` + `resume_unwind` (Race B's test
  already models this).
- **`--locked` everywhere.** All build/test invocations pass `--locked` so the
  gate matches CI and cannot silently drift `Cargo.lock`.
- **No production behaviour change.** These are test-isolation fixes; the OODA
  config resolution and the meeting cost-ledger write path behave identically
  in production before and after.

## Verification gate

A change on this page is done only when all of the following pass under
`--locked`:

1. **Race A determinism:** `ooda_config_default_values` passes ≥ 50 consecutive
   iterations of the whole lib binary at `--test-threads=8`:

   ```bash
   for i in $(seq 1 50); do
     cargo test --locked --lib -- --test-threads=8 || { echo "FAILED iter $i"; exit 1; }
   done
   ```

2. **Guard enforcement:** the `serial_guard` meta-test
   (`src/test_support/serial_guard.rs`) passes, and — as a red-phase check —
   temporarily removing the new key/clear from `ooda_config_default_values`
   makes the guard *fail*, proving clause (d) actually catches the reader shape.
3. **Race B (when in scope):** the step-1 stress loop above reaches ≥ 200
   consecutive passes across `--test-threads` ∈ {1, 4, 8, 16}, from a captured
   reproduction, before the flake is declared closed.
4. **Docs integrity:** `cargo test --locked --test docs_integrity` is green —
   this page's nav entry resolves and it has no dead intra-repo links.
