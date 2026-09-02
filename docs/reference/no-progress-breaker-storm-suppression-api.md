---
title: No-progress breaker issue-storm suppression API reference
description: >
  Reference for the two additive fixes that stop the OODA no-progress breaker
  from auto-filing duplicate `UNCLEAR-CRITERIA` "no-progress breaker" tracking
  issues (~15 in ~2 days, all sharing one title) and from misclassifying
  derivable-criteria goals as unclear. Specifies the durable suppression marker
  (`NO_PROGRESS_SUPPRESSION_MARKER_KIND`), the storm-safe rewrite of
  `escalate_with_tracking_issue` (persist a Blocked suppression marker BEFORE and
  independent of `gh` URL-parse success, then upgrade to a linked tracking ref on
  success without appending a duplicate), the additive `is_breaker_tracking_ref`
  marker recognition, and the pure `derive_criteria` terminal-rung helper that
  lets a derivable-criteria goal proceed as `GENUINELY-STUCK` instead of tripping
  `UNCLEAR-CRITERIA`.
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./no-progress-reinvestigation-api.md
  - ./completion-evidence-gate-api.md
  - ./goal-board-api.md
  - ../concepts/no-progress-breaker-storm-suppression.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../concepts/no-progress-terminal-investigation.md
  - ../howto/diagnose-a-no-progress-breaker-issue-storm.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/goal_curation/types.rs
---

# No-progress breaker issue-storm suppression API reference

> **Status: implemented.** The durable suppression marker, the storm-safe
> `escalate_with_tracking_issue`, the additive `is_breaker_tracking_ref`
> recognition, and the pure `derive_criteria` terminal-rung helper all live in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
> The [`WipRef`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs)
> schema the marker is persisted as is unchanged. The rustdoc on those items is
> the canonical API; the signatures below are kept in sync with it.

This page specifies two **additive, non-breaking** fixes layered on the
[root-cause resolution](./no-progress-root-cause-resolution-api.md) breaker:

1. **Storm suppression (primary).** The escalation side effect now persists a
   durable, restart-surviving suppression marker to the goal board **before and
   independent of** the `gh` issue link succeeding. A failed
   `NoProgressIssueFiler::file_issue()` (or an unparsed `gh` URL) can no longer
   cause the breaker to re-file the same tracking issue every cycle.
2. **Terminal-rung derivation (secondary).** A pure, total `derive_criteria`
   helper runs at the bottom rung of the reasoner before it defaults an
   empty-evidence stall to `UNCLEAR-CRITERIA`. A goal whose done-criteria are
   *derivable from its own description/known artifacts* now proceeds as
   `GENUINELY-STUCK` rather than being misclassified as structurally unmeasurable.

For the rationale — the observed storm of ~15 identical
`OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)`
issues in ~2 days — see
[The no-progress breaker suppresses its own issue storm](../concepts/no-progress-breaker-storm-suppression.md).

## Contents

- [Design invariants](#design-invariants)
- [The storm: why a failed link re-filed every cycle](#the-storm)
- [`NO_PROGRESS_SUPPRESSION_MARKER_KIND`](#no_progress_suppression_marker_kind)
- [Suppression-marker `WipRef` schema](#suppression-marker-wipref-schema)
- [`is_breaker_tracking_ref` (extended)](#is_breaker_tracking_ref-extended)
- [`escalate_with_tracking_issue` (storm-safe rewrite)](#escalate_with_tracking_issue-storm-safe-rewrite)
  - [Deliberate trade-off: a bare marker is never re-linked](#deliberate-trade-off-a-bare-marker-is-never-re-linked)
- [`derive_criteria`](#derive_criteria)
- [Terminal-rung wiring](#terminal-rung-wiring)
- [Fail-closed and security properties](#fail-closed-and-security-properties)
- [What is unchanged](#what-is-unchanged)

## Design invariants

1. **Durable, restart-surviving suppression.** Dedup keys on **durable goal
   identity** persisted to the goal-board store, not on in-memory tracker state
   that resets on the daemon's periodic exec-reload. At most **one** breaker
   suppression marker exists per goal.
2. **Suppression independent of linking.** The suppression marker is written
   *before* `file_issue()` is attempted and does not depend on the `gh` URL
   parsing to a bare issue number. A `None` from `file_issue()` never re-opens
   the re-filing loop.
3. **No duplicate refs.** On a successful file, the bare suppression marker is
   **upgraded in place** to a linked tracking ref — never appended as a second
   `WipRef`. `<= 1` breaker marker/tracking ref per goal is an enforced
   invariant.
4. **Additive / non-breaking.** No existing `WipRef` field, enum variant
   (`NoProgressClass`, `NoProgressResolution`, `StuckGoalDisposition`), or public
   signature changes. Clear-criteria goals produce byte-identical outcomes.
5. **Conservative derivation.** `derive_criteria` returns `None` on anything it
   cannot positively derive, falling to the legacy `UNCLEAR-CRITERIA` behavior. It
   never returns `Some(empty)` and never opens a new unbounded re-investigation
   loop (the `SURFACED_INVESTIGATION_FAILURE_LIMIT` bound still applies).
6. **Forward/backward compatible store.** An older reader ignores the new marker
   kind (the `WipRef` filter fall-through is `_ => None`), so a downgrade cannot
   corrupt or misread `goal_board.json`.
7. **Structured observability only.** All new logging is `tracing` + OTel
   structured fields — no `print!`/`println!`, no Bridge naming.

## The storm

The [root-cause escalation](./no-progress-root-cause-resolution-api.md#block-reason-contract)
files a `gh` tracking issue and links it back to the goal so the done-criteria
become measurable. The idempotence guard that prevents duplicate issues was
keyed on the presence of a **linked** tracking `WipRef`:

```text
escalate → file_issue() → parse gh URL → Some(FiledIssue) → link_tracking_issue()
                                    │
                                    └─ None (gh failed OR URL not a bare number)
                                         → NO wip_ref written
                                         → next cycle: still no tracking ref
                                         → idempotence guard sees "untracked"
                                         → file_issue() AGAIN … every cycle
```

For a goal that classifies `UNCLEAR-CRITERIA` — one with **no** tracked
PR/issue, precisely the population most likely to hit a `gh` edge — the link
never landed, so every subsequent cycle re-filed the identical
`OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)`
issue. The observed result was ~15 duplicate issues in ~2 days from a single
stuck-goal population. The fix decouples **suppression** (a durable board write
the breaker fully controls) from **linking** (a best-effort `gh` side effect).

## `NO_PROGRESS_SUPPRESSION_MARKER_KIND`

```rust
/// `WipRef.kind` for the breaker's durable *suppression marker* — the
/// restart-surviving record that this goal has already been escalated by the
/// no-progress breaker, written BEFORE and INDEPENDENT of any `gh` issue link.
///
/// Its sole job is idempotence: a goal carrying this marker is never re-filed,
/// even if `file_issue()` returned `None` (gh failed, or its URL did not parse
/// to a bare issue number). It is distinct from the linked tracking ref
/// (`kind = "issue"`, label-prefixed `[no-progress-tracking] `) that a
/// *successful* filing upgrades it into.
const NO_PROGRESS_SUPPRESSION_MARKER_KIND: &str = "ooda-breaker-marker";

/// Fixed sentinel `WipRef.ref_id` for the suppression marker. A constant — NEVER
/// derived from goal text — so goal descriptions can never smuggle content into
/// the marker (no argv/flag injection, no path traversal).
const NO_PROGRESS_SUPPRESSION_MARKER_REF_ID: &str = "ooda-breaker";
```

The marker reuses the existing `NO_PROGRESS_TRACKING_LABEL_PREFIX`
(`[no-progress-tracking] `) for its human-readable `label`, so the same label
scan recognizes both the bare marker and the upgraded linked ref.

## Suppression-marker `WipRef` schema

The marker is persisted through the existing atomic goal-board save path as an
ordinary [`WipRef`](./goal-board-api.md) — no schema change:

| Field    | Value                                                     |
| -------- | --------------------------------------------------------- |
| `kind`   | `"ooda-breaker-marker"` (`NO_PROGRESS_SUPPRESSION_MARKER_KIND`) |
| `ref_id` | `"ooda-breaker"` (fixed sentinel constant)                |
| `label`  | `"[no-progress-tracking] ooda-breaker (unlinked)"`        |
| `url`    | `None`                                                    |

On a successful filing the **same** entry is upgraded in place to the linked
tracking ref (`kind = "issue"`, `ref_id = <number>`, label
`[no-progress-tracking] #<number>`, `url = Some(...)`) — the marker is replaced,
not supplemented.

## `is_breaker_tracking_ref` (extended)

The dedup predicate additively recognizes the new suppression-marker kind. Its
signature is unchanged; only the body gains one additional recognized kind:

```rust
/// True when `wip` is a breaker-authored escalation artifact — EITHER the
/// durable suppression marker (`NO_PROGRESS_SUPPRESSION_MARKER_KIND`, written
/// before/independent of linking) OR the upgraded linked tracking issue
/// (`kind = "issue"`, label-prefixed `[no-progress-tracking] `). Either one
/// means "this goal has already been escalated by the breaker", so the
/// idempotence guard in `escalate_with_tracking_issue` suppresses re-filing.
fn is_breaker_tracking_ref(wip: &WipRef) -> bool {
    (wip.kind.eq_ignore_ascii_case(NO_PROGRESS_SUPPRESSION_MARKER_KIND))
        || (wip.kind.eq_ignore_ascii_case("issue")
            && wip.label.starts_with(NO_PROGRESS_TRACKING_LABEL_PREFIX))
}
```

Because the marker satisfies `is_breaker_tracking_ref`, the existing
`already_tracked` guard now short-circuits re-filing whether or not the `gh`
link ever landed.

## `escalate_with_tracking_issue` (storm-safe rewrite)

The escalation side effect is decoupled into **suppress-then-link**. The public
signature is unchanged; the ordering and durability change:

```rust
/// Escalate a stuck goal: set it `Blocked` with `blocked_reason`, DURABLY mark
/// it suppressed so it is never re-filed, then best-effort file + link a `gh`
/// tracking issue. Storm-safe and restart-surviving.
///
/// Ordering (the fix): the durable suppression marker and the `Blocked` status
/// are written FIRST, through the existing atomic goal-board save path, so the
/// goal is idempotently suppressed BEFORE `file_issue()` is attempted. A `None`
/// from `file_issue()` therefore leaves the goal Blocked + suppressed (no
/// re-file next cycle) instead of Blocked + untracked (re-file forever). On a
/// `Some`, the bare marker is UPGRADED IN PLACE to the linked tracking ref via
/// `link_tracking_issue` — never appended as a duplicate.
fn escalate_with_tracking_issue(
    state: &mut OodaState,
    goal_id: &str,
    blocked_reason: String,
    issue_title: &str,
    issue_body: &str,
    filer: &dyn NoProgressIssueFiler,
);
```

Behavior, per call:

| Goal state on entry                                   | `file_issue()` | Result                                                                 |
| ----------------------------------------------------- | -------------- | ---------------------------------------------------------------------- |
| no breaker marker/ref                                 | `Some(issue)`  | Blocked; bare marker written then **upgraded** to linked tracking ref (1 ref) |
| no breaker marker/ref                                 | `None`         | Blocked; **bare suppression marker** persisted (durable, no re-file)   |
| already carries suppression marker (prior `None`)     | not called     | Blocked; existing marker left as-is (idempotent, no re-file)           |
| already carries linked tracking ref                   | not called     | Blocked; existing linked ref left as-is (idempotent, no re-file)       |

Reference shape (the marker write precedes the filer call):

```rust
// 1. Durable, link-independent suppression FIRST — survives a gh failure and a
//    restart. Idempotent: skip if the goal already carries any breaker artifact.
let already = state
    .active_goals
    .active
    .iter()
    .find(|g| g.id == goal_id)
    .is_some_and(|g| g.wip_refs.iter().any(is_breaker_tracking_ref));

if let Some(g) = state.active_goals.active.iter_mut().find(|g| g.id == goal_id) {
    g.status = GoalProgress::Blocked(blocked_reason);
    if !already {
        g.wip_refs.push(WipRef {
            kind: NO_PROGRESS_SUPPRESSION_MARKER_KIND.to_string(),
            ref_id: NO_PROGRESS_SUPPRESSION_MARKER_REF_ID.to_string(),
            label: format!("{NO_PROGRESS_TRACKING_LABEL_PREFIX}ooda-breaker (unlinked)"),
            url: None,
        });
    }
}

// 2. Best-effort link SECOND. Only attempt when we just wrote a fresh marker.
if !already {
    if let Some(filed) = filer.file_issue(issue_title, issue_body) {
        if let Some(g) = state.active_goals.active.iter_mut().find(|g| g.id == goal_id) {
            upgrade_suppression_marker_to_link(g, &filed); // replaces marker; no duplicate
        }
    }
}
```

`upgrade_suppression_marker_to_link` is an **illustrative** private helper name,
not a required public symbol: it stands for whatever in-place rewrite the
implementation uses to fold the bare suppression marker into the linked tracking
`WipRef` (or, if `link_tracking_issue` already added the linked ref, to drop the
bare marker) so the `<= 1 breaker marker/ref per goal` invariant holds. The
durable state write goes through the existing **single-writer** save/load guard —
no new unsynchronized writer.

### Deliberate trade-off: a bare marker is never re-linked

This is the one intended limitation, called out so implementers and reviewers
confirm it is acceptable. The `already_tracked` guard keys on the presence of a
breaker artifact, and a bare suppression marker satisfies
`is_breaker_tracking_ref`. So once a goal receives a **bare** marker from a
failed first-cycle `file_issue()` (e.g. a `gh` outage), later cycles short-circuit
before calling `file_issue()` again — the marker is **never upgraded to a linked
issue on a subsequent cycle**, and that goal's done-criteria never become
measurable. The goal stays `Blocked` + suppressed but unlinked, permanently.

This is chosen on purpose: **storm suppression takes priority over eventual
linking.** Re-attempting the link every cycle is exactly the re-filing loop this
feature exists to kill, and the stall is still durably surfaced via the `Blocked`
status and its WHY. The escape hatch is manual (see the
[operator runbook](../howto/diagnose-a-no-progress-breaker-issue-storm.md)):
remove the bare marker and the next cycle re-escalates cleanly.

A future enhancement could **re-link on the next cycle only if the goal still
carries a bare marker** (attempt `file_issue()` when the sole breaker artifact is
an unlinked marker, without lifting suppression). That is intentionally **out of
scope** here to keep the fix additive and the storm guarantee unconditional; it
is recorded so it is a deliberate design decision rather than an oversight.

## `derive_criteria`

A pure, total, panic-free helper on the terminal rung of the deterministic
reasoner. It attempts to derive checkable done-criteria from the goal's **own**
description and already-known artifacts, without any external clarification.

```rust
/// Attempt to derive checkable done-criteria for a stalled goal from its OWN
/// description and known artifacts — no external clarification, no brain call.
///
/// Returns:
/// * `Some(evidence)` — non-empty, bounded — when criteria are derivable (the
///   goal names a concrete, checkable target such as a referenced repo/module,
///   a "PR merged" / "issue closed" phrasing, or a measurable threshold). The
///   caller proceeds as `GENUINELY-STUCK` with this evidence.
/// * `None` — when nothing checkable can be derived. The caller falls to the
///   legacy `UNCLEAR-CRITERIA` classification (byte-identical to before).
///
/// Totality/safety contract: never panics, never returns `Some(vec![])`, and
/// bounds its work by a fixed input-length cap so adversarial goal text
/// (very long, control chars, `--`-prefixed) cannot cause a panic or pathological
/// backtracking. Goal text is treated as untrusted and length-normalized.
fn derive_criteria(goal: &ActiveGoal) -> Option<Vec<Evidence>>;
```

Semantics:

- **Derivable ⇒ proceed.** A goal whose criteria are derivable is no longer
  swept into `UNCLEAR-CRITERIA`; it classifies `GENUINELY-STUCK` with the derived
  evidence and takes the same one-guided-engineer rung — so it gets a real
  investigation instead of being flagged structurally unmeasurable.
- **Conservative default.** Anything not positively derivable returns `None`, so
  a genuinely-unclear goal (e.g. the synthetic `simard-identity-*` goals with no
  tracked artifact) keeps the exact legacy `UNCLEAR-CRITERIA` outcome.
- **Never `Some(empty)`.** Guarantees the terminal rung never emits an
  `evidence=[(none)]` block via this path (the invariant
  [`unclear_criteria_evidence`](./no-progress-root-cause-resolution-api.md) already
  protects the `None` branch).

## Terminal-rung wiring

`derive_criteria` is consulted at rung 5 of `DeterministicNoProgressReasoner::investigate`
— **before** the empty-`stuck_evidence` case defaults to `UNCLEAR-CRITERIA`:

```rust
// 5. No machine-resolvable cause found. Prefer still-open artifacts; else try to
//    DERIVE criteria from the goal itself; only if neither yields evidence do we
//    fall to UNCLEAR-CRITERIA (structurally unmeasurable).
let open_artifacts = stuck_evidence(goal);
if !open_artifacts.is_empty() {
    Ok(NoProgressWhy::new(NoProgressClass::GenuinelyStuck, open_artifacts))
} else if let Some(derived) = derive_criteria(goal) {
    Ok(NoProgressWhy::new(NoProgressClass::GenuinelyStuck, derived))
} else {
    Ok(NoProgressWhy::new(
        NoProgressClass::UnclearCriteria,
        unclear_criteria_evidence(goal),
    ))
}
```

The change is purely additive: the `open_artifacts` branch and the
`unclear_criteria_evidence` fallback are unchanged; `derive_criteria` only
intercepts stalls that would otherwise fall straight to `UNCLEAR-CRITERIA`.

## Fail-closed and security properties

- **The storm is the abuse vector.** Idempotent, bounded, restart-surviving
  suppression (the durable marker) is the fix's core security property — it caps
  breaker-authored issue creation at **one filing attempt per stuck goal**, even
  across daemon restarts and `gh` failures.
- **No shell surface added.** The suppression-marker write is a pure JSON
  goal-board mutation — no `gh`, no shell — so the marker path is off the command
  surface entirely. `gh` is still invoked only via discrete `Command::args(...)`
  (never `sh -c`, never a `format!`-built command line); goal text is never
  interpolated into a flag.
- **Fixed sentinel identity.** The marker `ref_id` is a constant; `goal_id` is
  never used to build a filesystem path — no path traversal, no argv smuggling.
- **Untrusted goal text.** `derive_criteria` caps/normalizes input length before
  matching and routes any goal-derived issue title/body through the existing
  truncation + secret-scrubbing used by the escalation filer. New tracing fields
  avoid log-injection via newline/control chars and never log the `gh`
  credential.
- **Crash-safe store.** Marker writes are additive `Vec<WipRef>` appends through
  the existing atomic save path, so a crash mid-write cannot corrupt
  `goal_board.json`; the marker carries no secrets.

## What is unchanged

- `NO_PROGRESS_BREAKER_THRESHOLD`, both sentinel constants,
  `is_no_progress_marker`, `no_progress_blocked_reason`,
  `no_progress_blocked_reason_with_why`, and the full
  [root-cause ladder](./no-progress-root-cause-resolution-api.md) rungs —
  unchanged.
- `NoProgressClass`, `NoProgressResolution`, and `StuckGoalDisposition` — no
  variant added, removed, or reordered.
- `link_tracking_issue` and `parse_issue_number` — unchanged; the upgrade path
  reuses them.
- The `NO_PROGRESS_TRACKING_LABEL_PREFIX` linked-tracking-ref contract, the
  [re-investigation](./no-progress-reinvestigation-api.md) population predicates,
  and the terminal-rung [surfaced-failure bound](../concepts/no-progress-terminal-investigation.md)
  (`SURFACED_INVESTIGATION_FAILURE_LIMIT`) — unchanged.
- Clear-criteria goals — byte-identical outcomes (guarded by regression tests).

## See also

- [Concept: the no-progress breaker suppresses its own issue storm](../concepts/no-progress-breaker-storm-suppression.md) — the storm incident and the suppress-then-link rationale.
- [No-progress root-cause resolution API reference](./no-progress-root-cause-resolution-api.md) — the classification, WHY types, and the escalation the marker guards.
- [No-progress breaker API reference](./no-progress-breaker-api.md) — the base threshold, sentinel, tracker, and self-heal.
- [No-progress re-investigation API reference](./no-progress-reinvestigation-api.md) — the already-blocked re-investigation pass that shares `escalate_with_tracking_issue`.
- [How-to: diagnose a no-progress breaker issue storm](../howto/diagnose-a-no-progress-breaker-issue-storm.md) — the operator runbook.
- [Unblock stuck OODA goals](../howto/unblock-stuck-ooda-goals.md) — clearing a suppressed/blocked goal by hand.
