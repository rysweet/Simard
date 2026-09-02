---
title: Overseer operator-notification dedup / rate-limiting reference
description: >
  Reference for the Overseer notifier's signature-based dedup rail (#4579): the
  process-global suppression of IDENTICAL repeated operator push notices (e.g. the
  self-deploy-refused red-canary notice re-emitted every deploy cycle), keyed by a
  stable notification signature. Documents the suppressible-kind allowlist, the
  volatile-text normalization that makes the signature stable, the cooldown/digest
  window and its SIMARD_OVERSEER_NOTIFY_DEDUP_SECS override, the fail-open
  guarantees, the process-global state that survives tick-rebuilds, and the exact
  dispatch-vs-suppress contract on NotifyReport.
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: reference
issues: ["#4579"]
related:
  - ../index.md
  - ../design/overseer.md
  - ./overseer-operator-notifications.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./overseer-backoff-gate-api.md
  - ../howto/configure-overseer-notification-dedup.md
  - ../howto/configure-overseer-email-notifications.md
  - ../howto/configure-overseer-signal-rpc-notifications.md
---

# Overseer operator-notification dedup / rate-limiting reference

The Overseer notifies the operator on **every** merge, deploy, whisper, and blocked
"needs human review" escalation. Some of those events *recur every tick*: while the
running binary is behind `origin/main`, the Overseer re-attempts self-deploy every
deploy cycle (~15–25 min); each attempt whose canary reds emits an **identical**
`deploy-refused` notice. Before this rail, the operator (Signal + email) received one
identical `self-deploy refused … red canary (gate unit-test …) … Drop t…` notice per
attempt — indefinitely.

This rail adds **signature-based dedup / rate-limiting inside the notifier layer**
(`src/overseer/notify.rs`) so that repeated **identical** notices are suppressed,
while **every attempt is still logged at WARN and reflected in the overseer status
field** — only the operator *push* notification is deduped. It is the notification
analogue of the [gap-scan backoff rail](./overseer-backoff-gate-api.md) and the
[deploy throttle](../design/overseer.md): surface a distinct problem **once**, then
stay quiet until something changes or a digest window elapses.

> **Scope.** The dedup lives entirely in the notifier and its types. The deploy path
> (`src/overseer/deploy.rs`) and deploy trigger (`src/overseer/deploy_trigger.rs`) are
> **untouched**: they still call `OperatorNotification::deploy_refused` +
> `notifier.notify()` unconditionally every attempt, still WARN-log every attempt, and
> still update the overseer status field every attempt. The notifier decides whether
> that call results in an actual channel dispatch.

---

## What is (and is not) deduped

Dedup applies **only** to pure-failure notification kinds via an explicit allowlist.
Genuine **state-change** notices always dispatch immediately and are never suppressed.

| `kind` | Deduped? | Why |
|--------|----------|-----|
| `deploy-refused` | **yes** | Recurs identically every deploy cycle while behind `origin/main`. |
| `goal-blocked` | **yes** | The same blocked-goal escalation can recur tick after tick. |
| `workstream-gap` | **yes** | Same uncovered-backlog set re-flagged each scan. |
| `merge-reasoning-disabled` | **yes** | A one-shot "LOUD" alert that must not repeat per tick. |
| `whisper` | **yes** | Low-urgency recurring nudges. |
| `merge` | no | A completed merge is a distinct, real event each time. |
| `deploy` | no | A **succeeded** deploy is a state change (recovery); always send. |
| `deploy-starting` | no | Start-of-deploy is a state change; always send. |

The allowlist is the safety mechanism: because state-change kinds are **absent** from
it, they provably always dispatch, so callers that assert delivery (e.g.
`debug_assert!(report.dispatched())` on the deploy-succeeded / deploy-starting paths)
are unaffected.

> **A distinct failure is not a repeat.** "Identical" means *same signature*. A
> different failing test, a new `target_commit`, or the canary going green all produce
> a **new** signature and therefore dispatch immediately — the rail suppresses only
> byte-for-byte-equivalent repeats of an already-notified failure.

---

## The notification signature

Suppression is keyed by a **stable signature** derived entirely from existing
`OperatorNotification` fields, so `deploy.rs` needs no change to pass extra data:

```
signature = kind ␟ repo ␟ normalize_volatile(headline) ␟ normalize_volatile(problem)
```

- `␟` is `U+001F` (unit separator); control characters — including a literal `U+001F`
  — are stripped by normalization so a field value can never forge the separator.
- `next_step`, `link`, and `autonomous` are **excluded**: they do not discriminate one
  failure from another (they are advice / provenance, not identity).
- The commit shortcodes, gate names, and failing-test identifiers that *do*
  discriminate live **inside** `headline` / `problem`; normalization preserves them.

### `normalize_volatile()` — what gets stripped

The raw red-canary detail carries volatile bytes that differ between two otherwise
identical attempts (an animated spinner frame, changing elapsed durations, timestamps,
ANSI color). If those were part of the signature, "identical" failures would never
match and dedup would never fire. `normalize_volatile()` removes them, **in order**:

1. **ANSI escape sequences** — CSI/color sequences (`\x1b[…m`).
2. **Spinner glyphs** — the Braille spinner set `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` and the ASCII spinner
   `| / - \`.
3. **Durations & timestamps** — `12.3s`, `450ms`, `2m`, `HH:MM[:SS]`, and ISO-8601
   timestamps.
4. **Control characters** — including `U+001F`, so no field can inject the separator.
5. **Whitespace** — every run of whitespace collapses to a single space, then trims.

**Preserved** (never stripped): commit shortcodes (e.g. `a1b2c3d4e5f6`), gate names
(e.g. `unit-test`), and test identifiers. Two red-canary details that differ only in
spinner frame / duration / whitespace therefore **hash equal**; a detail naming a
**different failing test** hashes **distinct**.

The implementation is a hand-rolled character scan — **no regex dependency** — which
eliminates the ReDoS risk class on operator-influenced text.

---

## Dispatch vs. suppress contract

`DualChannelNotifier::notify()` applies the gate **before** the per-channel delivery
loop:

```text
notify(n):
    if n.kind NOT in SUPPRESSIBLE_KINDS:
        → fall through to normal per-channel dispatch   (state-change: always send)
    sig  = notify_signature(n)
    now  = now_secs()                                   (monotonic, process-start epoch)
    cd   = notify_dedup_secs()                          (env-tunable, default 3600)
    if dedup_allow(sig, now, cd):
        record last-dispatch[sig] = now
        → normal per-channel dispatch
    else:
        → SUPPRESS: return NotifyReport { per_channel: [] }
          emit tracing::info!(suppressed = true, kind, signature_hash, …)
```

### `dedup_allow(signature, now_secs, cooldown_secs) -> bool`

A **pure, clock-injectable** decision over a process-global map of
`signature → last-dispatch-secs`:

| Situation | `last-dispatch[sig]` | Result |
|-----------|----------------------|--------|
| First occurrence of a signature | absent | `true` (dispatch) |
| Identical signature, **within** cooldown | `now - last < cooldown` | `false` (suppress) |
| Identical signature, **after** cooldown | `now - last >= cooldown` | `true` (dispatch — digest reminder) |
| Signature changed | new key, absent | `true` (dispatch immediately) |
| Lock poisoned / any internal error | — | `true` (**fail-open** — never suppress on error) |

- **Record-on-dispatch**: the timestamp is updated **only** when the notice is actually
  dispatched, so the digest window measures *time since last real send*, not time since
  last attempt. A still-stuck identical failure therefore produces **at most one
  reminder per cooldown window** — not one per attempt.
- `now - last` uses `saturating_sub`, so a non-monotonic clock reading cannot underflow.

### What a suppressed call returns

A suppressed notification returns `NotifyReport { per_channel: vec![] }`, i.e.
`report.dispatched() == false` and `report.all_sent() == false`. This is safe for the
`deploy-refused` callers, which use `let _ = notifier.notify(...)` and never assert
delivery. Suppression is **observable** via a structured `info` log — never silent:

```text
INFO overseer::notify: operator notification suppressed (dedup)
     suppressed=true kind="deploy-refused" signature_hash="9f3a1c7b" cooldown_secs=3600
```

The log emits `kind` + a short **display-only hash** of the signature — never the raw
`headline` / `problem` / signature text — so no failure-detail or credential-adjacent
text leaks into the summary, mirroring the existing secret-safe dispatch summary.

---

## Process-global state (survives tick-rebuilds)

The Overseer rebuilds the acting instance **every tick**, so per-instance dedup state
would reset each tick and never actually suppress anything. The dedup map is therefore
a **process-global `static`**, exactly mirroring
[`global_deploy_throttle_allow`](../design/overseer.md)'s `static LAST_DEPLOY_ATTEMPT_SECS`:

```rust
// src/overseer/notify.rs
static NOTIFY_DEDUP: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
static NOTIFY_START: OnceLock<Instant> = OnceLock::new();   // now_secs() epoch
```

- **Survives ticks**: because the map is a `static`, the last-dispatch timestamps
  persist across as many tick-rebuilds of the Overseer as occur within the process.
- **Ephemeral across restarts**: a daemon restart clears the map (fresh `static`), so
  the first post-restart occurrence of any failure always notifies. This is intentional
  — a restart is itself a state change worth re-surfacing once.
- **Bounded**: `dedup_allow` opportunistically prunes entries older than `2 ×
  cooldown` and enforces a hard size cap, so a long-uptime daemon with high signature
  variance cannot grow the map without bound.
- **Poison-tolerant**: the mutex is taken with `lock().unwrap_or_else(|e|
  e.into_inner())`; a poisoned lock never panics and the call fails open (dispatch).

---

## Configuration

| Env var | Default | Meaning | Fail-safe |
|---------|---------|---------|-----------|
| `SIMARD_OVERSEER_NOTIFY_DEDUP_SECS` | `3600` (60 min) | Cooldown / digest window. Within it, an identical failure is suppressed; after it, a still-identical failure dispatches once as a reminder. | Unset / empty / non-numeric → `3600`. Parsed as `env::var().ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(3600)`. |

Notes:

- The value is read **per `notify()` call** (cheap), so an operator can change it
  without a restart taking effect on the next notification, and tests can override it.
- `0` effectively disables suppression (every identical repeat's `now - last >= 0` is
  always true → always dispatch). There is no floor clamp; setting `0` is a supported
  way to turn the rail off without a code change. The one nuance: the gate still runs and
  the first occurrence still **records** its last-dispatch timestamp — but since every
  subsequent comparison passes, behavior is indistinguishable from "always send".
- There is **no on/off boolean**. To disable, set the window to `0`.

See the [operator howto](../howto/configure-overseer-notification-dedup.md) for tuning
guidance and verification steps.

---

## Invariants preserved

The rail is **additive and non-breaking**. It never reduces signal:

- **WARN logs stay per-attempt.** `deploy.rs` still logs every refused attempt at WARN
  (`log_degraded` and the deploy-refused WARN are unchanged).
- **Overseer status stays per-attempt.** The status field the daemon exposes reflects
  every attempt; dedup touches only the push notification.
- **State-change notices always dispatch.** `deploy-starting`, a **succeeded** `deploy`
  (recovery), a completed `merge`, a **new distinct** failure, and a canary going green
  are never in the suppressible set (or produce a new signature) and always go through.
- **Delivery-assert paths unaffected.** Kinds that `debug_assert!(dispatched())`
  (deploy-starting / deploy-succeeded) are non-suppressible, so the assertion holds.
- **No new external dependency.** Normalization is hand-rolled; no regex crate is
  enabled for this feature.

---

## Behavior summary (acceptance-level)

| Scenario | Outcome |
|----------|---------|
| N identical `deploy-refused` within the cooldown | Exactly **one** operator notice dispatched; the rest suppressed (each still WARN-logged + status-updated). |
| A **changed** failure signature (different test / new `target_commit`) | Dispatched **immediately** (new key). |
| A `deploy-starting` or **succeeded** `deploy` notice | Dispatched **immediately** (non-suppressible kind). |
| Same identical failure **after** the cooldown elapses | Dispatched again — a single **digest reminder** per window. |
| Simulated tick-rebuild between two identical failures | Second is still suppressed — the process-global `static` survived the rebuild. |

## Source & tests

- Implementation: `src/overseer/notify.rs` (`SUPPRESSIBLE_KINDS`, `NOTIFY_DEDUP`,
  `now_secs`, `notify_dedup_secs`, `normalize_volatile`, `notify_signature`,
  `dedup_allow`, wired into `DualChannelNotifier::notify`).
- Unit tests (same file), serialized against the shared `static` via a
  `NOTIFY_DEDUP_TEST_LOCK` guard + a `#[cfg(test)] reset_notify_dedup()` helper —
  the same isolation pattern the deploy throttle uses (`DEPLOY_THROTTLE_TEST_LOCK`):
  the four acceptance scenarios above, plus a normalization test (spinner/duration
  variants hash equal; a different failing test hashes distinct) and a tick-rebuild
  survival test.
