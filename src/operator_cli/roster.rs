//! `simard roster` operator subcommands: `list`, `add <slug> [note…]`,
//! `remove <slug>`. The agentic curation surface for Simard's governed-fleet
//! roster — the set of sibling repos whose CI she sweeps and whose merge queue
//! she reasons over.
//!
//! The roster is **identity-scoped, mutable, deploy-durable state**, not a
//! committed framework file: it lives under
//! `<state_root>/identity/<identity>/curated/stewarded_repos.toml` (see
//! [`crate::identity_state`]), seeded once from Simard's identity default
//! ([`crate::overseer::ecosystem_observe::default_simard_roster_seed_toml`]) and
//! thereafter mutated only through this surface (or Simard's own reasoning). A
//! `self-deploy`/`install` swaps the binary and prompt assets but NEVER writes
//! under the state root, so a repo Simard adds here survives every redeploy.
//!
//! There is exactly ONE source of truth: the Overseer's `ecosystem-observe`
//! rail, the merge-queue reasoner, and the `ci-health` sweep all resolve this
//! same curated document. The identity/seed target is resolved from
//! `SIMARD_IDENTITY` exactly as the daemon resolves it
//! ([`crate::overseer::ecosystem_observe::daemon_identity_and_seed`]) so the CLI
//! curates the very roster the running daemon reads.
//!
//! Honours `SIMARD_STATE_ROOT` (via [`crate::state_root::simard_state_root`]).

use std::error::Error;

use crate::overseer::ecosystem_observe::{
    add_stewarded_repo, daemon_identity_and_seed, remove_stewarded_repo, resolve_stewarded_roster,
};
use crate::state_root::simard_state_root;

use super::args::{next_required, reject_extra_args};

pub(super) const ROSTER_HELP: &str = "\
Simard roster subcommand

Usage: simard roster <command> [args]

Curate Simard's governed-fleet roster — the sibling repos whose CI she sweeps
and whose merge queue she reasons over. The roster is identity-scoped, mutable,
deploy-durable state (never a committed file): it is seeded once from Simard's
identity default and survives every self-deploy. All stewards (ecosystem-observe
rail, merge-queue reasoner, ci-health sweep) read this one curated source.

Commands:
  list                         Print the resolved roster (one owner/name slug
                               per line, in document order). Seeds from the
                               identity default on first use.
  add <owner/name> [note...]   Add a repo to the roster with an optional note.
                               Idempotent (adding a listed repo is a no-op).
                               Rejects a malformed slug.
  remove <owner/name>          Remove a repo from the roster. Idempotent
                               (removing an absent repo is a no-op). Refuses to
                               remove the LAST repo — an empty roster is a
                               fail-loud error (it would report the fleet green).
  help, -h, --help             Show this help message and exit.

The target identity is resolved from SIMARD_IDENTITY (unset = Simard herself);
the state root honours SIMARD_STATE_ROOT.
";

/// Top-level `simard roster …` dispatcher.
pub(super) fn dispatch_roster_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn Error>> {
    let subcommand = next_required(&mut args, "roster command")?;
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{ROSTER_HELP}");
            Ok(())
        }
        "list" => {
            reject_extra_args(args)?;
            handle_list()
        }
        "add" => {
            let slug = next_required(&mut args, "repo slug (owner/name)")?;
            let note = args.collect::<Vec<_>>().join(" ");
            handle_add(&slug, &note)
        }
        "remove" => {
            let slug = next_required(&mut args, "repo slug (owner/name)")?;
            reject_extra_args(args)?;
            handle_remove(&slug)
        }
        other => Err(format!("unsupported command 'roster {other}'").into()),
    }
}

fn handle_list() -> Result<(), Box<dyn Error>> {
    let state_root = simard_state_root();
    let (identity, seed) = daemon_identity_and_seed();
    let roster = resolve_stewarded_roster(&state_root, &identity, &seed)
        .map_err(|reason| -> Box<dyn Error> { reason.into() })?;
    for slug in roster {
        println!("{slug}");
    }
    Ok(())
}

fn handle_add(slug: &str, note: &str) -> Result<(), Box<dyn Error>> {
    let state_root = simard_state_root();
    let (identity, seed) = daemon_identity_and_seed();
    let outcome = add_stewarded_repo(&state_root, &identity, &seed, slug, note)
        .map_err(|reason| -> Box<dyn Error> { reason.into() })?;
    println!("{}", outcome.summary);
    Ok(())
}

fn handle_remove(slug: &str) -> Result<(), Box<dyn Error>> {
    let state_root = simard_state_root();
    let (identity, seed) = daemon_identity_and_seed();
    let outcome = remove_stewarded_repo(&state_root, &identity, &seed, slug)
        .map_err(|reason| -> Box<dyn Error> { reason.into() })?;
    println!("{}", outcome.summary);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI-facing add→list→remove cycle mutates and reads the SAME curated
    /// store, proving the operator surface is durable and self-consistent. Uses
    /// an explicit tempdir state root (never the ambient `~/.simard`).
    #[test]
    fn add_then_remove_round_trips_through_the_curated_store() {
        let dir = tempfile::tempdir().unwrap();
        let state_root = dir.path();
        let identity = crate::overseer::ecosystem_observe::DEFAULT_IDENTITY_SLUG;
        let seed = crate::overseer::ecosystem_observe::default_simard_roster_seed_toml();

        let added = add_stewarded_repo(state_root, identity, seed, "octo/widget", "test repo")
            .expect("add must succeed");
        assert!(added.changed, "adding a fresh repo must change the roster");
        assert!(added.roster.iter().any(|s| s == "octo/widget"));

        // Idempotent re-add is a no-op.
        let again = add_stewarded_repo(state_root, identity, seed, "octo/widget", "")
            .expect("idempotent add must succeed");
        assert!(!again.changed, "re-adding a listed repo must be a no-op");

        let removed = remove_stewarded_repo(state_root, identity, seed, "octo/widget")
            .expect("remove must succeed");
        assert!(
            removed.changed,
            "removing a listed repo must change the roster"
        );
        assert!(!removed.roster.iter().any(|s| s == "octo/widget"));
    }

    #[test]
    fn removing_the_last_repo_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state_root = dir.path();
        let identity = "solo";
        // Seed a single-repo roster for a fresh identity.
        let seed = "schema_version = 1\n[[repo]]\nslug = \"only/one\"\nnote = \"\"\n";
        let err = remove_stewarded_repo(state_root, identity, seed, "only/one")
            .expect_err("removing the last repo must be refused");
        assert!(
            err.contains("last stewarded repo"),
            "error must explain the empty-roster fail-loud guard: {err}"
        );
    }
}
