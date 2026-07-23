---
title: Governed-roster identity-state resolution
description: >
  How ecosystem-observe, agentic merge-queue reasoning, and ci-health resolve
  the same identity-curated governed repository roster from durable identity
  state; documents IdentityStateStore, the governed_repos key, seed-once
  initialization, deploy durability, resolve_governed_roster, fail-loud
  validation, and the simard roster curation CLI.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
issue: 2419
related:
  - ../design/ecosystem-observe.md
  - ./ooda-engineer-lifecycle-recipe.md
  - ./recipe-brain-api.md
  - ../fail-open-audit.md
  - ./state-root-resolution.md
---

# Governed-roster identity-state resolution

> **Status: current.** This page is the contract for how Simard resolves the
> governed repository roster. The roster is **mutable, identity-scoped state**
> under the durable state root. It is not a committed framework asset, and it is
> not overwritten by `simard install`.

The roster is one payload in the generic `IdentityStateStore`: per-identity,
per-key TOML stored at `<state_root>/identity_state/<identity>/<key>.toml`.
For Simard's governed repositories the key is `governed_repos`, so the active
file is:

```text
<state_root>/identity_state/<identity>/governed_repos.toml
```

The same resolved, validated slug list feeds the `ecosystem-observe` rail, the
agentic merge-queue reasoner, and the `ci-health` sweep. There is one curated
source of truth for the active identity.

---

## Why identity-curated durable state

A self-deploy (`simard install`) refreshes framework assets under
`~/.simard/prompt_assets`. Operator-curated stewardship data must survive that
refresh. Putting the roster under the state root makes curation durable across
upgrades and identity-specific: `simard` can steward one set of repos, while a
future identity can store different typed data under its own keys without the
framework core learning a hardcoded "roster" concept.

The framework mechanism is intentionally generic:

- `IdentityStateStore` stores TOML payloads by `(identity, key)`.
- Each identity can have differently typed curated data.
- `governed_repos` is Simard's payload key, not a framework-wide special case.

---

## Resolution

Resolution uses two inputs:

| Input | Source | Default |
|---|---|---|
| `state_root` | `$SIMARD_STATE_ROOT` | `$HOME/.simard` |
| `identity` | `$SIMARD_IDENTITY` | `simard` |

The governed roster path is derived directly from those inputs:

```text
${SIMARD_STATE_ROOT:-$HOME/.simard}/identity_state/${SIMARD_IDENTITY:-simard}/governed_repos.toml
```

There is no source-tree fallback and no prompt-assets roster file. The on-disk
identity-state payload is the authoritative curated copy after first
initialization.

---

## Seed-once behavior

On first use, if the active identity has no `governed_repos` payload, Simard
seeds it once from the in-code identity seed constant
`DEFAULT_SIMARD_GOVERNED_ROSTER` in `src/overseer/ecosystem_observe.rs`, then
persists it to identity state.

After that first write:

1. the on-disk curated copy is authoritative;
2. the seed constant is not reapplied;
3. operator or agentic curation survives `simard install`;
4. empty or all-invalid curated data is a hard error, never a silent green pass.

This seed is a bootstrap default, not a perpetual template.

---

## API

The resolution entry point is the roster-specific helper layered on the generic
identity-state store:

```rust
/// Resolve, seed if missing, parse, and validate the active identity's governed
/// repository roster.
fn resolve_governed_roster(
    state_root: &Path,
    identity: &str,
    seed_toml: &str,
) -> SimardResult<Vec<String>>;
```

### Parameters

| Parameter | Meaning |
|---|---|
| `state_root` | Durable state root. Production resolves it from `$SIMARD_STATE_ROOT`, else `$HOME/.simard`. |
| `identity` | Active identity. Production resolves it from `$SIMARD_IDENTITY`, else `simard`. |
| `seed_toml` | In-code bootstrap TOML used only when the identity has no persisted `governed_repos` payload yet. |

### Return value

`Ok(Vec<String>)` returns validated `owner/name` slugs from the active identity's
curated roster. An absent payload is created from `seed_toml` before validation.
Malformed slugs are rejected; an empty or all-invalid roster returns an error.

---

## Wiring contract

All three consumers call the same roster resolution path:

| Consumer | Uses the resolved roster for |
|---|---|
| `ecosystem-observe` | Agentic multi-repo observation scope. |
| Agentic merge-queue reasoner | Default-ON issue and PR reasoning scope. |
| `simard ci-health` | Governed-fleet CI sweep. |

The `ecosystem-observe` and merge-queue rails still pass a recipe-level
`{{roster_path}}` context variable. That variable is a **per-invocation context
file** containing the already-resolved, validated slugs. It is not the identity
TOML file and not a source asset. Recipes should treat it as input data for that
run only.

Failures are visible:

| Condition | Behavior |
|---|---|
| Missing payload | Seed once from `DEFAULT_SIMARD_GOVERNED_ROSTER`, persist, then validate. |
| Malformed entries | Reject invalid slugs and warn. |
| Empty / all-invalid roster | Hard error; the consumer skips/fails the pass loudly. |
| `simard install` | Refreshes prompt assets only; leaves identity state untouched. |

---

## Configuration and curation

Roster curation is done through the operator CLI, which mutates the active
identity's `governed_repos` payload durably:

```bash
simard roster list              # same as: simard roster
simard roster add rysweet/tool "why this repo is stewarded"
simard roster remove rysweet/tool
```

The commands are idempotent. They operate on the active identity, so set
`SIMARD_STATE_ROOT` or `SIMARD_IDENTITY` first when curating a non-default state
root or identity.

To inspect the backing state directly when debugging:

```bash
state_root=${SIMARD_STATE_ROOT:-$HOME/.simard}
identity=${SIMARD_IDENTITY:-simard}
ls "$state_root/identity_state/$identity/governed_repos.toml"
```

Prefer `simard roster` for normal changes; direct file edits are an escape hatch
for recovery, not the curation surface.

---

## Examples

### First run on a fresh state root

```bash
SIMARD_STATE_ROOT="$PWD/.simard-state" SIMARD_IDENTITY=simard simard roster list
```

If the payload is absent, Simard seeds
`.simard-state/identity_state/simard/governed_repos.toml` from
`DEFAULT_SIMARD_GOVERNED_ROSTER`, validates it, and prints the active roster.
Later runs read the persisted file.

### Add a stewarded repo

```bash
simard roster add rysweet/new-tool "Reason this repo is now stewarded"
simard roster list
```

The next ecosystem observation, merge-queue reasoning pass, and CI-health sweep
use the same updated active-identity roster.

### Recipe invocation by hand

When invoking a recipe manually, pass `roster_path` as a context file containing
resolved slugs for that invocation:

```bash
mkdir -p .simard-run-context
cat > .simard-run-context/roster.txt <<'EOF'
rysweet/Simard
rysweet/azlin
EOF
: > .simard-run-context/inflight-refs.json

amplihack recipe run prompt_assets/simard/recipes/ecosystem-observe.yaml \
  -c roster_path=.simard-run-context/roster.txt \
  -c inflight_refs_path=.simard-run-context/inflight-refs.json \
  -c observed_problems_path=.simard-run-context/observed-problems.txt \
  -c escalation_note=""
```

On the live cadence the rail creates that context file after resolving identity
state; operators do not pass the TOML backing file to the agent.

---

## Verifying resolution

Use the CLI to verify the active identity and roster contents:

```bash
simard roster list

state_root=${SIMARD_STATE_ROOT:-$HOME/.simard}
identity=${SIMARD_IDENTITY:-simard}
test -f "$state_root/identity_state/$identity/governed_repos.toml" && echo "identity roster present"
```

Then watch the relevant consumer:

```bash
journalctl --user -u simard -f | grep -E 'ecosystem-observe|merge_queue|ci-health'
```

Expect roster faults to be logged loudly. Do not accept a green ecosystem or
fleet result if the active roster is empty or invalid; that is a bug.

See [Watch Overseer activity](../howto/watch-overseer-activity.md) for the
broader journal-tailing workflow.

---

## Testing

Coverage should prove the durable-state contract, not a path ladder:

| Case | Expectation |
|---|---|
| Missing identity payload | Seeds from `DEFAULT_SIMARD_GOVERNED_ROSTER`, persists, then returns valid slugs. |
| Existing identity payload | Reads the persisted payload and does not reseed. |
| Different identity | Resolves a separate `<identity>/governed_repos.toml`. |
| `SIMARD_STATE_ROOT` override | Uses that state root instead of `$HOME/.simard`. |
| Invalid entries | Rejects malformed slugs before they reach `gh`. |
| Empty / all-invalid roster | Returns a hard error. |
| Self-deploy | Leaves the identity-state payload untouched. |

Gates: full `cargo test` and `cargo clippy --all-targets -- -D warnings` should
cover code changes. Documentation-only edits do not require Rust validation.

---

## Security notes

- **Identity/key path is controlled.** The store derives paths from the active
  state root, identity, and a fixed payload key; roster entries do not influence
  filesystem paths.
- **No shell expansion.** Validated `owner/name` slugs are passed to downstream
  `gh` calls as positional argv, not interpolated shell text.
- **Fail loud.** Empty/all-invalid curated state is an error, preventing a false
  green pass over zero repos.
- **Durable curation.** Deployment refreshes prompt assets, not identity state,
  so operator curation is not lost during upgrades.
- **Logs avoid contents where possible.** Diagnostics should identify the active
  state path and validation failures without treating roster text as commands.

---

## See also

- [Ecosystem Observe](../design/ecosystem-observe.md) — the full agentic
  OBSERVE→BRIEF chain and the roster's role as an allowlist.
- [State-root resolution](./state-root-resolution.md) — state-root override and
  threat-model background.
- [Fail-open audit](../fail-open-audit.md) — fail-visible posture across rails.
- [Watch Overseer activity](../howto/watch-overseer-activity.md) — how to
  confirm the live rail runs.
