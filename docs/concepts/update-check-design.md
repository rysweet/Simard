---
title: Update check design
description: "Why the update check works the way it does — fire-and-forget for CLI, channel-based for TUI, prerelease-aware semver, shared platform detection."
last_updated: 2026-06-10
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/update-check.md
  - ../howto/check-for-updates.md
  - ../safe-self-update.md
---

# Update check design

This document explains the design decisions behind Simard's automatic
update check. For usage, see the [how-to](../howto/check-for-updates.md).
For the full API, see the [reference](../reference/update-check.md).

## Problem

Users running older versions of Simard miss bug fixes and new
features. The update check ensures they are informed without
disrupting their workflow.

Three constraints shaped the design:

1. **Never block startup.** A CLI tool that stalls for 5 seconds on
   every launch is unusable. The check must be fully asynchronous.
2. **Never corrupt the TUI.** The TUI runs in raw mode on an alternate
   screen. Any uncoordinated write to stderr or stdout corrupts the
   display. The check must communicate through a controlled channel.
3. **Never lie about versions.** A prerelease like `1.0.0-rc1` must
   not be treated as equal to or newer than `1.0.0`. Version
   comparison must be prerelease-aware.

## Design decisions

### Fire-and-forget for CLI

`run_update_check()` spawns a thread and does **not** join it. The
thread runs independently: it queries GitHub, compares versions, and
prints to stderr if needed. If `main()` finishes before the thread
completes, the thread is silently terminated by process exit.

This means:

- CLI startup latency is zero (thread spawn is ~microseconds).
- For very fast commands (`simard --version`), the notice may not
  appear because the process exits before the HTTP request completes.
  This is acceptable — the user will see the notice on the next
  command that takes longer.
- The thread writes to stderr, which is safe for CLI because no other
  code is competing for stderr in a meaningful way.

### Channel-based for TUI

`run_update_check_background()` returns an `Option<mpsc::Receiver<String>>`.
The TUI stores the receiver in the `App` struct and calls `try_recv()`
on each event-loop tick (the poll timeout is 2 seconds, but this is not
a dedicated timer — any input event also triggers a tick). When the
notice arrives, it is stored in `App.update_notice`, the receiver is
set to `None` (one-shot), and the notice is rendered in the footer.

This follows the same pattern already used for GitHub stats in the
Stats tab — a background thread sends data through a channel, and the
TUI polls it during its normal render loop. The pattern works because:

- `mpsc::Receiver` is `!Sync` but `Send`, matching the TUI's
  single-threaded event loop.
- `try_recv()` is non-blocking, so it adds no latency to the render
  cycle.
- The notice is rendered by the TUI's own draw code, so it respects
  the alternate screen and raw mode.

### Prerelease-aware semver

`parse_semver()` returns a 4-tuple `(major, minor, patch, is_release)`
where `is_release` is `true` for releases and `false` for prereleases.
Rust's tuple comparison evaluates left-to-right, and `false < true`,
so:

```
(1, 0, 0, false)  <  (1, 0, 0, true)
 ↑ 1.0.0-rc1          ↑ 1.0.0
```

This means:

- `is_newer("1.0.0", "1.0.0-rc1")` → `true` (release is newer than
  its prerelease).
- `is_newer("1.0.0-rc1", "1.0.0")` → `false` (prerelease is not
  newer than its release).
- `is_newer("1.0.1-beta", "1.0.0")` → `true` (higher patch wins
  regardless of prerelease).

The alternative — stripping prerelease metadata entirely — would make
`1.0.0-rc1` compare equal to `1.0.0`, hiding the fact that the user
is on a prerelease and a stable release is available.

### Shared platform detection

The update check reuses `cmd_self_update::platform::platform_suffix()`
for asset detection, rather than maintaining its own platform strings.
This ensures that:

- Platform names match the release asset naming convention (`macos-*`,
  not `darwin-*`).
- Changes to the naming convention only need to be made in one place.
- The `self-update` command and the update check agree on which
  platforms have pre-built binaries.

### No caching

The check makes a fresh HTTP request on every launch. The cost is one
small JSON response (~2KB) per launch. Caching was considered and
rejected because:

- It adds disk I/O and cache-invalidation logic for negligible gain.
- The check runs in a background thread, so the network latency is
  invisible to the user.
- Stale-cache bugs (showing "up to date" when a new release exists)
  are worse than the cost of one HTTP request.

### Dual transport (gh + curl)

The check tries `gh api` first because it handles authentication
(private repos, rate limits) automatically. If `gh` fails for any
reason — missing binary, auth failure, timeout, non-zero exit — it
falls back to `curl` against the public GitHub API. Both have hard
timeouts with `child.kill()` to prevent indefinite hangs.

## Relationship to safe-update

The update check is *informational only* — it tells the user a newer
version exists. It does not download, install, or execute anything.

`simard self-update` is the manual upgrade command. `simard safe-update`
is the autonomous upgrade flow with drain, snapshot, pre-test, swap,
validate, and rollback phases. The update check and these commands are
independent: the check runs on every launch, the upgrade commands run
only when explicitly invoked.

## See also

- [Update check reference](../reference/update-check.md) — full API.
- [Check for updates how-to](../howto/check-for-updates.md) — usage.
- [Safe self-update](../safe-self-update.md) — autonomous upgrade flow.
