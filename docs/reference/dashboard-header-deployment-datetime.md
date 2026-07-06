---
title: Dashboard header — deployment datetime (PT)
description: Reference for the deployment-datetime field the dashboard header renders next to the build number. Covers the compile-time build-timestamp signal baked in by build.rs (SIMARD_BUILD_TIMESTAMP, SOURCE_DATE_EPOCH-aware), the additive `deployed` field on GET /api/status, the America/Los_Angeles PST/PDT conversion helpers in routes.rs, the header render path, back-compatible degradation, and the DST tests (#2727).
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../dashboard.md
  - ./status-snapshot-api.md
  - ../concepts/truthful-runtime-metadata.md
  - ./release-integrity.md
  - ./multi-binary-self-update.md
  - ../concepts/reconcile-and-self-deploy.md
---

# Dashboard header — deployment datetime (PT)

The [operator dashboard](../dashboard.md) header renders, next to the build /
version number it already shows, **when the running binary was built and
deployed** — formatted in US Pacific Time (`America/Los_Angeles`) with the
**date-correct `PST`/`PDT` abbreviation**. Operators can tell at a glance both
*which* build is live and *when* it went out, without opening a terminal.

The header now reads, for a summer (daylight-saving) build:

```
🌲 Simard Dashboard      v0.27.0.1234 (e5764c6) · deployed 2026-07-06 11:03 PDT
```

and for a winter (standard-time) build:

```
🌲 Simard Dashboard      v0.27.0.1234 (e5764c6) · deployed 2026-01-06 11:03 PST
```

The abbreviation is derived from the tz database for the *instant of the
build*, so `PST` (UTC−8) and `PDT` (UTC−7) are chosen automatically. Nothing is
hardcoded to a fixed offset.

> **Modules:** `build.rs` (bakes `SIMARD_BUILD_TIMESTAMP`);
> `src/operator_commands_dashboard/routes.rs` (`deployed_timestamp_utc()`,
> `format_deployed_pt()`, `deployed_pt()`, and the additive `deployed` field on
> `status()`); `src/operator_commands_dashboard/index_html/part_00.rs` (the
> `#header-version` span) and `part_01.rs` (`fetchStatus()` appends the field);
> tests in `src/operator_commands_dashboard/tests_routes_a.rs`.

## Contents

- [The deployment signal](#the-deployment-signal)
- [API — the `deployed` field on `GET /api/status`](#api-the-deployed-field-on-get-apistatus)
- [Pacific-time formatting and DST](#pacific-time-formatting-and-dst)
- [The header render path](#the-header-render-path)
- [Configuration](#configuration)
- [Back-compatibility and degradation](#back-compatibility-and-degradation)
- [Examples](#examples)
- [Testing](#testing)
- [See also](#see-also)

## The deployment signal

The datetime is a **compile-time build timestamp** baked into the binary by
`build.rs`, emitted as a `rustc-env` value:

```
cargo:rustc-env=SIMARD_BUILD_TIMESTAMP=<RFC3339 UTC>
```

for example `SIMARD_BUILD_TIMESTAMP=2026-07-06T18:03:00Z`.

This is the durable, deterministic "when was this deployed?" signal, chosen to
sit symmetrically alongside the two build signals the same `build.rs` already
emits — `SIMARD_BUILD_NUMBER` (the git commit count) and `SIMARD_GIT_HASH`.

**Why this signal, and not the alternatives.** In this project a build *is* a
deploy: the daemon runs the freshly compiled binary (see
[reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md)), so the
compile-time timestamp is the most accurate durable marker of when the running
build went live.

| Candidate signal | Why not chosen |
|---|---|
| Binary file `mtime` | Mutated by copies, `install`, `touch`, container layer extraction — not durable. |
| A deploy-marker file | No such marker exists in the deploy path; would be a new moving part to keep truthful. |
| Wall-clock at request time | Not a deployment fact at all — it would answer "what time is it now", which is a lie about deployment. |
| **Compile-time `SIMARD_BUILD_TIMESTAMP`** | **Durable, deterministic, travels inside the binary, symmetric with the existing build signals.** |

This never fabricates a placeholder. The value is set at build time from a real
clock (or a reproducible-build override — see [Configuration](#configuration)).
`build.rs` writes it through a sanctioned `cargo:` stdout directive; it is
**not** a stray `println!` in product code.

## API — the `deployed` field on `GET /api/status`

`GET /api/status` gains a single **additive** field, `deployed`, carrying the
pre-formatted Pacific-time string:

```json
{
  "version": "0.27.0.1234",
  "git_hash": "e5764c6d3b2a19f0c4e7a8b1d2f3a4b5c6d7e8f9",
  "ooda_daemon": "running",
  "active_processes": 3,
  "disk_usage_pct": 41,
  "timestamp": "2026-07-06T18:05:12+00:00",
  "deployed": "2026-07-06 11:03 PDT"
}
```

Contract:

- **Additive and back-compatible.** No existing key (`version`, `git_hash`,
  `ooda_daemon`, `active_processes`, `disk_usage_pct`, `timestamp`,
  `daemon_health`) is renamed, removed, or changed. Consumers that do not know
  `deployed` ignore it.
- **Value shape.** `deployed` is a `String` matching
  `^\d{4}-\d{2}-\d{2} \d{2}:\d{2} P[SD]T$` — date, 24-hour minute-precision
  local time, and the `PST`/`PDT` abbreviation, e.g. `2026-07-06 11:03 PDT`.
- **Optional by design.** When the build-timestamp env was not emitted (an
  unusual toolchain, or a compilation path that does not run `build.rs`), the
  key is **omitted entirely** rather than emitted empty or faked. See
  [degradation](#back-compatibility-and-degradation).
- **Formatted server-side.** The server produces the final display string; the
  browser never does timezone math. This keeps all DST logic in one testable
  place and out of the client.
- **Distinct from `timestamp`.** The pre-existing `timestamp` field is the
  *request* time (`chrono::Utc::now()` when `/api/status` is served, RFC3339
  UTC). `deployed` is the *build/deploy* time (fixed for the life of the binary,
  Pacific-time string). They answer different questions and are never
  interchangeable.

## Pacific-time formatting and DST

All timezone and daylight-saving logic lives in three pure, `pub(crate)`
functions in `routes.rs`, so it is unit-testable against fixed instants with no
clock, I/O, or network:

```rust
/// The UTC build/deploy instant baked at compile time (#2727).
/// `None` on toolchains where the env was not emitted → the field is skipped.
pub(crate) fn deployed_timestamp_utc() -> Option<chrono::DateTime<chrono::Utc>>;

/// Format a UTC instant as Los Angeles wall-clock with the date-correct
/// PST/PDT abbreviation. Pure; all DST logic is contained here.
pub(crate) fn format_deployed_pt(dt: chrono::DateTime<chrono::Utc>) -> String;

/// Composed, header-ready string. `None` → the `deployed` field is omitted.
pub(crate) fn deployed_pt() -> Option<String>;
```

`format_deployed_pt` converts the UTC instant to `America/Los_Angeles` using the
[`chrono-tz`](https://docs.rs/chrono-tz) tz database and formats it with chrono's
`%Y-%m-%d %H:%M %Z` pattern. The `%Z` token is the **primary** source of the
abbreviation: it renders whatever the tz database selects for that instant —
`PST` in standard time, `PDT` in daylight time — so the correct offset (UTC−8 vs
UTC−7) is applied automatically. **No fixed offset and no literal `PST`/`PDT` is
ever hardcoded.**

The abbreviation is part of the output contract, so it is not taken on faith.
`format_deployed_pt` is guarded by the [PST/PDT unit tests](#testing), which
assert the exact `PST`/`PDT` output against the **pinned** `chrono-tz` version.
`%Z` is the intended and verified path; the documented fallback — should a future
`chrono-tz` ever render `%Z` differently — is to derive the abbreviation from the
resolved offset via `.offset().abbreviation()`. The unit tests are the guard that
proves which path is in effect for the pinned version.

`chrono-tz` is the idiomatic crate for the runtime conversion and is added as a
**new `=`-pinned dependency** under `[dependencies]`, matching the repo
convention (e.g. `chrono = "=0.4.44"`). The exact pinned version is fixed at
implementation time and recorded in the pull request — the pin is what makes the
`%Z` abbreviation reproducible and lets the unit tests act as a regression guard.
Separately, `build.rs` gains a `=`-pinned `[build-dependencies]` `chrono` entry
so it can format the baked instant as an RFC3339 UTC string.

### Worked conversions

| Build instant (UTC) | Rendered (`America/Los_Angeles`) | Offset | Abbrev |
|---|---|---|---|
| `2026-07-06T18:03:00Z` | `2026-07-06 11:03 PDT` | UTC−7 | daylight |
| `2026-01-06T19:03:00Z` | `2026-01-06 11:03 PST` | UTC−8 | standard |
| `2026-03-08T10:30:00Z` (just after the 02:00→03:00 spring-forward) | `2026-03-08 03:30 PDT` | UTC−7 | daylight |
| `2026-11-01T09:30:00Z` (just after the 02:00→01:00 fall-back) | `2026-11-01 01:30 PST` | UTC−8 | standard |

## The header render path

The header markup is unchanged: the `<header>` already contains an empty
`#header-version` span (`part_00.rs`), styled at `.75rem` / `#8b949e`. The
deployment datetime is appended to that existing span, so it inherits the
header's styling and does not add a new flex child that could overflow on narrow
widths.

`fetchStatus()` in `part_01.rs` sets the span's text, appending the deployed
string only when the API returned it:

```js
document.getElementById('header-version').textContent =
  'v' + d.version + ' (' + shortHash + ')' +
  (d.deployed ? ' · deployed ' + d.deployed : '');
```

- Uses `textContent` (not `innerHTML`), so the server-formatted string is
  rendered as plain text — no injection surface.
- The `d.deployed` guard means an older payload without the field, or a build
  where the field was omitted, renders exactly the pre-#2727 header:
  `v0.27.0.1234 (e5764c6)`.

The Status panel (the **Overview → Health** "Version" row) is not changed by
this feature; the deployment datetime is a header-only addition.

## Configuration

There is **no runtime configuration** — the value is fixed at build time and
travels with the binary. Two build-time levers exist:

| Variable | Where | Effect |
|---|---|---|
| `SOURCE_DATE_EPOCH` | build environment (read by `build.rs`) | If set to a Unix-seconds integer, `build.rs` uses it as the build instant instead of the wall clock, for **reproducible builds**. `build.rs` declares `cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH`. |
| `SIMARD_BUILD_NUMBER` | build environment | Pre-existing; overrides the git-commit-count build number rendered next to the datetime. Unaffected by this feature, listed here because the two appear together in the header. |

If `SOURCE_DATE_EPOCH` is unset, `build.rs` bakes the current UTC wall-clock at
compile time.

**Reproducibility caveat.** Because the timestamp is baked per build, an
ordinary rebuild produces a new value — which is exactly the intent (a new
build is a new deploy). For deterministic, byte-reproducible builds, set
`SOURCE_DATE_EPOCH` so the baked timestamp is stable.

## Back-compatibility and degradation

Every path fails toward the pre-#2727 behaviour, never toward a fabricated
value:

| Condition | Result |
|---|---|
| `SIMARD_BUILD_TIMESTAMP` not emitted (unusual toolchain / no `build.rs`) | `option_env!` → `None` → `deployed` omitted from `/api/status` → header renders as before. |
| Malformed RFC3339 in the env (should not occur) | Parse fails → `None` → field omitted → header renders as before. |
| Old cached JS against a new payload | The extra `deployed` key is simply ignored by older client code. |
| New JS against an old payload (no `deployed`) | The `d.deployed` guard is falsy → header renders as before. |

No consumer of `/api/status` is required to change; this is a strictly additive
contract growth, consistent with the
[truthful runtime metadata](../concepts/truthful-runtime-metadata.md) principle
— the header shows a real deployment fact or shows nothing, never a placeholder.

## Examples

Read the deployment datetime over the authenticated status endpoint:

```bash
# Using the dashboard session cookie or SIMARD_DASHBOARD_TOKEN, as any /api/* call:
curl -fsS -H "Authorization: ****** \
  http://localhost:8080/api/status | jq -r '.deployed'
# → 2026-07-06 11:03 PDT
```

Confirm both facts the header shows are present together:

```bash
curl -fsS -H "Authorization: ****** \
  http://localhost:8080/api/status | jq '{version, deployed}'
# {
#   "version": "0.27.0.1234",
#   "deployed": "2026-07-06 11:03 PDT"
# }
```

Produce a byte-reproducible build whose baked datetime is fixed:

```bash
# 2026-07-06T18:03:00Z as Unix seconds:
SOURCE_DATE_EPOCH=1783404180 cargo build --release
# The header will render "deployed 2026-07-06 11:03 PDT" on every such build.
```

## Testing

`tests_routes_a.rs` covers both the DST correctness (the core requirement) and
the header/payload shape:

- **Summer → PDT.** `format_deployed_pt(2026-07-06T18:03:00Z)` equals
  `2026-07-06 11:03 PDT` (proves UTC−7 and the `PDT` abbreviation).
- **Winter → PST.** `format_deployed_pt(2026-01-06T19:03:00Z)` equals
  `2026-01-06 11:03 PST` (proves UTC−8 and the `PST` abbreviation).
- **Spring-forward edge.** An instant just after the 02:00→03:00 transition
  renders `PDT`.
- **Fall-back edge.** An instant just after the 02:00→01:00 transition renders
  `PST`.
- **Round-trip.** `deployed_timestamp_utc()` parses a known RFC3339 string back
  to the identical instant.
- **Payload / header shape.** The `/api/status` JSON, when the timestamp is
  present, carries a `deployed` string matching
  `^\d{4}-\d{2}-\d{2} \d{2}:\d{2} P[SD]T$` **alongside** the build/version
  number; the omission path (no env) leaves the object otherwise unchanged.

Tests build fixed instants with
`chrono::DateTime::parse_from_rfc3339(...).unwrap().with_timezone(&Utc)` so they
are deterministic and independent of the build machine's own
`SIMARD_BUILD_TIMESTAMP`.

The Summer→PDT and Winter→PST assertions are the **regression guard for the
abbreviation contract**: they pin the exact `%Z` output against the pinned
`chrono-tz` version. If a `chrono-tz` bump ever changed that rendering, these
tests fail first and signal the switch to the `.offset().abbreviation()`
fallback documented in [Pacific-time formatting and DST](#pacific-time-formatting-and-dst).

## See also

- [Dashboard](../dashboard.md) — the operator surface this header belongs to.
- [StatusSnapshot API reference](./status-snapshot-api.md) — the broader typed
  status surface the CLI, dashboard, and TUI share.
- [Concept: truthful runtime metadata](../concepts/truthful-runtime-metadata.md)
  — why runtime/deploy metadata must describe the build that is actually
  running, never a convenient placeholder.
- [Concept: reconcile-and-self-deploy](../concepts/reconcile-and-self-deploy.md)
  — why, in this system, a build is a deploy.
- [Release integrity reference](./release-integrity.md) and
  [multi-binary self-update](./multi-binary-self-update.md) — related
  build/version provenance surfaces.

See the [documentation index](../index.md) for the full set of Simard docs.
