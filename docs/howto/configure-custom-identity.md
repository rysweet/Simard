---
title: "How to configure a custom identity"
description: >
  Step-by-step guide for operators who want to define custom Simard identities
  using identity.toml instead of the builtin defaults.
last_updated: 2026-06-10
review_schedule: as-needed
owner: simard
---

# How to configure a custom identity

This guide walks you through creating a custom Simard identity using
`identity.toml`. By the end you will have a file-based identity that loads
custom prompt assets and can be activated with a single environment variable.

## Prerequisites

- Simard binary built from a commit that includes the pluggable identity
  feature (PR #2242+)
- A writable directory for your identity files
- Your prompt assets (markdown files) placed under `SIMARD_PROMPT_ROOT`

## Step 1: Create the identity directory

```bash
mkdir -p /opt/simard/identities
```

This directory will contain your `identity.toml` file. It can be anywhere on
the filesystem — Simard reads its location from an environment variable.

## Step 2: Write your prompt assets

Place custom prompt asset files under your prompt root. The prompt root is
already configured via `SIMARD_PROMPT_ROOT`.

```bash
# Example: custom engineer system prompt
mkdir -p "$SIMARD_PROMPT_ROOT/myteam"
cat > "$SIMARD_PROMPT_ROOT/myteam/engineer_system.md" << 'EOF'
You are a senior software engineer working on the Acme project.
Follow the team coding standards documented in CONTRIBUTING.md.
Always write tests before implementation.
EOF
```

## Step 3: Create identity.toml

```bash
cat > /opt/simard/identities/identity.toml << 'EOF'
[package]
name = "myteam-identities"
version = "1.0.0"
description = "Custom identities for my team"

[[identities]]
name = "myteam-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness", "terminal-shell", "copilot-sdk"]
required_capabilities = [
    "prompt-assets",
    "session-lifecycle",
    "memory",
    "evidence",
    "reflection",
]

[[identities.prompt_assets]]
id = "engineer-system"
path = "myteam/engineer_system.md"
EOF
```

Key points:

- **`name`** must be unique within the file. Use ASCII alphanumeric characters
  and hyphens. Maximum 128 characters.
- **`default_mode`** determines the operating mode. Valid values: `engineer`,
  `meeting`, `curator`, `improvement`, `gym`, `orchestrator`.
- **`path`** in `[[identities.prompt_assets]]` is relative to
  `SIMARD_PROMPT_ROOT`, not relative to the identity directory.

## Step 4: Activate the custom identity

Set the environment variables and run Simard:

```bash
export SIMARD_IDENTITY_PATH=/opt/simard/identities
export SIMARD_IDENTITY=myteam-engineer
export SIMARD_BOOTSTRAP_MODE=builtin-defaults  # or explicit-config with all vars set

simard engineer start
```

When `SIMARD_IDENTITY_PATH` is set, Simard uses `FileIdentityLoader` to look
up the requested identity from `identity.toml`. If the identity is not found
in the file, Simard falls back to the builtin loader.

## Step 5: Verify the identity loaded

Check the bootstrap reflection output:

```bash
simard bootstrap reflect
```

The output shows which identity was loaded and where configuration values came
from. Look for `identity_path` in the resolved config.

## Adding a composite identity

Composite identities merge multiple identities into one. Define the components
in the same `identity.toml` file:

```toml
[package]
name = "myteam-identities"
version = "1.0.0"

[[identities]]
name = "myteam-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets", "session-lifecycle"]

[[identities.prompt_assets]]
id = "engineer-system"
path = "myteam/engineer_system.md"

[[identities]]
name = "myteam-meeting"
default_mode = "meeting"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets", "session-lifecycle"]

[[identities.prompt_assets]]
id = "meeting-system"
path = "myteam/meeting_system.md"

[[identities]]
name = "myteam-composite"
default_mode = "engineer"
components = ["myteam-engineer", "myteam-meeting"]
```

Then activate with:

```bash
export SIMARD_IDENTITY=myteam-composite
```

The composite identity merges base types, capabilities, and prompt assets from
both components. The `default_mode` of the composite takes precedence.

## Configuring memory policy

Add an optional `[identities.memory_policy]` table to control memory writes:

```toml
[[identities]]
name = "readonly-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness"]

[identities.memory_policy]
allow_project_writes = false
summary_scope = "session-summary"
```

In v1, `allow_project_writes` must be `false`. Setting it to `true` returns a
validation error.

## Falling back to builtin identities

If you set `SIMARD_IDENTITY_PATH` but request an identity name that is not in
your `identity.toml`, Simard falls back to the builtin loader. This means you
can override some identities while keeping the defaults for others:

```bash
export SIMARD_IDENTITY_PATH=/opt/simard/identities
export SIMARD_IDENTITY=simard-gym  # not in your TOML → uses builtin
```

To prevent accidental fallback, make sure your identity names match what you
set in `SIMARD_IDENTITY`.

## Troubleshooting

### "identity path not under prompt root"

The identity directory (or a parent) resolves outside `SIMARD_PROMPT_ROOT`
after symlink resolution. Move your identity directory under the prompt root
or remove the offending symlink.

### "prompt asset path escapes prompt root (possible symlink attack)"

A prompt asset path resolves (after symlink resolution) to a file outside
the prompt root. Check the `path` field in `[[identities.prompt_assets]]`
and remove any symlinks that point outside the prompt root.

### "file size exceeds maximum"

Your `identity.toml` exceeds 1 MiB. Identity files should be small — move
large content into prompt asset files instead.

### "unknown field" parse error

`identity.toml` uses strict parsing at every level. Check for typos in field
names. Valid top-level tables are `[package]` and `[[identities]]`. Valid
fields for `[[identities]]` are: `name`, `default_mode`,
`supported_base_types`, `required_capabilities`, `prompt_assets`,
`components`, `memory_policy`.

### "circular component reference detected"

Two or more identities reference each other as components. Check your
`components` arrays for cycles.

### "composition depth exceeds maximum of 8"

Your composition tree is too deep. Flatten the hierarchy or reduce nesting.

## Environment variable reference

| Variable | Required | Description |
|---|---|---|
| `SIMARD_IDENTITY_PATH` | No | Directory containing `identity.toml`. When unset, only builtin identities are available. |
| `SIMARD_IDENTITY` | Yes | Name of the identity to load (from TOML or builtin). |
| `SIMARD_PROMPT_ROOT` | Yes* | Root directory for prompt asset files. *Not required in `builtin-defaults` mode. |
| `SIMARD_BOOTSTRAP_MODE` | No | `builtin-defaults` or `explicit-config`. Defaults to `explicit-config`. |

## Related documentation

- [identity.toml reference](../reference/identity-toml.md) — complete file format specification
- [Concept: Pluggable identity system](../concepts/pluggable-identity.md) — design rationale
- [FileIdentityLoader API reference](../reference/file-identity-loader-api.md) — Rust API
- [How to configure bootstrap and inspect reflection](../howto/configure-bootstrap-and-inspect-reflection.md) — general bootstrap configuration
