# Grant the Engineer Subprocess Write Permissions

> **Audience:** Operators wiring new engineer dispatch paths or debugging an
> engineer that is producing plans but never PRs.
>
> **You usually do not need this guide.** The default
> `AgentKind::Copilot` dispatch already grants the right permissions
> automatically. Read on if you are adding a *new* call site, building a
> *new* `AgentKind`, or chasing a regression of the original
> "every engineer ends with permission-denied" bug.

## TL;DR

Spawn the Copilot engineer subprocess through the existing helper —
`run_engineer_subprocess(prompt, workspace, AgentKind::Copilot)` from
`src/engineer_loop/agent_spawn.rs`. It already passes
`--allow-all-tools --allow-all-paths` and sets `COPILOT_ALLOW_ALL=1`,
which together let the subprocess write files, run `git commit`, run
`gh pr create`, and invoke `amplihack recipe run` non-interactively.

If you must reconstruct the contract in a new code path (e.g. a custom
dispatcher in `src/copilot_task_submit/` or a new
`operator_commands_*` surface), copy these four pieces — the order matters:

```rust
use std::process::{Command, Stdio};

let mut cmd = Command::new(amplihack_binary());
cmd.arg("copilot")
   .arg("--allow-all-tools")   // 1. tool allow-list
   .arg("--allow-all-paths")   // 2. filesystem allow-list
   .arg("-p")                  // 3. prompt flag (no `--` separator!)
   .arg(prompt)                // 4. prompt text
   .current_dir(workspace)     // worktree-scoped
   .env("COPILOT_ALLOW_ALL", "1") // belt & suspenders fallback
   .stdin(Stdio::null())
   .stdout(Stdio::piped())
   .stderr(Stdio::piped());
let child = cmd.spawn()?;
```

That is the entire contract. Everything else (timeout, output capture,
error mapping) is bookkeeping.

## When the engineer was producing plans but no PRs

The pre-fix dispatch passed only `--allow-all-paths`, which let the
subprocess *read* freely but left every *write* tool gated on an
interactive prompt that, with no TTY, failed closed. The fix is the
addition of `--allow-all-tools` (and the env-var twin
`COPILOT_ALLOW_ALL=1`).

Symptom you would see in `~/.simard/wip-snapshots/`:

```
amplihack-hygiene-plan-20260512T231555Z.md  ← thoughtful plan
                                              + permission-denied table
```

Cure: ensure your dispatch path is using `run_engineer_subprocess` and
that the live `amplihack` binary on the daemon's PATH was built from a
revision containing the contract. Walk the checklist in
[Engineer Copilot Subprocess Permissions §Example 4](../reference/engineer-copilot-permissions.md#example-4--debugging-a-engineer-produced-no-pr-report).

## Step-by-step: add a new engineer call site

1. Pick a workspace directory the subprocess is allowed to write inside
   (the engineer-loop convention is a per-engineer worktree under
   `worktrees/<branch>`). Construct it with `git worktree add` if it
   does not already exist; do **not** point at the shared
   `worktrees/main`.
2. Build your prompt as a single `String`. The contract sanitizes
   newlines for `gh` argv but otherwise passes the prompt verbatim;
   keep secrets out.
3. Call:
   ```rust
   let summary = run_engineer_subprocess(
       &prompt,
       workspace.as_path(),
       AgentKind::Copilot,
   )?;
   ```
4. Persist `summary` to your call site's log (`~/.simard/wip-snapshots/`
   is the engineer-loop convention).
5. Surface failures by `?`-propagating the `SimardError` — the
   `ActionExecutionFailed { action, reason }` variant already includes
   the stderr tail.

## Step-by-step: write a regression test for a new call site

The contract has two test entry points; mirror them.

### Unit-level pin

Add a test alongside your dispatcher that asserts your argv builder
emits `--allow-all-tools` *before* `--allow-all-paths` *before* `-p`:

```rust
#[test]
fn my_dispatcher_grants_copilot_write_permissions() {
    let argv = my_dispatcher::build_argv("p");
    let t = argv.iter().position(|a| a == "--allow-all-tools").unwrap();
    let pa = argv.iter().position(|a| a == "--allow-all-paths").unwrap();
    let p = argv.iter().position(|a| a == "-p").unwrap();
    assert!(t < pa && pa < p, "ordering load-bearing for Copilot CLI parser");
}
```

### Integration-level pin

Use the same stub-shim pattern as
`tests/engineer_copilot_permissions.rs`:

1. `TempDir`-scope a fake `bin/amplihack` script that writes
   `"$@"` and `printenv COPILOT_ALLOW_ALL` to `observations.log`.
2. Prepend that `bin/` to `PATH`.
3. Annotate the test `#[serial_test::serial]` because PATH is process-global.
4. Drive your dispatcher.
5. Assert `observations.log` contains both the flags and
   `COPILOT_ALLOW_ALL=1`.
6. Let `TempDir` clean itself up via `Drop`.

Do **not** install the stub into `~/.local/bin` — that would leak into
unrelated tests and into the live daemon.

## Step-by-step: narrow the permissions for a read-only audit dispatcher

If you need a Copilot subprocess that must *not* write — say, a code
auditor — do **not** mutate the existing `Copilot` arm. Instead:

1. Add a new variant to `AgentKind` (e.g. `CopilotReadOnly`).
2. In `engineer_argv`, give it its own argv shape that omits
   `--allow-all-tools` (keep `--allow-all-paths` if you want broad
   reads).
3. In `run_engineer_subprocess`, branch on `kind` to avoid setting
   `COPILOT_ALLOW_ALL=1` for the read-only kind.
4. Add unit + integration tests that pin the *absence* of
   `--allow-all-tools` for the new kind.
5. Wire `SIMARD_ENGINEER_AGENT=copilot-readonly` into
   `AgentKind::from_env` if you want operator selection.

This keeps the security boundary explicit (one kind = one allow-list
shape) instead of hidden behind a runtime flag.

## What you must *not* do

- ❌ Do not pass `--auto` to Copilot. It is a RustyClawd-only flag and
  Copilot's parser rejects it.
- ❌ Do not pass `--max-turns` to Copilot. Same reason.
- ❌ Do not insert a `--` separator before `-p`. Copilot's parser does
  not require it.
- ❌ Do not set `current_dir` outside the engineer's intended worktree —
  `--allow-all-paths` widens what Copilot will *attempt*, not what the
  filesystem will *permit*, but a misrouted `current_dir` makes that
  distinction much smaller.
- ❌ Do not stuff `GITHUB_TOKEN` into argv "for convenience". Inherit
  the parent env; Copilot already knows where to look.
- ❌ Do not commit `.github/hooks/amplihack-hooks.json` rewrites — the
  amplihack init step auto-rewrites it on every session start; discard
  the diff with `git checkout -- .github/hooks/amplihack-hooks.json`
  before staging.
- ❌ Do not edit the shared `worktrees/main` directly while the
  `simard-ooda` daemon is running. Branch into a fresh worktree
  (`git worktree add worktrees/<branch>`); concurrent agents share
  `worktrees/main` and your edits will race with theirs.

## Verification checklist

Before opening a PR that touches an engineer dispatch path:

- [ ] `cargo check --workspace --tests --quiet` exits 0 with no new
      warnings.
- [ ] `cargo test --lib -- engineer_loop::agent_spawn` is green.
- [ ] `cargo test --test engineer_copilot_permissions` is green.
- [ ] Manual: `copilot --help | grep -- '--allow-all-tools'` confirms
      the flag still exists upstream.
- [ ] Manual: a smoke run in a throwaway worktree produces a real PR
      URL (or, if your test is mocked, the stub `gh` echoed one).
- [ ] PR description cites the symptom evidence (a permission-denied
      table from a prior wip-snapshot, or the test output) so reviewers
      can see what the fix prevents.

## Cross-references

- [Engineer Copilot Subprocess Permissions](../reference/engineer-copilot-permissions.md)
  — the authoritative reference for flag order, env vars, and the
  security boundary.
- [Spawn engineers from the OODA daemon](spawn-engineers-from-ooda-daemon.md)
  — how the daemon decides *when* to dispatch.
- [Use Agent Orchestration: Engineer Loop](use-agent-orchestration-engineer-loop.md)
  — high-level orchestration model.
- [Inspect and Clean Engineer Worktrees](inspect-and-clean-engineer-worktrees.md)
  — what to do with the worktrees the engineer leaves behind.
