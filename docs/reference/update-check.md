---
title: Automatic update check
description: "API and behavior reference for the startup update-version check against GitHub Releases."
last_updated: 2026-06-10
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../safe-self-update.md
  - ./simard-cli.md
  - ./simard-tui.md
  - ../howto/monitor-simard-with-tui.md
---

# Automatic update check

On every launch, Simard checks GitHub Releases for a newer version and
prints a notice if one is available. The check is non-blocking — it
never delays CLI startup or freezes the TUI.

## Two execution modes

| Context | Function | Behavior |
|---------|----------|----------|
| CLI (`simard`) | `run_update_check()` | Fire-and-forget: spawns a detached thread that prints to stderr. The thread is **not** joined — the CLI proceeds immediately. |
| TUI (`simard-tui`) | `run_update_check_background()` | Channel-based: spawns a thread that sends the notice string through an `mpsc::Receiver<String>`. The TUI polls `try_recv()` in its event loop; after receiving the notice, the receiver is set to `None` (one-shot). No direct stderr writes. |

The distinction matters because the TUI runs in raw mode on an
alternate screen — any uncoordinated stderr write would corrupt the
display.

## Environment variables

| Variable | Effect |
|----------|--------|
| `SIMARD_NO_UPDATE_CHECK=1` | Skip the check entirely. Both `run_update_check()` and `run_update_check_background()` return immediately (the latter returns `None`). |

> **Note:** `SIMARD_NONINTERACTIVE=1` is a project-wide env var but is *not*
> checked by the update module. The update notice is always non-interactive
> (informational only, no prompt), so `SIMARD_NONINTERACTIVE` has no effect
> on update check behavior.

## Network behavior

1. **`gh api`** — tried first. Queries `repos/{GITHUB_REPO}/releases/latest`
   via the authenticated GitHub CLI. Hard 5-second timeout (the child
   process is killed if it exceeds this).
2. **`curl` fallback** — on *any* `gh` failure (missing binary, auth
   failure, timeout, non-zero exit), falls back to a direct
   `curl -sS` request to the GitHub REST API. Connect timeout 3 seconds,
   total timeout 5 seconds. Hard kill at 6 seconds if curl hasn't
   exited (deliberately 1 second longer than `--max-time` to allow
   graceful shutdown).
3. **No caching** — every launch makes one HTTP request. The cost is
   negligible (one small JSON response) and avoids stale-cache bugs.

If both `gh` and `curl` fail, no notice is shown. The check never
produces an error message — it fails silently.

## Version comparison

### Semver parsing

`parse_semver(v)` is an internal (private) function that parses a
`"major.minor.patch[-prerelease][+build]"` string into a 4-tuple
`(major, minor, patch, is_release)`. It is not part of the public
crate API — it is tested via `#[cfg(test)]` but not exported:

| Input | Parsed |
|-------|--------|
| `"1.2.3"` | `(1, 2, 3, true)` |
| `"1.0.0-rc1"` | `(1, 0, 0, false)` |
| `"1.0.0-beta.1"` | `(1, 0, 0, false)` |
| `"1.2.3+build.456"` | `(1, 2, 3, true)` |
| `"1.2.3+build-456"` | `(1, 2, 3, true)` — build metadata (after `+`) is stripped before checking for prerelease (`-`), so hyphens inside build metadata are handled correctly. |
| `"bad"` | `None` |

The `is_release` flag (`true` for releases, `false` for prereleases)
ensures that prereleases sort *before* the corresponding release
version. Since Rust tuple comparison evaluates left-to-right and
`false < true`, the ordering is:

```
1.0.0-rc1  → (1, 0, 0, false)
1.0.0      → (1, 0, 0, true)    ← this is newer
1.0.1-beta → (1, 0, 1, false)   ← this is even newer
```

### `is_newer(latest, current)`

Internal (private) function. Returns `true` if `latest` is strictly
greater than `current` by 4-tuple comparison. Returns `false` if
either string fails to parse.

## Platform asset detection

The check reports whether a pre-built binary exists for the current
platform by scanning the release's `assets` array. Platform detection
delegates to `crate::cmd_self_update::platform::platform_suffix()`,
which returns platform identifiers matching the release naming
convention:

| Platform | Suffix |
|----------|--------|
| Linux x86_64 | `linux-x86_64` |
| Linux aarch64 | `linux-aarch64` |
| macOS x86_64 | `macos-x86_64` |
| macOS aarch64 | `macos-aarch64` |
| Windows x86_64 | `windows-x86_64` |
| Unsupported | `None` (no asset match attempted) |

The `has_platform_asset` field in `UpdateInfo` controls whether the
notice says "Run `simard self-update` to upgrade" or "(no pre-built
binary for this platform — build from source)".

## `UpdateInfo` struct

```rust
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub has_platform_asset: bool,
}
```

Returned by `check_for_update()` when a newer version exists.
`run_update_check_background()` formats this into a single notice
string before sending it through the channel.

## CLI output

When a newer version is detected, the CLI prints to stderr:

```
simard: update available v0.19.0 → v0.20.0
  https://github.com/rysweet/Simard/releases/tag/v0.20.0
  Run `simard self-update` to upgrade.
```

If there is no pre-built binary for the current platform:

```
simard: update available v0.19.0 → v0.20.0
  https://github.com/rysweet/Simard/releases/tag/v0.20.0
  (no pre-built binary for this platform — build from source)
```

The notice is yellow (ANSI `\x1b[33m`). It goes to stderr so it
does not pollute `simard <command> | jq` pipelines.

## TUI rendering

The TUI displays the update notice in the footer bar, replacing
the default keybinding-only footer:

```
Update available: v0.19.0 → v0.20.0  https://...  Run `simard self-update` to upgrade.  | Alt+1‥6: tabs | ←/→: cycle | q: quit
```

The footer text is yellow when an update notice is present, dark gray
otherwise. The notice appears 1–5 seconds after launch (once the
background check completes) and persists for the lifetime of the TUI
session.

## Source layout

```
src/update_check.rs          # All update check logic + unit tests
src/main.rs                  # CLI entry: calls run_update_check()
src/bin/simard_tui/main.rs   # TUI entry: calls run_update_check_background()
src/bin/simard_tui/app.rs    # App.update_notice field, drained from receiver
src/bin/simard_tui/ui.rs     # Footer rendering with conditional notice
src/cmd_self_update/platform.rs  # platform_suffix() — shared by update check and self-update
```

## Tests

All tests live in `src/update_check.rs` under `#[cfg(test)] mod tests`:

| Test | What it verifies |
|------|------------------|
| `parse_semver_valid` | Standard `"1.2.3"` parses to `(1,2,3,true)` |
| `parse_semver_with_prerelease` | `"-beta.1"` and `"-rc1"` set `is_release=false` |
| `parse_semver_with_build_metadata` | `"+build.456"` is ignored (still a release) |
| `parse_semver_rejects_invalid` | Non-numeric, too few/many parts, empty string |
| `is_newer_returns_true_for_higher_version` | Major, minor, patch increments |
| `is_newer_returns_false_for_same_or_older` | Equal and lower versions |
| `is_newer_handles_invalid_input` | Unparseable strings → `false` |
| `is_newer_handles_prerelease` | `1.0.0 > 1.0.0-rc1`, `1.0.0-beta.1 < 1.0.0`, `1.0.1-beta.1 > 1.0.0` |
| `current_version_is_valid_semver` | `CARGO_PKG_VERSION` parses successfully |
| `platform_suffix_is_not_unknown` | Platform suffix resolves on CI platforms |
| `fetch_via_gh_returns_none_when_binary_missing` | `gh` call does not panic |
| `run_update_check_background_returns_receiver` | Disabled → `None`, enabled → `Some` |

Run with:

```bash
cargo test -- update_check
```

## Security considerations

- **No credentials exposed** — `gh` uses existing auth; `curl` hits
  the public API. No tokens are passed on the command line.
- **No auto-execution** — the notice is informational. The user must
  explicitly run `simard self-update` to upgrade.
- **Timeouts enforced** — both `gh` and `curl` have hard timeouts
  with `child.kill()` to prevent indefinite hangs.
- **stderr only** — update notices never appear in stdout, preventing
  injection into piped command output.
- **`mpsc::Receiver` is `!Sync`** — only polled from the TUI event
  loop thread, eliminating data races.

## See also

- [Safe self-update](../safe-self-update.md) — the full
  drain → snapshot → pre-test → swap → validate → rollback flow.
- [simard CLI reference](./simard-cli.md) — CLI command tree.
- [Monitor with TUI](../howto/monitor-simard-with-tui.md) — TUI usage guide.
