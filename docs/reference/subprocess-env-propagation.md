---
title: Subprocess environment propagation API
description: Reference for how Simard's recipe-runner shims read ~/.simard/config.toml and propagate AMPLIHACK_AGENT_BINARY to recipe-runner-rs subprocesses.
last_updated: 2026-05-27
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/config-driven-subprocess-env.md
  - ../howto/configure-llm-provider.md
  - ./disk-health-api.md
  - ./runtime-contracts.md
---

# Subprocess environment propagation API

Module: `simard::session_builder` (method), `simard::runtime_config` (config loading)
Source: `src/session_builder.rs`, `src/runtime_config.rs`, plus all recipe shim files

Simard's recipe-runner shims set `AMPLIHACK_AGENT_BINARY` on every
`recipe-runner-rs` subprocess using the `llm_provider` value from
`RuntimeConfig`. This ensures the recipe runner invokes the correct agent
binary (`copilot` or `rustyclawd`) regardless of the parent process's
environment.

## `LlmProvider::agent_binary_value()`

```rust
impl LlmProvider {
    /// The string value for `AMPLIHACK_AGENT_BINARY` env var on subprocesses.
    pub fn agent_binary_value(&self) -> &'static str {
        match self {
            Self::Copilot => "copilot",
            Self::RustyClawd => "rustyclawd",
        }
    }
}
```

Returns a static string suitable for passing to `Command::env()`. The
mapping is:

| `LlmProvider` variant | `agent_binary_value()` | `to_toml_string()` |
| ---------------------- | ---------------------- | ------------------- |
| `Copilot`              | `"copilot"`            | `"copilot"`         |
| `RustyClawd`           | `"rustyclawd"`         | `"rustyclawd"`      |

The values intentionally match `to_toml_string()` — both represent the same
provider identity in different contexts (subprocess env vs. config file).

## Config resolution chain

All shims resolve the agent binary through the same chain:

```
RuntimeConfig::load()
  → SIMARD_LLM_PROVIDER env var (if set)
  → ~/.simard/config.toml llm_provider key (if file exists)
  → Error (no silent default)
```

This chain is defined in `src/runtime_config.rs`. See the module-level
documentation for the full resolution order and anti-pattern guards.

## Affected shims

### Struct-based recipe shims

These shims store `agent_binary: &'static str` as a struct field, populated once
in `new()`:

| Struct                            | Source file                                  | Version-check Command | Execution Command       |
| --------------------------------- | -------------------------------------------- | --------------------- | ----------------------- |
| `RecipeDecideBrain`               | `src/ooda_brain/recipe_decide.rs`            | `--version`           | `judge_decision()`      |
| `RecipeOrientBrain`               | `src/ooda_brain/recipe_orient.rs`            | `--version`           | `judge_orientation()`   |
| `RecipeEngineerLifecycleBrain`    | `src/ooda_brain/recipe_engineer_lifecycle.rs` | `--version`          | `decide_engineer_lifecycle()` |
| `RecipeMergeJudge`                | `src/stewardship/recipe_merge_judge.rs`      | `--version`           | `judge()`               |
| `RecipeProgressChecker`           | `src/goal_curation/recipe_progress_checker.rs` | `--version`         | `check()`               |

Each struct follows this pattern in `new()`:

```rust
pub fn new(repo_root: &Path) -> Option<Self> {
    let recipe_path = resolve_recipe_path(repo_root)?;
    let agent_binary = RuntimeConfig::load()
        .ok()?
        .llm_provider
        .agent_binary_value()
        .to_string();

    // Version check uses a local variable (self doesn't exist yet)
    if Command::new("recipe-runner-rs")
        .arg("--version")
        .env("AMPLIHACK_AGENT_BINARY", &agent_binary)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return None;
    }

    Some(Self {
        recipe_path,
        agent_binary,
    })
}
```

And this pattern in execution methods:

```rust
let output = Command::new("recipe-runner-rs")
    .arg(self.recipe_path.as_os_str())
    .env("AMPLIHACK_AGENT_BINARY", &self.agent_binary)
    // ... recipe-specific -c context vars ...
    .output()?;
```

### Function-based shim: `disk_health.rs`

`run_disk_health_check` is a standalone function, not a struct. It reads
config at call time:

```rust
pub fn run_disk_health_check(
    repo_root: &Path,
    state_root: &Path,
) -> SimardResult<DiskHealthReport> {
    let agent_binary = RuntimeConfig::load()?
        .llm_provider
        .agent_binary_value();

    // ...
    let output = Command::new("recipe-runner-rs")
        .arg(recipe_path.as_os_str())
        .env("AMPLIHACK_AGENT_BINARY", agent_binary)
        .arg("-c")
        .arg(format!("state_root={}", state_root.display()))
        .output()?;
    // ...
}
```

Config failure propagates as `SimardResult::Err` via `?`. The daemon's
existing warn-and-continue handler catches it.

## Failure modes

### Config unavailable in struct `new()`

If `RuntimeConfig::load()` fails (no env var AND no config.toml), `.ok()?`
converts the error to `None`, and `new()` returns `None`. The caller falls
back to the deterministic brain — the same behaviour as when
`recipe-runner-rs` is not installed.

This is visible in the daemon's brain selection log:

```
[simard] decide brain: deterministic (recipe brain not available)
```

### Config unavailable in `disk_health.rs`

If `RuntimeConfig::load()` fails, `run_disk_health_check` returns
`SimardResult::Err`. The daemon logs the error and continues the OODA
cycle:

```
[simard] disk health check failed: missing required config: SIMARD_LLM_PROVIDER ...
```

### Wrong provider in config

If `config.toml` says `llm_provider = "copilot"` but the operator intends
`rustyclawd`, the recipe runner will invoke the wrong binary. This is an
operator configuration error, not a Simard bug. The fix is to update
`config.toml` and restart the daemon.

## Environment variable contract

The recipe-runner shims set exactly one env var on child processes:

| Variable                  | Value source                          | Scope        |
| ------------------------- | ------------------------------------- | ------------ |
| `AMPLIHACK_AGENT_BINARY`  | `LlmProvider::agent_binary_value()`   | Child only   |

The shims use `Command::env()` (child process scope), never
`std::env::set_var()` (parent process scope). This is a safety invariant —
modifying the parent environment in multi-threaded code is undefined
behaviour in Rust.

No other env vars are added or modified by this feature. Existing env vars
(`SIMARD_STATE_ROOT`, `CARGO_TARGET_DIR`, etc.) continue to be inherited
naturally from the parent process.

## Security considerations

| Property              | Guarantee                                                     |
| --------------------- | ------------------------------------------------------------- |
| No user input         | `agent_binary_value()` returns hardcoded `&'static str`       |
| No injection          | Values are `"copilot"` or `"rustyclawd"` — no interpolation   |
| Child-only scope      | `Command::env()` never modifies parent process environment    |
| No credentials        | `AMPLIHACK_AGENT_BINARY` is a selector, not a secret          |
| Audit trail           | Config source is logged at daemon startup via `bootstrap_from_env` |

## Tests

### `LlmProvider::agent_binary_value()` unit test

```rust
#[test]
fn agent_binary_value_returns_expected_strings() {
    assert_eq!(LlmProvider::Copilot.agent_binary_value(), "copilot");
    assert_eq!(LlmProvider::RustyClawd.agent_binary_value(), "rustyclawd");
}
```

### Struct test literals

Each struct-based shim's existing test constructs a bare struct literal.
These tests include the new `agent_binary` field:

```rust
let brain = RecipeDecideBrain {
    recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
    agent_binary: "copilot",
};
```

This ensures the struct compiles with the new field and that tests use an
explicit value rather than inheriting from the environment.

## Configuration

To set the LLM provider that flows into `AMPLIHACK_AGENT_BINARY`:

```bash
# Option 1: config file (recommended — survives tmux, systemd, ssh)
mkdir -p ~/.simard
echo 'llm_provider = "copilot"' > ~/.simard/config.toml

# Option 2: env var (wins over config file when set)
export SIMARD_LLM_PROVIDER=copilot
```

See [Configure LLM provider](../howto/configure-llm-provider.md) for the
full operator guide.

## Related

- [Config-driven subprocess env (concept)](../concepts/config-driven-subprocess-env.md) — design rationale
- [Configure LLM provider (how-to)](../howto/configure-llm-provider.md) — operator guide
- [Disk health API](./disk-health-api.md) — disk health integration
- [Runtime contracts](./runtime-contracts.md) — broader runtime contract surface
