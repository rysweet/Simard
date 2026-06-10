---
title: "FileIdentityLoader API reference"
description: >
  Rust API for the file-based identity loader, including construction,
  configuration, error types, and integration patterns.
last_updated: 2026-06-10
review_schedule: as-needed
owner: simard
---

# FileIdentityLoader API reference

`FileIdentityLoader` is the file-based implementation of the `IdentityLoader`
trait. It reads `identity.toml` from a configurable directory and falls back
to `BuiltinIdentityLoader` when the file or requested identity is not found.

## Module path

```rust
use simard::identity::FileIdentityLoader;
```

Re-exported from `crate::identity::file_loader::FileIdentityLoader`.

## Construction

```rust
pub fn new(
    identity_path: impl Into<PathBuf>,
    prompt_root: impl Into<PathBuf>,
) -> Self
```

| Parameter | Type | Description |
|---|---|---|
| `identity_path` | `impl Into<PathBuf>` | Directory containing `identity.toml`. |
| `prompt_root` | `impl Into<PathBuf>` | Root directory for prompt asset resolution. Typically `SIMARD_PROMPT_ROOT`. |

The constructor does not perform I/O. File access is deferred to `load()`.

```rust
let loader = FileIdentityLoader::new(
    "/opt/simard/identities",
    "/opt/simard/prompt_assets",
);
```

## Accessor

```rust
pub fn identity_path(&self) -> &Path
```

Returns the configured identity directory path.

## IdentityLoader trait

```rust
impl IdentityLoader for FileIdentityLoader {
    fn load(&self, request: &IdentityLoadRequest) -> SimardResult<IdentityManifest>;
}
```

### Load sequence

1. Construct `toml_path = identity_path/identity.toml`.
2. **Name validation**: Verify the requested identity name is ASCII
   alphanumeric + hyphens and within `IDENTITY_NAME_MAX_LEN`.
3. **Directory containment**: Canonicalize `identity_path` and verify it
   resolves under the prompt root. Returns
   `SimardError::IdentityPathNotUnderPromptRoot` on failure.
4. **Size guard**: Check `fs::metadata(&toml_path).len()` against
   `MAX_IDENTITY_FILE_SIZE` (1 MiB). If the file does not exist, delegate
   to `BuiltinIdentityLoader`.
5. **Read and parse**: `fs::read()` then `toml::from_str()`.
6. **Identity lookup**: Find `[[identities]]` entry matching
   `request.identity`.
7. **Not found in TOML**: Delegate to `BuiltinIdentityLoader`.
8. **Resolve**: Build `IdentityManifest` from TOML fields, recursively
   resolving components. Prompt asset paths are canonicalized and verified
   to resolve under the prompt root.

### Error conditions

| Error | Type | When |
|---|---|---|
| Directory escape | `SimardError::IdentityPathNotUnderPromptRoot` | Identity directory canonical path not under prompt root |
| Asset escape | `SimardError::IdentityTomlParseError` | Prompt asset canonical path not under prompt root |
| Parse failure | `SimardError::IdentityTomlParseError` | TOML syntax error, unknown field, invalid value |
| File too large | `SimardError::IdentityTomlParseError` | `metadata.len() > MAX_IDENTITY_FILE_SIZE` |
| Circular components | `SimardError::IdentityTomlParseError` | DFS cycle detected |
| Depth exceeded | `SimardError::IdentityTomlParseError` | Composition depth > `MAX_COMPOSITION_DEPTH` |
| Invalid identity name | `SimardError::IdentityTomlParseError` | Non-ASCII, too long, or contains path separators |

## Configuration integration

`FileIdentityLoader` is activated by the `SIMARD_IDENTITY_PATH` environment
variable. The wiring differs by call site:

### Bootstrap (assembly.rs)

`BootstrapConfig` includes an optional `identity_path` field. When present,
`assemble()` constructs a `FileIdentityLoader`:

```rust
let loader: Box<dyn IdentityLoader> = match &config.identity_path {
    Some(cfg) => Box::new(FileIdentityLoader::new(
        &cfg.value,
        &config.prompt_root.value,
    )),
    None => Box::new(BuiltinIdentityLoader),
};
let manifest = loader.load(&request)?;
```

### Gym executor and state root

These call sites lack `BootstrapConfig` access. They read the env var
directly:

```rust
let loader: Box<dyn IdentityLoader> = match std::env::var_os("SIMARD_IDENTITY_PATH") {
    Some(path) => Box::new(FileIdentityLoader::new(
        PathBuf::from(path),
        prompt_root.clone(),
    )),
    None => Box::new(BuiltinIdentityLoader),
};
```

## Constants

| Constant | Value | Description |
|---|---|---|
| `MAX_IDENTITY_FILE_SIZE` | 1,048,576 (1 MiB) | Maximum `identity.toml` size |
| `MAX_COMPOSITION_DEPTH` | 8 | Maximum recursive composition depth |
| `IDENTITY_NAME_MAX_LEN` | 128 | Maximum identity name length |

## Related types

- `IdentityLoader` — trait that `FileIdentityLoader` and
  `BuiltinIdentityLoader` both implement
- `IdentityLoadRequest` — request struct with identity name, package version,
  and manifest contract
- `IdentityManifest` — resolved identity with base types, capabilities, prompt
  assets, and operating mode
- `PromptAssetRef` — reference to a prompt asset file (id + relative path)

## Related documentation

- [identity.toml reference](../reference/identity-toml.md) — file format
- [Concept: Pluggable identity system](../concepts/pluggable-identity.md) — design rationale
- [How to configure a custom identity](../howto/configure-custom-identity.md) — operator playbook
