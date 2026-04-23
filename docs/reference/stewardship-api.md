# Goal Stewardship — Orchestrator Failure API Reference

Module: `simard::stewardship`
Source: `src/stewardship/`

This page documents the public surface of the orchestrator-failure sub-mode
of Goal Stewardship. For the conceptual overview, see
[Goal Stewardship Mode — Orchestrator Failure Loop](../concepts/stewardship-mode.md).
For the broader Goal Stewardship Mode, see `Specs/ProductArchitecture.md`
§ *Stewardship Mode* and § *Goal Stewardship Mode*.

## Module Layout

```
src/stewardship/
├── mod.rs           public entrypoint and re-exports (the only public surface)
├── types.rs         OrchestratorRunSummary, StewardshipOutcome, TargetRepo
├── routing.rs       route_failure
├── dedup.rs         normalize, failure_signature, find_existing
├── gh_client.rs     trait GhClient, GhIssue, RealGhClient, FakeGhClient (cfg(test))
└── tests.rs         unit and end-to-end tests
```

`mod.rs` re-exports the public API:

```rust
pub use gh_client::{GhClient, GhIssue, RealGhClient};
pub use routing::route_failure;
pub use types::{OrchestratorRunSummary, StewardshipOutcome, TargetRepo};

// Test-only helpers re-exported for downstream test consumers.
#[cfg(any(test, feature = "test-utils"))]
pub use gh_client::FakeGhClient;
```

`stewardship` depends on `goal_curation` (one direction only). It does **not**
depend on `engineer_loop`, `base_type_*`, or `self_improve`.

## Entrypoint

```rust
pub fn process_orchestrator_run(
    run:   &OrchestratorRunSummary,
    gh:    &dyn GhClient,
    board: &mut GoalBoard,
) -> SimardResult<StewardshipOutcome>;
```

Validate `run`, route it to a target repo, search for an existing issue with
the same signature, file a new issue if none is found, and enqueue the
resulting issue handle into the curation `board`.

### Errors

| Variant                          | When                                                       |
|----------------------------------|------------------------------------------------------------|
| `StewardshipInvalidRunSummary`   | A required field on `OrchestratorRunSummary` is empty.     |
| `StewardshipRoutingAmbiguous`    | `source_module` matches no routing keyword set.            |
| `StewardshipGhCommandFailed`     | `gh` is missing, exited non-zero, or returned malformed JSON. |

No success branch is taken on any of these errors; `board` is left untouched.

## Types

### `OrchestratorRunSummary`

```rust
pub struct OrchestratorRunSummary {
    pub run_id:        String,
    pub recipe_name:   String,
    pub failed_step:   String,
    pub source_module: String,
    pub failure_kind:  String,
    pub error_text:    String,
}
```

Input contract. All fields are required and non-empty; an empty value yields
`StewardshipInvalidRunSummary { field: <name> }`.

`source_module` is the routing key — see [Routing Matrix](../concepts/stewardship-mode.md#routing-matrix).

### `TargetRepo`

```rust
pub enum TargetRepo { Amplihack, Simard }

impl TargetRepo {
    pub fn slug(&self) -> &'static str;  // "rysweet/amplihack" | "rysweet/Simard"
}
```

### `StewardshipOutcome`

```rust
pub enum StewardshipOutcome {
    FiledNew        { repo: String, issue_number: u64, url: String, signature: String },
    MatchedExisting { repo: String, issue_number: u64, url: String, signature: String },
}
```

`FiledNew` is returned when no existing open issue carried the signature;
exactly one `gh issue create` was performed. `MatchedExisting` is returned
when an open issue with the signature was found; no creation occurred.

In both cases, `enqueue_stewardship_issue` was called with the issue handle.

## Routing

```rust
pub fn route_failure(source_module: &str) -> SimardResult<TargetRepo>;
```

Pure, total over the routing matrix; performs zero I/O. See the
[routing matrix](../concepts/stewardship-mode.md#routing-matrix) for the
keyword sets.

## Deduplication

```rust
pub fn normalize(message: &str) -> String;
pub fn failure_signature(failure_kind: &str, error_text: &str) -> String;
pub fn find_existing<'a>(issues: &'a [GhIssue], signature: &str) -> Option<&'a GhIssue>;
```

- `normalize` strips ANSI escapes, timestamps, paths, hex hashes, run ids,
  and stack-frame line/column numbers, then collapses whitespace.
- `failure_signature` returns the first 16 hex characters of
  `sha256(failure_kind + "\n" + normalize(error_text))`.
- `find_existing` looks for the literal substring
  `stewardship-signature: <signature>` in each issue body.

All three are pure, deterministic, and `cfg(test)`-friendly.

## `GhClient` Trait

```rust
pub struct GhIssue {
    pub number: u64,
    pub url:    String,
    pub title:  String,
    pub body:   String,
}

pub trait GhClient {
    fn search_issues(&self, repo: &str, signature: &str) -> SimardResult<Vec<GhIssue>>;
    fn create_issue (&self, repo: &str, title: &str, body: &str) -> SimardResult<GhIssue>;
}
```

The only subprocess surface in the stewardship module. Two implementations
are shipped:

### `RealGhClient`

`std::process::Command`-based.

  - **search**: `gh issue list -R <repo> --state open --search "stewardship-signature:<hex> in:body" --json number,url,title,body`
  - **create**: `gh issue create -R <repo> --title <…> --body-file -`, with the
    body piped on stdin so argv-length and shell-quoting are not concerns.

Search is intentionally scoped to `--state open`. Closed stewardship issues
are not matched: if a failure recurs after manual close, a fresh issue is
filed. See *Goal Stewardship Mode — Orchestrator Failure Loop* § Out of Scope.

Any non-zero exit, missing binary, or JSON parse failure becomes
`SimardError::StewardshipGhCommandFailed { reason }` with `reason` containing
trimmed stderr or the parse diagnostic.

### `FakeGhClient` (test-only)

Defined in `gh_client.rs` and re-exported from `stewardship::mod` under
`#[cfg(any(test, feature = "test-utils"))]`. Records every call so tests
assert both outcomes and call counts.

```rust
#[cfg(any(test, feature = "test-utils"))]
pub struct FakeGhClient {
    pub search_calls: RefCell<Vec<(String, String)>>,         // (repo, signature)
    pub create_calls: RefCell<Vec<(String, String, String)>>, // (repo, title, body)
    // ...preconfigured search results / create responses...
}
```

Test consumers import it from the public surface:

```rust
#[cfg(test)]
use simard::stewardship::FakeGhClient;
```

## `goal_curation` Helper

```rust
// src/goal_curation/operations.rs
pub const DEFAULT_STEWARD_SCORE: f64 = 0.6;

pub fn enqueue_stewardship_issue(
    board: &mut GoalBoard,
    repo: &str,
    issue_number: u64,
    url: &str,
    signature: &str,
) -> SimardResult<()>;
```

Constructs a `BacklogItem`:

| Field         | Value                                                                  |
|---------------|------------------------------------------------------------------------|
| `id`          | `stewardship-<repo_with_/_replaced_by_underscore>-<issue_number>`      |
| `description` | `"Investigate stewardship-filed failure <url> (sig <signature>)"`      |
| `source`      | `"stewardship:<repo>#<issue_number>"`                                  |
| `score`       | `DEFAULT_STEWARD_SCORE` (`0.6`)                                        |
| `url`         | `Some(url.into())`                                                     |

…and calls the existing `add_backlog_item`, which deduplicates by `id`.
Repeated `MatchedExisting` outcomes therefore do not grow the backlog.

## Error Variants

```rust
// src/error/mod.rs
pub enum SimardError {
    // ...existing variants...
    StewardshipRoutingAmbiguous { source: String },
    StewardshipGhCommandFailed  { reason: String },
    StewardshipInvalidRunSummary{ field: &'static str },
}
```

Each variant has a `Display` arm and an associated unit test alongside
existing error-variant tests.
