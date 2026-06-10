---
title: How to configure pluggable identities
description: Create an identity.toml file to define custom agent personas, operating modes, prompt assets, and memory policies for a repository.
last_updated: 2026-06-10
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/pluggable-identity.md
  - ../reference/pluggable-identity-api.md
  - ../reference/simard-cli.md
  - ../howto/move-from-terminal-recipes-into-engineer-runs.md
---

# How to configure pluggable identities

Pluggable identities let you define custom agent personas per repository.
Instead of using Simard's built-in identities, you declare identities in
an `identity.toml` file. Simard loads them at startup and uses the
matching identity for each session.

## Prerequisites

- Simard binary built (`cargo build --quiet`)
- A repository where you want custom identities
- Prompt assets directory configured (the `prompt_root`)

## Create a minimal identity

Create `identity.toml` in your identity directory (typically
`.simard/identity/` under your repo root):

```toml
[package]
name = "my-project-identity"
version = "0.1.0"

[[identities]]
name = "my-engineer"
default_mode = "engineer"
```

This defines one identity named `my-engineer` that operates in engineer
mode. It inherits Simard's default capabilities, base types, and memory
policy.

## Add prompt assets

Custom prompt assets let your identity inject repo-specific system prompts:

```toml
[package]
name = "my-project-identity"
version = "0.1.0"

[[identities]]
name = "my-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness", "rusty-clawd"]
required_capabilities = ["prompt-assets", "session-lifecycle", "memory"]

[[identities.prompt_assets]]
id = "engineer-system"
path = "engineer_system.md"

[[identities.prompt_assets]]
id = "code-style-guide"
path = "code_style.md"
```

Place the referenced markdown files alongside `identity.toml`:

```
.simard/identity/
├── identity.toml
├── engineer_system.md      # Your custom system prompt
└── code_style.md           # Additional prompt asset
```

!!! warning "Path security"
    Prompt asset paths must be **relative** and must not contain `../`
    traversal. Absolute paths like `/etc/passwd` and traversal paths
    like `../../secrets.md` are rejected with a hard error. This
    prevents identity files from reading outside the identity directory.

## Configure memory policy

Control what sessions with this identity are allowed to write to
long-term memory:

```toml
[[identities]]
name = "restricted-engineer"
default_mode = "engineer"

[identities.memory_policy]
allow_project_writes = false
summary_scope = "session-summary"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allow_project_writes` | bool | `false` | Whether sessions may write to project-scoped memory. **v1 only supports `false`**; setting `true` produces `UnsupportedMemoryPolicy` error. |
| `summary_scope` | string | `"session-summary"` | Memory scope for session summaries. Valid values: `"session-scratch"`, `"session-summary"`, `"decision"`, `"project"`, `"benchmark"`, `"untagged"`. |

If `[identities.memory_policy]` is omitted entirely, the identity uses
the default policy (`allow_project_writes = false`,
`summary_scope = "session-summary"`).

## Define multiple identities

A single `identity.toml` can contain multiple identities for different
operating modes:

```toml
[package]
name = "team-identity"
version = "1.0.0"
description = "Engineering team identity package"

[[identities]]
name = "team-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness", "rusty-clawd", "copilot-sdk"]
required_capabilities = ["prompt-assets", "session-lifecycle", "memory", "evidence", "reflection"]

[[identities.prompt_assets]]
id = "engineer-system"
path = "engineer_system.md"

[[identities]]
name = "team-reviewer"
default_mode = "engineer"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets"]

[[identities.prompt_assets]]
id = "review-system"
path = "review_system.md"
```

Simard selects the identity whose `name` matches the requested identity
at session startup. If no match is found, Simard falls back to built-in
identities.

## Build composite identities

Composite identities merge prompt assets, capabilities, and base types
from multiple component identities defined in the same file:

```toml
[package]
name = "composite-package"
version = "0.1.0"

[[identities]]
name = "base-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness", "rusty-clawd"]
required_capabilities = ["prompt-assets", "session-lifecycle"]

[[identities.prompt_assets]]
id = "engineer-system"
path = "engineer_system.md"

[[identities]]
name = "security-reviewer"
default_mode = "engineer"
supported_base_types = ["local-harness", "rusty-clawd"]
required_capabilities = ["evidence", "reflection"]

[[identities.prompt_assets]]
id = "security-checklist"
path = "security_checklist.md"

[[identities]]
name = "secure-engineer"
default_mode = "engineer"
components = ["base-engineer", "security-reviewer"]
```

The `secure-engineer` identity:

- Merges prompt assets from both components (deduplicated by id + path)
- Intersects `supported_base_types` (only types common to all components)
- Unions `required_capabilities` (all capabilities from any component)
- Requires all components to agree on `memory_policy` — mismatched
  policies produce an `InvalidIdentityComposition` error

!!! note "Composition limits"
    - Maximum composition depth: **8 levels** of recursive nesting
    - Circular references are detected and produce a hard error
    - All components must share at least one common base type, or
      composition fails with `InvalidIdentityComposition`
    - All components must agree on `memory_policy`, or composition
      fails with `InvalidIdentityComposition`

## Configure developer watches

Developer watches track GitHub users and their focus areas for the
research tracker. Define them in a `watches.toml` file:

```toml
[[watches]]
github_id = "octocat"
focus_areas = ["rust", "wasm", "llm"]

[[watches]]
github_id = "ferris"
focus_areas = ["systems-programming", "embedded"]

[[watches]]
github_id = "alice"
focus_areas = ["security", "cryptography"]
```

Place `watches.toml` in your identity directory alongside
`identity.toml`. If the file does not exist, Simard uses the compiled-in
default watch list. If the file exists but is malformed, Simard returns a
hard error.

Each watch entry has `last_checked: None` when loaded from TOML — the
research tracker fills this in as it polls for activity.

## Verify your configuration

After creating `identity.toml`, test that it parses correctly by running
Simard with the custom identity:

```bash
simard engineer run \
  --identity my-engineer \
  --identity-path .simard/identity \
  --prompt-root . \
  "$PWD/target/simard-state"
```

If the TOML file has errors, Simard exits with an `IdentityTomlParseError`
that includes the parse error details. Fix the TOML and retry.

If the identity name is not found in the TOML file, Simard falls back to
built-in identities. To confirm your custom identity loaded, check the
session log for the identity name.

## Identity name rules

Identity names must satisfy all of these constraints:

- Non-empty
- Maximum 128 characters
- ASCII only: letters (`a-z`, `A-Z`), digits (`0-9`), and hyphens (`-`)
- No spaces, underscores, dots, or other special characters

Examples of valid names: `my-engineer`, `team-a-reviewer`,
`security-audit-v2`, `simard-custom`.

Examples of invalid names: `my engineer` (spaces), `identité` (non-ASCII),
`my_engineer` (underscores are not allowed), empty string.

## File reference

### identity.toml schema

```toml
# Required. Package metadata.
[package]
name = "string"         # Required. Package name.
version = "string"      # Required. Semver version string.
description = "string"  # Optional. Human-readable description.

# Zero or more identity definitions.
[[identities]]
name = "string"                      # Required. Identity name (see naming rules above).
default_mode = "string"              # Required. One of: engineer, meeting, curator, improvement, gym, orchestrator.
supported_base_types = ["string"]    # Optional. List of base type IDs. Default: [].
required_capabilities = ["string"]   # Optional. List of capability names. Default: [].
components = ["string"]              # Optional. Names of other identities in this file to compose. Default: [].

# Zero or more prompt asset references per identity.
[[identities.prompt_assets]]
id = "string"    # Required. Prompt asset identifier.
path = "string"  # Required. Relative path to the prompt asset file.

# Optional memory policy per identity.
[identities.memory_policy]
allow_project_writes = false           # Optional. Default: false. Must be false in v1.
summary_scope = "session-summary"      # Optional. Default: "session-summary".
```

### watches.toml schema

```toml
# Zero or more developer watch entries.
[[watches]]
github_id = "string"           # Required. GitHub username to track.
focus_areas = ["string"]       # Required. List of focus area tags.
```

### Error behavior summary

| Condition | Behavior |
|-----------|----------|
| `identity.toml` not found | Soft fallback to built-in identities |
| Identity name not in TOML | Soft fallback to built-in identities |
| Malformed TOML syntax | Hard error: `IdentityTomlParseError` |
| Unknown fields in TOML | Hard error: `IdentityTomlParseError` |
| Missing required fields | Hard error: `IdentityTomlParseError` |
| File > 1 MB | Hard error: `IdentityTomlParseError` |
| Invalid identity name | Hard error: `IdentityTomlParseError` |
| Absolute prompt asset path | Hard error: `IdentityTomlParseError` |
| Path traversal (`../`) in prompt asset | Hard error: `IdentityTomlParseError` |
| Identity path outside prompt root | Hard error: `IdentityPathNotUnderPromptRoot` |
| Circular component reference | Hard error: `IdentityTomlParseError` |
| Composition depth > 8 | Hard error: `IdentityTomlParseError` |
| `allow_project_writes = true` | Hard error: `UnsupportedMemoryPolicy` |
| `watches.toml` not found | Soft fallback to default watches |
| Malformed `watches.toml` | Hard error: `IdentityTomlParseError` |
