---
title: ready_prs sensor API reference
description: >
  The Observe-path sensor that populates ObservedState.ready_prs — the survey
  seam on PrOps, the SIMARD_AUTOMERGE_REPOS allowlist and SIMARD_AUTOMERGE_AUTHOR
  identity resolvers, the SIMARD_ENGINEER_PR_LABEL + engineer branch-prefix
  allow-list and is_engineer_branch() engineer-PR gate that keeps the operator's
  own review PRs out, the OpenPrSummary.author + OpenPrSummary.labels fields, the
  deterministic author → engineer-PR → objective-gate pre-filter pipeline,
  fail-to-empty semantics, and the run_cycle enrichment call site.
last_updated: 2026-07-16
owner: simard
doc_type: reference
status: reference
related:
  - ../concepts/autonomous-self-merge-sensor.md
  - ../concepts/draft-pr-merge-exclusion.md
  - ./cross-repo-merge-authority.md
  - ./draft-pr-exclusion-gate.md
  - ./enrichment-observability-api.md
  - ./overseer-tick-details.md
  - ../howto/enable-autonomous-self-merge-canary.md
---

# `ready_prs` sensor API reference

This reference documents the deterministic sensor that populates
`ObservedState.ready_prs` in the acting Overseer's Observe pass, activating the
already-built autonomous self-merge path. For the *why* and the safety posture,
see [the autonomous self-merge sensor concept](../concepts/autonomous-self-merge-sensor.md).

The sensor produces a **candidate list only**. The authoritative merge decision
stays in [`merge_authority`](./cross-repo-merge-authority.md).

## Data types

### `PrRef`

Each entry in `ObservedState.ready_prs` is a `PrRef` — the minimal
`{ repo, pr }` reference the signal layer turns into a
`Signal::PrReadyToMerge { repo, pr }`.

```rust
pub struct PrRef {
    /// `owner/name`, e.g. "rysweet/Simard".
    pub repo: String,
    /// PR number.
    pub pr: u32,
}
```

### `OpenPrSummary.author` and `OpenPrSummary.labels`

The existing open-PR listing summary
([`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs))
already carries an `author` field (used by the author filter); the engineer-PR
gate adds a `labels` field so candidates can be scoped without a second `gh`
round-trip:

```rust
pub struct OpenPrSummary {
    pub number: u32,
    pub title: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub mergeable: String,
    pub checks: Vec<CheckRollupEntry>,
    pub url: String,
    /// PR author login, from `gh pr list --json ...,author` (`author.login`).
    /// Used by the author filter to keep Simard's own login and drop other human
    /// logins. (Pre-existing field — the server-side `--author` push already
    /// relies on it.)
    pub author: String,
    /// PR label names, from `gh pr list --json ...,labels` (each `.name`).
    /// Added so the ready_prs sensor's engineer-PR gate can require the durable
    /// `simard-autonomous` marker. Missing/null/empty ⇒ `Vec::new()` (never
    /// panics), which the gate treats as "no label" (fail-closed).
    pub labels: Vec<String>,
}
```

Both `gh pr list` call sites in
[`RealPrGhClient`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs)
already request `author`; the engineer-PR gate adds one field — `labels` — to
**both** of them: `list_open_prs` (the dashboard path) and `list_prs_by_author`
(the survey path this sensor uses). Because both deserialize through the shared
`parse_pr_list_json`, the field must be added to both `--json` strings or the
survey path silently parses an empty `labels`. The survey path pushes the author
filter server-side via `--author`:

```
# list_prs_by_author — the survey path (author filtered server-side):
gh pr list --repo <owner/repo> --state open --author <login> \
  --json number,title,headRefName,baseRefName,mergeable,statusCheckRollup,url,author,labels \
  --limit <limit>
```

`parse_pr_list_json` reads `author.login` into `OpenPrSummary.author` and each
`labels[].name` into `OpenPrSummary.labels`. A missing or null `author` parses to
an empty string (rejected by the case-insensitive whole-login filter); a
missing/null/empty `labels` array parses to `Vec::new()` — both fail-closed.

## Configuration resolvers

Both resolvers follow the repo's established `*_from(lookup)` testable pattern in
[`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs):
a pure `_from` function takes an environment lookup closure (unit-testable), and
a thin production entry reads the real process environment.

### `SIMARD_AUTOMERGE_REPOS` — the allowlist (default OFF)

```rust
/// Parse the comma-separated allowlist. Unset/empty/whitespace-only ⇒ empty set
/// ⇒ autonomous self-merge OFF. Entries are trimmed; blank entries dropped.
pub fn automerge_repos_from(lookup: impl Fn(&str) -> Option<String>) -> Vec<String>;

/// Production entry: reads SIMARD_AUTOMERGE_REPOS from the process environment.
pub fn automerge_repos() -> Vec<String>;
```

| `SIMARD_AUTOMERGE_REPOS` | Eligible repos |
|---|---|
| unset | **none** — autonomous self-merge OFF |
| `""` / whitespace | **none** — OFF |
| `rysweet/Simard` | just `rysweet/Simard` (canary) |
| `rysweet/Simard,rysweet/azlin` | both listed repos |

A repo is eligible **only** on exact `owner/name` match. Unknown or unset ⇒ the
repo is not surveyed and contributes zero candidates.

### `SIMARD_AUTOMERGE_AUTHOR` — the own-PR identity

```rust
/// The gh login whose PRs Simard may auto-merge, read from
/// `SIMARD_AUTOMERGE_AUTHOR`. Pure env projection: `None` when unset.
pub fn automerge_author_from(lookup: impl Fn(&str) -> Option<String>) -> Option<String>;

/// Production entry. Reads ONLY the explicit `SIMARD_AUTOMERGE_AUTHOR` env var;
/// unset/empty => `None` => the sensor yields no candidates (fail-closed),
/// identical to the pure `_from` resolver it delegates to. There is
/// deliberately NO ambient `gh api user` fallback — see the design note.
pub fn automerge_author() -> Option<String>;
```

> **Design note.** There is deliberately no ambient `gh api user` fallback when
> `SIMARD_AUTOMERGE_AUTHOR` is unset. An autonomous self-merge must never adopt
> whatever identity the daemon's `gh` token happens to resolve to: if that token
> were authenticated as a human operator (a personal `gh auth login`, a PAT in
> CI), the sensor would treat that human's own open PRs as self-merge
> candidates, and the recursion guard — which only refuses the distinct
> `simard-overseer[bot]` login — would not catch them. Both gates (author and
> repo allowlist) therefore require explicit operator opt-in, and
> `automerge_author` fails closed on unset exactly like the pure `_from`
> resolver. Keeping the resolver pure also keeps it deterministic and
> unit-testable.

This is the **OODA/engineer** identity — the login Simard authors her PRs under —
and is **distinct** from `SIMARD_OVERSEER_AUTHOR_LOGIN`
(`simard-overseer[bot]`), which the `RecursionGuard` refuses. Keeping the two
identities separate is what lets Simard's own engineering PRs survive the guard
while the overseer-bot's PRs stay refused. If the author cannot be resolved, the
sensor returns an **empty** candidate list (fail-closed).

### `SIMARD_ENGINEER_PR_LABEL` + engineer branch-namespace allow-list — the engineer-PR gate

The author filter is **not sufficient** on its own: Simard's engineers *and* the
operator both author PRs under the same login (`rysweet`). Setting
`SIMARD_AUTOMERGE_AUTHOR=rysweet` would therefore make the operator's **own review
PRs** eligible — which must **never** happen. Worse, engineers and the operator
also share the **common branch prefixes** — `feat/`, `fix/`, and `chore/` are all
used by both — so a branch prefix like `feat/` cannot discriminate between an
engineer's merge-ready PR and the operator's own review PR either. The engineer-PR
gate closes that gap with a durable, self-identifying **label** as the primary
marker, backed by a narrow set of branch namespaces that only Simard's automation
ever creates. These are **compile-time constants** in
[`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs),
not env vars — the scoping is a code-controlled safety property, not operator
tunable:

```rust
/// The durable machine label Simard's engineers apply to every PR they open —
/// the PRIMARY, general-purpose engineer marker. Because it is independent of
/// branch naming, it is the only marker that can positively identify Simard's own
/// work on a shared branch prefix (`feat/`, `fix/`, `chore/`). Matched by
/// whole-string, case-sensitive equality (a substring/prefix match could admit a
/// spoofed look-alike label).
pub const SIMARD_ENGINEER_PR_LABEL: &str = "simard-autonomous";

/// Head-branch namespaces that ONLY Simard's automation creates, assigned
/// deterministically in Rust — the SECONDARY, defense-in-depth marker.
/// Deliberately restricted to namespaces that are provably never operator-authored:
///   - `engineer/`       — the engineer worktree
///                          (`src/engineer_worktree/mod.rs`: `format!("engineer/{dir_name}")`)
///   - `chore/advisory-` — the supply-chain steward
///                          (`src/supply_chain_steward/execute.rs`: `format!("chore/advisory-{id}")`)
/// It deliberately EXCLUDES every shared prefix — `feat/`, `fix/`, and bare
/// `chore/` — because operators author review PRs under those too, so admitting
/// them would re-open the exact gap this gate closes. Non-empty by construction;
/// an empty prefix would match every branch.
pub const ENGINEER_BRANCH_PREFIXES: &[&str] = &["engineer/", "chore/advisory-"];

/// True iff `head_ref` starts with one of ENGINEER_BRANCH_PREFIXES. Empty
/// prefixes are guarded against, and an empty `head_ref` matches nothing.
pub fn is_engineer_branch(head_ref: &str) -> bool;
```

A PR passes the engineer-PR gate iff:

```
labels contains SIMARD_ENGINEER_PR_LABEL   (whole-string, case-sensitive)          ← primary
  OR
is_engineer_branch(head_ref_name) == true  (anchored starts_with, non-empty prefix) ← secondary
```

The gate is an **OR**, but the two arms are not co-equal:

- The **`simard-autonomous` label is the primary marker.** It is the only marker
  that works on the shared branch prefixes (`feat/`, `fix/`, `chore/`) that
  engineers and the operator both use, so every engineer PR must carry it. The
  engineer applies it at `gh pr create` time.
- The **engineer branch namespace is a secondary, defense-in-depth marker.** It
  covers only `engineer/` and `chore/advisory-` — namespaces Simard's automation
  assigns deterministically in Rust and that no operator review PR ever uses. It
  exists so an engineer PR is still caught if the label was forgotten, and because
  those namespaces are engineer-exclusive it can **never** admit an operator PR.

Either arm alone proves Simard-origin; **neither ⇒ excluded**, even if the author
matches and CI is green. Critically, a shared-prefix branch (`feat/…`, `fix/…`,
bare `chore/…`) that lacks the label is **excluded** — that is exactly the
operator's own review PR case.

> **Narrowing gate, not the trust boundary.** The engineer-PR gate runs *after*
> and *within* the author filter — it can only ever *remove* PRs the author
> filter already admitted, never add one. The author filter
> (`SIMARD_AUTOMERGE_AUTHOR`) plus GitHub server-side branch protection remain
> the external-attacker trust boundary; the engineer-PR gate is the intra-author
> boundary that separates Simard's engineers from the operator. A label applied
> by a triage-permission collaborator on a *foreign-authored* PR cannot make it a
> candidate, because that PR fails the author filter first.

## The survey seam

A single trait method on the `PrOps` capability
([`src/overseer/capabilities.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/capabilities.rs))
carries the survey. It returns a **`Vec`, not a `Result`**, so the type system
forces fail-to-empty — there is no error variant that could accidentally be
mapped to a merge.

```rust
pub trait PrOps {
    // ... existing methods ...

    /// Survey allowlisted repos for Simard-engineer-authored, green + MERGEABLE
    /// PRs. Returns candidate references only — never merges. Candidates are
    /// scoped by author login AND the engineer-PR gate (the `simard-autonomous`
    /// label OR an engineer-exclusive branch namespace), so the operator's own
    /// review PRs — which share the author login and the common branch prefixes —
    /// are never included. Any per-repo error is logged (`tracing::warn!`) and
    /// that repo contributes nothing; the method never returns an error and never
    /// panics.
    ///
    /// Default impl returns an empty Vec so existing fakes need no stub.
    fn survey_ready_prs(&self, _repos: &[String]) -> Vec<PrRef> {
        Vec::new()
    }
}
```

The production implementation lives on `MergePrOps`
([`src/overseer/merge_ops.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/merge_ops.rs)).

### Pipeline (`MergePrOps::survey_ready_prs`)

For each `repo` in the allowlist, in order:

1. **List (author pushed server-side)** — `list_prs_by_author(repo, author, 100)`.
   The author is passed to `gh pr list --author <login>` so GitHub filters
   server-side; a generous 100-PR cap then can't let a busy repo crowd Simard's
   own eligible PRs out of the window. On error: `warn!` and skip this repo.
2. **Author re-check (defense-in-depth)** — keep PRs where `summary.author`
   equals `automerge_author()` by whole-login, case-insensitive equality
   (`eq_ignore_ascii_case`). The server already filtered by `--author`, but
   re-checking in-process guards against any `gh`/`--author` matching-semantics
   drift. No `contains` / prefix / regex — an empty or mismatched author is
   dropped.
3. **Engineer-PR gate** — keep only PRs that carry the durable engineer marker:
   the primary `summary.labels.iter().any(|l| l == SIMARD_ENGINEER_PR_LABEL)`
   **OR** the secondary `is_engineer_branch(&summary.head_ref_name)` (the
   engineer-exclusive namespaces `engineer/` and `chore/advisory-` only). A PR
   matching **neither** is dropped (with a `debug!` exclusion note) even though its
   author matched. This runs **after** the author filter (so it can only narrow,
   never widen) and **before** the objective pre-filter. It is what keeps the
   operator's own review PRs (shared author login, shared branch prefix such as
   `feat/…`, no `simard-autonomous` label) out of the candidate set.
4. **Objective pre-filter** — project each surviving `OpenPrSummary` to a
   `PrSnapshot` (via `to_snapshot()`) and run the existing
   `evaluate_objective_gates(&snap, &self.base_allowlist)`: base-branch
   allowlist, `mergeable == "MERGEABLE"`, and every `statusCheckRollup` entry in
   `{SUCCESS, NEUTRAL, SKIPPED}`. The base allowlist passed here **must be the
   same `self.base_allowlist` the authoritative gate uses** (seeded from
   `base_allowlist_from_env()`); reusing it is what guarantees the *additive
   strictness* invariant — a looser base list in the sensor could admit a
   candidate the gate rejects. The merge-judge is **not** run here.
5. **Collect** the survivors as `PrRef { repo, pr: number }`.

The result is the concatenation of survivors across all allowlisted repos. An
empty allowlist means the loop body never runs and the result is empty.

## Call site (enrichment)

`ObservedState.ready_prs` is populated in the acting `run_cycle` enrichment path
([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs)),
after `observed_from_snapshot` produces the side-effect-free projection:

```rust
// Enrichment (acting run_cycle), alongside blocked_goals / workstream_gaps / recall.
let allowlist = config::automerge_repos();
observed.ready_prs = caps.prs.survey_ready_prs(&allowlist); // empty allowlist ⇒ empty
```

`config::automerge_repos()` reads the process environment. Because a
systemd-managed daemon's environment is fixed for the life of the process,
changing `SIMARD_AUTOMERGE_REPOS` requires a daemon **restart** to take effect
regardless of whether the value is cached or re-read per cycle.

> **Observability requirement.** The survey impl **must** emit a structured
> `tracing` line (target `overseer::merge_ops`, e.g. an `info!` naming each
> included candidate and a `warn!` per skipped repo). The
> [canary runbook](../howto/enable-autonomous-self-merge-canary.md) relies on
> operators grepping this line to confirm the wire is live; without it, the only
> observable signal is the downstream `PrReadyToMerge`.

`observed_from_snapshot`
([`src/overseer/sensor.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/sensor.rs))
continues to set `ready_prs: Vec::new()`; its comment now points here rather than
describing an unfilled "M2 capability".

## Downstream (unchanged)

Once `ready_prs` is non-empty, the existing chain runs untouched:

| Stage | Location |
|---|---|
| `Signal::PrReadyToMerge { repo, pr }` per entry | `src/overseer/signal.rs` |
| `ProblemKind::DeliveryReady` | Orient |
| `Intervention::VerifyAndMergePr` (`allow_verify_merge = true`) | `src/overseer/mod.rs` |
| `caps.prs.merge()`: RecursionGuard admit → `verify()` checklist → poll-until-green | `src/overseer/merge_ops.rs` |
| Authoritative gate `merge_pr_if_merge_ready_with_judge` (objective gates + merge-judge) → `gh pr merge --squash --delete-branch` | `src/stewardship/merge_authority.rs` |

## Error & edge-case matrix

| Condition | Result |
|---|---|
| Allowlist unset/empty | `ready_prs = []` (OFF) |
| Author unresolved | `ready_prs = []` |
| `list_open_prs` errors for a repo | `warn!`, that repo skipped, others continue |
| PR authored by a different human login | excluded (author filter) |
| PR authored by `simard-overseer[bot]` | excluded (wrong identity) + refused later by `RecursionGuard` |
| PR author matches but branch is a shared prefix (`feat/…`, `fix/…`, bare `chore/…`) and has no `simard-autonomous` label | **excluded** (engineer-PR gate) — this is the operator-review-PR case |
| PR author matches, has `simard-autonomous` label | passes engineer-PR gate (primary marker) |
| PR author matches, branch is an engineer-exclusive namespace (`engineer/…`, `chore/advisory-…`) | passes engineer-PR gate (secondary marker; label not required) |
| PR `mergeable != "MERGEABLE"` (e.g. `CONFLICTING`) | excluded |
| Any check `FAILURE`/`PENDING`/`IN_PROGRESS`/… | excluded |
| PR base branch not in allowlist | excluded |
| Own-author + engineer marker (label OR engineer branch) + green + `MERGEABLE` + allowlisted repo | **included** as candidate |

## Invariants

- The sensor **never merges**. It returns `Vec<PrRef>` and nothing else.
- Fail-closed and fail-visible: every failure yields an empty list plus a log
  line, never a silent wrong merge.
- The engineer-PR gate runs **after** the author filter and only **narrows** it:
  it can never make a foreign-authored PR a candidate, and a PR carrying neither
  the `simard-autonomous` label nor an engineer-exclusive branch namespace is
  never a candidate — including any PR on a shared prefix (`feat/`, `fix/`,
  `chore/`) without the label. This is what excludes the operator's own review PRs.
- Marker matching is **exact**: the label is compared by whole-string,
  case-sensitive equality; the branch by anchored `starts_with` on a curated,
  engineer-**exclusive**, non-empty namespace list (`engineer/`, `chore/advisory-`).
  No substring/regex matching, and no shared prefix (`feat/`, `fix/`, bare
  `chore/`) is ever in the list.
- The authoritative merge gate in `merge_authority` is unchanged and still
  never uses `--admin` or `--no-verify`.
- The pre-filter is additive strictness only — it cannot admit a merge the
  authoritative gate would refuse.
