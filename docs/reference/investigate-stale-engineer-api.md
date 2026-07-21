---
title: "Reference: Investigate-Before-Reap API"
description: >
  The API contract for investigating a HeartbeatStale engineer before reclaiming
  its claim: the investigate() seam and InvestigationOutcome verdicts, the
  idempotent-per-claim archive_stale_engineer_evidence() returning ArchiveOutcome,
  find_recent_archive_epoch() and the ARCHIVE_FRESHNESS_WINDOW, the shared
  sanitize_claim_key_for_archive() allowlist, the stable STALE_INVESTIGATION_DEDUP_PREFIX
  dedup token honored by recipe_dedup_key(), and the bounded run_with_deadline()
  subprocess helper (JOURNAL_CAPTURE_TIMEOUT) that bounds capture_journal_slice().
  Includes constants, evidence-path/permission semantics, fail-closed / bounded
  contracts, and the T1-T3 regression tests.
last_updated: 2026-07-21
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/investigate-stale-engineer-before-reap.md
  - ../concepts/stale-engineer-claim-reaper.md
  - ./claim-reaper-api.md
  - ./overseer-recipe-launch-idempotency.md
  - ./engineer-worktree-sweep-safety.md
  - ../operations/claim-reaper-kill-switch.md
---

# Reference: Investigate-Before-Reap API

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
> and `recipe_dedup_key()` in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> Conceptual overview:
> [Investigate a Quiet/Idle Engineer Before Reaping It](../concepts/investigate-stale-engineer-before-reap.md).
> This page documents the investigate-before-reap layer on top of the base
> [Claim-Reaper API](./claim-reaper-api.md).

## Overview

A `Dead { HeartbeatStale }` verdict from the base liveness probe does **not**
reclaim a claim. It triggers an **investigation** through the
`StaleEngineerInvestigator` seam. The reaper reclaims a heartbeat-stale claim
only when a **completed** investigation concludes the engineer is `Dead`; it
**fails closed** on every other verdict.

Two contracts make the investigation safe to run on the synchronous Overseer
tick:

- **Idempotent per claim.** Across ticks, a still-stale claim produces exactly
  one evidence directory and exactly one admitted investigation recipe until it
  concludes or its evidence ages out of the freshness window.
- **Bounded.** The `journalctl` capture the investigation performs is bounded by
  a subprocess deadline, so a slow journalctl can never stall the tick.

## Constants

Defined in
[`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs)
(and `mod.rs` for the shared prefix):

```rust
/// Reuse an existing evidence dir for a claim while it is younger than this.
/// A genuinely-new recurrence after this window earns a fresh dir.
pub const ARCHIVE_FRESHNESS_WINDOW: Duration = Duration::from_secs(3600); // 1 h

/// Reserved, single-line dedup token embedded in the investigation brief. Its
/// value is the sanitized claim signature. DISTINCT from OVERSEER_OBS_PREFIX.
pub const STALE_INVESTIGATION_DEDUP_PREFIX: &str = "stale-investigation:";

/// Hard deadline for the bounded journalctl capture on the meta-OODA tick.
pub const JOURNAL_CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
```

## Investigation seam: `StaleEngineerInvestigator`

The reaper delegates the heartbeat-stale path to an injectable investigator so
the whole sweep stays hermetically testable with fakes.

```rust
/// Verdict of investigating a heartbeat-stale claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvestigationVerdict {
    /// Positively concluded dead — the reaper may reclaim.
    Dead,
    /// Investigation is outstanding this sweep (dispatched or already running).
    /// should_reap() == false — claim is kept.
    Pending,
    /// Engineer is (or may be) alive; keep the claim (fail-closed).
    StillAlive,
    /// Blocked / recoverable — keep the claim (fail-closed).
    Blocked,
    Recoverable,
}

/// Outcome returned to the reaper: a verdict plus any interventions to emit.
#[derive(Debug, Clone)]
pub struct InvestigationOutcome {
    pub verdict: InvestigationVerdict,
    /// Interventions to dispatch (e.g. a single LaunchRecipe). Empty on a
    /// deduped/short-circuited sweep.
    pub interventions: Vec<Intervention>,
}

impl InvestigationVerdict {
    /// Only `Dead` permits reclaiming a heartbeat-stale claim. Everything else
    /// keeps the claim (fail-closed).
    pub fn should_reap(&self) -> bool {
        matches!(self, InvestigationVerdict::Dead)
    }
}

pub trait StaleEngineerInvestigator: Send + Sync {
    /// Investigate a heartbeat-stale claim. Idempotent per claim across ticks.
    fn investigate(&self, claim_key: &str, idle_age_secs: u64) -> InvestigationOutcome;
}
```

### Production `investigate()`

`RecipeStaleEngineerInvestigator::investigate()`
([`claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs))
computes the evidence directory via the reuse-or-mint archive step, then branches
on whether the directory was **reused** or **freshly minted**:

```text
outcome = archive_stale_engineer_evidence(state_root, claim_key, idle_age_secs)  // ArchiveOutcome

if outcome.minted:
    # first sweep for this stale claim (or window lapsed): the archive step
    # already wrote manifest.json + evidence.txt + the bounded journal slice
    # into outcome.dir and locked it to 0700/0600.
    brief = investigation_brief(claim_key, goal_id, idle_age_secs, outcome.dir)  // -> RecipeBrief
    return InvestigationOutcome {
        verdict: Pending,
        interventions: vec![ Intervention::LaunchRecipe { brief } ],
    }
else:
    # within-window reuse => an investigation is already outstanding
    return InvestigationOutcome { verdict: Pending, interventions: vec![] }  // no re-archive, no re-dispatch
```

Idempotency is **disk-derived**: `investigate()` never inspects the Overseer's
`inflight_investigations` map. Reusing a within-window directory means an
investigation is already outstanding, so it short-circuits. The `mod.rs` dedup
token guard (below) is the concurrent-race backstop.

## Idempotent archive: `archive_stale_engineer_evidence`

```rust
/// Result of resolving an evidence directory for a claim.
pub struct ArchiveOutcome {
    /// The evidence directory (0700), owner-restricted.
    pub dir: PathBuf,
    /// true  => freshly minted this call (manifest/evidence/journal written;
    ///          investigate() archives + dispatches).
    /// false => reused an existing within-window dir (investigate() short-circuits;
    ///          NO re-write, NO re-dispatch).
    pub minted: bool,
}

/// Resolve the evidence dir for `claim_key`, REUSING the most-recent existing
/// `<sanitized_claim_key>-<ts>` dir within `ARCHIVE_FRESHNESS_WINDOW` if one
/// exists, else MINTING a new `<sanitized_claim_key>-<now_ts>` dir and writing
/// its evidence. Idempotent per claim: at most one live dir per claim while an
/// investigation is outstanding.
///
/// FAIL-VISIBLE: an IO error is surfaced as `Err(String)` (the caller keeps the
/// claim rather than reaping blind), never a panic.
pub fn archive_stale_engineer_evidence(
    state_root: &Path,
    claim_key: &str,
    idle_age_secs: u64,
) -> Result<ArchiveOutcome, String>;
```

| Directory state under `reaped-engineers/` | Result |
|---|---|
| Most-recent `<key>-<ts>` dir **within** `ARCHIVE_FRESHNESS_WINDOW` | `ArchiveOutcome { dir, minted: false }` — reused as-is; **no** re-write, so the bounded journal capture does **not** re-run |
| No matching dir, or newest is **older** than the window | `ArchiveOutcome { dir, minted: true }` — new dir minted, evidence written, perms set |

- Only the **mint** path writes `manifest.json` / `evidence.txt` / `journal.txt`
  and calls `restrict_to_owner` (`0700` dir, `0600` files). A reused dir already
  carries those permissions from when it was minted; the reuse path re-writes
  nothing, so the bounded `journalctl` capture runs at most once per epoch.
- The stable `stale-investigation:` dedup token (below) is the concurrent-race
  backstop for the archive's read-then-mint sequence.

### `find_recent_archive_epoch`

```rust
/// Return the most-recent `<sanitized_key>-<ts>` DIRECT child of the
/// reaped-engineers root whose parsed `<ts>` is within ARCHIVE_FRESHNESS_WINDOW
/// of `now_ts`, if any.
///
/// - Scans DIRECT children only; ignores non-directories.
/// - Matches ONLY `<sanitized_key>-<pure_digits>` (a different claim whose
///   sanitized key merely shares the prefix leaves a non-numeric tail that fails
///   the parse), so it is never mistaken for this claim's epoch.
/// - Parses the trailing `-<ts>` to apply the freshness window.
fn find_recent_archive_epoch(root: &Path, sanitized_key: &str, now_ts: u64) -> Option<PathBuf>;
```

The path-traversal guarantee comes from `sanitize_claim_key_for_archive` (below):
a raw `claim_key` is neutralized to a single `[A-Za-z0-9_-]` component before it
ever names a directory, so a malformed or hostile key can only **fail to match** —
it can never resolve to a reuse target outside the archive root.

## Claim-key sanitizer: `sanitize_claim_key_for_archive`

A single shared strict-allowlist sanitizer builds **both** the archive directory
name and the dedup token, so the two validations can never diverge.

```rust
/// Strict allowlist sanitizer for a claim_key used in a filesystem path AND a
/// single-line dedup token.
///
/// Guarantees the result:
///   - contains only [A-Za-z0-9_-]  (every other byte, including '.', '/', '\\',
///     ':', NUL, and CR/LF, is replaced with '_'),
///   - is a single line (no CR/LF),
///   - contains no NUL, no '/', no '\\', no ".." traversal segment,
///   - contains no ':' (so it cannot collide with the dedup-token prefix or
///     forge a second token line),
///   - is never empty (folds to "claim") and is length-capped.
pub fn sanitize_claim_key_for_archive(claim_key: &str) -> String;
```

This is the primary defense against path traversal (into the archive path) and
log/prompt (XPIA) injection (into the brief / dedup token). See
[Security](#security-considerations).

## Dedup token contract

`investigation_brief()` returns a `RecipeBrief { task_description, target_repo,
sequence_group }`. Its `task_description` embeds a single reserved line:

```
stale-investigation:<sanitize_claim_key_for_archive(claim_key)>
```

The value is a **stable claim signature** (the sanitized `claim_key`, which
encodes the goal_id/engineer identity — stable across ticks, distinct per
genuinely-new claim). Volatile prose (`evidence_dir`, `idle_age_secs`) may appear
in the brief for humans, but is **excluded** from the deduped portion.

### `recipe_dedup_key` (mod.rs)

```rust
/// Derive the cross-tick dedup key for a launched recipe.
///
/// If `brief.task_description` contains a `stale-investigation:<sig>` line, key
/// on that stable signature (dedup contract for investigate-before-reap). This is
/// DISTINCT from OVERSEER_OBS_PREFIX ("overseer-obs:", #4128) and MUST NOT reuse
/// it. Otherwise fall back to the existing keying (the `overseer-obs:` token when
/// present, else the whole trimmed description).
fn recipe_dedup_key(brief: &RecipeBrief) -> String;
```

> **Dedup contract (must hold).** Two ticks for the same still-stale claim
> produce identical `recipe_dedup_key` output and therefore **one** admitted
> `LaunchRecipe`; the Overseer in-flight-investigation guard suppresses the
> duplicate. The volatile prose in the brief never changes the key.

The prefix is intentionally different from `OVERSEER_OBS_PREFIX` so the two
dedup semantics never collide.

## Bounded subprocess: `run_with_deadline`

```rust
/// Run `cmd` with `args` under a hard deadline. Spawn -> poll try_wait to the
/// deadline -> kill + wait/reap on expiry. Drains stdout via a reader thread.
/// Returns the collected stdout on a prompt SUCCESSFUL exit, or None on timeout,
/// a non-zero exit, or a spawn failure.
///
/// On timeout: emits tracing::warn!, kills+reaps the child (no zombie, no leaked
/// thread), returns best-effort None. Never panics, never blocks past
/// `timeout`. journalctl-only bound — NOT a wall-clock cap on any agentic step.
fn run_with_deadline(cmd: &str, args: &[&str], timeout: Duration) -> Option<String>;
```

Adapts the wait-with-deadline idiom from
[`src/safe_update/pretest.rs`](https://github.com/rysweet/Simard/blob/main/src/safe_update/pretest.rs).
`cmd` and `args` are passed directly to `Command` — **no shell, no
interpolation** — so nothing from a claim key is ever expanded as an argument.

### `capture_journal_slice`

```rust
/// Capture a bounded slice of the daemon journal for the investigation brief.
/// Runs `journalctl --user -u simard-ooda.service ...` through run_with_deadline
/// with JOURNAL_CAPTURE_TIMEOUT, then keeps only lines mentioning `goal_id`
/// (bounded tail). On timeout / failure / no match: None (best-effort).
fn capture_journal_slice(goal_id: &str) -> Option<String>;
```

- Bounded by `JOURNAL_CAPTURE_TIMEOUT` (5 s). A slow/hung `journalctl` yields a
  `warn` + partial/empty slice; the tick is never blocked past the deadline.
- The systemd `--user` unit is hardcoded (`--user -u simard-ooda.service`) — a
  deliberate divergence from the operator-tunable `service_unit` the
  health-review rail uses, so no claim-derived value can widen the journal
  scope; the claim key is never interpolated into the argv.
- Runs at most once per claim thanks to the idempotent archive, but the per-call
  block is bounded regardless.

## Reaper gating (unchanged regression guard)

`reap_stale_claims()` gating is preserved exactly:

| Probe verdict | Path |
|---|---|
| `Dead { NoWorktree }` | **Reap immediately** (no investigation; [#4099](https://github.com/rysweet/Simard/issues/4099)). |
| `Dead { HeartbeatStale }` | Investigate; reap **iff** `InvestigationVerdict::Dead`. |
| `StillAlive` / `Blocked` / `Recoverable` / `Pending` | **Keep** the claim (fail-closed). |
| `Live` | Keep the claim. |

Evidence is preserved before any cleanup. The pre-existing fake-driven
quiet-but-alive-kept and genuinely-dead-reaped tests continue to pass.

## Regression coverage

Tests live inline in `src/overseer/claim_reaper.rs` (`#[cfg(test)]`) and
`src/overseer/mod.rs`, using fake seams and injected commands. Each new test
**fails before** the fix and **passes after**.

| Test | Asserts |
|---|---|
| **T1 — dedup / multi-tick** | N ticks over the same still-stale claim ⇒ **exactly one** evidence dir (no `<key>-<ts2>` sibling), **exactly one** admitted `LaunchRecipe`; later ticks return `Pending` with **no** new archive/launch; the claim is **not** reaped. |
| **T2 — no permanent wedge** | A `NoWorktree` engineer is still reaped immediately; a later genuinely-new recurrence (dir outside the window / prior investigation completed) is investigated again — **fresh dir + new launch**. |
| **T3 — journal timeout** | An injected slow command (`sleep 10`) into `run_with_deadline` returns **within** the deadline, does **not** block past it, emits a `warn`, and yields `None`; a prompt command (`printf`) returns `Some(stdout)`; an un-spawnable command folds to `None`. |
| **Sanitizer** | Traversal/injection vectors (`..`, `/`, `\`, NUL, CR/LF, `:`) are neutralized in both the dir name and the dedup token. |
| **mod.rs dedup key** | Two briefs for the same claim (differing only in volatile prose) ⇒ **equal** `recipe_dedup_key`; the key is **distinct** from an `OVERSEER_OBS_PREFIX` brief. |
| **Permissions** | A minted evidence dir is `0700` / files `0600`; a reused dir retains those perms (the reuse path re-writes nothing). |

Required gates (merge blockers): `cargo build --lib`, `cargo clippy --lib -- -D
warnings`, and `cargo fmt --check` must pass; all pre-existing `claim_reaper`
tests plus T1–T3 pass; no new TODO/dead code; errors surfaced via `tracing`
(never swallowed); no wall-clock timeout on any agentic step.

## Security considerations

| Surface | Control |
|---|---|
| **Path traversal** (claim_key → archive dir name) | `sanitize_claim_key_for_archive` strict allowlist collapses every `/`, `\`, `:`, `.`, NUL, and control byte to `_`, so a raw key becomes a single `[A-Za-z0-9_-]` component before it ever names a dir. `find_recent_archive_epoch` only matches `<sanitized>-<pure_digits>` direct children — a reuse target is always a dir discovered on disk, never a path built from a raw key. |
| **Log / prompt (XPIA) injection** (newline/`:` in claim_key forging a brief line or a second dedup token) | Single-line sanitize; exactly one dedup token per brief; token prefix distinct from `OVERSEER_OBS_PREFIX`. |
| **Argument injection** into `journalctl` | `cmd` + `args` passed directly to `Command` (no shell), hardcoded unit, claim_key never interpolated into argv. |
| **Secret leakage** | Evidence (journals/env/tracebacks) is written `0700`/`0600` on mint and retained on reuse; raw journal bytes are excluded from the dedup key and the recipe `task_description`; the slice is line-bounded (≤500 matching lines). |
| **Subprocess DoS** | `run_with_deadline` kills + reaps on the 5 s deadline; fail-closed, non-panicking, never blocks the tick. |
| **No privilege escalation** | No `sudo`, no widened `journalctl` scope, no files written outside the owner-restricted `reaped-engineers/` tree. |

## Related

- [Investigate a Quiet/Idle Engineer Before Reaping It](../concepts/investigate-stale-engineer-before-reap.md)
  — the concept companion (dedup contract, freshness window, journal bound).
- [Claim-Reaper API](./claim-reaper-api.md) — the base sweep, liveness probe,
  cleanup seam, and config resolvers this layer extends.
- [Overseer Recipe-Launch Idempotency](./overseer-recipe-launch-idempotency.md)
  — `recipe_dedup_key` and the in-flight guard.
- [Worktree Reaping Safety Guards](./engineer-worktree-sweep-safety.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
