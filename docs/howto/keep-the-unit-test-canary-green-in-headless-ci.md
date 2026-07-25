---
title: How to keep the unit-test canary green in headless CI
description: Explains why the deploy gate's `unit-test` canary reddened on the meeting-greeting and gym-list e2e checks in a headless (no-LLM, no-memory-backend) context, and documents the shipped fix — emit the greeting banner before the fallible cognitive-memory launch so `Simard v…` always reaches stderr, and the gym-list determinism guarantee that keeps `simard gym list` output pinned to the scenario roster.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: howto
status: active
related:
  - ./converge-a-stuck-red-canary-self-deploy.md
  - ../testing/ci-resilient-test-patterns.md
  - ../reference/canary-gate-convergence.md
  - ./start-a-meeting.md
  - ./run-the-coin-gym-harness.md
---

# How to keep the unit-test canary green in headless CI

> **Status: active.** This describes shipped behaviour: the greeting banner
> emits **before** the fallible cognitive-memory launch in
> `meeting_session::run_meeting_repl_command`, so the `unit-test` deploy-gate
> canary is deterministic in a headless context. It also documents the
> gym-list roster determinism the `gym_list_shows_all_scenarios` check relies
> on. For the surrounding convergence runbook, see
> [How to converge a stuck red-canary self-deploy](./converge-a-stuck-red-canary-self-deploy.md).

Use this page when the deploy gate refuses every self-deploy with
`failing_gate="unit-test"` and the un-truncated gate output names
`meeting_repl_shows_greeting` and/or `gym_list_shows_all_scenarios`. Both are
integration tests in `tests/e2e_engineer_external_repo.rs` that exec the
compiled `target/debug/simard` binary; the gate labels the canary `unit-test`
regardless of where the tests live.

## 1. Confirm the symptom

The signature is an identical `unit-test` refusal on every tick with
`DeployDrift` climbing:

```bash
journalctl --user -u simard -o cat \
  | grep -E 'overseer::deploy' | tail -n 20
```

```
WARN overseer::deploy: self-deploy refused by deploy gate
    failing_gate=unit-test
    failing_detail="2 tests failed (exit 101 of 9366): \
        meeting_repl_shows_greeting, gym_list_shows_all_scenarios"
    refusal="red canary (gate unit-test: 2 tests failed)"
```

This is the **genuine-regression** row of the
[convergence decision table](./converge-a-stuck-red-canary-self-deploy.md#3-decide-genuine-regression-vs-missing-signal):
fix the failing behaviour at its origin so the canary goes green legitimately.
Do **not** weaken, skip, or delete the assertions to force a pass.

## 2. Why the greeting check reddened

`meeting_repl_shows_greeting` runs `simard meeting repl integration-test` and
asserts that stderr contains `Simard v` (or `simard`):

```rust
// tests/e2e_engineer_external_repo.rs
assert!(
    stderr.contains("Simard v") || stderr.contains("simard"),
    "meeting REPL should show Simard greeting:\n{stderr}"
);
```

In a headless canary/CI context there is **no LLM provider and no
cognitive-memory backend**. The pre-fix ordering launched memory first and only
printed the banner afterwards:

```rust
// BEFORE (reddens in headless CI)
let memory = launch_real_meeting_client()?;   // fallible → early return in CI
print_greeting_banner(Some(&*memory));         // banner never reached
```

`launch_real_meeting_client()` fails when the memory IPC daemon / native writer
is unavailable, so `?` returns **before** the banner is emitted. stderr never
contains `Simard v`, and the assertion fails — a deterministic red, not a flake.

The greeting text itself did **not** drift: `build_greeting_banner` still emits
`🌲 Simard v{CARGO_PKG_VERSION}` as its first line. The regression was purely
**ordering** — a hard error was allowed to pre-empt the banner.

## 3. The shipped fix: banner before the fallible launch

`run_meeting_repl_command` now launches the backend as an **`Option`** (the
fallible call no longer early-returns), emits the greeting banner **before** the
memory requirement is enforced, and only then re-requires memory via
`ok_or(...)?`. Passing `memory.as_deref()` keeps the rich memory-backed banner
in the success path while still emitting the version-only banner in headless CI:

```rust
// AFTER (deterministic in headless CI)
pub fn run_meeting_repl_command(topic: &str) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Attempt the real backend as an Option — failure is logged (sanitized)
    //    via structured tracing but does NOT early-return yet, so the banner
    //    below is always reached.
    let memory = match launch_real_meeting_client() {
        Ok(memory) => Some(memory),
        Err(error) => {
            tracing::warn!(error = %error, "meeting memory backend unavailable");
            None
        }
    };

    // 2. Greeting is unconditional — it must reach stderr even when no
    //    memory/LLM backend exists (deploy-gate `unit-test` canary depends on
    //    this). `print_greeting_banner` already tolerates `None`, and
    //    `as_deref()` yields the live backend on the success path so the
    //    richer memory-backed sections are still shown.
    print_greeting_banner(memory.as_deref());

    // 3. Re-require memory: fail closed if the backend was unavailable. The
    //    meeting REPL contract (memory required) is preserved.
    let memory = memory.ok_or_else(|| -> Box<dyn std::error::Error> {
        "meeting REPL requires a cognitive-memory backend".into()
    })?;
    tracing::info!("Cognitive memory active");

    // …unchanged: agent session, live context, REPL loop…
}
```

> **Note on `print_greeting_banner`'s parameter type.** The banner takes an
> `Option<&dyn CognitiveMemoryOps>`; `memory.as_deref()` produces exactly that
> from `Option<Box<dyn CognitiveMemoryOps>>`. Confirm the coercion compiles —
> if `as_deref()` does not infer the trait-object target, use
> `memory.as_deref().map(|m| m as &dyn CognitiveMemoryOps)` or an explicit
> `Option<&dyn CognitiveMemoryOps>` binding.

Guarantees this preserves:

- **No new `print!`/`println!`/`eprintln!` in the ordering fix.** The banner
  reuses the existing `writeln!`-to-stderr sink in `print_greeting_banner`; the
  new failure path uses `tracing::warn!` (structured tracing + OTel only). The
  pre-existing `eprintln!` calls in `open_meeting_agent_session` (provider-config
  errors) and the final `println!("Meeting closed.")` are unrelated to this fix
  and are left untouched.
- **No silent fallback.** The meeting REPL still *requires* memory — the
  `Err` is logged, the launch result becomes `None`, and `ok_or(...)?`
  hard-errors immediately after the banner. Banner-first is deterministic
  output, not a permissive degrade.
- **Sanitized diagnostics.** The `tracing::warn!` carries the error category
  only — never state-root paths, IPC socket names, backend hostnames, or stack
  traces (see the security invariants below).
- **Success path unchanged.** When a backend is present, `launch_real_meeting_client`
  returns `Some(memory)`, `print_greeting_banner(memory.as_deref())` receives the
  live backend, the richer memory-backed banner sections render in the same
  session, and the REPL behaves exactly as before.

Because the memory-backed banner (known projects, active goals, memory stats)
needs a live backend, those sections are only populated when `memory.as_deref()`
is `Some`. In headless CI the `None` path is shown, which still includes the
version header the canary asserts on.

## 4. Why the gym-list check reddened (and its determinism guarantee)

`gym_list_shows_all_scenarios` runs `simard gym list` and asserts the roster:

```rust
assert!(stdout.contains("repo-exploration-deep-scan"));
assert!(stdout.contains("doc-generation-public-fn"));
assert!(stdout.contains("safe-code-change-add-derive"));
assert!(stdout.contains("session-quality-memory-export"));
assert!(stdout.contains("interactive-terminal-driving"));
```

All five scenario IDs are present in the roster
(`src/gym/scenarios/data.rs`) and `run_gym_list` prints them to stdout. When
this check reddened alongside the greeting check it was because the canary
exercised a **stale binary** — the gate ran against a build that predated the
current roster — not because the roster drifted. Rebuilding the binary as part
of the gate clears the stale-artifact hypothesis, and the empirical
`simard gym list` output then matches the asserted IDs.

Determinism contract for the gym list:

- `simard gym list` prints the full scenario roster to **stdout**, one entry
  per scenario, using the scenario's stable ID.
- The five IDs above are the canonical baseline roster and MUST remain listable.
- Adding a scenario is additive — extend `src/gym/scenarios/data.rs` **and**
  the assertion. Removing or renaming one is a breaking change to the roster
  contract and must update the assertion in the same change.
- The assertion is a genuine authorization control on the roster; never weaken
  it to force green.

## 5. Verify

Integration tests exec the compiled binary, so **build first**:

```bash
# 1. Build the binary the e2e tests exec.
cargo build

# 2. The two named checks now pass (exit 0, not 101).
cargo test --test e2e_engineer_external_repo meeting_repl_shows_greeting
cargo test --test e2e_engineer_external_repo gym_list_shows_all_scenarios

# 3. Greeting-banner unit tests still green (banner text unchanged).
cargo test -p simard greeting_banner

# 4. Full lib suite — no exit 101.
cargo test --locked -p simard --lib

# 5. The backend-launch + banner ordering uses tracing, not new print sinks.
#    (The pre-existing eprintln! in open_meeting_agent_session and the final
#    println!("Meeting closed.") are unrelated to this fix.)
grep -n 'tracing::warn!.*memory backend unavailable' src/operator_commands_meeting/meeting_session.rs
```

## 6. Confirm the loop converges

Once the canary passes, confirm the self-deploy advances past the stuck SHA:

```bash
# Every gate line reports "gate passed"; no "gate reddened the canary".
journalctl --user -u simard -o cat | grep 'self_relaunch::gate' | tail -n 8

# DeployDrift returns to 0 and the running commit catches up to main.
simard status | grep -Ei 'deploy_drift|running_commit'
```

The Overseer should emit `deploy_starting` → successful swap instead of another
`deploy_refused` on the same commit. If gates are green but the loop still
refuses, a **different** gate is red — return to the
[convergence runbook, step 2](./converge-a-stuck-red-canary-self-deploy.md#2-read-which-gate-is-red--do-not-re-run-blindly).

## Security invariants

- **Public info only.** The `None`-path banner and the `warn!` on backend
  failure emit only public data (version, source-file count, GitHub issue/PR
  counts). No state-root paths, IPC socket names, backend hostnames, or stack
  traces.
- **Fail closed.** The meeting REPL still requires memory; the ordering fix
  must never become a permissive silent fallback — the `Err` is returned after
  logging.
- **Sanitize the warn.** `tracing::warn!(error = %error, …)` carries the error
  category/message, not full path-bearing IO errors from
  `memory_ipc::launch_writer_client`.
- **No new attack surface.** No added input parsing, file reads, or network
  calls; the `topic` argument flow is unchanged.

## Related reading

- [How to converge a stuck red-canary self-deploy](./converge-a-stuck-red-canary-self-deploy.md) —
  the general runbook and the regression-vs-missing-signal decision table.
- [CI-resilient test patterns](../testing/ci-resilient-test-patterns.md) —
  how to make binary-driven e2e checks deterministic in headless CI.
- [Canary gate isolation and self-deploy convergence](../reference/canary-gate-convergence.md) —
  per-gate spans, `canary_env`, and the preserved fail-closed invariants.
- [How to start a meeting](./start-a-meeting.md) — the meeting REPL from an
  operator's perspective, including the greeting banner.
- [How to run the coin gym harness](./run-the-coin-gym-harness.md) — the gym
  scenarios exercised by `simard gym list`.
