---
title: "identity.toml reference"
description: >
  File format specification for the pluggable identity system.
  Describes every table, field, and constraint in identity.toml.
last_updated: 2026-06-10
review_schedule: as-needed
owner: simard
---

# identity.toml reference

`identity.toml` defines one or more Simard identities in a single file. It is
read by `FileIdentityLoader` when the `SIMARD_IDENTITY_PATH` environment
variable points to the directory containing the file.

## File location

```
$SIMARD_IDENTITY_PATH/
└── identity.toml
```

`SIMARD_IDENTITY_PATH` must be an absolute path or resolvable relative to the
working directory. The directory must exist and be readable. The file must be
named exactly `identity.toml`.

## Minimal example

```toml
[package]
name = "my-team-identity"
version = "1.0.0"

[[identities]]
name = "custom-engineer"
default_mode = "engineer"
supported_base_types = ["local-harness"]
required_capabilities = ["prompt-assets", "session-lifecycle"]

[[identities.prompt_assets]]
id = "engineer-system"
path = "prompts/custom_engineer_system.md"
```

The prompt asset path `prompts/custom_engineer_system.md` resolves relative to
`SIMARD_PROMPT_ROOT`, not relative to the identity directory.

## Full example

```toml
[package]
name = "acme-identities"
version = "2.0.0"
description = "Custom Simard identities for Acme Corp"

# --- Simple identity ---

[[identities]]
name = "acme-engineer"
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
path = "acme/engineer_system.md"

[[identities.prompt_assets]]
id = "code-review-guide"
path = "acme/code_review.md"

[identities.memory_policy]
allow_project_writes = false
summary_scope = "session-summary"

# --- Meeting identity ---

[[identities]]
name = "acme-meeting"
default_mode = "meeting"
supported_base_types = ["local-harness", "copilot-sdk"]
required_capabilities = ["prompt-assets", "session-lifecycle", "memory"]

[[identities.prompt_assets]]
id = "meeting-system"
path = "acme/meeting_system.md"

# --- Composite identity (composes the above) ---

[[identities]]
name = "acme-composite"
default_mode = "engineer"
components = ["acme-engineer", "acme-meeting"]
```

## Tables and fields

The file uses strict parsing at every nesting level. All TOML deserialization
types use `#[serde(deny_unknown_fields)]`. Unrecognized tables or fields at
any level — top-level, `[package]`, `[[identities]]`, `[[identities.prompt_assets]]`,
or `[identities.memory_policy]` — are rejected at parse time.

### `[package]`

Required metadata about the identity package.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Package name. Used for logging and diagnostics. |
| `version` | string | yes | Semantic version string. |
| `description` | string | no | Human-readable description. |

### `[[identities]]`

Each `[[identities]]` entry defines one identity. At least one is required for
the file to be useful (though the parser accepts an empty array).

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | string | yes | — | Unique identity name. ASCII alphanumeric and hyphens only, max 128 chars. |
| `default_mode` | string | yes | — | Operating mode. One of: `engineer`, `meeting`, `curator`, `improvement`, `gym`, `orchestrator`. |
| `supported_base_types` | string[] | no | `[]` | Base type IDs this identity supports. |
| `required_capabilities` | string[] | no | `[]` | Required capabilities. Values: `prompt-assets`, `session-lifecycle`, `memory`, `evidence`, `reflection`. |
| `prompt_assets` | table[] | no | `[]` | Prompt asset references (see below). |
| `components` | string[] | no | `[]` | Names of other identities in this file to compose. |
| `memory_policy` | table | no | `null` | Memory write permissions (see below). |

Typos like `defaut_mode` or `prompt_asset` (singular) are caught at parse time
rather than silently ignored.

### `[[identities.prompt_assets]]`

Each prompt asset maps an ID to a file path under the prompt root.

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Unique asset identifier. Referenced by the prompt delivery system. |
| `path` | string | yes | Path relative to `SIMARD_PROMPT_ROOT`. Must not contain `..` or absolute segments. |

Unrecognized fields are rejected.

**Path resolution**: The `path` field is validated and stored as a path
relative to `SIMARD_PROMPT_ROOT`. At load time, `FilePromptAssetStore`
resolves it via `prompt_root.join(relative_path)`. The file must exist
under the prompt root after symlink resolution.

### `[identities.memory_policy]`

Optional. Controls what a session running this identity can write to long-term
memory.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `allow_project_writes` | bool | no | `false` | Whether to allow writes to project-scoped memory. Currently must be `false` (v1 constraint). |
| `summary_scope` | string | no | `"session-summary"` | Memory scope for session summaries. |

Unrecognized fields are rejected.

## Composition

When an identity has a non-empty `components` array, `FileIdentityLoader`
recursively resolves each component from the same `identity.toml` file. The
parent identity's `default_mode` takes precedence. Base types, capabilities,
and prompt assets are merged from all components using
`compose_with_precedence`.

**Cycle detection**: Uses DFS stack-based detection. A diamond graph (A→B,
A→C, B→D, C→D) is valid. A true cycle (A→B→A) returns an error.

**Depth limit**: Maximum composition depth is 8. Exceeding this returns an
error.

**Component not found**: If a component name does not match any
`[[identities]]` entry in the same file, the loader returns an error (no
fallback to builtin for components).

## Size and safety limits

| Limit | Value | Enforced at |
|---|---|---|
| Maximum file size | 1 MiB (`MAX_IDENTITY_FILE_SIZE`) | `fs::metadata().len()` check before `fs::read()` |
| Maximum identity name length | 128 characters | Name validation before TOML lookup |
| Maximum composition depth | 8 levels | Recursive resolver |
| Allowed name characters | `[a-zA-Z0-9-]` | Name validation regex |

## Symlink containment

Both `identity.toml` and all prompt asset files are subject to symlink
containment:

1. The path is canonicalized (resolves all symlinks).
2. The canonical path must start with the canonical prompt root.
3. If the canonical path escapes the prompt root, the load fails with an error.

This prevents a symlink inside the identity directory from pointing to
`/etc/shadow` or any file outside the prompt root.

## Error handling

| Condition | Behavior |
|---|---|
| `SIMARD_IDENTITY_PATH` not set | `BuiltinIdentityLoader` is used (no file loading attempted) |
| Identity directory not under prompt root | Returns `SimardError::IdentityPathNotUnderPromptRoot` (no fallback) |
| `identity.toml` does not exist | Falls back to `BuiltinIdentityLoader` |
| `identity.toml` exists but requested identity not found | Falls back to `BuiltinIdentityLoader` |
| `identity.toml` parse error | Returns `SimardError::IdentityTomlParseError` (no fallback) |
| File exceeds 1 MiB | Returns `SimardError::IdentityTomlParseError` |
| Prompt asset symlink escapes prompt root | Returns `SimardError::IdentityTomlParseError` (no fallback) |
| Unknown TOML field | Returns parse error (`deny_unknown_fields`) |
| Circular component reference | Returns `SimardError::IdentityTomlParseError` |

## Related documentation

- [Concept: Pluggable identity system](../concepts/pluggable-identity.md) — design rationale
- [How to configure a custom identity](../howto/configure-custom-identity.md) — operator playbook
- [FileIdentityLoader API reference](../reference/file-identity-loader-api.md) — Rust API
