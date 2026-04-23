# How to file stewardship issues from orchestrator runs

This guide shows how to wire an orchestrator caller into the
orchestrator-failure sub-mode of Goal Stewardship Mode so that every failed
run is routed to the right upstream repo, deduplicated, and added to
Simard's backlog.

For the loop's design and invariants, see
[Goal Stewardship Mode — Orchestrator Failure Loop](../concepts/stewardship-mode.md).
For the public types, see the
[Goal Stewardship — Orchestrator Failure API reference](../reference/stewardship-api.md).
For the broader Goal Stewardship Mode this extends, see
`Specs/ProductArchitecture.md` § *Stewardship Mode* and § *Goal Stewardship Mode*.

## Prerequisites

- The `gh` CLI is installed, on `PATH`, and authenticated against both
  `rysweet/amplihack` and `rysweet/Simard` (or a token with `repo` scope on
  both).
- Your orchestrator can produce a populated `OrchestratorRunSummary` for each
  failed run.
- You have a mutable handle to the active `GoalBoard`.

## 1. Construct an `OrchestratorRunSummary`

Populate every field. Empty values are rejected with
`StewardshipInvalidRunSummary`.

```rust
use simard::stewardship::OrchestratorRunSummary;

let run = OrchestratorRunSummary {
    run_id:        "run-2026-04-22-abc123".into(),
    recipe_name:   "smart-orchestrator".into(),
    failed_step:   "decompose".into(),
    source_module: "amplihack::recipe-runner".into(),
    failure_kind:  "NonZeroExit".into(),
    error_text:    stderr.trim().to_string(),
};
```

Pick `source_module` carefully — it is the routing key. The amplihack family
includes `amplihack`, `recipe-runner`, `orchestrator`, and `recipe::`; the
Simard family includes `engineer_loop`, `base_type`, `self_improve`,
`goal_curation`, `agent_loop`, `session_builder`, and `simard::`.

## 2. Choose a `GhClient` implementation

For production:

```rust
use simard::stewardship::RealGhClient;

let gh = RealGhClient::default();
```

For tests, use `FakeGhClient` (re-exported from the public surface under
`#[cfg(any(test, feature = "test-utils"))]`):

```rust
#[cfg(test)]
use simard::stewardship::FakeGhClient;
```

## 3. Run the loop

```rust
use simard::stewardship::{process_orchestrator_run, StewardshipOutcome};

match process_orchestrator_run(&run, &gh, &mut board)? {
    StewardshipOutcome::FiledNew { repo, issue_number, url, signature } => {
        tracing::info!(%repo, issue_number, %url, %signature,
            "stewardship filed new issue");
    }
    StewardshipOutcome::MatchedExisting { repo, issue_number, url, signature } => {
        tracing::info!(%repo, issue_number, %url, %signature,
            "stewardship matched existing issue");
    }
}
```

`board` is mutated in both cases — the issue handle is enqueued via
`enqueue_stewardship_issue` with a deterministic id, so re-invoking the loop
with the same `OrchestratorRunSummary` is idempotent.

## 4. Handle errors loudly

The loop has no fallbacks. Propagate every error up to your orchestrator's
failure path; do not catch and swallow. The snippet below is shown inside a
function returning `SimardResult<()>` so the `return Err(other)` arm
type-checks:

```rust
use simard::error::{SimardError, SimardResult};
use simard::stewardship::process_orchestrator_run;

fn handle_failed_run(
    run:   &OrchestratorRunSummary,
    gh:    &dyn GhClient,
    board: &mut GoalBoard,
) -> SimardResult<()> {
    if let Err(err) = process_orchestrator_run(run, gh, board) {
        match err {
            SimardError::StewardshipRoutingAmbiguous { source } => {
                // Add the missing keyword to the routing matrix and re-run;
                // do not pick a default repo.
                tracing::error!(%source, "stewardship routing ambiguous");
            }
            SimardError::StewardshipGhCommandFailed { reason } => {
                // gh is broken / unauthenticated / rate-limited; surface as a
                // first-class operational failure.
                tracing::error!(%reason, "stewardship gh command failed");
            }
            SimardError::StewardshipInvalidRunSummary { field } => {
                // Bug in the caller — fix the producer of OrchestratorRunSummary.
                tracing::error!(field, "stewardship invalid run summary");
            }
            other => return Err(other),
        }
    }
    Ok(())
}
```

## 5. Verify the outcome

After a successful `FiledNew`:

```bash
gh issue view <issue_number> -R <repo>
```

The body begins with the metadata block including
`stewardship-signature: <hex>`. A second invocation against the same failure
will find this signature and return `MatchedExisting` with the same
`issue_number`.

To inspect the backlog handoff, see
[Inspect the durable goal register](./inspect-durable-goal-register.md). The
new entry has id `stewardship-<repo_with_underscores>-<issue_number>` and
score `0.6`.

## Common Pitfalls

- **Routing ambiguous.** Means your `source_module` does not contain any
  known keyword. Fix: change the producer to emit a routable string (e.g.
  prefix it with `simard::` or `amplihack::`). Do not edit the routing
  matrix without also adding a test.
- **`gh` not authenticated.** `StewardshipGhCommandFailed` will carry the
  trimmed stderr — usually a hint to run `gh auth login`.
- **Backlog appears unchanged after a match.** Expected: the deterministic
  id means the entry already existed.
- **Body too long for `gh`.** `RealGhClient::create_issue` pipes the body on
  stdin via `--body-file -`, so argv-length and shell quoting are not
  concerns; this pitfall does not apply here.
- **A previously filed issue was closed and the failure recurred.** Expected:
  signature search uses `--state open`, so a fresh issue is filed. To prevent
  re-filing, leave the original issue open or fix the underlying cause.

## Related

- [Goal Stewardship Mode — Orchestrator Failure Loop](../concepts/stewardship-mode.md)
- [Goal Stewardship — Orchestrator Failure API reference](../reference/stewardship-api.md)
- [Inspect the durable goal register](./inspect-durable-goal-register.md)
- PRD: `Specs/ProductArchitecture.md` § *Stewardship Mode* and § *Goal Stewardship Mode*
