---
title: De-flaking the cost-ledger append race (meeting cost-writer HOME race)
description: >
  How the shared `pre-commit` failure across the Simard PR fleet — the flaky
  `meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` test —
  was made deterministic. The regression test redirects `$HOME` to a per-test
  temp dir and reads back `$HOME/.simard/costs/ledger.jsonl`; concurrent
  meeting tests that also call `record_cost` were appending into that
  process-global HOME-redirected ledger, so the target test intermittently
  failed to find its own `copilot-meeting` entry. The flake is closed by
  serializing every cost-recording test under the `cognitive_memory` serial
  key; the ledger `write_entry` is additionally hardened into a single atomic
  append (via a `write_entry_to` helper) as defense-in-depth, with a planned
  recurrence guard.
last_updated: 2026-07-20
review_schedule: when a new test records LLM cost via cost_tracking::record_cost, or when serial_test is upgraded
owner: simard
doc_type: reference
related:
  - ./cognitive-memory-serial-isolation.md
  - ./deflaking-known-flaky-tests.md
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ./COVERAGE_BASELINE.md
---

# De-flaking the cost-ledger append race

This page documents the finished state of the work that made the cost-ledger
regression test deterministic under parallel `cargo test`. It is the
test-author and reviewer contract for cost-recording tests, and it records the
production hardening that makes the ledger safe against concurrent appends.

## The flake

| Field | Value |
| ----- | ----- |
| Flaky test | `base_type_copilot::tests::meeting_turn_records_full_enriched_prompt_tokens_not_bare_objective` (`src/base_type_copilot/tests.rs`) |
| Symptom | Intermittent panic: `a copilot-meeting cost entry for this session must be recorded` — the ledger file exists but the expected entry is missing |
| Blast radius | The `pre-commit` job's `cargo test` step failed identically across the open-PR fleet (#4369, #4355, #4354, #4331, #4328, #4325, #4322, #4324) and on `main` HEAD; the `verify` workflow flapped (green/red on the identical single test); #4331's `coverage` gate (`cargo llvm-cov`) reddened because it re-runs the same suite |
| Class | Same race class as the [`cognitive_memory` env-tear flakes](./cognitive-memory-serial-isolation.md): a process-global resource mutated by one test is observed by a concurrent, unrelated test |

### Root cause

`cargo test --lib` runs many tests concurrently in **one** process. The
regression test (issue #4164) isolates the cost ledger by redirecting the
process-global `HOME` env var to a per-test `TempDir`, then reads back
`$HOME/.simard/costs/ledger.jsonl` and searches for the entry matching its
unique `session_id` + `model == "copilot-meeting"`.

Four sibling meeting tests also drive `run_fake_meeting_turn` /
`run_fake_meeting_turn_with_session`, which call `cost_tracking::record_cost` →
`write_entry`. `write_entry` resolves the ledger path from `HOME` at call time
(`ledger_path()`), so while the target test held `HOME` pointed at its temp dir,
a concurrent sibling turn appended **its** entry into the **same** file.

The **dominant** failure mode — the one that produced the observed panic — is a
**missing entry**: the `setenv("HOME", ...)` from one test tore a concurrent
`HOME` read in another, or a sibling turn ran under the target's redirected
`HOME` such that the target test's own `copilot-meeting` entry for its unique
session id was not present when it read the ledger back, so the `.expect(...)`
panicked. This is the same process-global race class as the
[`cognitive_memory` env-tear flakes](./cognitive-memory-serial-isolation.md),
and it is fixed by the serial key (fix #1).

A **secondary, latent** hazard is **interleaved partial lines**: `write_entry`
used `writeln!(file, "{line}")`, which under `O_APPEND` could issue the JSON
payload and the trailing newline as separate `write(2)` calls. In principle two
concurrent writers could interleave, producing a torn JSONL row. In practice
this did **not** cause the observed panic — both `read_entries`
(`cost_tracking.rs`) and the target test's reader `filter_map`/skip lines that
fail to parse, and under `O_APPEND` a racing writer appends *after* the target's
completed payload rather than into it. We still harden the writer (fix #2) as
defense-in-depth so the ledger cannot tear under any future concurrent writer.

## The fix

Two changes close the flake; a third is a planned recurrence guard. All are
additive and non-breaking — production JSONL on-disk format and every public
signature are unchanged.

| # | Change | File | Purpose |
| - | ------ | ---- | ------- |
| 1 | Serialize the cost-writer tests | `src/base_type_copilot/tests.rs` | The four sibling meeting tests carry `#[serial_test::serial(cognitive_memory)]`, so no concurrent turn appends into a HOME-redirected ledger — and no concurrent `setenv("HOME", ...)` tears a HOME read — while the target test reads it. **This is the correctness fix that closes the observed panic.** |
| 2 | Atomic append (defense-in-depth) | `src/cost_tracking.rs` | A private `write_entry_to(path, entry)` serializes the entry to a `String`, pushes a single `\n`, and issues **one** `file.write_all(...)`; `write_entry` delegates to it via `ledger_path()`. For cost lines (well under the pipe/`PIPE_BUF`-scale sizes at which a single `write` may split), Linux regular-file `O_APPEND` lands each such `write(2)` atomically, so concurrent writers never interleave partial lines. This hardens the format against future concurrent writers; it was **not** the cause of the observed panic (readers already skip unparseable lines). |
| 3 | Recurrence guard (planned extension) | `src/test_support/serial_guard.rs` | The AST meta-test today flags `#[test]`s that mutate `HOME`/env or read the state root without the `cognitive_memory` key. A **planned** extension adds `record_cost` reachers to that set, closing the HOME-derived-writer blind spot documented in [cognitive-memory-serial-isolation.md](./cognitive-memory-serial-isolation.md). Until that extension ships, the test-author rule below is enforced by review, not by the scanner. |

Both shipping mechanisms preserve full suite parallelism — only the small set of
cost-recording tests is serialized against each other, and only against tests
that already share the `cognitive_memory` key.

## The `write_entry` atomicity guarantee (API)

`cost_tracking::record_cost(session_id, model, prompt_chars, completion_chars,
context) -> io::Result<CostEntry>` appends one JSON-lines record to
`$HOME/.simard/costs/ledger.jsonl` and returns the written `CostEntry`.

Guarantee: **each recorded entry is appended with a single `write_all` of the
serialized line plus its `\n`.** Internally `write_entry` delegates to a private
`write_entry_to(path, entry)` so the same atomic-append path is exercised whether
the target is the real `ledger_path()` or a test-supplied temp path. For cost
lines — which are far smaller than the sizes at which a single `write(2)` may be
split into multiple syscalls — Linux regular-file `O_APPEND` lands that write
atomically, so concurrent callers (separate threads in the same process, or
separate processes) never observe or produce an interleaved or partially-written
line. Every line in `ledger.jsonl` is therefore a complete, parseable
`CostEntry` (or an intentionally-blank line, which `read_entries` skips).

The on-disk format is unchanged: one `serde_json` object per line, terminated
by `\n`. `CostEntry` fields (`timestamp`, `session_id`, `model`,
`prompt_tokens_est`, `completion_tokens_est`, `cost_usd_est`, `context`) and
their JSON names are unchanged. `daily_summary()` / `weekly_summary()` and
`simard status` read the ledger exactly as before.

## Test-author contract

**Rule:** any lib-binary `#[test]` that reaches
`cost_tracking::record_cost` (directly, or transitively via
`run_fake_meeting_turn` / `run_fake_meeting_turn_with_session` / a base-type
adapter turn) **MUST** carry the `cognitive_memory` serial key:

```rust
#[cfg(unix)]
#[test]
#[serial_test::serial(cognitive_memory)]
fn my_cost_recording_test() {
    // ...records an LLM cost via a base-type turn or record_cost directly...
}
```

Rationale: the ledger path derives from the process-global `HOME`. A test that
redirects `HOME` to isolate the ledger must not run concurrently with another
test that appends to (or reads through) that same redirected path, and a
`setenv("HOME", ...)` can itself tear a concurrent env read (the
`cognitive_memory` race class). The single serial key covers both the env-tear
and the shared-ledger hazards.

Today this rule is enforced by **review**. The `serial_guard` meta-test already
fails the build for a hand-written `#[test]` that mutates `HOME`/env or reads the
state root without the key; a **planned extension** (fix #3) adds `record_cost`
reachers to that scan so the rule becomes machine-enforced. The scanner is
designed to never emit a false positive — an offender is reported only when a
concrete trigger is observed without the key. See
[cognitive-memory-serial-isolation.md](./cognitive-memory-serial-isolation.md)
for the allowlist mechanism if you have a legitimate exception (e.g. a test
that stubs the ledger path and provably never touches `HOME`).

### Isolating the ledger in a test

When a test needs to read back what it wrote, redirect `HOME` to a `TempDir`,
restore it before propagating any panic, and match your entry by a **unique**
session id so a sibling's entry can never be mistaken for yours:

```rust
#[cfg(unix)]
#[test]
#[serial_test::serial(cognitive_memory)]
fn records_expected_cost_entry() {
    let home = tempfile::TempDir::new().unwrap();
    let prev_home = std::env::var_os("HOME");
    // SAFETY: serialised via #[serial(cognitive_memory)] — no concurrent env
    // mutation can tear this write.
    unsafe { std::env::set_var("HOME", home.path()); }

    let result = std::panic::catch_unwind(|| {
        let session_id = "session-00000000-0000-0000-0000-0000000042ab";
        // ...drive a turn that records cost under `session_id`...

        let ledger = home.path().join(".simard").join("costs").join("ledger.jsonl");
        let contents = std::fs::read_to_string(&ledger)
            .expect("turn must write a cost ledger entry under the temp HOME");
        let entry = contents
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|e| e.get("session_id").and_then(|v| v.as_str()) == Some(session_id))
            .expect("a cost entry for this session must be recorded");
        // ...assert on `entry`...
    });

    // SAFETY: restore HOME before propagating any panic (same serial key).
    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    if let Err(e) = result { std::panic::resume_unwind(e); }
}
```

Because `write_entry` is now atomic, `filter_map(... serde_json ...)` never
trips over a half-written line even if another cost-recording test races (it
won't, given the serial key, but the atomicity holds regardless).

## The concurrent-append regression test

`cost_tracking.rs` carries a `#[cfg(test)]` guard for the atomicity property:
`N` threads call the private `write_entry_to(path, entry)` helper concurrently
against **one explicit temp ledger path**; the test asserts that all `N` lines
parse as `CostEntry` and that the entry count is exactly `N`. Because it targets
an explicit path, it never touches `HOME` and needs no serial key. It fails on
the old `writeln!`-based writer (interleaved partial lines) and passes on the
atomic `write_all`. The public `record_cost` / `write_entry` signatures are
unchanged — only the internal delegation to `write_entry_to` is added.

## Verification gate

The flake is closed when all of the following hold:

- The named test passes deterministically under stress — 50+ consecutive local
  runs including the full `cognitive_memory` serial group:
  ```bash
  cargo test -p simard base_type_copilot::tests -- --test-threads=8
  ```
- The `serial_guard` meta-test passes (and, once fix #3 ships, flags any
  unguarded `record_cost` reacher).
- The concurrent-append regression test passes.
- `pre-commit` is green on all affected PRs (#4369, #4355, #4354, #4331,
  #4328, #4325, #4322, #4324) and on `main`.
- The `verify` workflow is green and deterministic — validated across repeated
  `push` **and** `pull_request` runs, not a single pass.
- #4331's `coverage` gate (`cargo llvm-cov --workspace --lib --bins`) passes.

## What was deliberately not done

- **No masking.** No `#[ignore]`, `#[cfg]`-skip, conditional skip, retry loop,
  `--retries`, or sleep-as-fix. The write→read is made deterministic instead.
- **No production behavior change.** `record_cost` still records the full
  enriched prompt tokens (issue #4164), still emits the `LLM_TOKENS` OTel
  counters, and still writes the same JSONL format to the same path. Only the
  *how* of the append changed (atomic `write_all`), not the *what*.
- **No blanket serialization.** Only cost-recording tests are serialized,
  reusing the existing `cognitive_memory` key.
