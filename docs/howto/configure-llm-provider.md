---
title: Configure the LLM provider for Simard
description: Operator guide for setting up ~/.simard/config.toml so Simard's recipe-runner subprocesses use the correct agent binary.
last_updated: 2026-05-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/subprocess-env-propagation.md
  - ../concepts/config-driven-subprocess-env.md
  - ../howto/configure-bootstrap-and-inspect-reflection.md
  - ../reference/runtime-contracts.md
  - ../howto/configure-disk-health-check.md
---

# Configure the LLM provider for Simard

Simard reads `~/.simard/config.toml` to determine which LLM provider to use.
This config drives two things: which agent adapter `SessionBuilder` creates,
and which binary `recipe-runner-rs` subprocesses invoke via
`AMPLIHACK_AGENT_BINARY`.

This guide shows how to set, verify, and troubleshoot the LLM provider
configuration.

## When to use this

Use this guide when:

- You are deploying Simard for the first time
- Recipe-runner shims fail with "binary not found" or "recipe-runner-rs spawn failed"
- The daemon falls back to deterministic brains when recipe brains should be active
- You are switching between Copilot and RustyClawd deployments
- You see `missing required config: SIMARD_LLM_PROVIDER` in logs

## Set the LLM provider

### Option 1: config file (recommended)

Create `~/.simard/config.toml` with the `llm_provider` key:

```bash
mkdir -p ~/.simard
cat > ~/.simard/config.toml << 'EOF'
# Simard runtime configuration
# Loaded by every Simard process at startup. Subprocesses
# (engineer, meeting REPL, etc.) read this file so the
# operator does not have to plumb env vars through tmux,
# systemd, or ssh wrappers.
llm_provider = "copilot"
EOF
```

Accepted values:

| Value         | Agent binary | Provider                                     |
| ------------- | ------------ | -------------------------------------------- |
| `"copilot"`   | `copilot`    | GitHub Copilot SDK via `gh` auth             |
| `"rustyclawd"`| `rustyclawd` | RustyClawd / Anthropic (needs `ANTHROPIC_API_KEY`) |

The config file is the recommended approach because it survives:

- tmux session creation (which does not propagate parent env)
- systemd service restarts (no need to list env vars in unit files)
- SSH sessions (no need for `.bashrc` exports)
- Subprocess nesting (recipe-runner spawning further processes)

### Option 2: environment variable

Set `SIMARD_LLM_PROVIDER` in the shell that launches the daemon:

```bash
export SIMARD_LLM_PROVIDER=copilot
simard ooda run ...
```

The env var **wins over** the config file when both are set. This is useful
for temporary overrides or testing, but the config file is more reliable for
production.

### Auto-bootstrap from env

If you launch the daemon with `SIMARD_LLM_PROVIDER` set but no config.toml
exists, Simard writes the config file automatically on first startup. This
is a one-time bootstrap — subsequent runs read the file directly.

```bash
# First run: env var is set, config.toml doesn't exist
export SIMARD_LLM_PROVIDER=copilot
simard ooda run ...
# → writes ~/.simard/config.toml with llm_provider = "copilot"

# Second run: env var not needed, config.toml is read
simard ooda run ...
# → reads ~/.simard/config.toml
```

## Verify the configuration

### Check the config file

```bash
cat ~/.simard/config.toml
```

Expected output:

```toml
# Simard runtime configuration
llm_provider = "copilot"
```

### Check the env var (if using option 2)

```bash
echo $SIMARD_LLM_PROVIDER
```

### Verify recipe brains are active

After starting the daemon, check the log for brain selection:

```bash
grep -E "decide brain|orient brain|engineer lifecycle" ~/.simard/ooda.log | tail -5
```

When config is correct and `recipe-runner-rs` is installed:

```
[simard] decide brain: recipe (ooda-decide.yaml)
[simard] orient brain: recipe (ooda-orient.yaml)
```

When config is missing or `recipe-runner-rs` is not installed:

```
[simard] decide brain: deterministic (recipe brain not available)
[simard] orient brain: deterministic (recipe brain not available)
```

### Verify disk health check

```bash
grep "disk health" ~/.simard/ooda.log | tail -3
```

When config is correct:

```
[simard] disk health: 72% used, freed 0 bytes, 0 actions
```

When config is missing:

```
[simard] disk health check failed: missing required config: SIMARD_LLM_PROVIDER ...
```

## Troubleshoot

### "missing required config: SIMARD_LLM_PROVIDER"

Neither `SIMARD_LLM_PROVIDER` env var nor `~/.simard/config.toml` is set.

**Fix:** Create the config file:

```bash
mkdir -p ~/.simard
echo 'llm_provider = "copilot"' > ~/.simard/config.toml
```

### "recipe-runner-rs spawn failed"

`recipe-runner-rs` is not on `$PATH`, or `AMPLIHACK_AGENT_BINARY` resolved
to a binary that doesn't exist.

**Check:** Verify the binary exists:

```bash
which recipe-runner-rs
recipe-runner-rs --version
```

**Check:** Verify the agent binary exists:

```bash
# For copilot provider:
which copilot

# For rustyclawd provider:
which rustyclawd
```

### Recipe brains fall back to deterministic

This happens when either:

1. `RuntimeConfig::load()` fails (config missing) — the shim's `new()` returns `None`
2. `recipe-runner-rs` is not installed — the version check fails
3. The recipe YAML file is missing — `resolve_recipe_path()` returns `None`

**Diagnose:** Check each condition:

```bash
# 1. Config exists?
cat ~/.simard/config.toml

# 2. recipe-runner-rs installed?
which recipe-runner-rs

# 3. Recipe files exist?
ls prompt_assets/simard/recipes/ooda-decide.yaml
ls prompt_assets/simard/recipes/ooda-orient.yaml
```

### Config file has wrong provider

If you set `llm_provider = "rustyclawd"` but are running on a Copilot
deployment (or vice versa), recipe invocations will fail with binary-not-found
errors for the wrong binary.

**Fix:** Update the config file to match your deployment:

```bash
echo 'llm_provider = "copilot"' > ~/.simard/config.toml
```

Then restart the daemon.

### Changes to config.toml not taking effect

The struct-based recipe shims (decide, orient, engineer lifecycle, merge
judge, progress checker) read config once at construction time and cache
the result. Changes to `config.toml` while the daemon is running are not
picked up until the daemon restarts.

**Fix:** Restart the daemon after changing `config.toml`.

The disk health check reads config on every call, so it picks up changes
immediately — but for consistency, restart the daemon after any config
change.

## Understand the resolution order

Simard resolves the LLM provider through this chain:

```
1. SIMARD_LLM_PROVIDER env var (wins when set)
       ↓ (not set)
2. ~/.simard/config.toml → llm_provider key
       ↓ (file missing or key absent)
3. Error — no silent default
```

The env var always wins. This is intentional — it allows temporary overrides
without modifying the config file. But for production, the config file is
the primary mechanism.

There is **no silent default**. If neither source provides a value, Simard
fails with an explicit error rather than guessing. This follows the project's
no-silent-defaults principle (see [truthful runtime metadata](../concepts/truthful-runtime-metadata.md)).

## How it flows to subprocesses

When a recipe shim spawns `recipe-runner-rs`, it sets
`AMPLIHACK_AGENT_BINARY` on the child process using the value from
`LlmProvider::agent_binary_value()`:

```
~/.simard/config.toml                    (llm_provider = "copilot")
    → RuntimeConfig::load()              (LlmProvider::Copilot)
    → LlmProvider::agent_binary_value()  ("copilot")
    → Command::env("AMPLIHACK_AGENT_BINARY", "copilot")
    → recipe-runner-rs reads env         (uses 'copilot' binary)
```

This happens automatically. The operator's only responsibility is to set
`llm_provider` once in `config.toml`.

## Related

- [Subprocess environment propagation API](../reference/subprocess-env-propagation.md) — full API surface
- [Config-driven subprocess env (concept)](../concepts/config-driven-subprocess-env.md) — design rationale
- [Configure bootstrap and inspect reflection](./configure-bootstrap-and-inspect-reflection.md) — broader runtime configuration
- [Configure disk health check](./configure-disk-health-check.md) — disk health uses the same config
- [Runtime contracts reference](../reference/runtime-contracts.md) — overall runtime contract
