---
title: Objective merge-judge fallback API reference
description: >
  The typed surface of the opt-in objective merge-judge tier that lets
  build_merge_judge() return a non-refusing merge authority for trusted authors
  past the objective gates — the ObjectiveMergeJudge type, the
  MergeJudgeKind::Objective variant and is_configured(), the
  SIMARD_MERGE_OBJECTIVE_FALLBACK and SIMARD_MERGE_TRUSTED_AUTHORS environment
  config with hardened parsing, the additive PrSnapshot.author_login field that
  gives the judge the authenticated author, the project_ready_prs gate #3
  trusted-author admission and gate #5 is_draft hydration, and the full
  fail-closed / edge-case matrix.
last_updated: 2026-07-21
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/objective-merge-judge-fallback.md
  - ../concepts/autonomous-merge-review-gate.md
  - ./autonomous-merge-review-gate.md
  - ./ready-prs-sensor-api.md
  - ./draft-pr-exclusion-gate.md
  - ./cross-repo-merge-authority.md
  - ../howto/enable-objective-merge-fallback.md
  - ../../src/stewardship/objective_merge_judge.rs
  - ../../src/stewardship/merge_judge.rs
  - ../../src/stewardship/merge_authority.rs
  - ../../src/overseer/mod.rs
---

# Objective merge-judge fallback API reference

> **Status: implemented.** The `ObjectiveMergeJudge`, the
> `MergeJudgeKind::Objective` variant, and the env resolvers below live in
> [`src/stewardship/objective_merge_judge.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/objective_merge_judge.rs)
> and
> [`src/stewardship/merge_judge.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_judge.rs);
> the `project_ready_prs` gate corrections live in
> [`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs).
> The judgment-path selection is covered by unit tests (author-spoof rejection,
> bot self-merge exclusion, default fail-closed, gate admission).

This reference specifies the API, configuration, and edge-case matrix of the
opt-in objective merge-judge tier. For the *why* and the safety narrative, see
[the objective merge-judge fallback concept](../concepts/objective-merge-judge-fallback.md).

**One-line summary:** with `SIMARD_MERGE_OBJECTIVE_FALLBACK` set,
`build_merge_judge()` resolves to an `ObjectiveMergeJudge` that returns
`Verdict::Ready` for trusted-author PRs already past the objective gates;
otherwise it stays `RefusingMergeJudge` (fail-closed).

## Contents

- [`MergeJudgeKind`](#mergejudgekind)
- [`ObjectiveMergeJudge`](#objectivemergejudge)
- [Required `PrSnapshot` extension](#required-prsnapshot-extension-committed-design)
- [`build_merge_judge()` resolution](#build_merge_judge-resolution)
- [Environment configuration](#environment-configuration)
- [`project_ready_prs` gate corrections](#project_ready_prs-gate-corrections)
- [Edge-case & fail-closed matrix](#edge-case-and-fail-closed-matrix)
- [Telemetry](#telemetry)

## `MergeJudgeKind`

An additive enum that names the resolved judgment tier for selection and
telemetry. Existing variants are unchanged; `Objective` is new.

```rust
/// Which merge-judgment tier build_merge_judge() resolved to.
/// Existing variants (Llm, Recipe, Refusing) are UNCHANGED; `Objective` is new.
/// Keeps the existing `#[serde(rename_all = "snake_case")]` telemetry vocabulary.
pub enum MergeJudgeKind {
    /// LlmMergeJudge — production impl backed by an LLM provider (unchanged).
    Llm,
    /// RecipeMergeJudge — recipe-runner-rs backed impl (unchanged).
    Recipe,
    /// Fail-closed default: refuses every PR (Verdict::NotReady) (unchanged).
    Refusing,
    /// Opt-in objective tier: Ready for trusted-author PRs past objective gates.
    Objective,
}

impl MergeJudgeKind {
    /// Whether this tier can issue a non-refusal verdict. The existing contract
    /// (`Llm | Recipe`) is widened to include `Objective`; drives the dashboard
    /// `judge_configured` field and the `merge_judge_kind` telemetry label.
    /// Takes `self` by value (the enum is `Copy`), matching the current impl.
    pub fn is_configured(self) -> bool {
        matches!(
            self,
            MergeJudgeKind::Llm | MergeJudgeKind::Recipe | MergeJudgeKind::Objective
        )
    }
}
```

## `ObjectiveMergeJudge`

A `MergeJudge` implementation that performs **no** LLM/recipe review. It returns
`Verdict::Ready` **only** when both hold:

1. The PR's authenticated `author.login` — read from `PrSnapshot.author_login`
   (see [the required extension below](#required-prsnapshot-extension-committed-design)) —
   is in the trusted-author allowlist (exact equality, lowercased), and is
   **not** the daemon's own bot identity.
2. The PR has already passed every objective gate upstream (CI-green,
   `MERGEABLE`, base + repo allow-lists, non-draft).

Otherwise it returns `Verdict::NotReady` with a structured reason.

```rust
pub struct ObjectiveMergeJudge {
    trusted_authors: BTreeSet<String>, // lowercased logins; bot identity excluded
    bot_login: String,                 // never trusted (anti self-merge)
}

impl ObjectiveMergeJudge {
    /// Build from the resolved env config. Returns None if the trusted-author
    /// set is empty after excluding the bot identity (⇒ caller keeps Refusing).
    pub fn from_env(bot_login: &str) -> Option<Self> { /* … */ }
}

impl MergeJudge for ObjectiveMergeJudge {
    fn judge(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
    ) -> SimardResult<JudgeOutcome> {
        // Ready iff the AUTHENTICATED author (snapshot.author_login, see the
        // PrSnapshot extension below) is trusted and is not the bot:
        //   let author = snapshot.author_login.to_lowercase();
        //   if !author.is_empty()
        //       && author != self.bot_login
        //       && self.trusted_authors.contains(&author)
        //   => JudgeOutcome { verdict: Verdict::Ready, rationale, blockers: vec![] }
        //   else
        //   => JudgeOutcome { verdict: Verdict::NotReady, rationale, blockers }
        // An empty author_login (author object missing from the API) fails closed.
    }

    /// Required by the `MergeJudge` trait (used by the dashboard without
    /// invoking the judge). Reports the objective tier.
    fn kind(&self) -> MergeJudgeKind {
        MergeJudgeKind::Objective
    }
}
```

### Required `PrSnapshot` extension (committed design)

`judge()` receives only `pr_number`, `repo`, and
[`PrSnapshot`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs),
which today carries **no** author field (`body`, `mergeable`, `review_decision`,
`checks`, `base_ref_name`, `labels`). Because the merge authority consults the
**judge itself** as the sole review step, the judge must return `Verdict::Ready`
on its own — a trusted-author admission in `project_ready_prs` gate #3 only lets
the PR *reach* the ready set; it does not make `RefusingMergeJudge` (or any
judge) say Ready. Gate #3 alone therefore **cannot** deliver the merge.

The committed design is to **add an `author_login` field to `PrSnapshot`** and
hydrate it from the *existing* `gh pr view` call by adding `author` to its
`--json` field list — no new `gh` invocation, no new token scope:

```rust
// src/stewardship/merge_authority.rs — additive field (default = "" ⇒ fail-closed)
pub struct PrSnapshot {
    pub body: String,
    pub mergeable: String,
    pub review_decision: String,
    pub checks: Vec<CheckRollupEntry>,
    pub base_ref_name: String,
    pub labels: Vec<String>,
    /// `author.login` from `gh pr view --json ...,author`. Empty when the
    /// author object is absent from the API response ⇒ the objective judge
    /// fails closed (never Ready). Never sourced from `body`/title/trailers.
    pub author_login: String,
}
```

The hydration site changes from
`gh pr view <pr> --repo <repo> --json body,statusCheckRollup,mergeable,reviewDecision,baseRefName,labels`
to `...,labels,author`, parsing `author.login` into the new field. This is
additive and non-breaking: every existing caller keeps compiling (the field
defaults to `""`), and the LLM/recipe/refusing judges simply ignore it. The
judge must **never** infer the author from the spoofable `PrSnapshot.body`.

> The objective gates are evaluated **before** the judge is consulted (in the
> merge authority / `verify()` objective pre-filter). `ObjectiveMergeJudge`
> assumes they passed; it never re-opens or bypasses them.

## `build_merge_judge()` resolution

The resolver is a fail-closed cascade. Its **signature is unchanged** —
`build_merge_judge() -> Box<dyn MergeJudge>` — because all three callers
(`merge_authority`, `merge_ops`, and `merge_readiness`, the last via
`build_merge_judge().kind()`) depend on it; the resolved tier is read back
through [`MergeJudge::kind()`](#mergejudgekind), not a returned tuple. The
`Objective` branch is inserted **only** ahead of the refusing default and
**only** when explicitly enabled.

```rust
// Signature UNCHANGED (no MergeJudgeConfig param, no tuple return): the tier is
// read via `.kind()`. Objective is inserted only before the Refusing default.
pub fn build_merge_judge() -> Box<dyn MergeJudge> {
    let repo_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // 1. Recipe-runner-rs judge, if binary + recipe are available (unchanged).
    if let Some(j) = RecipeMergeJudge::new(&repo_root) {
        tracing::info!("merge-judge: using recipe-runner-rs backed judge");
        return Box::new(j);
    }
    // 2. Direct LLM judge, if a provider resolves (unchanged).
    if let Ok(provider) = LlmProvider::resolve() {
        return Box::new(LlmMergeJudge::new(SessionLlmSubmitter::new(provider)));
    }
    // 3. Opt-in objective fallback — trusted authors past objective gates.
    //    `bot_login` comes from the overseer identity (overseer_login()), NOT a
    //    new config type. from_env() → None ⇒ fall through to Refusing.
    if objective_fallback_enabled() {
        if let Some(j) = ObjectiveMergeJudge::from_env(overseer_login()) {
            return Box::new(j);
        }
    }
    // 4. Fail-closed default (unchanged). NOTE: this fix also replaces the
    //    existing stray `eprintln!` in this function with `tracing`.
    Box::new(RefusingMergeJudge)
}
```

## Environment configuration

Both variables are parsed with the hardened env pattern (trimmed, case-insensitive
boolean, CSV split with per-entry validation). Neither grants new token scopes.

| Variable | Type | Default | Meaning |
|---|---|---|---|
| `SIMARD_MERGE_OBJECTIVE_FALLBACK` | bool | **off** (unset) | Master switch. `1`/`true`/`yes`/`on` (case-insensitive) enables the objective tier in `build_merge_judge()`. Any other value, or unset, keeps `RefusingMergeJudge`. |
| `SIMARD_MERGE_TRUSTED_AUTHORS` | CSV of logins | `rysweet` | Allowlist of authenticated `author.login`s eligible for `Verdict::Ready`. Compared lowercased, exact-match. The daemon's bot identity is always removed from this set. |

**Parsing rules (hardened):**

- Whitespace around the whole value and each CSV entry is trimmed.
- Empty entries are dropped; a value that reduces to an empty set leaves the
  daemon on `RefusingMergeJudge`.
- An entry containing whitespace, `/`, or other non-login characters is
  **rejected** (logged via `tracing::warn!`, entry skipped) — never used to
  build an argv or branch.
- The bot login is excluded even if explicitly listed (no self-merge).

## `project_ready_prs` gate corrections

Two narrowing/hydration fixes in `project_ready_prs` (in `src/overseer/mod.rs`)
let delivery-ready PRs reach the ready set. Every other gate (G2 author,
objective gates, MergeJudge) is unchanged.

### Gate #3 — trusted-author admission

Previously gate #3 required an engineer label **or** engineer branch prefix.
It now **also** admits a PR whose authenticated `author.login` is in
`SIMARD_MERGE_TRUSTED_AUTHORS`, so rysweet-authored non-engineer PRs are no
longer silently dropped.

```text
admit if is_engineer_pr(pr) OR trusted_authors.contains(pr.author_login)
```

### Gate #5 — `is_draft` hydration

The projection now hydrates `ProjectionCandidate.is_draft` from the `isDraft`
field of the listing JSON. The fail-closed `Option<bool>` semantics are
**preserved**: admit only `Some(false)`; exclude `Some(true)` and `None`. The
bug fixed here is that `is_draft` was previously left `None` for known-non-draft
PRs, causing fail-closed exclusion of eligible PRs. See the
[draft-PR exclusion gate](./draft-pr-exclusion-gate.md).

## Edge-case and fail-closed matrix

| Situation | Result |
|---|---|
| `SIMARD_MERGE_OBJECTIVE_FALLBACK` unset | `RefusingMergeJudge` (fail-closed) — no change from before |
| Fallback on, author **not** in allowlist | `Verdict::NotReady` → escalate |
| Fallback on, author **is** the bot identity | Excluded from allowlist → `NotReady` (no self-merge) |
| Fallback on, author object missing from API (`author_login` empty) | Fail-closed → `NotReady` (never Ready on an unverifiable author) |
| Fallback on, spoofed trailer/body claims trusted author | Ignored — only authenticated `author.login` is matched → `NotReady` unless the login itself is trusted |
| Fallback on, trusted author, objective gate fails (red CI, conflict, draft) | Objective pre-filter blocks upstream; judge never returns Ready |
| `SIMARD_MERGE_TRUSTED_AUTHORS` empty / all invalid | Objective tier not built → `RefusingMergeJudge` |
| Recipe/LLM judge wired | Takes precedence; objective tier not consulted |
| Gate #3: rysweet PR, no engineer label | Admitted via trusted-author branch of gate #3 |
| Gate #5: `isDraft` absent from JSON | `None` → excluded (fail-closed preserved) |

## Telemetry

- `merge_judge_kind` label (`llm` / `recipe` / `refusing` / `objective`, the
  snake_case `serde` tags of `MergeJudgeKind`) is emitted on the merge-judgment
  metric so operators can confirm which tier fired.
- Objective-tier `Ready` verdicts and every rejected/invalid trusted-author
  entry are recorded with structured `tracing` fields (OTel) — no `println!`,
  no secrets, and never the PR body.
