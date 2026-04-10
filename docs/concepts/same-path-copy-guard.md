---
title: Same-path copy guard
description: Design rationale for the canonicalize()-based guard that prevents self-copy crashes during install and update operations.
last_updated: 2026-04-10
owner: simard
---

# Same-path copy guard

[Home](../index.md) · [Concepts](../index.md#start-here)

## Problem

When `simard install` runs from the installed location (`~/.simard/bin/simard`), the source and destination paths resolve to the same file. Without a guard, `fs::copy` truncates the file to zero bytes before reading — destroying the running binary.

This mirrors [amplihack issue #4296](https://github.com/rysweet/amplihack/issues/4296) where `shutil.copytree` crashed with `SameFileError` when source and destination directories coincided.

## Guard pattern

The guard canonicalizes both paths and compares them before any I/O:

```rust
// src/cmd_install.rs — handle_install()
if let (Ok(src_canon), Ok(dst_canon)) = (current_exe.canonicalize(), dest.canonicalize())
    && src_canon == dst_canon
{
    println!("simard is already installed at {}", dest.display());
    return Ok(());
}
```

### Design decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Path comparison method | `canonicalize()` | Resolves symlinks, `..`, and relative segments; covers the case where a symlink in `$PATH` points to the installed binary |
| Failure mode when canonicalize fails | Proceed with copy | If the destination doesn't exist yet, `canonicalize()` returns `Err` — which means the paths can't be the same, so copying is safe |
| Guard scope | `handle_install()` only | `cmd_self_update` downloads to a temp directory first, so source and destination are never the same file |
| Response on match | Informational message + early return | No error — the user's intent (have simard installed) is already satisfied |

### Why not `fs::metadata` + `ino` comparison?

Inode comparison (`dev` + `ino`) would also work on Unix but:

1. Requires platform-specific code (`#[cfg(unix)]` with `MetadataExt`)
2. `canonicalize()` already resolves the same cases (symlinks, hardlinks on same path)
3. The `if let` pattern with two `Result` values is idiomatic Rust and readable

### TOCTOU consideration

There is a theoretical time-of-check-time-of-use gap between the canonicalize check and the `fs::copy` call. This is acceptable because:

- The install command is operator-initiated, not a concurrent service
- The race window is microseconds
- The consequence of a missed guard is a truncated binary, which the operator would notice immediately and can recover from by rebuilding

## Affected functions

| File | Function | Guard behaviour |
|------|----------|-----------------|
| `src/cmd_install.rs` | `handle_install()` | Prints "already installed" and returns `Ok(())` |

## Testing

The guard is exercised implicitly: running `simard install` from `~/.simard/bin/simard` prints the "already installed" message instead of corrupting the binary.

Unit tests in `cmd_install.rs` cover the path structure and copy mechanics. The canonicalize guard is best verified by integration testing (running the installed binary with `simard install`).

## Cross-reference

The Python amplihack project has an equivalent guard using `os.path.samefile()` in `src/amplihack/install.py`. See the [amplihack troubleshooting guide](https://github.com/rysweet/amplihack/blob/main/docs/troubleshooting/copytree-same-file-crash.md) for the Python-side documentation.
