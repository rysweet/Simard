# Reference: Progress-evidence API

Crate: `simard` · Module: `simard::goal_curation::progress_evidence`

This module implements the gatekeeper described in
[Progress-evidence gating](../concepts/progress-evidence-gating.md). It
exposes one trait (`ProgressEvidenceChecker`), two production helper traits
for shell-out seams (`GitRunner`, `GhRunner`), two concrete implementations
(`DefaultProgressEvidenceChecker`, `NoopProgressEvidenceChecker`), and a
single façade function (`update_goal_progress_with_evidence`) in the
sibling `simard::goal_curation::operations` module.

All public symbols below are re-exported from `simard::goal_curation`.

---

## `EvidenceDecision`

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum EvidenceDecision {
    /// Evidence found — the caller may apply the progress update.
    Accept { reason: String },
    /// No evidence — the caller must keep the prior percent and emit
    /// a hallucination audit episode.
    Reject { reason: String },
}
```

The `reason` string in both variants is human-readable, ASCII-safe, and
suitable for inclusion in cognitive-memory episodes verbatim.

---

## `ProgressEvidenceChecker`

```rust
pub trait ProgressEvidenceChecker: Send + Sync {
    fn check(
        &self,
        goal: &ActiveGoal,
        old_percent: u32,
        new_percent: u32,
        since: DateTime<Utc>,
    ) -> EvidenceDecision;
}
```

The trait is `Send + Sync` so a single `Arc<dyn ProgressEvidenceChecker>`
can be installed on `OodaBridges` and shared across all OODA actions.

### Contract

- `check` MUST NOT mutate the goal board, cognitive memory, or any other
  daemon state. It is a read-only function over the local repo and remote
  GitHub.
- `check` MAY perform blocking I/O (git/gh shellouts). It is called at most
  a few times per OODA cycle, only on progress-increase attempts.
- `check` MUST return `Accept` when evidence is found and `Reject`
  otherwise. It MUST NOT return `Accept` on shellout failure — tooling
  absence is treated as "no evidence" (see
  [`SIMARD_PROGRESS_EVIDENCE`](../operations/progress-evidence-kill-switch.md)
  for the operator escape hatch).
- The `since` argument is provided by the caller; the trait does not
  source it.

### Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `goal` | `&ActiveGoal` | The goal whose progress is being claimed. Used for `goal.id` (engineer-branch slug) and `goal.wip_refs` (issue/PR cross-reference). |
| `old_percent` | `u32` | The current percent (0–100). |
| `new_percent` | `u32` | The proposed new percent (0–100). Only called when `new_percent > old_percent`. |
| `since` | `DateTime<Utc>` | The cutoff timestamp; only artifacts at or after this instant count as evidence. |

### Decision rules (DefaultProgressEvidenceChecker)

The production implementation accepts on any of:

| # | Source | Match | `reason` template |
|---|---|---|---|
| 1 | `git log` on `engineer/{slug(goal.id)}-*` | ≥1 commit with author-date `>= since` | `"commit <sha7> on <branch> at <iso8601>"` |
| 2 | `gh pr list` on `remote_slug` | ≥1 PR (any state) with title or body containing the goal slug **or** any `wip_refs[].ref_id` of kind `issue` or `pr` | `"PR #<n> references goal"` |
| 3 | `gh pr list` on `remote_slug` | ≥1 PR with `state == "MERGED"`, `mergedAt >= since`, body matching `(?i)\b(close[sd]?|fix(?:es|ed)?|resolve[sd]?)\s+#(\d+)\b` where `\2` is in `wip_refs` issues | `"PR #<n> closed #<issue> at <iso8601>"` |
| 4 | — | none of the above | `Reject` with concatenated `"no commits on engineer/<slug>-*, no PRs referencing goal, no merged PRs closing #<issue-list> since <iso8601>"` |

Rules are evaluated top-to-bottom; the first match short-circuits and
returns `Accept`.

---

## `GitRunner`

```rust
pub trait GitRunner: Send + Sync {
    fn list_branches(
        &self,
        repo_root: &Path,
        pattern: &str,
    ) -> io::Result<Vec<String>>;

    fn commits_since(
        &self,
        repo_root: &Path,
        branch: &str,
        since: DateTime<Utc>,
    ) -> io::Result<Vec<String>>;
}
```

Test seam for the local-git half of `DefaultProgressEvidenceChecker`. The
production impl (`SystemGitRunner`) wraps:

- `git -C <repo_root> for-each-ref --format=%(refname:short) refs/heads/<pattern>`
- `git -C <repo_root> log --since <iso8601> --pretty=%H <branch>`

`io::Error` from either call is propagated as a `Reject` from `check`.

---

## `GhRunner`

```rust
pub trait GhRunner: Send + Sync {
    fn search_prs(
        &self,
        repo_slug: &str,
        query: &str,
    ) -> io::Result<Vec<GhPr>>;
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct GhPr {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "mergedAt")]
    pub merged_at: Option<DateTime<Utc>>,
}
```

Test seam for the GitHub half of `DefaultProgressEvidenceChecker`. The
production impl (`SystemGhRunner`) shells out to:

```
gh pr list --repo <repo_slug> --search <query> --state all \
   --json number,title,body,state,createdAt,mergedAt
```

Authentication is the daemon process's existing `gh` token (same
credentials used by other Simard subsystems that read GitHub).

---

## `DefaultProgressEvidenceChecker`

```rust
pub struct DefaultProgressEvidenceChecker {
    pub repo_root: PathBuf,
    pub remote_slug: String,
    pub git: Arc<dyn GitRunner>,
    pub gh:  Arc<dyn GhRunner>,
}
```

The production checker. Constructed at daemon startup with:

| Field | Production value |
|---|---|
| `repo_root` | `std::env::current_dir()` at boot |
| `remote_slug` | `"rysweet/Simard"` |
| `git` | `Arc::new(SystemGitRunner)` |
| `gh` | `Arc::new(SystemGhRunner)` |

### Custom constructor for non-default deployments

```rust
impl DefaultProgressEvidenceChecker {
    pub fn new(repo_root: PathBuf, remote_slug: impl Into<String>) -> Self;
}
```

Use this when the daemon runs from a directory other than the repo root or
when targeting a fork.

---

## `NoopProgressEvidenceChecker`

```rust
pub struct NoopProgressEvidenceChecker;

impl ProgressEvidenceChecker for NoopProgressEvidenceChecker {
    fn check(&self, _: &ActiveGoal, _: u32, _: u32, _: DateTime<Utc>)
        -> EvidenceDecision
    { /* always Accept */ }
}
```

Always returns `Accept { reason: "noop checker (no evidence enforced)" }`.
Used in two places:

1. **Tests.** Default test-helper `OodaBridges::for_tests()` installs this
   so existing tests do not need to mock `git`/`gh`.
2. **Operator escape hatch.** Selected at daemon boot when
   `SIMARD_PROGRESS_EVIDENCE=off`. See
   [the kill-switch operations doc](../operations/progress-evidence-kill-switch.md).

---

## `update_goal_progress_with_evidence` (façade)

Located in `src/goal_curation/operations.rs`.

```rust
pub fn update_goal_progress_with_evidence(
    board:   &mut GoalBoard,
    goal_id: &str,
    proposed: GoalProgress,
    checker: &dyn ProgressEvidenceChecker,
    memory:  &dyn crate::cognitive_memory::CognitiveMemoryOps,
    now:     DateTime<Utc>,
) -> SimardResult<EvidenceDecision>;
```

### Behavior

1. Look up the goal on `board`. Map current and proposed status to
   `(old_percent, new_percent)`:

    | `GoalProgress` variant | Percent |
    |---|---|
    | `NotStarted` | `0` |
    | `InProgress { percent }` | `percent` |
    | `Blocked(_)` | the goal's *current* percent (no change) |
    | `Completed` | `100` |

2. Determine `since` via the [three-step fallback chain](../concepts/progress-evidence-gating.md#sourcing-since--the-last-update-timestamp).

3. **Bypass set.** If any of the following hold, call the underlying
   `update_goal_progress` directly and return
   `Accept { reason: "bypass: non-increase" }` (or `"bypass: <variant>"`)
   **without** emitting a memory episode:

   - `proposed` is `Blocked(_)`
   - `proposed` is `NotStarted`
   - `new_percent <= old_percent`

4. **Otherwise** call `checker.check(...)`:

    - On `Accept`:
      - Call `update_goal_progress(board, goal_id, proposed)`.
      - Set `goal.last_progress_update_at = Some(now)`.
      - Emit one episode:
        ```
        goal progress accepted: <old>%→<new>% on <goal-id>
          — evidence: <checker reason>
        ```
        importance `0.4`.
      - Return `Ok(Accept { reason })`.
    - On `Reject`:
      - Do **not** mutate the board.
      - Emit one episode:
        ```
        brain hallucination detected: rejected progress <old>%→<new>% on <goal-id>
          — no git evidence since last update: <checker reason>
        ```
        importance `0.7`.
      - Return `Ok(Reject { reason })`. **This is not an error.** The
        caller treats it as informational and proceeds without a percent
        bump.

`SimardResult::Err` is returned only for genuine failures: the goal id is
not on the board, the underlying `update_goal_progress` writer fails, or
the memory store fails to record an audit episode.

### Calling convention

The façade is invoked from four production sites. A fifth historical
caller — `subordinate.rs:262`, which writes `Blocked(reason)` — stays a
direct caller of `update_goal_progress` because `Blocked` is in the
bypass set (it does not increase the percent).

| Caller | Bypass path expected | Notes |
|---|---|---|
| `ooda_actions::goal_session::advance::assess_only_outcome` | Sometimes | Bumps come from brain text — exactly the case the gate targets. |
| `ooda_actions::goal_session::advance` pre-spawn site | Sometimes | Same as above. |
| `ooda_actions::advance_goal::subordinate` heartbeat (50%) | Sometimes | Engineer alive ≠ evidence. |
| `ooda_actions::advance_goal::subordinate` Completed | Always Accept (rule 1) | Routed for audit, never rejected in practice. |

### Error mapping for the OODA layer

Both `Accept` and `Reject` are returned as `Ok(...)`. Callers in
`ooda_actions` distinguish them like this:

```rust
match update_goal_progress_with_evidence(
    board, goal_id, new_progress,
    &*bridges.progress_evidence, &*bridges.memory, Utc::now(),
)? {
    EvidenceDecision::Accept { .. } => { /* happy path */ }
    EvidenceDecision::Reject { reason } => {
        return make_outcome(
            action,
            true,
            format!("no-action: progress claim rejected (no evidence): {reason}"),
        );
    }
}
```

`Reject` is **not** treated as a cycle failure: the OODA loop continues,
the rejection is observable via cognitive memory, and the percent stays
where it was.

---

## `OodaBridges` extension

`src/ooda_loop/types.rs` adds two fields:

```rust
pub struct OodaBridges {
    // ... existing fields ...
    pub repo_root: std::path::PathBuf,
    pub progress_evidence: std::sync::Arc<
        dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker
    >,
}
```

| Field | Default at daemon boot | Default in tests |
|---|---|---|
| `repo_root` | `std::env::current_dir().unwrap_or_else(\|_\| PathBuf::from("."))` | `PathBuf::from(".")` |
| `progress_evidence` | `Arc::new(DefaultProgressEvidenceChecker::new(repo_root.clone(), "rysweet/Simard"))`, or `Arc::new(NoopProgressEvidenceChecker)` when `SIMARD_PROGRESS_EVIDENCE=off` | `Arc::new(NoopProgressEvidenceChecker)` |

A new `OodaBridges::for_tests()` constructor wires the test defaults so
that existing OODA-loop tests need only a single-line change to adopt the
new fields.

---

## `ActiveGoal` schema extension

`src/goal_curation/types.rs`:

```rust
pub struct ActiveGoal {
    // ... existing fields ...

    /// Wall-clock timestamp of the last accepted progress update.
    /// `None` for goals created before #1967; the gate falls back
    /// to a memory scan, then to daemon process-start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_progress_update_at: Option<chrono::DateTime<chrono::Utc>>,
}
```

The `#[serde(default, skip_serializing_if = "Option::is_none")]` attribute
combination preserves both forward and backward compatibility:

- Older JSON files load with `last_progress_update_at = None`.
- Goals that have never reached the gate (e.g. pure `Blocked` history)
  continue to serialize without the field, keeping snapshots minimal.

No data migration is required.

---

## Stability

| Item | Stability |
|---|---|
| `EvidenceDecision`, `ProgressEvidenceChecker` | Public stable — semver-tracked. |
| `GitRunner`, `GhRunner`, `GhPr` | Public stable — needed for test seams in external crates. |
| `update_goal_progress_with_evidence` | Public stable. |
| `DefaultProgressEvidenceChecker` internals (private helpers) | Implementation detail; may change. |
| `NoopProgressEvidenceChecker` | Public stable; safe to use in any test. |
| Episode prefix strings (`"goal progress accepted:"`, `"brain hallucination detected:"`) | **Behaviorally stable.** The dashboard and consolidation jobs match these prefixes verbatim; changing them is a breaking change. |

---

## See also

- [Progress-evidence gating (concept)](../concepts/progress-evidence-gating.md)
- [Diagnose rejected progress claims (how-to)](../howto/diagnose-rejected-progress-claims.md)
- [`SIMARD_PROGRESS_EVIDENCE` kill switch (operations)](../operations/progress-evidence-kill-switch.md)
- [Goal board API](goal-board-api.md)
- [Goal board corruption guard API](goal-board-corruption-guard-api.md)
- [Cognitive memory bridge helpers](cognitive-memory-bridge-helpers.md)
