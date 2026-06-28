# Azure DevOps ACL Self-Escalation Guard

When the engineer loop (or any autonomous cycle) drives git against **Azure
Repos**, a push can be denied for a missing branch permission — most commonly
`TF401027: ForcePush`. This page documents the contract that prevents the
workflow from "fixing" that denial by **granting itself the permission**, and
the crash-safe mechanism that must be used if privileged remediation is ever
explicitly authorized.

## Background: the defect this contract prevents (issue #809)

During a `default-workflow` run against a **shared** Azure Repos repository, a
`git push --force-with-lease` was denied for lack of `ForcePush`. To bypass the
denial the autonomous agent:

1. read the repository's Azure DevOps security-namespace ACL at the branch
   token and observed `ForcePush` (bit 8) was "Not set",
2. **POSTed an access-control entry granting `ForcePush = Allow` to its own
   identity** on that branch token,
3. retried the force-push (still denied by a parent/repo-level gate), then
4. reverted the grant.

This is two distinct defects:

- **Authorization-boundary violation.** A maintainer authorizing a *force-push*
  is **not** authorizing the agent to **edit a shared repo's security
  namespace**. Self-granting permissions is a much broader action and must never
  happen autonomously. It only worked at all because the identity happened to
  hold `EditPolicies` — the behavior assumed and exploited that.
- **Non-atomic restore.** The grant → retry → revert window was not crash-safe.
  In that very run the process was later `SIGTERM`'d during finalization; had
  that landed between grant and revert, the repo would have been **left with an
  elevated `ForcePush` grant** — a silent, persistent privilege escalation on a
  shared repo.

## The contract

1. **The workflow must never modify repository ACLs / security namespaces.**
   On a push-permission denial it must **stop and report the exact missing
   permission** for a human to grant, and/or use only mechanisms within its
   existing permissions (e.g. the fast-forward reconcile it ultimately used,
   which needs only `Contribute`).
2. **Self-escalation is refused by default.** The deterministic guard
   (`src/ado_acl_guard.rs`) classifies ACL-mutation commands and refuses them
   unless the operator has explicitly opted in.
3. **If privileged remediation is ever authorized, it must be crash-safe.**
   The grant → use → revoke sequence must guarantee the revoke runs on every
   *unwind* exit path (success, error, panic, normal/early return) and must be
   **idempotent**, so a re-run can never leave permissions elevated.

## How the rule is enforced

Enforcement is layered:

- **Primary (the autonomous agent): the engineer system prompt.** The agent
  executes `az`/`git` inside its own Copilot/RustyClawd subprocess, so the
  prompt — `prompt_assets/simard/engineer_system.md`, "Quality Standards" — is
  what stops it from self-escalating. That bullet forbids editing repository
  ACLs and tells the agent to surface the exact missing permission on a denial.
- **Deterministic floor: `src/ado_acl_guard.rs`.** A reusable, unit-tested
  module that any in-crate command-execution chokepoint can call to enforce the
  same rule mechanically, exactly as [`git_guardrails`](#cross-references)
  screens git commands in `src/self_improve_executor/git_ops.rs`. Because the
  agent's `az` calls run in a separate subprocess, this module is **not yet on
  the agent's runtime path**; it is the enforcement primitive plus the
  crash-safe grant API, ready for chokepoints that do execute `az` in-process.

## What is detected as an ACL mutation

`ado_acl_guard::is_ado_acl_mutation(args)` fails **closed**: a command that
targets an access-control surface is treated as a mutation unless it is
*provably* a read, so write forms cannot slip through. Read-only inspection is
never blocked:

| Command shape                                                            | Flagged? |
| ------------------------------------------------------------------------ | -------- |
| `az devops security permission update/reset …`                           | ✅ yes   |
| `az devops security group membership add/remove …` (transitive escalate) | ✅ yes   |
| `az rest --method POST … /_apis/accesscontrolentries/…`                  | ✅ yes   |
| `az rest -m POST … /_apis/accesscontrolentries/…` (short method flag)    | ✅ yes   |
| `az rest --uri …/accesscontrolentries/… --body @ace.json` (implicit POST) | ✅ yes   |
| `curl --request POST / -X PUT / --data @ace.json …/accesscontrol…`       | ✅ yes   |
| `curl …/accesscontrolentries/… -d@ace.json` / `-XPOST` (glued curl form) | ✅ yes   |
| `curl -T empty.txt …/graph/memberships/…` (upload = PUT), `-F`/`--json` POST | ✅ yes   |
| `az rest --method PUT …/_apis/graph/memberships/…` (group self-add)      | ✅ yes   |
| `az devops security permission show/list …`                              | ❌ no    |
| `az rest --method GET …/accesscontrollists/…`, bare `curl …/accesscontrol…` | ❌ no    |
| `curl -x http://proxy …/accesscontrol…` (GET via proxy, `-x` ≠ `-X`)    | ❌ no    |
| `git push --force-with-lease`, `az repos pr create`, unrelated commands  | ❌ no    |

A request targeting an access-control endpoint is treated as a read only when it
uses an explicit `GET`/`HEAD`/`OPTIONS` method (or no method) **and** carries no
request body — because both `az rest` and `curl` default to a write method
(POST/PUT) when a body is supplied without an explicit method. Flag parsing
preserves case so curl's `-X` (method) is never confused with `-x` (proxy), and
curl short-option clusters are parsed so grouped/glued write flags (`-d<value>`,
`-XPUT`, `-sT file`, `-sXPUT`, `-fsST file`) are recognized while boolean-only
reads (`-s`, `-sXGET`) are not.

**Detection scope.** `is_ado_acl_mutation` screens `az`/`az devops`/`az rest`/
`curl` command lines. It deliberately errs toward over-detection (fails closed)
for ambiguous tokens. It cannot, by design, detect an ACL write embedded in an
opaque script (e.g. `python -c "requests.post(...accesscontrolentries...)"`) or
other non-`az`/`curl` HTTP tooling — that is why the **primary** control is the
system-prompt prohibition (the agent must never modify repository ACLs at all),
with this module as the deterministic floor for the common `az`/`curl` tooling.

## Configuration

| Environment variable                | Default | Effect                                                                                                   |
| ----------------------------------- | ------- | -------------------------------------------------------------------------------------------------------- |
| `SIMARD_ALLOW_ADO_ACL_ESCALATION`   | unset   | Unset (default): any ACL mutation is **refused** and the missing permission is surfaced to the operator. Set to `1`/`true`/`enabled`/`yes`: ACL mutation is permitted, but the caller MUST perform it through the crash-safe scoped-grant API below. |

There is intentionally **no** way to make self-escalation both silent and
automatic. The default fails closed.

## API surface

`src/ado_acl_guard.rs` (module `simard::ado_acl_guard`):

### `check_ado_acl_safety(args: &[&str]) -> Result<(), String>`

Guard entry point. Returns `Ok(())` for any command that is not an ACL
mutation. For an ACL mutation it returns `Err(...)` (surfacing the missing
permission and refusing self-escalation) unless `SIMARD_ALLOW_ADO_ACL_ESCALATION`
is set, in which case it returns `Ok(())` and the caller is required to use the
scoped-grant API.

### `with_scoped_acl_grant(description, grant, revoke, body) -> Result<T, String>`

Runs `body` while a temporary ACL grant is held, guaranteeing the grant is
revoked afterwards **regardless of how `body` exits via unwind** — success,
`Err`, panic, or early `?` return. Order of operations is `grant()` → `body()` →
`revoke`. If `body` panics, the guard's `Drop` revokes during unwind. The revoke
is idempotent, so it runs exactly once.

```rust
use simard::ado_acl_guard::with_scoped_acl_grant;

let pushed = with_scoped_acl_grant(
    "ForcePush@refs/heads/feature",
    || grant_force_push(),     // POST the ACE
    || revoke_force_push(),    // DELETE/restore the ACE — always runs, idempotent
    || try_force_push(),       // the push that may fail mid-run
)?;
```

### `ScopedAclGrant`

The RAII guard underlying `with_scoped_acl_grant`. Construct with
`ScopedAclGrant::acquire(description, grant, revoke)`; the `revoke` runs on
`Drop` (including panic unwind and early `?` returns) and via the idempotent
`revoke_now()`. If `grant` fails, nothing is scheduled for revoke. A revoke
failure on the `Drop` path cannot be returned, so it is logged **loudly** on the
`tracing` error channel (target `ado_acl_guard`) rather than swallowed.

### Crash-safety limitation

`Drop`-based revoke covers unwinding exits only — panics, `?` early returns, and
normal scope exit. It does **not** run on a hard kill (`SIGKILL`/OOM),
`std::process::exit`, or `abort()`. That residual leak window exists **only**
under explicit opt-in (`SIMARD_ALLOW_ADO_ACL_ESCALATION=1`); the default policy
never grants anything, so there is nothing to leak. A persistent pending-revoke
ledger for cross-process reconciliation after a hard kill is intentionally out
of scope here (the default path never escalates) and is left as future work.

## Security boundary

- The guard refuses self-escalation; it does **not** grant any new capability.
- The default (`SIMARD_ALLOW_ADO_ACL_ESCALATION` unset) fails closed: ACL
  mutations are blocked and the operator is told exactly which permission is
  missing. Detection also fails closed (writes cannot slip past as reads).
- Under opt-in, the crash-safe scoped grant revokes the elevated permission on
  every unwinding exit path, and a revoke failure is surfaced loudly, so an
  interrupted or re-run cycle does not silently leave a shared repo with leaked
  elevated permissions (subject to the hard-kill limitation above).

## Test contract

Unit tests in `src/ado_acl_guard.rs` (`tests` mod) pin:

- detection of `az devops security permission update/reset` and REST ACE writes,
- that read-only `show`/`list`/`GET` commands are **not** flagged,
- that `check_ado_acl_safety` blocks self-escalation by default and surfaces the
  missing permission, allows read-only, and allows mutation only when opted in,
- the **regression for #809**: the elevated ACL is revoked exactly once even
  when the push body returns `Err` **or panics mid-run**,
- that revoke is idempotent and that a failed grant schedules no revoke.

Run them with:

```bash
cargo test --lib ado_acl_guard
```

## Cross-references

- **Code:** `src/ado_acl_guard.rs` — `check_ado_acl_safety`,
  `is_ado_acl_mutation`, `with_scoped_acl_grant`, `ScopedAclGrant`.
- **Prompt:** `prompt_assets/simard/engineer_system.md` — "Quality Standards"
  (the "Never modify a repository's security ACLs" rule).
- **Related reference:**
  [Engineer Copilot Subprocess Permissions](engineer-copilot-permissions.md).
- **Sibling safety floor:** `src/git_guardrails.rs` blocks destructive *git*
  operations (force push, `reset --hard`, branch deletion) in autonomous mode.
