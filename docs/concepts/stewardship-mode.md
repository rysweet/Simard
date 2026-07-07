# Goal Stewardship Mode — Orchestrator Failure Loop

Simard is the engineering steward over the amplihack ecosystem (see
`Specs/ProductArchitecture.md` § *Stewardship Mode* and § *Goal Stewardship Mode*). This
document describes a **new sub-mode of Goal Stewardship**: the autonomous
loop that turns Simard's own orchestrator failures into tracked, deduplicated
GitHub issues and feeds those issues back into Simard's curation backlog.

The existing Goal Stewardship Mode (PRD § *Goal Stewardship Mode*) is about maintaining a
durable backlog and explicit top-5 goals across sessions. The orchestrator
failure loop documented here extends that mode with an automated
*failure-to-issue* path: when the orchestrator fails, stewardship turns the
failure into a tracked upstream issue and a curated backlog item, so the
durable backlog reflects observed system breakage and not just human-entered
goals.

## Purpose

When a Simard orchestrator run fails, the failure should not be lost in a log
file. The orchestrator-failure sub-mode of Goal Stewardship:

1. **Detects** the failure from an `OrchestratorRunSummary` produced by the
   orchestrator caller.
2. **Routes** the failure to the correct upstream repository — `rysweet/amplihack`
   for outer-loop / recipe-runner / orchestrator infrastructure failures, and
   `rysweet/Simard` for inner-loop module failures (`engineer_loop`,
   `base_type`, `self_improve`, `goal_curation`, `agent_loop`,
   `session_builder`, `simard::*`). A `source_module` that matches **no**
   keyword — for example the Overseer's own `"overseer"` workstream-gap briefs —
   routes to the configured **default repo** (`rysweet/Simard`, the
   `DEFAULT_TARGET_REPO` constant) and the fallback is recorded with
   `tracing::warn!`. Routing is therefore *total*: it never errors and never
   drops a gap on the floor.
3. **Deduplicates** the failure against existing open issues using a stable
   SHA-256 signature embedded in each issue body.
4. **Files** a new issue (or matches an existing one) via the `gh` CLI.
5. **Enqueues** the resulting issue into Simard's own backlog through
   `src/goal_curation`, so the next curation cycle can pick it up.

The loop **never silently degrades**. Any `gh` failure or invalid input
surfaces as a `SimardError`, and the loop never files duplicate issues. Routing
is the one deliberately *total* step: an unmatched `source_module` is **not** an
error — it falls back to the default repo (`rysweet/Simard`) and is logged via
`tracing::warn!`, so the originating gap or failure is always tracked somewhere
rather than lost. This is a *logged, non-error* recovery, not a silent one.

## Loop at a Glance

```
OrchestratorRunSummary
        │
        ▼
  validate(run)                          ── invalid → StewardshipInvalidRunSummary
        │
        ▼
  route_failure(source_module)           ── unknown → default repo (rysweet/Simard), warn-logged
        │
        ▼
  failure_signature(kind, error_text)
        │
        ▼
  gh.search_issues(repo, signature)      ── non-zero → StewardshipGhCommandFailed
        │
        ├── match found → enqueue_stewardship_issue → MatchedExisting
        │
        └── no match  → gh.create_issue → enqueue_stewardship_issue → FiledNew
```

## Invariants

- **Routing totality.** `route_failure` is total — every input yields a
  `TargetRepo`. A matched keyword pins the repo; an unmatched `source_module`
  falls back to the default repo (`rysweet/Simard`) and is `tracing::warn!`-logged.
  Routing can therefore never short-circuit the loop.
- **No filing without valid input.** Validation failure (an empty required
  field) short-circuits before any I/O. Routing never blocks filing, but a
  malformed `OrchestratorRunSummary` still does.
- **No filing without search.** `create_issue` runs only after a successful
  search returns no signature match.
- **At most one create per call.** Each call yields exactly one of
  `FiledNew` (one create) or `MatchedExisting` (zero creates).
- **End-to-end idempotency.** Re-running with the same `OrchestratorRunSummary`
  after a `FiledNew` yields `MatchedExisting` with the same issue number and
  adds no new backlog row.
- **Closed issues do not match.** Signature search is scoped to open issues
  only; recurrence after manual close files a fresh issue.
- **Fail-loud.** Missing `gh`, non-zero `gh` exit, malformed JSON, or empty
  required fields all produce errors. There is no silent recovery path. The
  routing default-fallback is the one exception, and it is **not silent** — it
  is a logged (`tracing::warn!`), first-class routing outcome, not an error and
  not a swallow.

## Routing Matrix

`route_failure(source_module: &str) -> SimardResult<TargetRepo>` matches the
lowercased source string against ordered keyword sets:

| Order | Keywords (substring match)                                                                       | Target repo        |
|-------|--------------------------------------------------------------------------------------------------|--------------------|
| 1     | `amplihack`, `recipe-runner`, `orchestrator`, `recipe::`                                         | `rysweet/amplihack` |
| 2     | `engineer_loop`, `base_type`, `self_improve`, `goal_curation`, `agent_loop`, `session_builder`, `simard::` | `rysweet/Simard`    |
| —     | none (default fallback)                                                                          | `rysweet/Simard` (`DEFAULT_TARGET_REPO`), logged via `tracing::warn!` |

Amplihack keywords are checked first. If a source string contains both
families (e.g. `amplihack::engineer_loop`), the outer-system tag wins by
design; this precedence is pinned by tests.

When no keyword matches, `route_failure` **does not error**. It returns the
`DEFAULT_TARGET_REPO` constant (`TargetRepo::Simard` → `rysweet/Simard`) and
emits a single `tracing::warn!` carrying the unmatched `source_module` and the
chosen default slug. `DEFAULT_TARGET_REPO` is a compile-time constant — the one
named source of truth for the default — so the fallback can never be an
attacker-chosen or dynamically-supplied repo. This is what lets the Overseer's
own `source_module = "overseer"` gap-scan briefs file/upsert tracking issues in
`rysweet/Simard` instead of failing every tick.

## Failure Signature

Stewardship deduplicates by a stable, prefix of a SHA-256 hash:

```
signature = sha256(failure_kind + "\n" + normalize(error_text))[..16]
```

`normalize` strips noise that varies between otherwise-identical failures so
that two runs of the same bug collapse to the same signature:

1. ANSI escape sequences (`\x1B\[[0-9;]*[A-Za-z]`).
2. ISO-8601 timestamps → `<TS>`.
3. Absolute paths → `<PATH>`.
4. Hex hashes of length ≥ 7 → `<HEX>`.
5. Run identifiers matching `run-[A-Za-z0-9_-]+` → `<RUN>`.
6. `:line:col` in stack frames → `:<L>:<C>`.
7. Whitespace collapse + trim.

The hex signature is embedded verbatim in every filed issue body as
`stewardship-signature: <hex>`. The next invocation finds it via
`gh issue list --search "stewardship-signature:<hex> in:body"`.

## Issue Body Template

Every issue Simard files looks like this (outer fence shown with `~~~` so the
inner ```` ``` ```` fence around `<error_text>` renders correctly):

~~~
filed-by: simard-stewardship
stewardship-signature: <hex>
originating-run: <run_id>
recipe: <recipe_name>
failed-step: <failed_step>
source-module: <source_module>
failure-kind: <failure_kind>

## Error
```
<error_text>
```
~~~

The leading metadata block is intentionally machine-readable so future
tooling (or a human triager) can re-derive the routing and signature without
re-running Simard.

## Backlog Handoff

After filing or matching, Stewardship calls
`goal_curation::operations::enqueue_stewardship_issue`, which constructs a
`BacklogItem` with a deterministic id of the form
`stewardship-<repo_slug_with_underscores>-<issue_number>` and a default
steward score of `0.6`. Because `add_backlog_item` already deduplicates on
`id`, repeated `MatchedExisting` outcomes never grow the backlog.

## Out of Scope

The following are explicit non-goals for the orchestrator-failure
sub-mode:

- Auto-closing stale stewardship issues.
- Cross-repo deduplication (signatures are scoped per repo).
- Triage labels or assignees beyond the `[stewardship]` title prefix.
- Rate-limiting `gh` calls. Callers invoke the loop at most once per
  orchestrator run.
- Reading orchestrator logs directly. The caller is responsible for
  constructing `OrchestratorRunSummary`.
- Scheduling a follow-up `smart-orchestrator` run against upstream — this was
  excluded from the issue #1167 acceptance criteria during clarification.
- **Matching against closed issues.** Search uses `--state open` only. If a
  stewardship issue is manually closed and the same failure recurs, the loop
  files a new issue. Signatures are not consulted on closed issues.

## See Also

- API reference: [`docs/reference/stewardship-api.md`](../reference/stewardship-api.md)
- How-to: [`docs/howto/file-stewardship-issues-from-orchestrator-runs.md`](../howto/file-stewardship-issues-from-orchestrator-runs.md)
- PRD: `Specs/ProductArchitecture.md` § *Stewardship Mode* and § *Goal Stewardship Mode*
