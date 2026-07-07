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
`goal_curation`, `agent_loop`, `session_builder`, and `simard::`. A source that
matches **no** keyword (for example a bare `"overseer"`) is not rejected — it
falls back to the **default repo** (`rysweet/Simard`) and the fallback is
logged via `tracing::warn!`. Prefer an explicit `simard::` / `amplihack::`
prefix when you know the right home; rely on the default only when you
deliberately want unclassified work tracked in `rysweet/Simard`.

> **Keep secrets out of the payload.** `error_text` is rendered **verbatim into
> a public GitHub issue body**, and an unmatched `source_module` is echoed at
> `WARN` log level by the routing fallback. Never place tokens, credentials, or
> PII in either field — redact them in the producer before constructing the
> `OrchestratorRunSummary`. The default-repo fallback broadens where issues can
> be written, so this hygiene is what keeps that reach safe.

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

The loop surfaces `gh` and input failures as first-class errors — propagate
every one up to your orchestrator's failure path; do not catch and swallow.
Routing itself no longer errors: an unmatched `source_module` falls back to the
default repo (`rysweet/Simard`) and is `tracing::warn!`-logged, so you never
need to handle a routing error. The snippet below is shown inside a function
returning `SimardResult<()>` so the `return Err(other)` arm type-checks:

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
            SimardError::StewardshipGhCommandFailed { reason } => {
                // gh is broken / unauthenticated / rate-limited; surface as a
                // first-class operational failure.
                tracing::error!(%reason, "stewardship gh command failed");
            }
            SimardError::StewardshipInvalidRunSummary { field } => {
                // Bug in the caller — fix the producer of OrchestratorRunSummary.
                tracing::error!(field, "stewardship invalid run summary");
            }
            // Routing never returns StewardshipRoutingAmbiguous anymore — an
            // unmatched source falls back to the default repo (see step 1). The
            // variant is retained only for API stability.
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

- **Unmatched `source_module`.** A source that contains no known keyword is
  **not** an error — it routes to the default repo (`rysweet/Simard`) and emits
  a `tracing::warn!` naming the source and the chosen default. If you meant it
  to land elsewhere, change the producer to emit a routable string (e.g. prefix
  it with `simard::` or `amplihack::`), or add the keyword to the routing matrix
  (with a test). Watch for the warn line if issues show up in an unexpected repo.
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
