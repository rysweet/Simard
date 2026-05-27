---
title: "Config-driven subprocess environment propagation"
description: Design rationale for reading ~/.simard/config.toml to set AMPLIHACK_AGENT_BINARY on recipe-runner-rs subprocesses, replacing the unreliable env-var-only approach.
last_updated: 2026-05-27
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/subprocess-env-propagation.md
  - ../howto/configure-llm-provider.md
  - ../reference/disk-health-api.md
  - ./truthful-runtime-metadata.md
---

# Config-driven subprocess environment propagation

On 2026-05-27, Simard's recipe-runner shims (decide, orient, engineer
lifecycle, merge judge, progress checker, and disk health) were invoking
`recipe-runner-rs` without setting `AMPLIHACK_AGENT_BINARY` in the child
process environment. The recipe runner defaulted to `claude`, which is the
wrong binary on Copilot-based deployments.

This document explains the problem, the design of the fix, and why
`~/.simard/config.toml` is the single source of truth.

## The problem

### Environment variables don't survive process boundaries

Simard launches `recipe-runner-rs` as a subprocess from six different Rust
shims. Each shim calls `Command::new("recipe-runner-rs")` and passes recipe
context via `-c` flags. None of them set `AMPLIHACK_AGENT_BINARY` on the
child process.

The recipe runner reads `AMPLIHACK_AGENT_BINARY` to decide which agent
binary to use. When the variable is absent, it defaults to `claude` — a
binary that does not exist on Copilot-based deployments. The result: every
recipe invocation fails with "binary not found" unless the operator
manually exports the variable in their shell.

This is fragile for three reasons:

1. **tmux does not propagate env** — The OODA daemon often runs inside
   tmux. `tmux new-session` does not forward parent env vars to the
   server-attached pane. An operator who sets `AMPLIHACK_AGENT_BINARY=copilot`
   in their shell sees it disappear when the daemon starts.

2. **systemd does not propagate env** — Production daemons run under
   systemd. The unit file must explicitly list every env var. Missing one
   means silent failure at runtime, not at deployment time.

3. **Subprocess spawning is deep** — Simard spawns `recipe-runner-rs`,
   which may itself spawn further processes. Each hop is another opportunity
   for env vars to vanish. A config file read at the point of use is
   reliable across any depth of subprocess nesting.

### The existing config.toml already had the answer

`RuntimeConfig::load()` reads `~/.simard/config.toml` and resolves the
`llm_provider` field through a well-defined precedence chain:

1. `SIMARD_LLM_PROVIDER` env var (wins when set)
2. `~/.simard/config.toml` (used when env unset)
3. Error (no silent default)

This mechanism already existed and was used by `SessionBuilder` and
`LlmProvider::resolve()`. The recipe-runner shims simply weren't using it.

## Design

### One new method: `LlmProvider::agent_binary_value()`

Each `LlmProvider` variant maps to a binary name:

| `LlmProvider` variant | `agent_binary_value()` | Meaning                     |
| ---------------------- | ---------------------- | --------------------------- |
| `Copilot`              | `"copilot"`            | GitHub Copilot SDK via `gh` |
| `RustyClawd`           | `"rustyclawd"`         | Anthropic / RustyClawd      |

The method returns `&'static str` — no allocation, no user input, no
injection risk. The mapping matches `to_toml_string()` in `runtime_config.rs`.

### Each shim reads config once at construction

The five struct-based shims (`RecipeDecideBrain`, `RecipeOrientBrain`,
`RecipeEngineerLifecycleBrain`, `RecipeMergeJudge`, `RecipeProgressChecker`)
store an `agent_binary: &'static str` field. The field is populated in `new()` by:

```rust
let agent_binary = RuntimeConfig::load()
    .ok()?
    .llm_provider
    .agent_binary_value();
```

If config loading fails (no env var AND no config.toml), `new()` returns
`None` — the same behaviour as when the recipe binary is not found. The
caller falls back to the deterministic brain, which is the correct
degradation path.

The stored field is then applied to both Commands (version check and
execution):

```rust
Command::new("recipe-runner-rs")
    .env("AMPLIHACK_AGENT_BINARY", &self.agent_binary)
    // ... existing args ...
```

### disk_health.rs propagates with `?`

`run_disk_health_check` is a function, not a struct. It loads config at
call time and propagates failure via `?`:

```rust
let agent_binary = RuntimeConfig::load()?.llm_provider.agent_binary_value();
```

A config failure here returns `SimardResult::Err`, which the daemon handles
with its existing warn-and-continue pattern. This is the correct behaviour —
if config is missing, the operator needs to know, not have the system guess.

## Why not just set the env var?

Three alternatives were considered and rejected:

### Alternative 1: Set `AMPLIHACK_AGENT_BINARY` globally at daemon startup

This would use `std::env::set_var()` in `main()`. Rejected because:

- `set_var` modifies the **parent** process environment, not just the child.
  This is a safety hazard in multi-threaded code.
- It doesn't help subprocesses launched through tmux or systemd —
  `set_var` only affects the current process tree.

### Alternative 2: Read env var in each shim

This would check `std::env::var("AMPLIHACK_AGENT_BINARY")` in each shim.
Rejected because:

- The env var might not be set (the original problem).
- We'd need a fallback, and the fallback would be... reading config.toml.
  So we'd have two codepaths for the same information.
- `RuntimeConfig::load()` already does the env → config → error resolution.

### Alternative 3: Pass `AMPLIHACK_AGENT_BINARY` through recipe context vars

This would use `-c agent_binary=copilot` instead of `.env()`. Rejected
because:

- `AMPLIHACK_AGENT_BINARY` is an environment variable consumed by the
  recipe runner itself, not a recipe-level context variable.
- The recipe runner reads its own env, not its context vars, for this
  setting.

## Tradeoffs

### Config must exist for recipe brains to activate

If `~/.simard/config.toml` does not exist AND `SIMARD_LLM_PROVIDER` is not
set, the recipe brain `new()` methods return `None`. This means no recipe
brain is available, and the OODA loop falls back to the deterministic brain.

This is intentional. The project's design rule — no silent defaults —
means that a missing provider must surface as a visible degradation (recipe
brain unavailable), not as a wrong provider (defaulting to `claude`).

Operators who encounter this see a log line explaining which brains are
active. The fix is to write the config file:

```bash
mkdir -p ~/.simard
echo 'llm_provider = "copilot"' > ~/.simard/config.toml
```

Or set the env var:

```bash
export SIMARD_LLM_PROVIDER=copilot
```

### Config is read once per struct, not once per call

The struct-based shims read config in `new()` and cache the result. If an
operator changes `config.toml` while the daemon is running, the change is
not picked up until the next daemon restart (which recreates the structs).

This is acceptable because:

1. LLM provider changes are rare — they happen during deployment, not
   during operation.
2. The daemon already rebuilds brain structs on restart.
3. Per-call config reads would add filesystem I/O on every OODA cycle
   for no practical benefit.

### `disk_health.rs` reads config per-call

Unlike the struct-based shims, `run_disk_health_check` reads config each
time it's called. This is because the function has no persistent state —
it's a stateless helper called once per cycle. The filesystem read is
negligible compared to the subprocess spawn that follows.

## Related

- [Subprocess environment propagation API](../reference/subprocess-env-propagation.md) — full API surface
- [Configure LLM provider (how-to)](../howto/configure-llm-provider.md) — operator guide
- [Disk health API reference](../reference/disk-health-api.md) — env propagation in disk health
- [Truthful runtime metadata](./truthful-runtime-metadata.md) — the no-silent-defaults principle
