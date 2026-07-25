---
title: Resource-isolated test suite
description: >
  How the simard lib test suite stays green under the self-deploy canary:
  every test is hermetic and load-tolerant, so 6-8 parallel copies of the
  compiled lib test binary run under host saturation with zero failures.
  Covers the two production fixes (durable append, spawn retry), per-test
  filesystem isolation, RAII env restore, and the parallel canary gate.
last_updated: 2026-07-25
review_schedule: when a new subprocess-spawn or shared-fs test is added
owner: simard
doc_type: how-to
related:
  - ../reference/durable-append-api.md
  - ../reference/spawn-retry-api.md
  - ./hermetic-tests.md
  - ./ci-resilient-test-patterns.md
  - ./cognitive-memory-serial-isolation.md
  - ./checkout-independent-workdir-tests.md
---

# Resource-isolated test suite

The `simard` self-deploy canary runs `cargo test` on the full lib test binary
**while the host is saturated** by 30+ concurrent engineer processes and, during
the canary, several parallel copies of the test binary itself. Any test that is
non-hermetic or load-sensitive reds the canary (exit 101) and refuses **all**
self-deploys. This page documents the finished state (issue
[#4577](https://github.com/rysweet/Simard/issues/4577)): the whole lib suite is
resource-isolated and load-tolerant, so the canary is stable.

The fix was to remove root causes, not to paper over symptoms. The suite does
**not** use `serial_test` as a load band-aid, `#[ignore]`, in-test retries of
assertions, loosened timing, `sleep`-to-pass, or a known-flaky registry.

## What "resource-isolated" means here

Two independent properties, both required to survive the parallel canary:

1. **Hermetic filesystem.** No two test processes — or two threads — share a
   fixed on-disk path. Each test owns a unique `tempfile::TempDir`, and any
   environment redirection (`HOME`, `SIMARD_STATE_ROOT`, ledger/metrics paths)
   is restored even if the test panics.
2. **Load tolerance.** Any operation the kernel can fail *transiently* under
   load — appending to a shared JSONL stream, or fork/exec-ing a real
   subprocess — is made durable or retried at the source, so a busy host never
   turns a correct test into a red.

## Fix 1 — Durable appends (production bug)

Concurrent appends to the cost ledger and the self-metrics stream dropped and
tore records under load: on the old two-syscall `writeln!` path, parallel binary
copies sharing one `$HOME` lost 630/1763 of 2000 metric writes. Both writers now
delegate to one loss-free helper, `util::durable_append::append_line`, which
serializes in-process writers with a process-global mutex and writes each record
as a single atomic `O_APPEND` `write_all` (then `flush`es). The cross-process
drop window is closed by that single sub-`PIPE_BUF` write; the mutex and shared
audited helper add in-process discipline and eliminate the divergence between the
two writers. See the [Durable line-append API](../reference/durable-append-api.md)
for the full contract, including why the self-metrics consolidation is
defense-in-depth on top of its already-shipped single-`write_all` fix.

Regression coverage:

- `cost_tracking::concurrent_appends_never_interleave_or_drop_entries` — many
  threads append to a **unique** `TempDir` ledger; the read-back must see every
  record (`seen.len() == 1024`, no duplicates, no torn lines).
- The self-metrics concurrency test asserts the same zero-loss property for
  `record_metric`.

## Fix 2 — Bounded spawn retry (production bug)

`gh` and agent subprocess launches failed transiently under fork/exec load with
`ETXTBSY`, `EAGAIN`, or `ENOMEM`. Every real-subprocess spawn now routes through
`util::spawn_retry`, which retries **only** those transient errno values with a
bounded, capped backoff and passes every other outcome straight through. See the
[Spawn-retry API](../reference/spawn-retry-api.md).

Regression coverage: the `gh_client` create-issue test and the `tool_executor`
Bash-tool tests pass on a saturated host because a transient spawn errno is
retried rather than `.expect(...)`-panicked.

## Fix 3 — Per-test filesystem isolation

Every test that touches a concurrent or shared filesystem path owns a unique
temp directory instead of a fixed name shared across parallel binary copies.

```rust
use tempfile::TempDir;

#[test]
fn concurrent_appends_never_interleave_or_drop_entries() {
    let dir = TempDir::new().expect("temp dir");        // unique per process
    let ledger = dir.path().join("cost-ledger.jsonl");  // no fixed shared path
    // ... spawn threads, append, read back, assert zero loss ...
    // `dir` drops at end of test — no mid-run remove_dir_all of a shared path
}
```

Rules:

- **Never** name a test artifact with a fixed path such as
  `target/test-<x>` or `env::temp_dir()/<fixed-name>` — parallel copies collide.
- **Never** `remove_dir_all` a directory another parallel copy might be using;
  let `TempDir` clean up its own unique directory on drop.

## Fix 4 — Panic-safe environment restore

Tests that redirect process-global env (`HOME`, `SIMARD_STATE_ROOT`) restore it
through an RAII guard so a panicking test cannot leak a temp `HOME` into a
sibling test:

```rust
struct EnvRestore { key: &'static str, prev: Option<String> }
impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}
```

Because `HOME` is process-global, tests that mutate it keep their existing
`#[serial(...)]` grouping — that is *required* in-process serialization, not a
load band-aid. Cross-process isolation still comes from the unique `TempDir`.

## Fix 5 — No fragile timing assertions

`ooda_actions::tests_dispatch_concurrency::concurrent_dispatch_parallelizes_and_respects_cap`
no longer asserts any wall-clock parallel-vs-serial ratio (that assertion was
removed). Concurrency correctness is proven **deterministically** by the peak
counters that remain:

```rust
assert!(peak_parallel >= 2, "cap should allow parallel dispatch");
assert!(peak_serial <= 1, "serial path must never run two at once");
```

Wall-clock ratios are inherently load-sensitive; peak-counter assertions are
not, so they hold under any host load.

## Fix 6 — Deterministic advisory-lock release under fork/exec (production bug)

The installer's advisory `flock` (`install::paths::acquire_install_lock`) is held
by an open file descriptor. On Unix a `flock` lock lives on the **open file
description**, so a bare `close(2)` releases it only once the *last* descriptor
referring to that description is closed. When Simard spawns any subprocess while
holding the install lock, the child transiently inherits a dup of the lock fd
during its `fork`→`exec` window (the fd is close-on-exec, but `CLOEXEC` only
fires at `exec`, not at `fork`). Under massively-parallel host load that window
stretches, so a concurrent spawn elsewhere in the process could keep the
inherited dup open past the installer's release — leaving the lock held after the
`InstallLock` guard dropped, and spuriously failing the next legitimate
`acquire_install_lock` with `EWOULDBLOCK`.

`InstallLock::Drop` now issues an explicit `flock(fd, LOCK_UN)` before the file
closes. `LOCK_UN` releases the lock on the shared open file description
immediately and deterministically, regardless of any outstanding inherited dups.
This is a real production robustness fix (the installer could otherwise wedge
itself out of a re-install after spawning a child), verified by
`install_lock_release_survives_forked_child_holding_inherited_fd`, which `fork`s
a child that deliberately holds the inherited fd past the guard drop and asserts
re-acquire still succeeds at once.

## Suite-wide sweep rules

Two standing rules keep new tests canary-safe:

1. **Subprocess spawns.** Any test or production site that spawns a real
   subprocess and needs its result must launch it through
   `util::spawn_retry::{retry_spawn_sync, retry_spawn_async}` — never a bare
   `.spawn()`/`.output()` followed by `.unwrap()`/`.expect(...)`.
2. **Shared-filesystem appends.** Any test that writes a concurrent or
   shared-filesystem path must use a unique `tempfile` path and, for append
   streams, `util::durable_append::append_line` — never a fixed shared path and
   never in-test serialization as a substitute for isolation.

## The parallel canary gate

The acceptance gate mirrors what the self-deploy canary does, amplified:

```bash
# Build the lib test binary once.
cargo test --lib --no-run

# Run 6-8 copies of the compiled binary in parallel, several rounds,
# while the host is under CPU + fork/exec load.
BIN=$(find target/debug/deps -maxdepth 1 -type f -executable -name 'simard-*' \
        ! -name '*.d' -printf '%T@ %p\n' | sort -rn | head -1 | cut -d' ' -f2-)
for round in 1 2 3 4 5; do
  for copy in $(seq 1 8); do "$BIN" --test-threads="$(nproc)" & done
  wait
done
```

**Pass criterion:** zero failures across every copy and every round, and the
originally-failing tests green under a plain `cargo test` on a saturated host:

- `concurrent_appends_never_interleave_or_drop_entries` (cost-ledger durability),
- the self-metrics concurrency test (metrics-stream durability),
- `create_issue_reports_nonzero_exit_and_stderr_without_body_content` (`gh_client`
  spawn),
- the Bash-tool `tool_executor` spawn tests, and
- `concurrent_dispatch_parallelizes_and_respects_cap` (dispatch concurrency).

The gate is enforced in CI. It never uses `--admin` or `--no-verify`.
