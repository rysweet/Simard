---
title: "Concept: self-deploy release adoption (closing the update-available gap)"
description: >
  Why the live operator could sit two minor versions behind a green, published
  release ("update available 0.31.0 → 0.33.1") even though detection worked, and
  how Simard closes the gap: a fail-closed semver adoption gate and a
  self-test-gated auto-adoption trigger that reuses the hardened safe-update
  chain so a newer published release is detected AND adopted — or the operational
  root cause is surfaced explicitly.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: concept
status: planned
related:
  - ./reconcile-and-self-deploy.md
  - ./update-check-design.md
  - ../safe-self-update.md
  - ../reference/self-deploy-release-adoption-api.md
  - ../reference/update-check.md
  - ../reference/self-deploy-api.md
  - ../reference/multi-binary-self-update.md
  - ../../src/update_check.rs
  - ../../src/cmd_self_update/update.rs
---

# Concept: self-deploy release adoption

> **Status: planned (spec).** This document specifies the fail-closed semver
> adoption gate and the self-test-gated auto-adoption trigger **to be built** in
> [`src/cmd_self_update/update.rs`](https://github.com/rysweet/Simard/blob/main/src/cmd_self_update/update.rs),
> reusing the release-detection surface in
> [`src/update_check.rs`](https://github.com/rysweet/Simard/blob/main/src/update_check.rs).
> As of this writing the code still gates with raw string equality
> (`version == CURRENT_VERSION`) at **both** `handle_self_update` (`update.rs:70`)
> and `handle_self_update_download_only` (`update.rs:128`), and `is_newer` is a
> private helper in `update_check.rs`. The adoption trigger will route exclusively
> through the hardened `download_and_replace` safe-update chain — it adds no
> bypass path.

## The gap this closes

`simard status` would report **"update available 0.31.0 → 0.33.1"** — the live
operator running two minor versions behind a release that built green
(`verify` + `release` succeeded on `main`). Detection was never the problem: the
[launch-time update check](../reference/update-check.md) correctly fetched the
latest GitHub release and printed the notice. The gap was **adoption**: nothing
turned a detected newer release into a running upgrade without an operator typing
`simard self-update`, and one fragile comparison could silently skip a valid
upgrade.

Two defects combined to keep the operator stale:

1. **Fragile version gate — on both update paths.** Both `handle_self_update`
   (`update.rs:70`) and the safe-update path `handle_self_update_download_only`
   (`update.rs:128`) decide "already current" with raw string equality
   (`version == CURRENT_VERSION`). Any tag-shape difference (a `v` prefix,
   differing normalization) made a strictly-newer release read as
   "not equal → keep going" or, worse in adjacent paths, silently no-op. A string
   compare cannot answer "is this strictly newer?". Hardening one path and not the
   other would leave the safe-update path fragile, so **both** sites move to the
   semver gate.
2. **No adoption trigger.** Detection printed a notice and stopped. Nothing
   closed the loop from "newer release exists" to "newer release is running".

## The fix: fail-closed semver + self-test-gated adoption

### 1. Fail-closed semver adoption gate

The adoption decision is made by `update_check::is_newer(current, latest)`,
which parses both sides with the `semver` crate and returns `true` **only** when
`latest` is a valid version strictly greater than `current`. Adoption is
**fail-closed**: a malformed or unparseable remote tag yields `false` (no
update), never a coerced "newer". A `v` prefix is stripped before comparison so
tag-shape differences can no longer suppress a real upgrade.

> **Terminology note (reconciling the existing code comment).** `is_newer`
> already exists in `update_check.rs`, and its current doc-comment describes it as
> **"fail-open: returns `false` on invalid semver, never panics."** That refers to
> *panic* safety — the function degrades gracefully rather than aborting. From the
> **adoption gate's** standpoint that same behavior is **fail-closed**: an
> unparseable tag returns `false`, which means *do not adopt*. There is no behavior
> change; the implementation only (a) makes `is_newer` `pub(crate)` so the
> adoption trigger in `cmd_self_update` can call it, and (b) updates the
> doc-comment so its wording matches this adoption-gate framing. This doc and the
> code comment must not contradict each other.

This replaces the string-equality short-circuit in **both**
`handle_self_update` (`update.rs:70`) and `handle_self_update_download_only`
(`update.rs:128`). The launch-time notice path already uses `is_newer`; the two
adoption paths now share the same authoritative predicate, so the notice and the
action can never disagree.

### 2. Self-test-gated auto-adoption trigger

A small, additive adoption trigger turns a detected newer release into a running
upgrade without operator keystrokes. It:

- **Short-circuits on `AlreadyLatest`** — when `is_newer` is false it does nothing
  and returns immediately (no download, no network beyond the cached check).
- **Routes exclusively through `download_and_replace`** — the same hardened
  safe-update chain used by `simard self-update`: SHA-256 checksum gate, cosign
  keyless verification (pinned issuer/identity), https-only transport, atomic
  install with `.old` backup/restore, and `0o755` mode. There is **no** bypass
  path that skips checksum, signature, or self-test.
- **Gates relaunch on a passing self-test.** The freshly installed binary must
  pass `gym run-suite starter` before the trigger execs into it. A failed
  self-test leaves the `.old` binary authoritative and surfaces the failure — it
  never relaunches an unhealthy binary.
- **Is frequency-bounded.** The trigger cannot re-download every cycle: the
  `is_newer` short-circuit plus a bounded check interval prevent an adoption loop.
- **Emits outcome enums via tracing only.** Every outcome
  (`Adopted`, `AlreadyLatest`, `SelfTestFailed`, `DeferredOperational`, …) is
  logged with structured `tracing` fields and OTel spans. No stray
  `print!`/`println!` is added in library code.

## Operational root cause is a valid terminal outcome

If investigation shows the stale operator is **not** a code defect — for example
the host simply must pull and restart, or the release channel is intentionally
pinned — that is surfaced explicitly as the `DeferredOperational` outcome and
documented, **not** forced into a code change. A correct "no code change needed,
here is the operational action" result is a success, not a failure. See the
[API reference](../reference/self-deploy-release-adoption-api.md#outcomes) for
the outcome taxonomy.

## What is preserved (non-negotiable)

The adoption trigger is additive and non-breaking. It preserves every existing
self-update control — regressing any of these is treated as a security defect:

- SHA-256 checksum verification of the downloaded asset.
- Cosign keyless signature verification with pinned issuer and identity.
- Https-only transport (`--proto =https --proto-redir =https --tlsv1.2`, `--`
  argument terminator, arg-vector `Command` calls — never `sh -c`).
- Self-test gate before the exec relaunch.
- Atomic install with `.old` backup, restore-on-failure, and `0o755` mode.
- The PRD; no "Bridge" naming; structured tracing + OTel only.

`tag_name` and asset names are treated as untrusted network input: validated via
`strip_prefix("v")` + `is_newer`, never interpolated into a shell.

## See also

- [Self-deploy release-adoption API](../reference/self-deploy-release-adoption-api.md)
- [Automatic update check](../reference/update-check.md)
- [Reconcile-and-self-deploy](./reconcile-and-self-deploy.md)
- [Safe self-update](../safe-self-update.md)
