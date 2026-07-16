# Tertiary Deep Dive — no_progress WHY reasoner + daemon wiring (the external kgpacks goal)

**Role:** TERTIARY investigator (amplihack:architect focus)
**HEAD:** `3e6b6933`  **Date:** 2026-07-16
**Focus (assigned):** `no_progress` WHY classification + `wiring.rs`; the WHY reasoner +
daemon wiring for the external `kgpacks` goal `goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca`.
**Drift precondition:** `git diff --name-only HEAD -- src/**/*.rs` (excl. tests) → **zero** — every
citation below re-grounded byte-for-byte at HEAD `3e6b6933` (same commit as the primary D1/D2/D3 report).

**Verdict:** EXTEND, do not restart. The primary's D1/D2/D3 (`primary_D1_writeback_D2_D3_decide_arm_HEAD_3e6b6933.md`)
stands. This report adds ONE specific, load-bearing architectural finding the prior waves under-specified:
**why the `goal:blocked:fix-agent-…` member of the composite never converges** is not (only) that
`decide_blocked_goal` is notify-only (primary D2). It is that the ONE subsystem that *resolves* a blocked
goal to a terminal disposition — the WHY reasoner + `reinvestigate_bare_blocked_goals` ladder — is
**structurally unreachable for `fix-agent-*` goals** because its selection predicate keys on the
*wrong OODA-SAFEGUARD prefix*.

---

## The wiring (daemon → reasoner), grounded

The self-resolving WHY ladder is wired ONLY inside a **double gate** (`ooda_loop/cycle.rs:582-699`):

```
cycle.rs:582  if let Some(source) = &memories.completion_evidence {          // Gate A
cycle.rs:583    if no_progress_investigation_enabled() {                     // Gate B (default ON)
cycle.rs:599      apply_no_progress_breaker_investigated(state, &outcomes, …) // ON-TRANSITION classify
cycle.rs:628      reinvestigate_bare_blocked_goals(state, source_ref, …)      // ALREADY-BLOCKED rescue
                } else { apply_no_progress_breaker(…) }                      // base bare-park ladder
              }                                                               // else: nothing
```

- Gate A source = production `GhCliEvidenceSource::new(repo_root)` (`daemon/mod.rs:462-464`).
- Reasoner = `DeterministicNoProgressReasoner::new(source_ref)` (`cycle.rs:593-594`,
  impl `ooda_loop/no_progress.rs:990-1058`).
- Healer = `CloneRepoHealer::new("rysweet")` (`cycle.rs:595`, impl `no_progress.rs:1076-1114`).

There are exactly **two entry doors** to this ladder. Both are closed to a `fix-agent-*` goal:

### Door 1 — on-transition breaker (`apply_no_progress_breaker_investigated`, `no_progress.rs:407`)
It bumps a goal's no-action counter and classifies it **only when the goal appears in `outcomes` with a
`goal_id` and produced no progress** (`no_progress.rs:458-491`; `record_no_action` @491, threshold check @492).
An `outcome` exists only if the brain **selected** the goal and acted on it. A goal already in
`GoalProgress::Blocked(_)` is **not selected** (Decide/dispatch reject it — `advance_goal/mod.rs:126-131`
returns "goal is blocked" without acting). ⇒ a Blocked goal produces no outcome ⇒ its counter never bumps
⇒ Door 1 **never fires for an already-blocked goal**. (Confirmed by the module doc, `no_progress.rs:780-783`.)

### Door 2 — already-blocked rescue (`reinvestigate_bare_blocked_goals`, `no_progress.rs:808`)
This is the ONLY door for a goal that is *already* Blocked. Its selection set (`no_progress.rs:826-837`):

```rust
GoalProgress::Blocked(reason) if is_bare_no_progress_block(reason) => Some(g.id.clone())
```

`is_bare_no_progress_block` (`no_progress_breaker.rs:108-113`) = `is_no_progress_marker(reason) && no class token`,
and `is_no_progress_marker` (`:88-90`) = `reason.starts_with(NO_PROGRESS_BLOCKED_PREFIX)`.

---

## The defect: observe/resolve prefix asymmetry (the load-bearing finding)

There are **two distinct `[OODA-SAFEGUARD]` block prefixes**, authored by two different subsystems:

| Prefix constant | Value | Author |
|---|---|---|
| `NO_PROGRESS_BLOCKED_PREFIX` (`no_progress_breaker.rs:70`) | `🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for ` | overseer no-progress breaker |
| `BRAIN_FAILURE_BLOCKED_PREFIX` (`spawn.rs:39`) | `🔒 [OODA-SAFEGUARD] OODA brain failing for ` | `dispatch_spawn_engineer` after 3 failed spawns (`spawn.rs:336-338`) |

A `fix-agent-*` goal is a guided-engineer / fix-agent **spawn**. When its spawn fails the 3× ceiling, it is
parked with the **brain-failure** prefix (`spawn.rs:336-338`), NOT the no-progress prefix.

Now trace the two sides of the loop:

- **OBSERVE side** treats both prefixes identically. `blocked_goals_from_board` → `blocked_goal_of`
  (`sensor.rs:204-221`) sets `needs_review = is_no_progress_marker(reason) || is_brain_failure_marker(reason)`
  (`sensor.rs:213`). ⇒ the brain-failure-blocked `fix-agent-kgpacks-…` goal **IS surfaced** as a needs-review
  `goal:blocked:{goal_id}` every overseer window (token stamped `mod.rs:1336`, per primary).

- **RESOLVE side** does NOT. `is_bare_no_progress_block` matches `NO_PROGRESS_BLOCKED_PREFIX` **only**
  (`:109` → `:88-90`). A `BRAIN_FAILURE_BLOCKED_PREFIX` reason starts with a *different* string ⇒
  `is_no_progress_marker` = **false** ⇒ `is_bare_no_progress_block` = **false** ⇒
  `reinvestigate_bare_blocked_goals` **skips it** (`no_progress.rs:831-836`).

**This is the coverage hole.** The observer and the resolver disagree on their prefix set:
`sensor.rs:213` observes `{no-progress ∪ brain-failure}`; `no_progress_breaker.rs:108-113` rescues only
`{no-progress}`. Every `fix-agent-*` goal that hit the brain-failure ceiling therefore lives permanently
in the difference set — **observed-but-never-resolved** — re-emitting `goal:blocked:fix-agent-…` each window.

### The second latch (defence-in-depth also closed)
There is a *sibling* auto-recovery for the brain-failure marker: `dispatch_advance_goal` clears it and
restores `NotStarted` (`advance_goal/mod.rs:101-125`, issue #1911). But that branch runs **only when the goal
is dispatched**, and a Blocked goal is never selected/dispatched. ⇒ this recovery is gated behind the very
selection the block prevents — the classic self-latch. So **both** resolution mechanisms for a
brain-failure-blocked goal (Door 2 rescue, and #1911 auto-recovery) are unreachable while it stays Blocked.

Net: the goal is pinned. `decide_blocked_goal` only notifies (primary D2, `mod.rs:1603-1630`), the two
resolvers are both unreachable, and `blocked_goals_from_board` keeps re-emitting it ⇒ `×2` recurrence.

---

## Reconciliation with the primary (no double-counting)

- Primary D2 = "the `goal:blocked` lane never *escalates* (2↔3 dead zone) because `Reported` records no
  Lane-B occurrence." **True and orthogonal.** That explains why the escalation *rung* is unreachable.
- **This report** explains why the goal is *Blocked in the first place and stays there*: it entered via the
  **brain-failure** door (`spawn.rs:336`), which is outside the WHY-ladder's selection predicate entirely.
  Even a fully-fixed D2 escalation rung would still only *notify a human*, not *resolve* the goal — because
  the deterministic self-resolving classifier (auto-complete / heal / defer / spawn) is never consulted for
  this prefix. The two findings compose: D2 is the dead escalation rung; this is the **dead entry door**.

- `resource:engineer_spawn` + `spawn=false` (primary's lead) fits precisely: the `fix-agent-*` goal
  *was* spawned (`engineer_spawn`), the spawn failed/was rejected → brain-failure park. Note the WHY ladder,
  even if it DID run, routes `GenuinelyStuck`/`UnclearCriteria` first to `SpawnEngineer`
  (`no_progress_breaker.rs:411-413`); a rejected `spawn_engineer` (`no_progress.rs:712-748`) leaves the goal
  un-unblocked and escalates next threshold. So `spawn=false` would defeat the ladder's non-terminal rung too
  — but here it never even reaches the ladder. **`spawn=false` is the correct lead; the PR-ID roster is noise.**

- The evidence boundary is NOT the primary constraint. `GhCliEvidenceSource` resolves state cross-repo via
  `gh` on `repo_slug(goal)` (`completion_gate.rs:669-694`) and `repo_present` checks `$HOME/src/<repo>`
  (`:706-726`); `CloneRepoHealer` clones `owner/repo` or `rysweet/<name>` (`no_progress.rs:1076-1099`). So an
  external `rysweet/kgpacks-rs` goal *could* be classified/healed **iff it reached the reasoner** — which,
  per the coverage hole above, it does not. External-repo visibility is a latent secondary risk, not the
  live cause.

---

## Structural recommendation (architecture, minimal-surface)

Close the observe/resolve asymmetry — make the resolver's selection set match the observer's:

1. **Widen Door 2's predicate** so `reinvestigate_bare_blocked_goals` also selects
   `is_brain_failure_marker(reason)` goals (or add a `is_bare_safeguard_block = is_bare_no_progress_block ||
   is_bare_brain_failure_block`). The WHY reasoner is prefix-agnostic — it classifies from live artifacts, not
   from the reason string — so it will correctly route a brain-failure-parked `fix-agent-*` goal to
   `AlreadyComplete`/`Heal`/`Defer`/`SpawnEngineer` just as it does a no-progress park.
   **INV:** re-authoring must still emit a marker the observer's `needs_review` and the `unblock-all` bulk-clear
   recognise; keep the idempotency guarantee (once a WHY/class token is attached, the reason is no longer
   "bare" and the pass will not re-process it — mirror the existing `is_bare_no_progress_block` self-exclusion).

2. **Break the #1911 self-latch** (defence-in-depth): the brain-failure auto-recovery
   (`advance_goal/mod.rs:101-125`) should have an entry that does not require the goal to be brain-selected —
   e.g. run the same reinvestigation sweep over brain-failure parks — so a Blocked `fix-agent-*` goal can
   accrue a recovery attempt without first being dispatched.

3. Land AFTER / alongside the primary's D2 gate+counter fix, so a goal that *legitimately* reaches
   `GenuinelyStuck` with `spawn=false` can still climb the escalation rung rather than churn.

**Landing order (dependency-safe):** (a) widen Door 2 predicate [this report]; (b) primary D2 gate+counter
atomically; (c) primary D3 gap closing rung; (d) primary D1 write-boundary self-provenance filter; (e)
regression-gate the H0–H8 matrix.

## Open questions (for synthesis / runtime verification)
- Runtime-confirm the live block reason on `fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca`:
  is it `BRAIN_FAILURE_BLOCKED_PREFIX` (this report's premise) or an operator/subordinate/dependency reason
  (`spawn.rs:467`, `typed_goal_session.rs:208`, `subordinate.rs:341`)? Any of the latter is ALSO outside
  Door 2's predicate, so the coverage-hole conclusion holds a fortiori, but the exact author changes the fix
  #1 predicate set.
- Should widening Door 2 be scoped to brain-failure parks only, or to *all* non-WHY `Blocked` reasons?
  The safe minimum is brain-failure (it shares the `[OODA-SAFEGUARD]` sentinel and is machine-authored);
  operator-set reasons (`spawn.rs:455-468`) should likely stay excluded so autonomy never overrides a human.
