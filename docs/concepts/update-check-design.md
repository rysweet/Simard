---
title: Update check design
description: "Why the launch-time update check works the way it does — in-process ureq, the semver crate, a 24h cache, a bounded upgrade prompt, and fail-open-but-surfaced error handling."
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: concept
related:
  - ../reference/update-check.md
  - ../howto/check-for-updates.md
  - ../safe-self-update.md
---

# Update check design

This document explains the design decisions behind Simard's launch-time
update check (issue #2250). For usage, see the
[how-to](../howto/check-for-updates.md). For the full API and behavior
contract, see the [reference](../reference/update-check.md).

## Problem

Operators running older versions of Simard miss bug fixes and new
features. The update check informs them at launch without disrupting the
workflow, and — in an interactive terminal — offers a friendly path to
upgrade.

Five constraints shaped the design:

1. **Never block startup.** A CLI tool that stalls on every launch is
   unusable. The check must be fully asynchronous and time-bounded.
2. **Never corrupt the TUI.** The TUI runs in raw mode on an alternate
   screen. Any uncoordinated write to stderr/stdout — or a blocking
   stdin prompt — corrupts the display. The TUI path must communicate
   through a controlled channel and never prompt.
3. **Never lie about versions.** A pre-release like `1.0.0-rc1` must not
   be treated as equal to or newer than `1.0.0`. Comparison must be
   pre-release-aware.
4. **Fail open, but surface the failure.** A GitHub API or network error
   is a convenience-check failure, not a core-logic failure — it must
   never block or crash the binary. But the failure must be *logged*,
   not silently swallowed (surface, don't hide).
5. **Be a safe notifier, not an installer.** The check reports that
   something newer exists. It must never download, install, or execute
   an upgrade on its own.

## Design decisions

### In-process HTTP via `ureq` (not a subprocess)

The check fetches the GitHub Releases API directly with the
[`ureq`](https://docs.rs/ureq/3.3.0) client — a `ureq::Agent` with a
5-second `timeout_global`, a `User-Agent`, and the standard GitHub
`Accept` / `X-GitHub-Api-Version` headers — over `rustls` TLS only.

Earlier iterations shelled out to `gh api` with a `curl` fallback. That
was replaced because:

- **No `PATH`-hijack surface.** Spawning `gh`/`curl` means trusting
  whatever binary happens to be first on `PATH`. An in-process client
  removes that attack surface entirely.
- **No process-management complexity.** No child-process spawn,
  timeout-kill, or stdout-plumbing code to get right.
- **`ureq` is already in the dependency tree.** Promoting it to a direct,
  pinned (`=3.3.0`) dependency reuses what the build already carries,
  per the #2250 "reuse existing deps" constraint.

The request is deliberately **unauthenticated**: this is a public,
read-only notifier, so no token is ever attached and there is no
credential to leak. Unauthenticated GitHub requests are rate-limited to
60/hour/IP; the 24-hour cache and fail-open behavior make that a
non-issue (a 403 rate-limit response just becomes `None` + a warning).

### The `semver` crate (not a hand-rolled parser)

Version comparison uses the [`semver`](https://docs.rs/semver/1.0.28)
crate (also promoted to a direct `=1.0.28` dependency), replacing the
earlier bespoke tuple parser. The GitHub `tag_name` (with any leading
`v` stripped) and `env!("CARGO_PKG_VERSION")` are each parsed into a
`semver::Version`, and an update is reported only when the latest is
**strictly greater**.

Delegating to `semver` gives correct, well-tested pre-release ordering
for free:

```
1.0.0-rc1  <  1.0.0  <  1.0.1-beta.1
```

So `is_newer("1.0.0", "1.0.0-rc1")` is `true` (release beats its
pre-release), while `is_newer("1.0.0-rc1", "1.0.0")` is `false`. The
alternative — stripping pre-release metadata — would make `1.0.0-rc1`
compare equal to `1.0.0` and hide the fact that a stable release is
available. Using the crate also means Simard's notion of "newer" matches
the ecosystem's, with less code to maintain.

### A 24-hour cache (reversing the earlier "no cache" call)

The result is cached at `~/.simard/update_cache.json` — under Simard's
standard state root (`SIMARD_STATE_ROOT` if set, otherwise
`$HOME/.simard`) — with a 24-hour TTL. A fresh cache short-circuits the
network entirely.

Placing the cache under the shared state root (rather than an XDG
`~/.config` path) keeps it alongside the rest of Simard's on-disk state —
agent registry, snapshots, `config.toml` — so a single `SIMARD_STATE_ROOT`
override relocates everything together, and there is no second location
convention to remember.

This **reverses an earlier design choice** that deliberately avoided
caching. The trade-off changed for two reasons:

- **Unauthenticated rate limits.** Once the check stopped using `gh`'s
  authenticated quota and moved to unauthenticated requests, one HTTP
  call per launch became a real concern for operators who launch `simard`
  frequently. A 24h cache bounds that to at most one request per day.
- **Stale-cache risk is contained.** The classic objection to caching —
  showing "up to date" when a release exists — is bounded to a 24-hour
  window, which is acceptable for a convenience notifier, and the TTL is
  short enough that operators still learn about releases promptly.

The cache is treated as **untrusted input**: on read it is re-validated
(semver re-parse, URL re-allowlist) and re-sanitized exactly like a live
response, written atomically (temp file + `rename`) with `0700`/`0600`
permissions, and never followed through a symlink. A corrupt, missing,
or unwritable cache is a harmless miss — never a hard error — which keeps
the whole path fail-open even when `HOME` is unset.

### Fail-open, but surfaced via `tracing`

Every failure mode — DNS/connect error, timeout, non-2xx status, rate
limit, oversized body, malformed JSON, unparseable semver — resolves to
`None`. The launch never blocks and the process never panics.

Crucially, these are **not silently swallowed**: each failure is logged
via `tracing::warn!` with the failure *category* (never the raw body or
headers). This satisfies the #2250 "surface, don't hide" requirement —
an operator debugging "why don't I see update notices?" can find the
reason in the logs, while normal launches stay quiet and fast.

### Fire-and-forget for CLI, channel-based for TUI

`run_update_check()` spawns a detached thread and does **not** join it.
CLI startup latency is effectively zero; for very fast commands
(`simard --version`) the notice may not appear because the process exits
first, which is fine — the operator sees it on the next longer command.
Writing to stderr is safe here because nothing else competes for it.

`run_update_check_background()` instead returns an
`Option<mpsc::Receiver<String>>`. The TUI polls `try_recv()` in its
event loop, renders the notice in its own draw cycle, then drops the
receiver (one-shot). This reuses the exact pattern already used for
GitHub stats in the TUI: a background thread feeds a channel that the
single-threaded render loop drains, so the notice always respects the
alternate screen and raw mode. The TUI therefore never writes stray
stderr and never blocks on a prompt.

The notice string sent through the channel is formatted with the **same
version convention as the CLI banner** — `X.Y.Z -> A.B.C`, an ASCII `->`
arrow and no `v` prefix. This is a deliberate reformat: an earlier TUI
build used a Unicode arrow with `v`-prefixed versions
(`v0.26.0 → v0.27.0`), which drifted from the CLI banner. Standardizing
both surfaces on one format keeps them consistent and lets a single
banner-format test cover the version rendering.

### A bounded, non-installing upgrade prompt

In an interactive terminal the CLI follows the banner with
`Upgrade now? [y/N]`, read on a background thread with a ~10-second
`recv_timeout` and a default of **No**. Timeout, EOF, a non-TTY stdin,
`SIMARD_NONINTERACTIVE=1`, and the TUI context all resolve to No, so the
prompt can never hang an unattended or piped launch.

The prompt is deliberately **display-only**: answering `y` prints a hint
to run `simard update` and does nothing else. Actual upgrading —
download, integrity verification, binary swap — stays behind the
explicit, sha256/signature-gated `simard update` command
(`cmd_self_update`). Keeping the notifier and the installer strictly
separate means a spoofed release response can, at worst, mislead; it can
never cause code execution.

### Exact, sanitized banner text

The banner's first line is the literal
`simard: update available X.Y.Z -> A.B.C` — an ASCII `->` and no `v`
prefix — so it is stable and testable. The versions printed are the
re-serialized semver values, and the `release_url` / `release_notes`
carried in `UpdateInfo` are stripped of ESC/BEL/CR and C0/C1 control
characters. Together with host-allowlisting the `release_url` to
`github.com/rysweet/Simard/`, this prevents a malicious release body from
injecting terminal escape sequences or phishing links through the notice.

## Relationship to the upgrade commands

The update check is *informational only* — it tells the operator a newer
version exists and, at most, hints at the upgrade command. It never
downloads, installs, or executes anything.

- `simard update` is the manual, integrity-verified upgrade command.
- `simard safe-update` is the autonomous upgrade flow with drain,
  snapshot, pre-test, swap, validate, and rollback phases (see
  [Safe self-update](../safe-self-update.md)).

The check and these commands are independent: the check runs at every
launch; the upgrade commands run only when explicitly invoked. Removing
`has_platform_asset` from `UpdateInfo` (#2250) reinforced this split —
platform-asset selection is the installer's job, not the notifier's.

## See also

- [Update check reference](../reference/update-check.md) — full API.
- [Check for updates how-to](../howto/check-for-updates.md) — usage.
- [Safe self-update](../safe-self-update.md) — autonomous upgrade flow.
