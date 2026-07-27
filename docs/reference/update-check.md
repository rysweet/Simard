---
title: Automatic update check
description: "API and behavior reference for the launch-time update-version check against GitHub Releases (in-process ureq, semver-crate comparison, 24h cache, fail-open)."
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../safe-self-update.md
  - ./simard-cli.md
  - ./simard-tui.md
  - ../howto/monitor-simard-with-tui.md
  - ../concepts/update-check-design.md
---

# Automatic update check

On every launch, Simard checks GitHub Releases for a newer version and,
if one is available, prints a one-line notice to stderr. In an
interactive terminal it may then offer a one-key upgrade prompt. The
check is **non-blocking** and **fail-open**: it never delays CLI
startup, never freezes the TUI, and a GitHub API or network failure
never blocks or crashes the binary (issue #2250).

The banner and prompt are the only user-visible output, and both live
in the binary launch path — the library code emits no stray prints.

## Which binaries run the check

The check is wired into the launch path of every Simard entry point:

| Entry point | Function | Notes |
|-------------|----------|-------|
| `simard` (`src/main.rs`) | `run_update_check()` | Covers every `simard` subcommand, including `simard meeting`, because the check runs once at process launch before subcommand dispatch. |
| `simard-tui` (`src/bin/simard_tui/main.rs`) | `run_update_check_background()` | Channel-based so the notice never writes directly to the alternate screen. |

Because `meeting` is a subcommand of `simard` (not a separate binary),
the meeting launch path inherits the `simard` check automatically —
there is no separate `simard-meeting` binary to wire.

> **Reader-list consistency (issue #1055).** The two rows above are the
> **complete and exact** set of launch paths that invoke the update
> check: the `simard` CLI (`src/main.rs`, calling `run_update_check()`)
> and `simard-tui` (`src/bin/simard_tui/main.rs`, calling
> `run_update_check_background()`). The `SIMARD_NO_UPDATE_CHECK`
> environment variable itself is read in one central place —
> `src/update_check.rs` — so both entry points honor it through the same
> guard. The `mod tests` suite asserts that **both** of those functions
> short-circuit on `SIMARD_NO_UPDATE_CHECK=1`; keeping this table aligned
> with the wired entry points is therefore a review-time invariant. (The
> unit tests cover the shared functions, not each binary's `main()`, so
> adding a *new* entry point still requires updating this table by hand.)

## Two execution modes

| Context | Function | Behavior |
|---------|----------|----------|
| CLI (`simard`) | `run_update_check()` | Fire-and-forget: spawns a detached thread that prints the banner to stderr and, when the session is interactive, offers the upgrade prompt. The thread is **not** joined — the CLI proceeds immediately. |
| TUI (`simard-tui`) | `run_update_check_background()` | Channel-based: spawns a thread that sends the notice string through an `mpsc::Receiver<String>`. The TUI polls `try_recv()` in its event loop, renders the notice in its own draw cycle, then drops the receiver (one-shot). No direct stderr writes, and **never** a prompt. |

The TUI runs in raw mode on an alternate screen; any uncoordinated
stderr write — or a blocking stdin prompt — would corrupt the display,
so the TUI path is banner-only and channel-delivered.

## Environment variables

| Variable | Effect |
|----------|--------|
| `SIMARD_NO_UPDATE_CHECK=1` | **Full short-circuit.** The check returns before touching the cache, the network, or the prompt. `run_update_check()` returns immediately; `run_update_check_background()` returns `None`. |
| `SIMARD_NONINTERACTIVE=1` | **Prompt suppressed, banner kept.** The update banner still prints (informational), but the interactive `Upgrade now? [y/N]` prompt is skipped. |

Precedence: `SIMARD_NO_UPDATE_CHECK` wins. If it is set to `1`, nothing
runs at all, regardless of `SIMARD_NONINTERACTIVE`.

The prompt is *also* suppressed automatically when stdin/stderr is not a
TTY (`std::io::IsTerminal`) and in the TUI, independent of any env var.

## `UpdateInfo` struct

```rust
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub release_notes: String,
}
```

Returned by `check_for_update()` when — and only when — a strictly
newer version exists. `run_update_check_background()` formats this into
a single notice string before sending it through the channel.

> **Changed in #2250.** The earlier `has_platform_asset` field was
> removed. Platform-asset selection now lives entirely in the
> `simard update` command (`cmd_self_update`); the update check is a
> pure "is there something newer?" notifier and carries the release
> notes instead.

## Public API

```rust
/// Fire-and-forget CLI entry point. Spawns a detached thread; never blocks.
pub fn run_update_check();

/// TUI entry point. Returns a channel the TUI polls for the notice string,
/// or `None` when the check is disabled via `SIMARD_NO_UPDATE_CHECK=1`.
pub fn run_update_check_background() -> Option<std::sync::mpsc::Receiver<String>>;

/// Core check. Returns `Some(UpdateInfo)` only when GitHub's latest release
/// is strictly newer than `env!("CARGO_PKG_VERSION")`; `None` otherwise
/// (including on any network/API/parse error — fail-open).
pub fn check_for_update() -> Option<UpdateInfo>;
```

These three entry points are the stable, wired surface. `main.rs` and
`simard_tui/main.rs` depend on `run_update_check()` and
`run_update_check_background()` respectively; their signatures are
frozen.

## Network behavior

The check queries the GitHub Releases API **in-process** using
[`ureq`](https://docs.rs/ureq/3.3.0) — no `gh`/`curl` subprocess:

```
GET https://api.github.com/repos/rysweet/Simard/releases/latest
```

Request characteristics:

| Property | Value |
|----------|-------|
| HTTP client | `ureq` `Agent`, TLS via `rustls` only |
| Global timeout | 5 seconds (`Agent` `timeout_global`) |
| `User-Agent` | `simard/{CARGO_PKG_VERSION}` (GitHub requires a UA) |
| `Accept` | `application/vnd.github+json` |
| `X-GitHub-Api-Version` | `2022-11-28` |
| Authentication | **None** — read-only, unauthenticated, no token ever attached |
| Body cap | 256 KiB (larger responses are rejected) |
| Status handling | Non-2xx responses are rejected → `None` |

On success the JSON is parsed defensively (no `unwrap`) and mapped to:

| JSON field | `UpdateInfo` field | Transform |
|------------|--------------------|-----------|
| `tag_name` | `latest_version` | Leading `v` stripped (`v0.27.0` → `0.27.0`) |
| `html_url` | `release_url` | Host-allowlisted (see [Security](#security-considerations)) |
| `body` | `release_notes` | Sanitized of terminal control characters |

Any failure at any step — DNS/connect error, timeout, non-2xx, rate
limit (HTTP 403), oversized body, malformed or missing JSON fields —
resolves to `None`. The failure is surfaced via `tracing::warn!`
(category only, never the raw body or headers), **not** silently
swallowed. Launch is never blocked and the process never panics.

## Cache

To avoid one HTTP request per launch, the result is cached on disk with
a 24-hour TTL.

| Property | Value |
|----------|-------|
| Path | `<state_root>/update_cache.json`, i.e. `~/.simard/update_cache.json`, honoring `SIMARD_STATE_ROOT` |
| TTL | 24 hours |
| Directory perms | `0700` (created if missing) |
| File perms | `0600` |
| Write | Atomic: write to a temp file, then `rename(2)` into place |
| Symlinks | Refused (the check will not follow a symlinked cache path) |

The cache path follows Simard's standard state-root convention:
`SIMARD_STATE_ROOT` if set, otherwise `$HOME/.simard`. It shares that
root with the rest of Simard's on-disk state (agent registry, snapshots,
`config.toml`) rather than using an XDG path, so a single override moves
all state together.

Serialized shape:

```json
{
  "last_check_epoch_secs": 1751771000,
  "latest_version": "0.27.0",
  "release_url": "https://github.com/rysweet/Simard/releases/tag/v0.27.0",
  "release_notes": "Fixes and improvements…"
}
```

Cache flow inside `check_for_update()`:

1. **Fresh cache (< 24h):** skip the network entirely and reuse the
   stored result. The cached strings are treated as **untrusted** — they
   are re-validated (semver re-parse, host re-allowlist) and re-sanitized
   before use, exactly as if they had just come off the wire.
2. **Missing / expired / corrupt cache:** perform the network fetch,
   then write the fresh result back to the cache. A corrupt or
   unreadable cache is never a hard error — it is treated as a miss.
3. **No writable cache location** (e.g. no `HOME` and no
   `SIMARD_STATE_ROOT`, so the state root cannot be resolved): the cache
   is simply disabled. The check proceeds with a network fetch and
   remains fail-open.

## Version comparison

Version comparison uses the [`semver`](https://docs.rs/semver/1.0.28)
crate (promoted to a direct dependency by #2250), not a hand-rolled
parser:

- The GitHub `tag_name` (with any leading `v` stripped) is parsed into a
  `semver::Version`.
- `env!("CARGO_PKG_VERSION")` is parsed the same way.
- A newer version is reported **only when the latest is strictly
  greater** than the current version, using semver's own ordering.

Semver ordering handles pre-releases correctly, so `1.0.0-rc1` is
*older* than `1.0.0` and the check will not offer to "upgrade" a stable
build to a pre-release:

| `current` | `latest` | Result |
|-----------|----------|--------|
| `0.26.0` | `0.27.0` | `Some(UpdateInfo)` (newer) |
| `0.26.0` | `0.26.0` | `None` (same) |
| `0.27.0` | `0.26.0` | `None` (older) |
| `1.0.0` | `1.0.0-rc1` | `None` (pre-release is not newer) |
| `1.0.0-rc1` | `1.0.0` | `Some(UpdateInfo)` (release beats its pre-release) |

If either string fails to parse as semver, the result is `None` (never a
panic).

## Banner output

When a strictly newer version is detected, the CLI prints exactly two
lines to **stderr**:

```
simard: update available 0.26.0 -> 0.27.0
  https://github.com/rysweet/Simard/releases/tag/v0.27.0
```

Contract details:

- The first line is the literal format
  `simard: update available X.Y.Z -> A.B.C` — an **ASCII** `->` arrow,
  and **no** `v` prefix on the versions. (Tests assert this exact
  string.)
- The second line is the allowlisted `release_url`.
- Output goes to **stderr only**, so it never pollutes
  `simard <command> | jq` pipelines.
- The versions printed are the parsed, re-serialized semver values, so
  no attacker-controlled bytes from the API response reach the terminal
  in the version line.

## Interactive upgrade prompt

In an interactive terminal (and only there), the banner is followed by a
prompt on stderr:

```
Upgrade now? [y/N] 
```

Behavior:

| Condition | Outcome |
|-----------|---------|
| User types `y` / `Y` + Enter | Prints the hint `Run \`simard update\` to upgrade.` — it does **not** install anything in place. |
| User types anything else, or just Enter | Treated as **No**. |
| ~10-second timeout elapses with no input | Treated as **No**. |
| EOF / non-TTY stdin | Treated as **No**. |
| `SIMARD_NONINTERACTIVE=1` | Prompt skipped (banner still shown). |
| TUI (`simard-tui`) | Prompt never shown. |

The prompt runs with a bounded (~10s) `recv_timeout` so it can never
hang startup. It is display-only: choosing `y` never triggers a
privileged install. Actual upgrading stays behind the explicit,
integrity-gated `simard update` command (`cmd_self_update`).

## TUI rendering

The TUI displays the update notice in the footer bar, prepended to the
default keybinding hints. The notice string uses the **same version
format as the CLI banner** — an ASCII `->` arrow and no `v` prefix — so
the two surfaces stay consistent:

```
Update available: 0.26.0 -> 0.27.0  https://github.com/rysweet/Simard/releases/tag/v0.27.0  | Alt+1–9: tabs | Tab/Shift+Tab: cycle | ←/→: prev/next | q: quit
```

The keybinding tail (`Alt+1–9: tabs | Tab/Shift+Tab: cycle | ←/→:
prev/next | q: quit`) is the TUI's existing footer, rendered by
`src/bin/simard_tui/ui.rs`; only the `Update available: …` prefix and the
highlight are added when a notice is present. The footer text is
highlighted while a notice is set. The notice appears shortly after
launch (once the background check completes, or immediately from a fresh
cache) and persists for the lifetime of the TUI session. The TUI never
shows the `[y/N]` prompt.

## Source layout

```
src/update_check.rs               # All update-check logic + unit tests
src/main.rs                       # CLI entry: run_update_check()
src/bin/simard_tui/main.rs        # TUI entry: run_update_check_background()
src/bin/simard_tui/app.rs         # App.update_notice, drained from the receiver
src/bin/simard_tui/ui.rs          # Footer rendering with the conditional notice
Cargo.toml                        # ureq = "=3.3.0", semver = "=1.0.28" (direct deps)
```

## Tests

All tests live in `src/update_check.rs` under `#[cfg(test)] mod tests`:

| Test intent | What it verifies |
|-------------|------------------|
| semver-newer → `Some` | A strictly newer latest version yields `Some(UpdateInfo)`. |
| same/older → `None` | Equal or older latest version yields `None`. |
| fresh cache → no network | A cache entry younger than 24h short-circuits before any HTTP request. |
| opt-out → no-op | `SIMARD_NO_UPDATE_CHECK=1` returns immediately (CLI) / `None` (TUI) with no cache or network access. |
| malformed API response → `None` | A truncated/garbage JSON body parses to `None` and never panics. |
| non-interactive → banner, no prompt | `SIMARD_NONINTERACTIVE=1` keeps the banner but skips the prompt. |
| banner format | Exact string `simard: update available X.Y.Z -> A.B.C` (ASCII arrow, no `v`). |
| reader-list consistency (#1055) | Both wired entry-point functions (`run_update_check`, `run_update_check_background`) short-circuit on `SIMARD_NO_UPDATE_CHECK=1`; the shared guard in `src/update_check.rs` keeps every launch path consistent. |

Environment-variable and cache-file tests are serialized with
`#[serial_test::serial(update_check_env, cognitive_memory)]` and use a
`tempdir`-isolated cache path, because glibc `getenv`/`setenv` are not
thread-safe under cargo's multi-threaded test runner (see issue #2360).
Each such test also **saves and restores** `SIMARD_NO_UPDATE_CHECK`
(and `XDG_CONFIG_HOME`) around its body so it cannot leak an env value
into a sibling test sharing the `update_check_env` serial token. The
PR #1055 fix delivered here is the **reader-list alignment** described
above — the reference table and the `mod tests` invariant now pin the
exact set of launch paths that honor `SIMARD_NO_UPDATE_CHECK` — so docs
and code cannot drift. The serial guards and existing env save/restore
are preserved exactly; no test logic was weakened.

Run with:

```bash
cargo test -- update_check
```

## Security considerations

- **Unauthenticated, read-only notifier.** No token or `Authorization`
  header is ever attached, so there is no credential to leak. No control
  flow is derived from the response body — a spoofed response can at
  worst mislead, never execute.
- **No auto-install.** Choosing `y` at the prompt only prints the
  `simard update` hint. Privileged install stays behind the
  explicit, sha256/signature-gated `cmd_self_update` path; the check
  never invokes it.
- **Terminal-escape sanitization.** All response-derived strings
  (`release_notes`, `release_url`) are stripped of ESC / BEL / CR and
  C0/C1 control characters before display, so a malicious release body
  cannot inject terminal escape sequences. The banner prints re-parsed
  semver, never raw tag bytes.
- **Phishing-resistant URL.** `release_url` is host-allowlisted to
  `github.com/rysweet/Simard/`; anything else falls back to a hardcoded
  releases URL, so a tampered `html_url` cannot redirect operators to an
  attacker's page.
- **No subprocess fetch.** Using in-process `ureq` (not `gh`/`curl`)
  removes the `PATH`-hijack attack surface of shelling out.
- **Bounded launch cost.** 5-second `timeout_global`, a 256 KiB body
  cap, a background thread, and the 24h cache bound the network work so
  the check can never become a launch-time DoS.
- **Untrusted cache.** The cache is treated as attacker-controllable:
  re-validated and re-sanitized on read, written atomically with `0700`
  dir / `0600` file permissions, and never followed through a symlink.
- **TLS-only transport** via `rustls`; failure logging records the
  failure category only — never the raw response body or headers.

## See also

- [Update check design](../concepts/update-check-design.md) — why it
  works this way.
- [Check for updates how-to](../howto/check-for-updates.md) — usage.
- [Safe self-update](../safe-self-update.md) — the autonomous daemon
  drain → snapshot → pre-test → swap → validate → rollback flow.
- [simard CLI reference](./simard-cli.md) — CLI command tree.
- [Monitor with TUI](../howto/monitor-simard-with-tui.md) — TUI usage guide.
