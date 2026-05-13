# Engineer Copilot Subprocess Permissions

When the engineer loop dispatches work to a Copilot CLI subprocess
(`AgentKind::Copilot`), the subprocess runs **non-interactively** — there is no
TTY for it to ask the operator "approve write to file X?". To make that
non-interactive mode useful for an engineer (which must write files, run
`git commit`, and call `gh pr create`), Simard pre-grants the Copilot CLI's
write- and tool-permission flags at spawn time.

This page documents the exact flags and environment variables passed, why each
one is necessary, the security boundary they live inside, and how to verify
the contract in tests.

## Background: the symptom this contract prevents

Without this contract, every dispatched Copilot engineer produced a thoughtful
plan and then ended its log with a permission-denied table:

```
echo > file                      Permission denied and could not request permission from user
tee                              Permission denied and could not request permission from user
python3 -c "open(...)"           Permission denied and could not request permission from user
gh issue create                  Permission denied and could not request permission from user
gh pr create                     Permission denied and could not request permission from user
gh api                           Permission denied and could not request permission from user
git checkout -b                  Permission denied and could not request permission from user
git commit                       Permission denied and could not request permission from user
git push                         Permission denied and could not request permission from user
amplihack recipe run             Permission denied and could not request permission from user
```

The Copilot CLI's tool-allow-list defaults to interactive prompting; with
no TTY attached the prompt fails closed. The flags below switch the
allow-list to "auto-approve every tool" for the lifetime of the subprocess,
which is the only mode that produces useful work in headless dispatch.

## The contract

Every Copilot subprocess spawned through
`run_engineer_subprocess(prompt, workspace, AgentKind::Copilot)` receives:

### Argv flags (in fixed order)

| Position                | Flag                  | Purpose                                                                   |
| ----------------------- | --------------------- | ------------------------------------------------------------------------- |
| 1 (after `copilot` sub) | `--allow-all-tools`   | Auto-approve every tool invocation (writes, shell, gh, git, amplihack…). |
| 2                       | `--allow-all-paths`   | Auto-approve filesystem reads/writes across the workspace.                |
| 3                       | `-p`                  | Inline prompt switch (Copilot's "prompt" flag, no `--` separator).        |
| 4                       | `<prompt>`            | The engineer prompt text.                                                 |

The order is **load-bearing**: both `--allow-all-tools` and
`--allow-all-paths` MUST appear *before* `-p`, otherwise the Copilot CLI's
arg parser treats them as positional content of the prompt itself.

### Environment variables

| Name                  | Value | Purpose                                                                                  |
| --------------------- | ----- | ---------------------------------------------------------------------------------------- |
| `COPILOT_ALLOW_ALL`   | `1`   | Belt-and-suspenders fallback. Set in addition to (not instead of) `--allow-all-tools`, on the hypothesis that an upstream Copilot CLI release may rename or restructure the CLI flag while keeping the env knob stable. If a future upstream version honors this env var, the contract degrades gracefully without an emergency redeploy; if it does not, the explicit argv flag is still the authoritative grant. |

All other environment from the parent (notably `GITHUB_TOKEN`,
`COPILOT_TOKEN`, `PATH`, and any `SIMARD_*` knobs) is inherited unchanged via
`Command::env`'s default behavior — the new env entry is *additive*, not
replacing.

### What is *not* added

- ❌ No `--auto`. Copilot CLI rejects this flag (it is a RustyClawd-only switch).
- ❌ No `--max-turns`. Copilot CLI does not accept this flag.
- ❌ No `--` separator before `-p`. Copilot's parser does not require it and
  rejects the form.
- ❌ No new authentication tokens or credentials in argv or env.
- ❌ No `sudo`, no `setuid`, no `chroot`, no privilege escalation.

## Security boundary

Granting `--allow-all-tools` widens what the subprocess may *attempt*, not
what it may *succeed at*. The actual blast radius is bounded by three layers
that are **not** modified by this contract:

1. **Filesystem scope** — the Copilot subprocess inherits
   `current_dir(workspace)` from `Command`, where `workspace` is the
   per-engineer worktree path constructed by the engineer loop. The shell
   the subprocess spawns inherits that working directory. Writes outside the
   worktree still go through the OS as the engineer-loop UID, so any
   destructive command (`rm -rf /`) is constrained by ordinary Unix
   permissions on the calling user, not by Copilot's allow-list.
2. **Process identity** — the subprocess runs under the same UID as the
   engineer loop (typically the `simard-ooda` daemon's user). No
   privilege escalation flag is added.
3. **Network egress** — the subprocess uses the parent process's existing
   `gh` and Copilot CLI authentication. No new tokens are minted, exposed
   in argv, or written to disk.

In other words: `--allow-all-tools` removes the *interactive prompt*; it does
not remove the *operating-system permissions check*. A misbehaving prompt
cannot, for example, write to `/etc/shadow` or push to a repository whose
remote requires credentials the engineer loop does not hold.

## API surface

### `engineer_argv(kind, prompt, max_turns) -> Vec<String>`

`pub(crate)` helper in `src/engineer_loop/agent_spawn.rs`. For the
`AgentKind::Copilot` arm it returns:

```rust
vec![
    "copilot".to_string(),          // kind.subcommand()
    "--allow-all-tools".to_string(),
    "--allow-all-paths".to_string(),
    "-p".to_string(),
    prompt.to_string(),
]
```

The `max_turns` parameter is accepted for trait uniformity with the
`RustyClawd` arm but intentionally ignored by the Copilot arm (Copilot CLI
does not expose a turn cap).

### `run_engineer_subprocess(prompt, workspace, kind) -> SimardResult<String>`

`pub(crate)` helper in `src/engineer_loop/agent_spawn.rs`. For
`AgentKind::Copilot` it builds a `Command` that:

- spawns `amplihack copilot …` (resolved via `amplihack_binary()`),
- chains the argv from `engineer_argv` above,
- sets `current_dir(workspace)`,
- sets `env("COPILOT_ALLOW_ALL", "1")`,
- captures stdout and stderr through `Stdio::piped()`,
- enforces the `AGENT_SESSION_TIMEOUT_SECS` deadline,
- returns the trailing `SUMMARY_TAIL_BYTES` of stdout (or stderr, if
  stdout is empty) on success.

Failure modes — spawn error, non-zero exit, timeout — surface as
`SimardError::ActionExecutionFailed` and `SimardError::CommandTimeout`
respectively, with the `action` field set to `"<bin> copilot"` for
greppability.

## Configuration

There is **no operator-facing knob** for these flags. The contract is
hard-coded in `engineer_argv` and `run_engineer_subprocess` because:

- Disabling the flags leaves the engineer subprocess unable to do useful work
  (every write is denied), which would silently regress to the
  pre-fix behavior.
- Narrowing the flags (e.g. allowing only `git` and `gh` but not arbitrary
  tools) is not expressible in the current Copilot CLI version's allow-list
  grammar.

If a future operator needs a narrower scope, the right place to express it
is a new `AgentKind` variant (e.g. `AgentKind::CopilotReadOnly`) with its own
argv shape — not a runtime flag on the current `Copilot` variant.

`SIMARD_ENGINEER_AGENT` (existing knob, see `AgentKind::from_env` at
`src/engineer_loop/agent_spawn.rs:74-99`) still selects between
`Copilot` (default) and `RustyClawd`. Switching to `RustyClawd` bypasses
this contract entirely because RustyClawd uses its own argv shape
(`--auto --subprocess-safe --no-reflection --max-turns N -- -p <prompt>`) and
does not need the Copilot allow-list flags.

## Test contract

The contract is pinned by three layers of tests in
`src/engineer_loop/agent_spawn.rs` (unit) and
`tests/engineer_copilot_permissions.rs` (integration).

### Unit: argv shape

```rust
// in src/engineer_loop/agent_spawn.rs tests mod
#[test]
fn engineer_argv_copilot_uses_p_without_dash_separator() {
    let argv = engineer_argv(AgentKind::Copilot, "do the thing", 5);
    assert_eq!(argv[0], "copilot");
    assert!(argv.contains(&"--allow-all-tools".to_string()));
    assert!(argv.contains(&"--allow-all-paths".to_string()));
    assert!(argv.contains(&"-p".to_string()));
    assert!(!argv.contains(&"--".to_string()),       "no -- separator");
    assert!(!argv.contains(&"--auto".to_string()),   "no --auto for Copilot");
    assert!(!argv.contains(&"--max-turns".to_string()), "no --max-turns for Copilot");
}

#[test]
fn engineer_argv_copilot_grants_tool_permissions_for_non_interactive() {
    let argv = engineer_argv(AgentKind::Copilot, "p", 1);
    let tools = argv.iter().position(|a| a == "--allow-all-tools").unwrap();
    let paths = argv.iter().position(|a| a == "--allow-all-paths").unwrap();
    let p     = argv.iter().position(|a| a == "-p").unwrap();
    assert!(tools < paths, "--allow-all-tools must precede --allow-all-paths");
    assert!(paths < p,     "permission flags must precede -p");
}
```

### Integration: stub-shim observation

`tests/engineer_copilot_permissions.rs` puts a stub `amplihack` shim on
`PATH` that records its argv and a whitelisted slice of its env to a
`observations.log` file under `TempDir`, then drives
`run_engineer_subprocess`. The integration test is `#[serial_test::serial]`
because it mutates `PATH`.

Asserted observations:

- `argv` contains the literal string `--allow-all-tools`.
- `argv` contains the literal string `--allow-all-paths`.
- `env COPILOT_ALLOW_ALL=1` is present.
- The stub exited 0 (i.e. the parent did not crash before spawning).

### Integration: end-to-end write/commit/PR smoke

A second integration test in the same file constructs a temp git repo with
stub `gh` and `git` shims on `PATH`, then exercises a minimal engineer
flow that:

1. writes a file inside the worktree,
2. runs `git commit -am "..."`,
3. runs `gh pr create --title "..." --body "..."` (the stub echoes a fake PR
   URL),

and asserts all three calls reach the stubs and exit 0. This is the
regression test for the original "engineers produce plans but zero PRs"
symptom.

## Examples

### Example 1 — reading the contract for a new engineer call site

```rust
use crate::engineer_loop::agent_spawn::{
    AgentKind, run_engineer_subprocess,
};

let prompt = "Read CHANGELOG.md and append a 0.18.0 entry";
let workspace = std::path::Path::new("/home/azureuser/src/Simard/worktrees/feat-x");
let summary = run_engineer_subprocess(prompt, workspace, AgentKind::Copilot)?;

// `summary` is the trailing tail of the Copilot subprocess's stdout.
// All writes inside `workspace`, `git commit`, and `gh pr create` will
// succeed without prompting because of --allow-all-tools / --allow-all-paths
// / COPILOT_ALLOW_ALL=1.
println!("engineer finished:\n{summary}");
```

### Example 2 — verifying the live contract from an interactive shell

```bash
# Confirm the flags Copilot CLI actually accepts.
copilot --help | grep -E -- '--allow-all(-tools|-paths)?'

# Expected (abbreviated):
#   --allow-all-tools     Allow all tool invocations (required for non-interactive mode)
#   --allow-all-paths     Allow filesystem access to all paths

# Smoke-test that setting the env var does not itself break startup.
# (Whether the env var is *honored* by a given upstream release is not
# directly observable from --help; the contract relies on the argv flag
# as authoritative and treats the env var as a forward-looking fallback.)
COPILOT_ALLOW_ALL=1 copilot --help | head -1
```

### Example 3 — running just the regression tests locally

```bash
cd /path/to/Simard/worktrees/fix-engineer-copilot-allow-all-tools

# Unit tests (fast, no PATH mutation).
cargo test --lib -- engineer_loop::agent_spawn

# Stub-shim integration tests (serial, mutates PATH inside its own scope).
cargo test --test engineer_copilot_permissions

# Full workspace gate.
cargo check --workspace --tests --quiet
```

### Example 4 — debugging a "engineer produced no PR" report

If a future engineer dispatch *still* ends with permission-denied lines,
walk this checklist:

1. Confirm the dispatched subprocess is actually Copilot, not RustyClawd:
   ```bash
   echo "$SIMARD_ENGINEER_AGENT"   # blank or "copilot" → Copilot
   ```
2. Confirm the live `amplihack` binary builds Copilot argv with the new
   flags:
   ```bash
   strings "$(which amplihack)" | grep -E -- '--allow-all-(tools|paths)'
   ```
3. Confirm the contract is intact in the source tree:
   ```bash
   grep -nE -- '--allow-all-tools|COPILOT_ALLOW_ALL' \
       src/engineer_loop/agent_spawn.rs
   ```
4. If all three pass and writes still fail, the upstream Copilot CLI has
   likely renamed the flag — bump the contract here and update the tests in
   the same PR. The `COPILOT_ALLOW_ALL=1` env var is the fallback that
   should keep the system limping along until that fix lands.

## Cross-references

- **Code:** `src/engineer_loop/agent_spawn.rs` — `engineer_argv`,
  `run_engineer_subprocess`, `AgentKind::from_env`.
- **Tests:** `src/engineer_loop/agent_spawn.rs` (`tests` mod),
  `tests/engineer_copilot_permissions.rs`.
- **Related how-to:**
  [Spawn engineers from the OODA daemon](../howto/spawn-engineers-from-ooda-daemon.md),
  [Inspect and clean engineer worktrees](../howto/inspect-and-clean-engineer-worktrees.md).
- **Related reference:**
  [Engineer Loop argv Sanitization](engineer-loop-argv-sanitization.md),
  [Engineer Worktree Isolation](engineer-worktree-isolation.md).
- **Stored memory:** "engineer agent default" — Copilot is the default
  engineer agent kind; RustyClawd remains supported via
  `SIMARD_ENGINEER_AGENT=rustyclawd`.
