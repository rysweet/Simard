//! `simard roster` — inspect and curate the ACTIVE identity's governed-repo
//! roster: the durable, identity-scoped set of repos Simard stewards (the
//! observe / merge-queue rails and the CI-health sweep all scan it).
//!
//! The roster is seeded once from the identity SEED
//! ([`crate::overseer::ecosystem_observe::DEFAULT_SIMARD_GOVERNED_ROSTER`]) into
//! mutable state under the durable state root
//! (`<state_root>/identity_state/<identity>/governed_repos.toml`), which a
//! self-deploy never overwrites — so `add`/`remove` edits persist across
//! upgrades. This is the write half of "Simard curates her roster agentically".
//!
//! See `docs/reference/ecosystem-roster-resolution.md`.

use crate::identity_state::active_identity_slug;
use crate::overseer::ecosystem_observe::{
    DEFAULT_SIMARD_GOVERNED_ROSTER, RosterMutation, add_governed_repo, load_governed_roster,
    remove_governed_repo,
};
use crate::state_root::simard_state_root;

pub(super) const ROSTER_HELP: &str = "\
Simard roster subcommand

Usage:
  simard roster [list]           List the active identity's governed roster.
  simard roster add <slug> [note...]
                                 Add a stewarded repo (owner/name) to the roster.
  simard roster remove <slug>    Remove a stewarded repo from the roster.

The roster is the identity-curated set of repos Simard stewards — the single
source of truth for the ecosystem-observe rail, the agentic merge-queue
reasoner, and the ci-health sweep. It is seeded once from Simard's identity
SEED into MUTABLE state under the durable state root
(<state_root>/identity_state/<identity>/governed_repos.toml), which a
self-deploy never overwrites, so add/remove edits survive an upgrade.

The active identity is chosen by $SIMARD_IDENTITY (default: simard); the state
root by $SIMARD_STATE_ROOT (default: $HOME/.simard). Both add and remove are
idempotent.
";

/// Dispatch `simard roster [list|add|remove]` against the ACTIVE identity's
/// curated roster under the durable state root. `list` (or no subcommand)
/// prints the roster; `add`/`remove` mutate it durably and are idempotent.
pub(super) fn dispatch_roster_command(
    args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = args;
    let state_root = simard_state_root();
    let identity = active_identity_slug();

    match args.next().as_deref() {
        None | Some("list") => {
            if let Some(extra) = args.next() {
                return Err(format!("unexpected argument after 'list': {extra:?}").into());
            }
            let roster =
                load_governed_roster(&state_root, &identity, DEFAULT_SIMARD_GOVERNED_ROSTER)?;
            println!(
                "governed roster for identity '{identity}' ({} repo(s)):",
                roster.repo.len()
            );
            for entry in &roster.repo {
                if entry.note.trim().is_empty() {
                    println!("  {}", entry.slug);
                } else {
                    println!("  {}  — {}", entry.slug, entry.note);
                }
            }
            Ok(())
        }
        Some("add") => {
            let slug = args
                .next()
                .ok_or("roster add: missing <slug> (expected owner/name)")?;
            let note = args.collect::<Vec<_>>().join(" ");
            let outcome = add_governed_repo(
                &state_root,
                &identity,
                DEFAULT_SIMARD_GOVERNED_ROSTER,
                &slug,
                &note,
            )?;
            match outcome {
                RosterMutation::Added => {
                    println!("added '{slug}' to identity '{identity}' governed roster");
                }
                RosterMutation::AlreadyPresent => {
                    println!(
                        "'{slug}' is already on identity '{identity}' governed roster (no-op)"
                    );
                }
                other => {
                    return Err(format!("unexpected add outcome: {other:?}").into());
                }
            }
            Ok(())
        }
        Some("remove") => {
            let slug = args
                .next()
                .ok_or("roster remove: missing <slug> (expected owner/name)")?;
            if let Some(extra) = args.next() {
                return Err(format!("unexpected argument after slug: {extra:?}").into());
            }
            let outcome = remove_governed_repo(
                &state_root,
                &identity,
                DEFAULT_SIMARD_GOVERNED_ROSTER,
                &slug,
            )?;
            match outcome {
                RosterMutation::Removed => {
                    println!("removed '{slug}' from identity '{identity}' governed roster");
                }
                RosterMutation::NotPresent => {
                    println!("'{slug}' was not on identity '{identity}' governed roster (no-op)");
                }
                other => {
                    return Err(format!("unexpected remove outcome: {other:?}").into());
                }
            }
            Ok(())
        }
        Some(other) => {
            Err(format!("unknown roster subcommand {other:?} (expected list|add|remove)").into())
        }
    }
}
