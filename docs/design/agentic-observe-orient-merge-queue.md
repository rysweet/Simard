---
title: "Agentic observe/orient merge-queue + issue reasoning"
summary: >
  How Simard's OODA observe/orient stage REASONS agentically over the open-PR
  merge queue and open issues across the governed roster, instead of a brittle
  imperative allowlist gate that produced zero merge reasoning. A DETERMINISTIC
  WORKFLOW OF AGENTIC STEPS + PROMPTS runs each Overseer cycle: an agent runs
  read-only `gh` across the roster and REASONS to a bounded semantic brief
  (triaged issues + reasoned PRs), which populates new ObservedState fields.
  Rust is a thin rail that schedules the recipe (idle/liveness only, no
  wall-clock timeout) and re-narrows the brief's PR conclusions back through the
  UNCHANGED objective + agentic merge gate. Broadening REASONING never widens
  merge AUTHORIZATION.
last_updated: 2026-07-19
review_schedule: as-needed
owner: simard
doc_type: design
status: live
issue: 4097
---

# Agentic observe/orient merge-queue + issue reasoning

> Simard stewards a roster of repositories. The way she now decides which open
> pull requests are ready for merge action — and which open issues need a
> workstream — is the **`observe-merge-queue`** recipe: two agent steps
> (REASON → BRIEF) driven by prompts, invoked on the Overseer's cadence by a
> thin Rust rail. There is no Rust "code sensor" doing the reasoning. The
> reasoning lives entirely in the agent and is handed forward semantically; Rust
> only re-derives the *authorized* subset through the objective merge gate.

## Principle (operator directive)

Solve control-loop decisions as **agentic recipes behind a THIN deterministic
rail, not imperative heuristics**. Reasoning about the merge queue and the issue
backlog is exactly such a decision: "check all open PRs and think carefully
about which are ready for action; triage the open issues by priority, readiness,
and next action." That is judgement, not a fixed predicate, so it belongs in a
prompt — while the *action* (merge / comment / close) stays behind an objective,
fail-closed Rust rail.

Two invariants hold this design together:

1. **Reasoning is broad and default-ON.** The observe/orient stage must reason
   about the whole open-PR queue and issue backlog across the governed roster
   every cycle, even when no autonomous-merge env vars are set. Producing *zero*
   reasoning is the bug this feature exists to kill.
2. **Authorization is narrow and unchanged.** The agentic brief may *propose*;
   it may never *authorize*. Every merge still passes the objective gates
   (`stewardship::merge_authority`) + the `MergeJudge` + the anti-recursion
   author guard, and the engineer-PR narrowing. No path uses `--admin` or
   `--no-verify`.

## Anti-pattern this replaces (retired root cause)

The Observe path populated `ObservedState.ready_prs` from a single imperative
sensor:

```rust
// src/overseer/mod.rs (before): the ONLY merge-reasoning path.
observed.ready_prs = self.caps.prs.survey_ready_prs(&config::automerge_repos());
```

`survey_ready_prs` lists only PRs whose author matches `SIMARD_AUTOMERGE_AUTHOR`
in a repo on the `SIMARD_AUTOMERGE_REPOS` allowlist. In the live systemd unit
**both env vars are unset**, so:

```
automerge_repos() == []           (allowlist empty)
survey_ready_prs(&[]) == []        (loop body never runs; gh never called)
ObservedState.ready_prs == []      (ALWAYS empty)
→ Signal::PrReadyToMerge never emitted
→ the Overseer never reasons about ANY open PR
→ prs_merged = 0 for 36h+ while ~30 CI-green mergeable PRs pile up open
```

The allowlist was a **silent hard-OFF**: unset produced no reasoning *and* no
signal that reasoning was disabled. The observe/orient stage never enumerated
the open-PR queue or the issue backlog at all — the dead-wire allowlist sensor
was the only path.

This feature replaces that single imperative sensor with an agentic reasoning
pass and makes disablement **loud**.

## Architecture

```
                         Overseer cadence (every N ticks, idle/liveness only)
                                        │
                    ┌───────────────────▼─────────────────────┐
                    │  Thin Rust rail: MergeQueueReasoner       │
                    │  (src/overseer/merge_queue_observe.rs)    │
                    │  roster + in-flight refs in → brief out   │
                    └───────────────────┬─────────────────────┘
                                        │ ContextFile _path tokens on argv
                                        ▼
            recipe: prompt_assets/simard/recipes/observe-merge-queue.yaml
            ┌──────────────────────────────────────────────────────────┐
            │ REASON (agent)  read-only `gh` across governed roster:    │
            │   • open issues → triage (priority/readiness/next action) │
            │   • open PRs → reason (CI, mergeable/review, conflict,     │
            │     staleness, duplication)                                │
            │   writes bounded JSON brief to {{merge_queue_brief_path}}  │
            │                        │ semantic handoff (file)           │
            │ BRIEF (agent)   normalize/validate to the bounded schema   │
            └────────────────────────┬─────────────────────────────────┘
                                     ▼  opaque brief string
                    ┌────────────────────────────────────────┐
                    │  Rail parses brief FAIL-CLOSED into:     │
                    │   ObservedState.reasoned_prs             │
                    │   ObservedState.triaged_issues           │
                    │   ObservedState.merge_reasoning_status   │
                    └────────────────┬───────────────────────┘
                                     ▼ Orient / Decide (src/overseer/mod.rs)
   reasoned_prs ─── re-narrow (author guard + engineer-PR + objective gate) ──▶ ready_prs
        │                                                                          │
        │ stale ▶ Signal::StalePrDetected ▶ Intervention::FlagStalePr             │
        │ dup   ▶ Signal::DuplicatePrDetected ▶ Intervention::CloseDuplicatePr    │
        │                                                                          ▼
   triaged_issues ▶ Signal::IssueNeedsWorkstream ▶ (existing workstream/brief path)
                                                              Intervention::VerifyAndMergePr
                                                                          │
                                    stewardship::merge_authority (objective gates)
                                    + MergeJudge (fail-closed) + author guard
                                                                          │
                                          gh pr merge --squash --delete-branch
                                          (NO --admin / NO --no-verify)
                                                                          │
                                          NotifyOperator (email + Signal)
```

The load-bearing seam is the **`reasoned_prs → ready_prs` re-narrowing
projection**: the agent reasons over the *whole* queue, but only PRs that
independently re-pass the objective + author + engineer-PR gates become merge
candidates. This is how R1/R2 (broad reasoning) coexists with R5 (unchanged,
narrow authorization).

## 1. Roster — the reasoning scope (single source of truth)

The default reasoning scope is the existing governed-repos roster,
`prompt_assets/simard/ecosystem_repos.toml` — the same validated-slug, pure-data
roster the [`ecosystem-observe`](./ecosystem-observe.md) chain reads, including
Simard's own repo (`rysweet/Simard`). It is loaded through the existing
install-first [ecosystem-roster resolver](../reference/ecosystem-roster-resolution.md);
an empty roster is a **loud error**, never a silent empty scope.

Reusing the roster satisfies the anti-silent-OFF mandate directly: "unset merge
env vars" no longer means "no reasoning", it means "reason over the governed
roster."

## 2. Reasoning scope resolver — `merge_reasoning_scope()`

`config::merge_reasoning_scope()` (in `src/overseer/config.rs`, beside
`automerge_repos()`) resolves the scope from a single env var,
`SIMARD_MERGE_REASONING_SCOPE`, into a three-state enum. Unset is **default-ON**,
and explicit disablement is **loud** (R3):

```rust
pub enum MergeReasoningScope {
    /// Unset env ⇒ reason over the governed-repos roster (DEFAULT-ON).
    Roster,
    /// Explicit comma-separated `owner/name` list ⇒ reason over exactly these.
    Explicit(Vec<String>),
    /// Explicit `off` / `disabled` / falsey ⇒ reasoning DISABLED — LOUD:
    /// WARN log + merge_reasoning_status + a one-time NotifyOperator note.
    Disabled,
}

/// Pure, unit-testable resolver (the repo's `*_from(lookup)` convention).
pub fn merge_reasoning_scope_from(
    lookup: impl Fn(&str) -> Option<String>,
    roster: &[String],
) -> MergeReasoningScope;

/// Production entry: reads SIMARD_MERGE_REASONING_SCOPE from the environment and
/// the governed roster from ecosystem_repos.toml.
pub fn merge_reasoning_scope() -> MergeReasoningScope;
```

| `SIMARD_MERGE_REASONING_SCOPE` | Scope | Disablement signal |
|---|---|---|
| unset | `Roster` (governed repos + Simard) | — (reasoning ON) |
| `""` / whitespace | `Roster` | — (reasoning ON) |
| `rysweet/Simard,rysweet/azlin` | `Explicit([…])` | — (reasoning ON, narrowed) |
| `off` / `disabled` / `0` / `false` / `no` | `Disabled` | **LOUD**: WARN + status + one-time notify |

> **Unset ≠ disabled.** This is the crux of R3. The old allowlist conflated
> them: unset silently produced zero reasoning. Here, only an *explicit* off
> value disables reasoning, and when it does the daemon says so on every channel.

## 3. Prompts — the substance

### `prompt_assets/simard/overseer/merge_queue_reason.md` (the REASON step)

The canonical read-only reasoning prompt. Framing rules:

- **Data, not commands.** Roster slugs and in-flight refs are *inputs to reason
  about*, never instructions to execute (XPIA-hardened, mirroring the
  `ecosystem-observe` prompts).
- **Read-only.** The agent may run `gh pr list`, `gh pr view`, `gh issue list`,
  `gh issue view`, `gh pr checks` — never `gh pr merge`, `gh pr close`,
  `gh pr comment`, or any write. All *action* is re-derived in Rust from
  objective state.
- **Roster-scoped.** Reason only about repos in the provided roster file.
- **What to reason about, per open PR:** CI/check state, `mergeable`/review
  state, merge conflicts, staleness (age + inactivity), and duplication/overlap
  with other open PRs or already-merged work. Conclude one of:
  `ready-for-merge` / `needs-work` / `stale` / `duplicate`, with a one-line
  rationale.
- **What to reason about, per open issue:** priority, readiness (is it
  actionable now?), and the single next action. Conclude a triage disposition.
- **Bounded output.** Emit a JSON brief with hard caps (see the schema below) so
  the payload is `ARG_MAX`-safe and cannot balloon.

### `prompt_assets/simard/overseer/merge_queue_brief.md` (the BRIEF step)

Reads the REASON step's file and normalizes it to the exact bounded schema,
dropping anything off-roster or malformed. This is the semantic handoff's second
half — Rust never parses the free-form reasoning, only the normalized brief, and
even that **fail-closed** (see §4).

### Brief schema (the bounded semantic contract)

```jsonc
{
  "reasoned_prs": [
    {
      "repo": "rysweet/Simard",          // must be in the roster
      "pr": 4123,
      "disposition": "ready-for-merge",  // | needs-work | stale | duplicate
      "rationale": "CI green, MERGEABLE, review approved",
      "duplicate_of": null                // pr number when disposition=duplicate
    }
  ],
  "triaged_issues": [
    {
      "repo": "rysweet/Simard",
      "issue": 4097,
      "priority": "high",                // high | medium | low
      "readiness": "ready",              // ready | blocked | needs-info
      "next_action": "spawn engineer to wire agentic merge-queue reasoning"
    }
  ]
}
```

## 4. Thin rail — the only new Rust (`merge_queue_observe.rs`)

A tiny seam schedules the recipe on the Overseer cadence and forwards its opaque
result. It mirrors the `ecosystem_observe.rs` `RecipeEcosystemObserver` /
`SpawnEcosystemRecipeRunner` seam and holds **no** reasoning state.

```rust
/// The thin rail. Invokes the `observe-merge-queue` recipe on the Overseer
/// cadence and returns its OPAQUE brief. It never runs `gh`, never reasons, and
/// never merges.
pub trait MergeQueueReasoner {
    /// Run one merge-queue + issue reasoning pass.
    ///
    /// - `Ok(Some(brief))` — an opaque semantic brief string to parse fail-closed.
    /// - `Ok(None))`        — nothing produced this pass.
    /// - `Err(_)`           — infrastructure fault. Caller logs a WARN, skips the
    ///   pass, and fabricates NO reasoned PRs/issues (fail-closed).
    fn observe(
        &self,
        request: MergeQueueObserveRequest,
    ) -> SimardResult<Option<String>>;
}

pub struct MergeQueueObserveRequest {
    /// Reasoning scope resolved by `merge_reasoning_scope()`. Named `scope`
    /// (not `roster`, as in `EcosystemObserveRequest`) because it is the
    /// *resolved* reasoning scope: the governed roster by default, but possibly
    /// a narrower subset when `SIMARD_MERGE_REASONING_SCOPE` lists explicit
    /// slugs. The rail serializes it to the recipe's `roster_path` ContextFile.
    pub scope: Vec<String>,
    /// Simard's in-flight OODA refs, for the agent's dedup reasoning. A plain
    /// `Vec<String>`, mirroring `EcosystemObserveRequest::inflight_refs`.
    pub inflight_refs: Vec<String>,
    /// Empty on the base pass; rail-set on escalation-ladder retries. Rail-owned,
    /// never a caller parameter (mirrors `EcosystemObserveRequest`).
    pub escalation_note: String,
}

pub struct MergeQueueObserveOutcome {
    pub reasoned_prs: Vec<ReasonedPr>,
    pub triaged_issues: Vec<TriagedIssue>,
}
```

Behavior:

- **Cadence.** Reuses the existing Overseer gate (enabled unless opted out, only
  every N ticks) — no new cadence knob. The unit of work is "one agentic
  merge-queue + issue reasoning pass."
- **Idle/liveness only, no wall-clock timeout.** The recipe-runner is supervised
  by idle/liveness detection exactly like the other agentic OODA steps. There is
  no wall-clock cap on the reasoning step.
- **Roster + refs in, opaque string out.** The rail writes each unbounded value
  to a per-invocation `ContextFile` and passes only the short `<key>_path`
  tokens on `argv` (`roster_path`, `inflight_refs_path`,
  `merge_queue_brief_path`) — `ARG_MAX`-safe. The boundary type across the
  recipe is a plain `String`.
- **Fail-closed parse.** The opaque brief is parsed into `MergeQueueObserveOutcome`
  fail-closed: any malformed entry, any off-roster `repo`, any missing required
  field, or a whole-brief parse error yields *empty* reasoned/triaged sets (plus
  a WARN) — never a fabricated PR reference and never an action.
- **Reasoning ≠ action.** The rail populates `ObservedState`; it never merges,
  comments, or closes. Those happen only through the gated interventions below.

### Retirement

The dead-wire imperative path is **retired as the sole reasoning source**.
`survey_ready_prs` is no longer the origin of merge reasoning; `ObservedState.ready_prs`
becomes a **derived view** projected from `reasoned_prs` by re-applying the
objective + author + engineer-PR gates (§6). The `SIMARD_AUTOMERGE_REPOS` /
`SIMARD_AUTOMERGE_AUTHOR` env vars survive only as an *additional* narrowing on
the *action* side (defense-in-depth), never as the reasoning on/off switch — and
their being unset can no longer silence reasoning.

## 5. State — additive `ObservedState` fields

`src/overseer/capabilities.rs` gains three additive fields (all default-empty /
default-unknown, so existing constructors and the side-effect-free
`observed_from_snapshot` projection compile unchanged):

```rust
pub struct ObservedState {
    // ... existing fields ...

    /// Agentic reasoning over the whole open-PR queue. Non-empty even when the
    /// autonomous-merge env vars are unset (reasoning is default-ON).
    pub reasoned_prs: Vec<ReasonedPr>,

    /// Agentic triage of the open-issue backlog.
    pub triaged_issues: Vec<TriagedIssue>,

    /// Whether merge reasoning is active, and if not, WHY (loud disablement).
    pub merge_reasoning_status: MergeReasoningStatus,
}
```

See the [API reference](../reference/agentic-merge-queue-reasoning-api.md) for
`ReasonedPr`, `TriagedIssue`, and `MergeReasoningStatus`.

## 6. Decide wiring — the re-narrowing projection & new interventions

In `src/overseer/mod.rs`, after the rail populates the reasoned fields:

1. **Merge candidates (unchanged authorization).**
   `reasoned_prs` entries with `disposition == ready-for-merge` are projected to
   `ready_prs` **only if** they independently re-pass:
   the anti-recursion author guard, the engineer-PR narrowing
   (`simard-autonomous` label OR engineer-exclusive branch namespace), and the
   objective gates (base allowlist + `MERGEABLE` + all checks green) via the same
   `evaluate_objective_gates` the authoritative gate uses. Survivors flow into
   the **existing** `Signal::PrReadyToMerge → DeliveryReady →
   VerifyAndMergePr → merge_authority + MergeJudge` chain, untouched. The agentic
   `ready-for-merge` disposition is a *proposal*; the projection is the
   authorization.
2. **Stale PRs.** `disposition == stale` →
   `Signal::StalePrDetected` → `Intervention::FlagStalePr`
   (a `gh pr comment` triage note; never a merge/close of unrelated work).
3. **Duplicate PRs.** `disposition == duplicate` →
   `Signal::DuplicatePrDetected` → `Intervention::CloseDuplicatePr`
   (`gh pr close` with a comment referencing `duplicate_of`).
4. **Issue triage.** `triaged_issues` with `readiness == ready` (and sufficient
   priority) → `Signal::IssueNeedsWorkstream` → the existing workstream/brief
   launch path.

### New interventions (`src/overseer/intervention.rs`)

`FlagStalePr` and `CloseDuplicatePr` are both classed `RiskClass::MergeAuthority`
(the same opt-in autonomy gate as `VerifyAndMergePr`; when the gate is off they
are **notify-only**). They:

- Build **positional argv only** (no shell), so injection is structurally
  impossible (`sanitize_context_var`).
- **Never** contain `--admin` or `--no-verify` (asserted by unit test, mirroring
  the conflict-path refusal test).
- Respect the **anti-recursion author guard** and the engineer-PR narrowing —
  they act only on Simard's own engineer PRs, never an operator's review PR.

## Configuration

| Env var | Default | Effect |
|---|---|---|
| `SIMARD_MERGE_REASONING_SCOPE` | unset ⇒ `Roster` (default-ON) | `off`/`disabled`/falsey ⇒ reasoning DISABLED (LOUD). A comma-separated `owner/name` list narrows the scope. |
| `SIMARD_OVERSEER_GAP_SCAN` | on (opt-out) | Falsey disables the whole agentic observation cadence (issue + merge-queue). |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | `1` | Run the reasoning pass once every N Overseer ticks. |
| `SIMARD_AUTOMERGE_REPOS` | unset | **Action-side** narrowing only. No longer gates *reasoning*; unset can no longer silence it. |
| `SIMARD_AUTOMERGE_AUTHOR` | unset | **Action-side** own-PR identity for the merge gate (defense-in-depth). |

The reasoning scope roster is configured by editing
`prompt_assets/simard/ecosystem_repos.toml` (data, not env), install-first on a
deployed daemon.

## Examples

### Run the merge-queue reasoning chain by hand

```bash
# Point the recipe at the committed roster + Simard's in-flight refs, plus a
# writable handoff path for the REASON→BRIEF semantic handoff. On the live
# cadence the rail creates these via ContextFile; by hand you pass real files.
amplihack recipe run observe-merge-queue \
  -c roster_path="$PWD/prompt_assets/simard/ecosystem_repos.toml" \
  -c inflight_refs_path="/tmp/inflight.json" \
  -c merge_queue_brief_path="/tmp/merge_queue_brief.json"

cat /tmp/merge_queue_brief.json   # the bounded JSON brief the rail parses
```

### Confirm reasoning is default-ON with the merge env vars unset

```bash
# With SIMARD_AUTOMERGE_REPOS and SIMARD_AUTOMERGE_AUTHOR unset, the OLD sensor
# produced ZERO reasoning. Now the observe/orient stage reasons over the roster.
unset SIMARD_AUTOMERGE_REPOS SIMARD_AUTOMERGE_AUTHOR SIMARD_MERGE_REASONING_SCOPE
journalctl --user -u simard-ooda -f | grep 'overseer::merge_queue'
# → INFO reasoned_prs=<n> triaged_issues=<m> scope=roster  (non-empty)
```

### Explicitly disable reasoning (LOUD)

```bash
systemctl --user set-environment SIMARD_MERGE_REASONING_SCOPE=off
systemctl --user restart simard-ooda
journalctl --user -u simard-ooda | grep 'merge reasoning DISABLED'
# → WARN merge reasoning DISABLED (SIMARD_MERGE_REASONING_SCOPE=off)
#   + one-time NotifyOperator note on email + Signal
```

## Tutorial — how one open PR becomes an autonomous merge

1. The Overseer cadence fires; the rail runs `observe-merge-queue` over the
   governed roster (idle/liveness supervised, no wall-clock timeout).
2. The REASON agent lists the open PRs in `rysweet/Simard`, checks CI + mergeable
   + review + conflicts + staleness + duplication, and concludes PR #4123 is
   `ready-for-merge`. It writes the bounded brief.
3. The rail parses the brief fail-closed into `reasoned_prs`.
4. Decide **re-narrows**: #4123 re-passes the author guard, carries the
   `simard-autonomous` label, is `MERGEABLE`, base is allowlisted, all checks
   green → it is projected into `ready_prs`.
5. The existing chain runs: `PrReadyToMerge → DeliveryReady →
   VerifyAndMergePr → merge_authority` (objective gates) → `MergeJudge`
   (fail-closed, six evidence sections) → `gh pr merge --squash --delete-branch`
   (**no** `--admin`, **no** `--no-verify`).
6. `NotifyOperator` sends a concise problem + PR summary to `rysweet` on **email
   and Signal**.
7. A stalled PR #4088 the same pass concluded `stale` gets a `FlagStalePr`
   triage comment; a `duplicate` PR #4090 gets a `CloseDuplicatePr` close
   referencing its original — both through positional argv, both notify-gated,
   neither using `--admin`/`--no-verify`.

## Testing

- **Rail / parse (`merge_queue_observe.rs`).** Fake `MergeQueueReasoner` returns
  a canned brief → asserts `reasoned_prs`/`triaged_issues` populate; malformed /
  off-roster / missing-field briefs → assert **empty** (fail-closed); whole-brief
  parse error → empty + WARN.
- **Scope resolver (`config.rs`).** unset ⇒ `Roster` (default-ON); explicit list
  ⇒ `Explicit`; `off`/`disabled`/falsey ⇒ `Disabled` (loud).
- **Projection (`mod.rs`).** `reasoned_prs → ready_prs` narrowing: a
  `ready-for-merge` PR that fails the author guard / engineer-PR gate / objective
  gate is **excluded**; an operator review PR is never projected. The **legacy
  "empty allowlist ⇒ zero reasoning" tests are deliberately retargeted** to the
  new invariant: empty env ⇒ reasoning over governed repos, merge action still
  gated.
- **Signals (`signal.rs`).** `StalePrDetected` / `DuplicatePrDetected` /
  `IssueNeedsWorkstream` detection from reasoned fields.
- **Interventions (`intervention.rs`).** `FlagStalePr` / `CloseDuplicatePr` argv
  **never** contains `--admin` or `--no-verify`; both are `RiskClass::MergeAuthority`
  opt-in; author-guard negative tests.
- **Wiring (`wiring.rs`).** Production `SpawnMergeQueueRecipeRunner` registration;
  `#[serde(default)]` forward-compat legacy-deserialize test for any reasoned
  field fed to the activity feed.

## Security

- **AUTHZ (critical): propose, never authorize.** The brief only proposes;
  `merge_authority` + `MergeJudge` + the author guard + the re-narrowing
  projection remain the sole merge authorization. Broadening reasoning scope
  never widens the action gate.
- **No `--admin` / `--no-verify` anywhere.** Asserted by unit test *and* the
  repo-wide grep guard. Branch protections are never bypassed.
- **XPIA (critical): brief is DATA, not COMMANDS.** Rust re-derives all actions
  from objective state; PR/issue refs are validated against the governed roster;
  the parse is fail-closed. The reasoning prompt is read-only.
- **Injection-proof argv.** New interventions use positional argv only via
  `sanitize_context_var`; no shell.
- **Roster is the reasoning trust boundary.** Default-ON but roster-bounded; off
  is loud, never quiet.

## See also

- [Concept: agentic merge-queue reasoning](../concepts/agentic-merge-queue-reasoning.md)
- [Reference: agentic merge-queue reasoning API](../reference/agentic-merge-queue-reasoning-api.md)
- [How to configure agentic merge-queue reasoning](../howto/configure-agentic-merge-queue-reasoning.md)
- [Design: Ecosystem Observe](./ecosystem-observe.md) — the sibling agentic-reasoning chain this mirrors
- [Concept: autonomous self-merge sensor](../concepts/autonomous-self-merge-sensor.md) — the retired imperative sensor
- [Reference: cross-repo merge authority](../reference/cross-repo-merge-authority.md) — the unchanged action gate
- [How to triage stale pull requests](../howto/triage-stale-pull-requests.md)
- [Concept: Overseer escalates the deploy-gate-converging PR](../concepts/deploy-gate-converging-pr-escalation.md) — the DeployDrift-aware ranking that sits behind this re-narrowing
