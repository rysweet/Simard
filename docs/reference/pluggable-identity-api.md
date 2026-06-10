---
title: Pluggable identity API reference
description: Rust API reference for FileIdentityLoader, TOML deserialization types, load_watches_from_file, and associated error variants for the pluggable identity system.
last_updated: 2026-06-10
owner: simard
doc_type: reference
related:
  - ../concepts/pluggable-identity.md
  - ../howto/configure-pluggable-identity.md
  - ../reference/runtime-contracts.md
---

# Pluggable identity API reference

Modules: `simard::identity::file_loader`, `simard::identity::toml_types`,
`simard::research_tracker::file_watches`

The pluggable identity system extends Simard's identity loading with
TOML-based configuration files. The `FileIdentityLoader` decorates the
existing `BuiltinIdentityLoader` with file-based identity resolution,
and `load_watches_from_file` provides the same pattern for developer
watch lists.

---

## FileIdentityLoader

Module: `simard::identity::file_loader`

```rust
pub struct FileIdentityLoader {
    identity_path: PathBuf,
    prompt_root: PathBuf,
    fallback: BuiltinIdentityLoader,
}
```

A file-based identity loader that reads `identity.toml` from a
configured directory path and falls back to `BuiltinIdentityLoader`
when the file does not exist or does not contain the requested identity.

### Construction

```rust
impl FileIdentityLoader {
    pub fn new(
        identity_path: impl Into<PathBuf>,
        prompt_root: impl Into<PathBuf>,
    ) -> Self;
}
```

| Parameter | Type | Description |
|-----------|------|-------------|
| `identity_path` | `impl Into<PathBuf>` | Directory containing `identity.toml`. Must be under `prompt_root` (verified at load time, not construction time). |
| `prompt_root` | `impl Into<PathBuf>` | Root directory for prompt asset resolution. Identity path is validated against this to prevent directory escape. |

Construction is infallible. Path validation happens during `load()`.

### Accessor

```rust
impl FileIdentityLoader {
    pub fn identity_path(&self) -> &Path;
}
```

Returns the configured identity directory path.

### IdentityLoader trait implementation

```rust
impl IdentityLoader for FileIdentityLoader {
    fn load(&self, request: &IdentityLoadRequest) -> SimardResult<IdentityManifest>;
}
```

Loads an identity by name. The algorithm:

1. **Validate identity name.** Must be non-empty, ≤128 characters, ASCII
   alphanumeric + hyphens only. On failure → `IdentityTomlParseError`.

2. **Validate path security.** Canonicalize both `identity_path` and
   `prompt_root`; verify `identity_path.starts_with(prompt_root)`. On
   failure → `IdentityPathNotUnderPromptRoot`.

3. **Read `identity.toml`.** If the file does not exist (`NotFound`) →
   delegate to `self.fallback.load(request)`.

4. **Check file size.** Reject files > 1 MB (`MAX_IDENTITY_FILE_SIZE`)
   → `IdentityTomlParseError`.

5. **Parse TOML.** Deserialize into `TomlIdentityFile`. On parse error
   → `IdentityTomlParseError`.

6. **Find matching identity.** Search `[[identities]]` array for an
   entry whose `name` matches `request.identity`. If not found →
   delegate to `self.fallback.load(request)`.

7. **Convert to domain types.** Map TOML strings to enums
   (`OperatingMode`, `BaseTypeCapability`, `MemoryScope`) via serde
   JSON roundtrip. Validate prompt asset paths (no absolute paths,
   no `../` traversal). Construct `IdentityManifest`.

8. **Resolve composition** (if `components` is non-empty). Recursively
   resolve each component from the same TOML file, tracking a visited
   set for cycle detection and a depth counter for
   `MAX_COMPOSITION_DEPTH`. Merge via `IdentityManifest::compose()`.

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `MAX_IDENTITY_FILE_SIZE` | `1_048_576` (1 MB) | Maximum `identity.toml` file size in bytes. Checked via `fs::metadata().len()` before `read_to_string()`. |
| `MAX_COMPOSITION_DEPTH` | `8` | Maximum recursive depth for composite identity resolution. |
| `IDENTITY_NAME_MAX_LEN` | `128` | Maximum length for identity names. |

---

## TOML deserialization types

Module: `simard::identity::toml_types`

All identity types use `#[serde(deny_unknown_fields)]` to reject
unexpected TOML keys at parse time. (The watch types in
`file_watches.rs` do not use `deny_unknown_fields`.)

### TomlIdentityFile

```rust
pub(crate) struct TomlIdentityFile {
    pub package: TomlPackage,
    pub identities: Vec<TomlIdentity>,  // #[serde(default)]
}
```

Top-level structure of an `identity.toml` file.

### TomlPackage

```rust
pub(crate) struct TomlPackage {
    pub name: String,
    pub version: String,
    pub description: Option<String>,  // #[serde(default)]
}
```

The `[package]` table. Both `name` and `version` are required.

### TomlIdentity

```rust
pub(crate) struct TomlIdentity {
    pub name: String,
    pub default_mode: String,
    pub supported_base_types: Vec<String>,       // #[serde(default)]
    pub required_capabilities: Vec<String>,      // #[serde(default)]
    pub prompt_assets: Vec<TomlPromptAsset>,      // #[serde(default)]
    pub components: Vec<String>,                  // #[serde(default)]
    pub memory_policy: Option<TomlMemoryPolicy>,  // #[serde(default)]
}
```

A single `[[identities]]` entry. `name` and `default_mode` are required;
all other fields are optional with empty/`None` defaults.

The `default_mode` string is converted to `OperatingMode` via serde JSON
roundtrip (the enum uses `#[serde(rename_all = "kebab-case")]`). Valid
values: `engineer`, `meeting`, `curator`, `improvement`, `gym`,
`orchestrator`.

### TomlPromptAsset

```rust
pub(crate) struct TomlPromptAsset {
    pub id: String,
    pub path: String,
}
```

A `[[identities.prompt_assets]]` entry. Both fields are required.
The `path` field is validated by `FileIdentityLoader` to reject absolute
paths and `../` traversal.

### TomlMemoryPolicy

```rust
pub(crate) struct TomlMemoryPolicy {
    pub allow_project_writes: bool,           // #[serde(default)] → false
    pub summary_scope: String,                // #[serde(default)] → "session-summary"
}
```

An optional `[identities.memory_policy]` table. Both fields have
defaults. The `summary_scope` string is converted to `MemoryScope` via
serde JSON roundtrip.

---

## load_watches_from_file

Module: `simard::research_tracker::file_watches`

```rust
pub fn load_watches_from_file(path: &Path) -> SimardResult<Vec<DeveloperWatch>>;
```

Loads developer watches from a TOML file.

| Condition | Return |
|-----------|--------|
| File exists and parses | `Ok(Vec<DeveloperWatch>)` with `last_checked: None` on each entry |
| File does not exist | `Ok(default_developer_watches())` — compile-time defaults |
| File exists but malformed | `Err(SimardError::IdentityTomlParseError { .. })` |

### TOML types (file-local)

```rust
struct TomlWatchesFile {
    watches: Vec<TomlWatch>,  // #[serde(default)]
}

struct TomlWatch {
    github_id: String,
    focus_areas: Vec<String>,
}
```

Each `TomlWatch` maps to a `DeveloperWatch` with:
- `github_id` → `DeveloperWatch.github_id`
- `focus_areas` → `DeveloperWatch.focus_areas`
- `last_checked` → `None` (populated later by the research tracker)

---

## Error variants

### IdentityTomlParseError

```rust
SimardError::IdentityTomlParseError {
    path: PathBuf,
    reason: String,
}
```

Produced by `FileIdentityLoader::load()` and `load_watches_from_file()`
for any validation or parse failure:

- TOML syntax errors
- Unknown fields (via `deny_unknown_fields`)
- Missing required fields
- File too large (> 1 MB)
- Invalid identity name (empty, too long, non-ASCII, invalid characters)
- Unsafe prompt asset path (absolute or `../` traversal)
- Circular component references
- Composition depth exceeded
- Invalid `default_mode`, `summary_scope`, or capability strings

### IdentityPathNotUnderPromptRoot

```rust
SimardError::IdentityPathNotUnderPromptRoot {
    identity_path: PathBuf,
    prompt_root: PathBuf,
}
```

Produced by `FileIdentityLoader::load()` when the canonicalized identity
directory is not a descendant of the canonicalized prompt root. This
prevents directory escape via symlinks or relative paths.

---

## Type conversion reference

The TOML types use plain strings for enum fields. Conversion to domain
enums happens via serde JSON roundtrip (`serde_json::from_str(&format!("\"{value}\""))`).
This works because the domain enums use `#[serde(rename_all = "kebab-case")]`
with both `Serialize` and `Deserialize`.

| TOML string | Domain type | Valid values |
|-------------|-------------|-------------|
| `default_mode` | `OperatingMode` | `engineer`, `meeting`, `curator`, `improvement`, `gym`, `orchestrator` |
| `supported_base_types[]` | `BaseTypeId` | Any string (wrapper type, no enum validation) |
| `required_capabilities[]` | `BaseTypeCapability` | `prompt-assets`, `session-lifecycle`, `memory`, `evidence`, `reflection`, `terminal-session` |
| `summary_scope` | `MemoryScope` | `session-scratch`, `session-summary`, `decision`, `project`, `benchmark`, `untagged` |

Invalid strings for enum types produce `IdentityTomlParseError`.

---

## Composition semantics

When an identity has a non-empty `components` list,
`IdentityManifest::compose()` is used to merge the resolved component
manifests:

| Field | Merge strategy |
|-------|---------------|
| `name` | Composite identity's own name |
| `version` | From the load request's `package_version` |
| `default_mode` | Composite identity's own `default_mode` |
| `contract` | From the load request's `contract` |
| `prompt_assets` | Union of all components, deduplicated by `(id, path)` |
| `supported_base_types` | Intersection — only types common to **all** components |
| `required_capabilities` | Union — all capabilities from any component |
| `memory_policy` | All components must agree; mismatched policies → `InvalidIdentityComposition` error |
| `components` | List of component identity names |

If the intersection of `supported_base_types` is empty (components share
no common base type), composition fails with
`InvalidIdentityComposition`.

---

## Integration with IdentityLoadRequest

`FileIdentityLoader::load()` receives an `IdentityLoadRequest`:

```rust
pub struct IdentityLoadRequest {
    pub identity: String,
    pub package_version: String,
    pub contract: ManifestContract,
}
```

The loader uses:
- `request.identity` — matched against `TomlIdentity.name`
- `request.package_version` — used as the manifest `version`
- `request.contract` — passed through to the manifest `contract`

This matches the pattern established by `BuiltinIdentityLoader`.
