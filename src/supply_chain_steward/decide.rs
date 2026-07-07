//! The pure remediation-decision reasoner (issue #2741).
//!
//! [`decide`] is pure, total, and I/O-free: given an advisory and pre-resolved
//! [`RemediationContext`], it returns the single [`Decision`] the execution
//! layer will carry out. This is the deterministic rail — the mapping is fixed
//! and exhaustively unit-tested (see `tests.rs`).
//!
//! ## The hard rail
//!
//! [`Decision::JustifiedIgnore`] is produced **only** from the
//! `patched == None` branch. A *fixable* advisory can never be routed to an
//! ignore — it becomes a [`Decision::Bump`] (fix applicable here) or a
//! [`Decision::Escalate`] (fix exists but not applicable here). This makes
//! "the reasoner cannot silently suppress an advisory that has a fix" a
//! statically-enforced, unit-tested property rather than a convention.
//!
//! ## Decision table
//!
//! | `patched` | resolvable patch | behind git dep | already ignored | Decision |
//! | --- | --- | --- | --- | --- |
//! | `None`  | — | — | yes | [`Decision::NoAction`] — ignore still justified |
//! | `None`  | — | — | no  | [`Decision::JustifiedIgnore`] |
//! | `Fixed` | `Some(v)` | no | any | [`Decision::Bump`] `{ to: v }` |
//! | `Fixed` | `None` | — | any | [`Decision::Escalate`] (fix not resolvable) |
//! | `Fixed` | any | yes | any | [`Decision::Escalate`] (bump belongs upstream) |

use super::types::{Advisory, Decision, PatchStatus, RemediationContext};

/// Decide the single remediation action for one advisory.
///
/// Pure and total: no I/O, no panics, every input maps to exactly one
/// [`Decision`]. See the module docs for the full decision table and the hard
/// rail that keeps a fixable advisory from ever becoming a silent ignore.
pub fn decide(advisory: &Advisory, ctx: &RemediationContext) -> Decision {
    match &advisory.patched {
        // ── No upstream fix exists ───────────────────────────────────────────
        // The ONLY branch that can yield a JustifiedIgnore. If the advisory is
        // already covered by a justified ignore in both gate files, it is still
        // justified (no fix has shipped) → nothing to do.
        PatchStatus::None => {
            if ctx.already_ignored {
                Decision::NoAction
            } else {
                Decision::JustifiedIgnore {
                    advisory_id: advisory.id.clone(),
                    crate_name: advisory.crate_name.clone(),
                    reason: format!(
                        "{id} in {krate}: no fixed upstream release is available; \
                         not reachable in Simard's usage — tracked for remediation",
                        id = advisory.id,
                        krate = advisory.crate_name,
                    ),
                }
            }
        }

        // ── A fix exists ─────────────────────────────────────────────────────
        // NEVER an ignore from here — the hard rail. `already_ignored` is
        // deliberately NOT consulted: an ignore for a now-fixable advisory is
        // stale and must be corrected (Bump / Escalate), not honoured.
        PatchStatus::Fixed { .. } => {
            if ctx.behind_git_dep {
                Decision::Escalate {
                    advisory_id: advisory.id.clone(),
                    reason: format!(
                        "{id}: a patched version exists but {krate} is reached only \
                         behind a first-party git dependency; the bump belongs in \
                         that upstream repo, not Simard's Cargo.lock",
                        id = advisory.id,
                        krate = advisory.crate_name,
                    ),
                }
            } else if let Some(to) = &ctx.resolvable_patch {
                Decision::Bump {
                    crate_name: advisory.crate_name.clone(),
                    from: advisory.installed.clone(),
                    to: to.clone(),
                }
            } else {
                Decision::Escalate {
                    advisory_id: advisory.id.clone(),
                    reason: format!(
                        "{id}: a patched version exists but no version satisfying the \
                         advisory resolves against Cargo.lock's constraints",
                        id = advisory.id,
                    ),
                }
            }
        }
    }
}
