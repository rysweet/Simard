---
title: No-progress breaker tracking-issue dedup & filing API
description: Reference for how the OODA no-progress breaker deduplicates its operator-facing `ooda-stuck` tracking issue across cycles by persisting the breaker `WipRef` through the goal store, and how `GhIssueFiler::file_issue` treats `gh` exit-0 as filed even when the created-issue URL is unparsable — fixing the duplicate-issue churn (rysweet/Simard#4508/#4504/#4502/#4499/#4497) and the "cannot file issue" defect (rysweet/Simard#4472, #4474).
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./no-progress-breaker-api.md
  - ./no-progress-root-cause-resolution-api.md
  - ./wip-ref-liveness-reconcile-api.md
  - ./goal-board-api.md
  - ./file-backed-goal-store.md
  - ../concepts/no-progress-root-cause-resolution.md
  - ../howto/unblock-stuck-ooda-goals.md
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/goals/types.rs
  - ../../src/goal_curation/operations.rs
  - ../../src/goal_curation/types.rs
---

# No-progress breaker tracking-issue dedup & filing API

> **Status: implemented.** The escalation/dedup path
> (`escalate_with_tracking_issue`, `is_breaker_tracking_ref`,
> `NO_PROGRESS_TRACKING_LABEL_PREFIX`, `GhIssueFiler::file_issue`,
> `parse_issue_number`, `link_tracking_issue`) lives in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs).
> The persisted `wip_refs` field lives on
> [`GoalRecord`](https://github.com/rysweet/Simard/blob/main/src/goals/types.rs);
> the `GoalRecord ⇄ ActiveGoal` mapping lives in
> [`src/goal_curation/operations.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/operations.rs).

When the no-progress breaker fires on a goal it classifies as
`UNCLEAR-CRITERIA` (a stalled goal with no machine-verifiable done-criteria),
it files **one** operator-facing tracking issue labelled `ooda-stuck` and
**links it back to the goal** so the done-gate gains a derivable completion
signal. Two defects broke this:

- **Duplicate-issue churn** (`#4508`, `#4504`, `#4502`, `#4499`, `#4497`): the
  dedup guard read `ActiveGoal.wip_refs`, but that field was **dropped every
  time the goal round-tripped through the persistent store** — so on the next
  OODA cycle the goal came back with empty `wip_refs`, `already_tracked`
  evaluated `false`, and the breaker filed a *new* `ooda-stuck` issue each
  cycle.
- **"Cannot file the tracking issue"** (`#4472`, `#4474`): `file_issue`
  returned `None` (observed as a filing failure) whenever `gh issue create`
  succeeded but its printed URL did not parse into a bare issue number — a
  *swallowed success*.

This reference specifies the finished, deduplicated, reliably-filing behaviour.
For the breaker threshold, sentinel marker, and cycle driver, see the
[No-progress breaker API reference](./no-progress-breaker-api.md).

## Contents

- [The breaker tracking `WipRef`](#the-breaker-tracking-wipref)
- [Persistence: `wip_refs` survives the goal store](#persistence-wip_refs-survives-the-goal-store)
- [Dedup guard: `escalate_with_tracking_issue`](#dedup-guard-escalate_with_tracking_issue)
- [Filing contract: `GhIssueFiler::file_issue`](#filing-contract-ghissuefilerfile_issue)
- [`NoProgressIssueFiler` trait & fake filer](#noprogressissuefiler-trait--fake-filer)
- [Observability](#observability)
- [Security contract](#security-contract)
- [What is unchanged](#what-is-unchanged)

## The breaker tracking `WipRef`

A breaker-authored tracking ref is a normal
[`WipRef`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs)
whose `kind` is `"issue"` and whose `label` is prefixed with the breaker
sentinel:

```rust
/// Prefix on a breaker-authored `WipRef.label`. Identifies a tracking issue the
/// breaker filed so a re-stall reuses it instead of filing a duplicate.
const NO_PROGRESS_TRACKING_LABEL_PREFIX: &str = "[no-progress-tracking] ";

/// True iff `wip` is a breaker-authored tracking issue (dedup key).
fn is_breaker_tracking_ref(wip: &WipRef) -> bool {
    wip.kind.eq_ignore_ascii_case("issue")
        && wip.label.starts_with(NO_PROGRESS_TRACKING_LABEL_PREFIX)
}
```

`link_tracking_issue` appends exactly one such ref per goal and is idempotent —
it is a no-op if the goal already references that issue. Crucially, it must
still produce a **dedup-able** ref when the filer reports success **without** a
parsed number (`FiledIssue.number == None`, the `#4472` exit-0/unparsable-URL
case): the dedup key is `is_breaker_tracking_ref`, which is keyed on
`kind == "issue"` **plus the sentinel label prefix — not on the number** — so a
URL-keyed ref dedups on the next cycle exactly like a number-keyed one:

```rust
fn link_tracking_issue(goal: &mut ActiveGoal, filed: &FiledIssue) {
    // Dedup identity: prefer the issue number; fall back to the URL when the
    // filer could not parse a number (exit-0 but unparsable URL, #4472). Both
    // branches emit a breaker tracking ref (kind=issue + sentinel label), so
    // `is_breaker_tracking_ref` dedups it next cycle even with no number.
    let (ref_id, label_suffix) = match filed.number.as_deref() {
        Some(n) => {
            let n = n.trim_start_matches('#').to_string();
            (n.clone(), format!("#{n}"))
        }
        None => match filed.url.as_deref() {
            Some(u) => (u.to_string(), u.to_string()), // URL-keyed fallback
            // Pathological: exit-0 with neither number nor URL. Nothing stable
            // to dedup on — skip-and-log; the goal stays Blocked and may re-file
            // next cycle. (gh always prints a URL on success, so this is inert.)
            None => { tracing::warn!(target: "simard::ooda",
                "no-progress breaker: filed issue had no number or url; not linked"); return; }
        },
    };
    // Idempotent: keyed on kind + normalized ref_id (number or URL).
    let already = goal.wip_refs.iter().any(|w| {
        w.kind.eq_ignore_ascii_case("issue")
            && w.ref_id.trim_start_matches('#') == ref_id.trim_start_matches('#')
    });
    if already { return; }
    goal.wip_refs.push(WipRef {
        kind: "issue".to_string(),
        ref_id,                                                   // number or URL
        label: format!("{NO_PROGRESS_TRACKING_LABEL_PREFIX}{label_suffix}"),
        url: filed.url.clone(),
    });
}
```

The dedup key is scoped to `is_breaker_tracking_ref` **only**, so ordinary
engineer/PR `wip_refs` never suppress a legitimate first escalation. Because the
sentinel label prefix is present whether the ref is number- or URL-keyed, a
number-less "filed" issue is fully dedup-able: on the next firing
`already_tracked` is `true` and no duplicate `ooda-stuck` issue is filed — this
is what prevents the churn recurring for exactly the `#4472` unparsable-URL
case.

## Persistence: `wip_refs` survives the goal store

The root cause of the churn was **loss of `wip_refs` across the persistence
boundary**. The finished implementation persists the field end to end:

1. **`GoalRecord` carries `wip_refs`.** A new serde-defaulted field is added to
   the persisted record so the tracking ref is written to
   `goal_records.json` / `goal_board.json`:

   ```rust
   /// Breaker/engineer tracked artifacts carried across OODA cycles.
   /// `#[serde(default)]` so pre-existing goal-store snapshots that predate this
   /// field still deserialise (backward compatible).
   #[serde(default, skip_serializing_if = "Vec::is_empty")]
   pub wip_refs: Vec<WipRef>,
   ```

2. **Forward map (`ActiveGoal → GoalRecord`)** copies `wip_refs` into the
   record instead of dropping it (`active_goals_as_records`).

3. **Reverse map (`GoalRecord → ActiveGoal`)** populates
   `ActiveGoal.wip_refs` from the record instead of `Vec::new()`. Only the
   **persistence-reload path** (`record_as_active_goal`, the inverse of
   `active_goals_as_records`) is changed — it is the sole `ActiveGoal`
   construction site that has a `GoalRecord` in scope. The **fresh-goal
   creation** sites in `operations.rs` (e.g. the promote/seed paths) have no
   record to read and correctly keep `wip_refs: vec![]`; they are *not*
   modified.

Because the tracking ref now survives a store reload, on the **second** firing
the goal rehydrates with its breaker ref present, `already_tracked` is `true`,
and no duplicate issue is filed.

> **Backward compatibility.** `wip_refs` is `#[serde(default)]`. Legacy
> `goal_records.json` / `goal_board.json` snapshots written before this field
> existed load with an empty list — no migration, no load failure.

### Rehydrated-ref validation

Validation runs at exactly **one choke point: the reverse map**
(`record_as_active_goal`, `GoalRecord → ActiveGoal`) as each persisted `WipRef`
is copied into the in-memory `ActiveGoal.wip_refs`. This is the sole boundary
where untrusted on-disk data (`goal_records.json` / `goal_board.json`) enters
the running state, so validating there means every downstream reader —
`escalate_with_tracking_issue`'s `already_tracked` check, `link_tracking_issue`,
and any reporting — sees only already-validated refs and needs no re-check.
Neither the forward map (`active_goals_as_records`, which serializes
already-in-memory refs) nor the escalate-time read re-validates.

Each rehydrated `WipRef` is validated before it is admitted as a dedup key:

- non-empty `kind` and `ref_id`;
- for a breaker tracking ref (`kind == "issue"` + sentinel label prefix), the
  `ref_id` is accepted if it is **digits only** (number-keyed) **or** a
  non-empty URL (URL-keyed fallback, matching the `#4472` number-less contract);
- a malformed ref is **dropped and logged** via `tracing::warn!` as it is
  loaded, so it never reaches the dedup guard, never fabricates a link, and
  never panics.

## Dedup guard: `escalate_with_tracking_issue`

Every breaker path funnels its side effect through one helper. It (a) checks the
persisted dedup key, (b) files at most one issue, and (c) links it back:

```rust
fn escalate_with_tracking_issue(
    state: &mut OodaState,
    goal_id: &str,
    blocked_reason: String,
    issue_title: &str,
    issue_body: &str,
    filer: &dyn NoProgressIssueFiler,
) {
    // Idempotence: never file a second tracking issue for a goal already linked
    // to one — a re-stall must not spam duplicate `ooda-stuck` issues.
    let already_tracked = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .is_some_and(|g| g.wip_refs.iter().any(is_breaker_tracking_ref));

    let filed = if already_tracked { None } else { filer.file_issue(issue_title, issue_body) };

    if let Some(g) = state.active_goals.active.iter_mut().find(|g| g.id == goal_id) {
        g.status = GoalProgress::Blocked(blocked_reason);
        if let Some(issue) = &filed {
            link_tracking_issue(g, issue);
        }
    }
}
```

Behaviour matrix:

| Goal already carries a breaker `WipRef`? | `file_issue` called? | Result |
| --- | --- | --- |
| yes (rehydrated from store) | **no** | goal re-`Blocked`; existing issue reused; no new issue |
| no | yes → `Some` with a number | goal `Blocked`; new `ooda-stuck` issue filed and **number-linked** |
| no | yes → `Some`, `number: None`, URL present (`#4472`) | goal `Blocked`; issue filed and **URL-linked** — a dedup-able breaker ref, so no duplicate next cycle |
| no | yes → `None` (real `gh` non-zero / spawn failure) | goal `Blocked`; no link; error surfaced via `tracing::error!` |

## Filing contract: `GhIssueFiler::file_issue`

The production filer shells out with an **argv-based** `gh` invocation (never
`sh -c`) and treats **process exit code, not URL parseability, as the source of
truth** for "was it filed":

```rust
impl NoProgressIssueFiler for GhIssueFiler {
    fn file_issue(&self, title: &str, body: &str) -> Option<FiledIssue> {
        match std::process::Command::new("gh")
            .args(["issue", "create", "--title", title, "--body", body,
                   "--label", "ooda-stuck"])
            .output()
        {
            Ok(out) if out.status.success() => {
                // gh printed the created issue URL. A number is best-effort:
                // exit-0 means FILED even if the URL doesn't parse (#4472 fix).
                let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let number = parse_issue_number(&url);
                tracing::warn!(target: "simard::ooda",
                    title = %title, issue = number.as_deref().unwrap_or("?"),
                    "no-progress breaker: tracking issue filed for stuck goal");
                Some(FiledIssue {
                    url: (!url.is_empty()).then(|| url.clone()),
                    number,                       // Option<String>: may be None
                })
            }
            Ok(out) => { /* non-zero exit → tracing::error! → None */ None }
            Err(e)  => { /* spawn failure → tracing::error! → None */ None }
        }
    }
}
```

Key change (`#4472`): `FiledIssue.number` becomes **optional**
(`Option<String>` — today it is a required `String`), and a successful
(`exit 0`) `gh issue create` returns `Some(FiledIssue { .. })` even when
`parse_issue_number` yields `None`. Previously the `number.map(...)` collapsed a
real success into `None`, so the goal never got linked and the breaker reported
it could not file its tracking issue.

Making `number` optional has a required ripple into
[`link_tracking_issue`](#the-breaker-tracking-wipref): a number-less success is
**still linked**, using the URL as the dedup identity (`ref_id = url`), and the
ref still carries the sentinel label prefix so `is_breaker_tracking_ref` dedups
it next cycle. Without this branch the `#4472` case would file, fail to produce
a dedup-able ref, and re-file every cycle — re-opening the churn
(`#4508`/`#4504`/…). Only the pathological "exit-0 with neither number nor URL"
case yields no link (skip-and-log); `gh` always prints a URL on success, so it
is inert in practice.

`parse_issue_number` remains strict — it only returns a number when the trailing
URL segment is all ASCII digits, so a malformed URL never fabricates a bogus
link:

```rust
fn parse_issue_number(url: &str) -> Option<String> {
    let last = url.trim().trim_end_matches('/').rsplit('/').next()?;
    (!last.is_empty() && last.chars().all(|c| c.is_ascii_digit())).then(|| last.to_string())
}
```

The `ooda-stuck` label is a **constant literal** passed as its own argv element.
The breaker assumes the label exists in the target repo; a missing label is a
non-zero `gh` exit that surfaces via `tracing::error!` rather than a silent
drop.

## `NoProgressIssueFiler` trait & fake filer

Filing is abstracted behind a trait so the escalation logic is unit-testable
without touching GitHub:

```rust
pub(crate) trait NoProgressIssueFiler {
    /// File one tracking issue. `None` means "not filed" (goal stays Blocked,
    /// unlinked) — never aborts the cycle.
    fn file_issue(&self, title: &str, body: &str) -> Option<FiledIssue>;
}
```

Tests inject a fake filer to assert the finished guarantees:

| Test | Asserts |
| --- | --- |
| dedup across store reload | after a simulated `GoalRecord ⇄ ActiveGoal` round-trip, a second firing calls `file_issue` **zero** additional times |
| unparsable-URL success (`#4472` regression) | a fake filer returning `FiledIssue { number: None, url: Some(..) }` links the goal with a **URL-keyed** breaker ref and reports filed |
| number-less dedup across reload | after the URL-keyed link round-trips through the store, a second firing files **zero** additional issues (URL-keyed ref dedups just like a number-keyed one) |
| `ooda-stuck` label present | the argv contains `--label ooda-stuck` |
| malformed rehydrated `WipRef` | a bad persisted ref is dropped-and-logged **at the reverse-map load** — never used as a dedup key or a link |

## Observability

All breaker filing/dedup outcomes are emitted as **structured `tracing`
events** under `target: "simard::ooda"` — there are **no `print!`/`println!`**
in this path:

- `tracing::warn!` on a successful file (with `title`, `issue`);
- `tracing::error!` on non-zero `gh` exit (with truncated `stderr`) and on
  spawn error (with `error`);
- `tracing::warn!` when a malformed rehydrated ref is skipped.

The [`NoProgressBreakerReport`](./no-progress-breaker-api.md#noprogressbreakerreport)
records which goals were escalated this cycle for assertion and logging.

## Security contract

- **argv-only subprocess.** `Command::new("gh").args([...])` — never `sh -c` or
  a string-built command. `--label ooda-stuck` stays a constant literal.
- **No secrets in issues or logs.** The issue title/body contain only the goal
  id, breaker reason, and a criteria summary. No env dumps, tokens, or file
  contents (public-repo issues are world-readable). Auth tokens are never read,
  echoed, or logged; `tracing` fields are bounded and untrusted `stderr` is
  truncated.
- **Store is a trust boundary.** `#[serde(default)]` prevents a load failure;
  every rehydrated `WipRef` is validated **at the reverse-map load choke point**
  (`record_as_active_goal`) before it reaches the dedup guard; malformed refs
  are dropped-and-logged, never fabricated into a link.

## What is unchanged

- The breaker **threshold**, sentinel `[OODA-SAFEGUARD]` Blocked marker, the
  standing/perpetual-goal exemption, and the load-time `heal_stale_no_progress_blocks`
  self-heal are unchanged (see the
  [No-progress breaker API reference](./no-progress-breaker-api.md)).
- The done-gate still treats a linked, `CLOSED` tracking issue as the derivable
  completion signal for an `UNCLEAR-CRITERIA` goal.
- This change references `#4474` and `#4472` and marks
  `#4508`/`#4504`/`#4502`/`#4499`/`#4497` as duplicates that the persistence +
  dedup fix prevents.
