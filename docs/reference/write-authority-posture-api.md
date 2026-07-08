---
title: Write-authority posture API reference
description: Rust and identity.toml reference for the write-authority posture contract — the IdentityAuthority manifest field, the [identities.authority] TOML block, posture-aware check_git_safety / check_ado_acl_safety / GitHub write-path enforcement, composition rules, error variants, and the `simard debug authority` verification command.
last_updated: 2026-07-08
owner: simard
doc_type: reference
related:
  - ../concepts/write-authority-posture.md
  - ../reference/pluggable-identity-api.md
  - ../reference/ado-acl-self-escalation-guard.md
  - ../reference/agent-instance-isolation.md
---

# Write-authority posture API reference

!!! warning "Implementation status — this page describes the PLANNED typed API (tracking #3067)"
    The typed `IdentityAuthority` / `WritePosture` types, the `[identities.authority]`
    TOML block, the `*_with_authority` guardrail entry points, and the
    `simard debug authority` verb documented on this page are **not yet implemented**;
    they are the target design tracked in
    [#3067](https://github.com/rysweet/Simard/issues/3067). **What ships today** is the
    env-driven read-only floor: `SIMARD_OBSERVE_ONLY=1` activates the fail-closed
    `read_only_guard` classifier (`observe_only_enabled`, `check_observe_only`,
    `check_observe_only_git`, `guard_observe_only_git`, `command_is_read`,
    `is_write_command`), which is wired into `git_guardrails::check_git_safety` and
    `ooda_actions::advance_goal::spawn::dispatch_spawn_engineer`. The runnable guardrail
    proof lives in the `rysweet/Crocutus` repo (`scripts/prove-guardrail.sh`).

Modules: `simard::identity::toml_types`, `simard::identity::manifest`,
`simard::git_guardrails`, `simard::ado_acl_guard`

The write-authority posture contract adds a typed, per-identity policy that
governs whether a session may write via git, Azure DevOps, or GitHub. For the
rationale see [Write-authority posture](../concepts/write-authority-posture.md).

!!! note "Planned types (tracking #3067)"
    `IdentityAuthority` / `WritePosture` (domain) and the `TomlAuthority`
    deserialization struct behind `[identities.authority]` are **planned, not yet
    shipped**. The target implementation adds `TomlAuthority` to
    `simard::identity::toml_types`, threads it into `IdentityManifest`, and maps the
    TOML block onto `IdentityAuthority` during load. Because every pluggable-identity
    TOML type uses `deny_unknown_fields`, a `[identities.authority]` block is a hard
    parse error against the **current** schema until this lands — so a shipped
    Crocutus identity today expresses its read-only mandate via the
    `SIMARD_OBSERVE_ONLY` environment floor, not this TOML block.

---

## `IdentityAuthority`

Module: `simard::identity::manifest`

```rust
pub struct IdentityAuthority {
    pub posture: WritePosture,
    pub allowed_write_repos: Vec<String>,
    pub allow_git_push: bool,
    pub allow_ado_writes: bool,
    pub allow_github_writes: bool,
}

pub enum WritePosture {
    ReadOnly,
    ScopedWrite,
    Full,
}
```

`IdentityAuthority` is a field on `IdentityManifest` — the same domain type
produced by both `BuiltinIdentityLoader` and
[`FileIdentityLoader`](./pluggable-identity-api.md#fileidentityloader), so
posture resolution is orthogonal to the loading mechanism.

### `Default`

```rust
impl Default for IdentityAuthority {
    fn default() -> Self {
        Self {
            posture: WritePosture::Full,
            allowed_write_repos: Vec::new(),
            allow_git_push: true,
            allow_ado_writes: true,
            allow_github_writes: true,
        }
    }
}
```

The default is **`Full`** so built-in identities and TOML identities that omit
`[identities.authority]` behave exactly as before this feature (backward
compatible).

### Posture semantics

| `posture` | `allow_*` fields | `allowed_write_repos` |
|-----------|------------------|-----------------------|
| `ReadOnly` | default to `false` and may be omitted; setting any `allow_*_writes = true` under `read-only` is a **hard parse error** (rejected, never silently coerced) | must be empty; non-empty is a hard parse error |
| `ScopedWrite` | honored per field | write is permitted **only** for repos whose canonical URL/slug is in the allowlist; all others refused |
| `Full` | honored per field (default all `true`) | ignored |

The `allow_*` **default is posture-dependent**: when `[identities.authority]`
is omitted entirely the posture is `Full` and the `allow_*` fields default to
`true` (the `Default` impl above); when the block is present with
`posture = "read-only"` the `allow_*` fields default to `false`. A read-only
manifest may thus omit the `allow_*` lines entirely — but it may not set them
to `true`.

`WritePosture` is resolved once at identity load time and threaded into the
guardrails; a running session cannot raise its own posture.

---

## `identity.toml` surface

```toml
[[identities]]
name = "crocutus"
default_mode = "engineer"

# Optional. Omit for the default `full` posture.
[identities.authority]
posture = "read-only"          # "read-only" | "scoped-write" | "full"
allowed_write_repos = []       # allowlist; only meaningful for "scoped-write"
allow_git_push = false
allow_ado_writes = false
allow_github_writes = false
```

The struct is deserialized with `deny_unknown_fields`, matching every other
[pluggable-identity TOML type](./pluggable-identity-api.md); an unexpected key
is a hard `IdentityTomlParseError`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `posture` | string | `"full"` | `"read-only"`, `"scoped-write"`, or `"full"`. |
| `allowed_write_repos` | `[string]` | `[]` | Repo URLs/slugs writable under `scoped-write`. Must be empty unless `posture = "scoped-write"`. |
| `allow_git_push` | bool | `true` (`full`) / `false` (`read-only`) | Permit `git push`/`git commit`. Under `read-only` it defaults to `false` and setting `true` is a parse error. |
| `allow_ado_writes` | bool | `true` (`full`) / `false` (`read-only`) | Permit Azure DevOps write verbs (push, PR create, work-item edit). Under `read-only` it defaults to `false` and setting `true` is a parse error. |
| `allow_github_writes` | bool | `true` (`full`) / `false` (`read-only`) | Permit GitHub writes (PR create, issue edit, comment). Under `read-only` it defaults to `false` and setting `true` is a parse error. |

---

## Posture-aware guardrails

Posture is enforced at the **existing** guardrail seams; there is no parallel
enforcement system.

### `check_git_safety`

Module: `simard::git_guardrails`

```rust
pub fn check_git_safety(workspace: &Path, args: &[&str]) -> Result<(), String>;

pub fn check_git_safety_with_authority(
    workspace: &Path,
    args: &[&str],
    authority: &IdentityAuthority,
) -> Result<(), String>;
```

`check_git_safety` retains its historical signature and behavior (blocks the
destructive `BLOCKED_PATTERNS` and, on a protected repo, restricts to the safe
command list). `check_git_safety_with_authority` layers posture on top:

| Posture | Added behavior over the existing destructive/protected checks |
|---------|---------------------------------------------------------------|
| `ReadOnly` | Refuses `push`, `commit`, `merge`, `tag -a`, `am`, `apply`, and any other mutating verb — returns `Err` with `GUARDRAIL BLOCKED (read-only): ...` |
| `ScopedWrite` | Allows `push`/`commit` only when `workspace` maps to a repo in `allowed_write_repos`; otherwise refuses |
| `Full` | No change — existing destructive/protected checks only |

Read verbs (`status`, `log`, `diff`, `show`, `fetch`, `rev-parse`, ...) are
never blocked by posture.

### `check_ado_acl_safety`

Module: `simard::ado_acl_guard`

```rust
pub fn check_ado_acl_safety(args: &[&str]) -> Result<(), String>;

pub fn check_ado_write_safety(
    args: &[&str],
    authority: &IdentityAuthority,
) -> Result<(), String>;
```

`check_ado_acl_safety` is unchanged — it still fails closed on
[ACL self-escalation](./ado-acl-self-escalation-guard.md). `check_ado_write_safety`
generalizes it to posture:

| Posture | Behavior |
|---------|----------|
| `ReadOnly` | Refuses **all** Azure DevOps write verbs — `az repos pr create`, `az repos ...` writes, `az boards work-item update/create`, `git push` to an `dev.azure.com` remote, and every ACL mutation. Read verbs (`show`, `list`, `GET`, read-only clone) pass. |
| `ScopedWrite` | ADO writes allowed only for repos in `allowed_write_repos`; ACL self-escalation still refused unless `SIMARD_ALLOW_ADO_ACL_ESCALATION` is set (see the [ADO ACL guard](./ado-acl-self-escalation-guard.md#configuration)). |
| `Full` | Existing `check_ado_acl_safety` behavior only. |

Detection **fails closed**, inheriting the ADO guard's rule that a command
targeting a write surface is treated as a write unless it is *provably* a read
(explicit `GET`/`HEAD`/`OPTIONS` and no body). See
[what is detected as an ACL mutation](./ado-acl-self-escalation-guard.md#what-is-detected-as-an-acl-mutation).

### GitHub write path

The GitHub client refuses PR creation, issue edits, comments, and label/branch
mutations when `posture == ReadOnly` (or the target is outside
`allowed_write_repos` under `scoped-write`), returning a visible error rather
than a silent no-op. Read calls (list issues, read PRs) are unaffected.

### Dispatch-layer enforcement (ACT phase)

Threading posture into the three guardrail functions is necessary but **not
sufficient**. The OODA **ACT** phase (`dispatch_spawn_engineer` and the other
`ooda_actions` dispatchers) can spawn engineer worktrees and sub-agents that
themselves shell out to `git` / `az` / `gh`. The contract's invariant is that
**every write path routes through a posture-aware seam**, and under `read-only`
the dispatchers **short-circuit**: they record proposed goals but dispatch zero
write-bearing actions. Concretely, `dispatch_spawn_engineer` (and the git write
sites in `self_improve_executor::git_ops` and `overseer::conflict`) consult the
resolved `IdentityAuthority` and refuse, fail-closed, before any subprocess is
launched. This is what produces `dispatched 0 actions (read-only), 0 writes` in
the OODA cycle summary. Implementations MUST verify this end-to-end, not only
at the guardrail-function unit level.

---

## Composition

Posture composes like [`memory_policy`](./pluggable-identity-api.md). When a
composite identity merges components:

- **All components must agree on `posture`.** A mismatch (e.g. one `read-only`,
  one `full`) is a hard `InvalidIdentityComposition` error. You cannot dilute a
  read-only component by composing it with a full one.
- `allowed_write_repos` under `scoped-write` is the **intersection** of the
  components' allowlists (the most restrictive wins).

---

## Error variants

| Condition | Error |
|-----------|-------|
| `posture` is not one of the three known values | `IdentityTomlParseError` |
| `allow_*_writes = true` while `posture = "read-only"` | `IdentityTomlParseError` (contradiction) |
| `allowed_write_repos` non-empty while `posture` ≠ `"scoped-write"` | `IdentityTomlParseError` |
| Unknown field in `[identities.authority]` | `IdentityTomlParseError` (`deny_unknown_fields`) |
| Components disagree on `posture` | `InvalidIdentityComposition` |
| Read-only identity attempts a git write | `Err("GUARDRAIL BLOCKED (read-only): ...")` from `check_git_safety_with_authority` |
| Read-only identity attempts an ADO write | `Err("GUARDRAIL BLOCKED (read-only): ...")` from `check_ado_write_safety` |
| Read-only identity attempts a GitHub write | `Err` from the GitHub client |

All enforcement errors are **hard and visible** (fail-closed); none degrade to
a silent no-op.

---

## `simard debug authority`

Prints the resolved write-authority posture for the selected identity, without
performing any writes — the posture analogue of
[`simard debug instance`](./agent-instance-isolation.md#simard-debug-instance).

```bash
SIMARD_IDENTITY=crocutus \
SIMARD_IDENTITY_PATH="$CROCUTUS/identity" \
SIMARD_PROMPT_ROOT="$CROCUTUS" \
simard debug authority
```

```
identity=crocutus
posture=read-only
allow_git_push=false
allow_ado_writes=false
allow_github_writes=false
allowed_write_repos=[]
git_push_check=REFUSED (read-only)
ado_write_check=REFUSED (read-only)
github_write_check=REFUSED (read-only)
```

### Dry-run refusal probe

`simard debug authority --probe-write <target>` performs a **dry-run** of the
git/ADO/GitHub write checks against a target and reports whether each would be
refused, executing nothing. Use it as the machine-checkable half of the
[read-only guardrail proof](../tutorials/deploy-crocutus-read-only-observer.md#step-6-prove-the-read-only-guardrail):

```bash
simard debug authority --probe-write \
  https://dev.azure.com/acs-mdash/acs-mdash/_git/hyenas
```

```
probe target=https://dev.azure.com/acs-mdash/acs-mdash/_git/hyenas
git push        => REFUSED (read-only)
az repos pr     => REFUSED (read-only)
work-item edit  => REFUSED (read-only)
exit=0 (all writes refused as expected)
```

The command exits non-zero if **any** probed write would be *allowed*, so it
can gate CI/acceptance.

---

## See also

- [Write-authority posture](../concepts/write-authority-posture.md) — design
  rationale and the four-layer defense-in-depth for a read-only identity.
- [Pluggable identity API reference](./pluggable-identity-api.md) — the loader
  and TOML types that carry `IdentityAuthority`.
- [Azure DevOps ACL self-escalation guard](./ado-acl-self-escalation-guard.md)
  — the pre-existing fail-closed guard that posture generalizes.
- [Agent instance-isolation reference](./agent-instance-isolation.md) — the
  per-instance isolation that pairs with posture.
