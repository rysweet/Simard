//! `simard roster` operator subcommands: `list`, `add <slug> [note…]`,
//! `remove <slug>`. The runtime **curation surface** for Simard's stewarded-repo
//! roster — the identity-scoped, install-durable `stewarded_repos` collection in
//! [`crate::identity_curated_state`].
//!
//! Storage, seeding, and resolution already live in the framework:
//!   - [`crate::identity_curated_state`] is the generic curated-item store under
//!     `<state_root>/identity-state/<identity>/<collection>.toml` (never
//!     overwritten by `install`, which only replaces `prompt_assets`).
//!   - [`crate::overseer::ecosystem_observe::load_stewarded_roster_from_env`]
//!     resolves the roster, seeding it on first use from the committed identity
//!     seed `prompt_assets/simard/identity/stewarded_repos.seed.toml`.
//!
//! This module adds the missing operator/agent verb: a reachable way to actually
//! *curate* that durable state at runtime (add a repo Simard should steward, drop
//! one she should not), so the roster is genuinely "who Simard is" — mutable and
//! agentically owned — not a static committed file. Every mutation goes through
//! the same `identity_curated_state` primitives the framework already ships, so a
//! curated edit survives re-installs and re-deploys.
//!
//! Subcommand semantics:
//!   - `roster list`          — print the durable roster (key + note) for the
//!     active identity. Seeds on first use so a fresh install shows the seeded
//!     roster, not an empty set.
//!   - `roster add <slug> [note…]` — upsert an `owner/name` repo (validated with
//!     [`crate::overseer::ecosystem_observe::is_valid_slug`] BEFORE any I/O, so a
//!     malformed slug can never reach the store or `gh`). Seeds first so `add`
//!     augments the full seeded roster on a fresh install.
//!   - `roster remove <slug>` — drop a repo (idempotent; a no-op if absent).
//!     Seeds first so a removal on a fresh install acts on the full seeded set.
//!
//! Honours `SIMARD_IDENTITY` (via
//! [`crate::identity_curated_state::active_identity`]) and `SIMARD_STATE_ROOT`
//! (via the store's path resolution), so curation and observation always agree on
//! the same durable file.

use std::error::Error;

use crate::identity_curated_state::{self, CuratedItem};
use crate::overseer::ecosystem_observe::{
    STEWARDED_REPOS_COLLECTION, is_valid_slug, load_stewarded_roster_from_env,
};

use super::args::{next_required, reject_extra_args};

pub(super) const ROSTER_HELP: &str = "\
Simard roster subcommand — curate the identity-scoped stewarded-repo roster

Usage: simard roster <command> [args]

Commands:
  list                     Print the durable roster (owner/name + note) for the
                           active identity. Seeds from the committed identity
                           seed on first use.
  add <owner/name> [note…] Add or update a stewarded repo (upsert by slug). The
                           slug is validated as a clean 'owner/name' before any
                           write, so it can never reach the store or `gh`.
  remove <owner/name>      Drop a stewarded repo (idempotent; no-op if absent).

The roster is identity-scoped mutable state under
<state_root>/identity-state/<identity>/stewarded_repos.toml — install never
overwrites it, so curated edits are durable across re-installs and re-deploys.
Honours SIMARD_IDENTITY and SIMARD_STATE_ROOT.
";

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
            let note = args.collect::<Vec<String>>().join(" ");
            handle_add(&slug, note.trim())
        }
        "remove" => {
            let slug = next_required(&mut args, "repo slug (owner/name)")?;
            reject_extra_args(args)?;
            handle_remove(&slug)
        }
        other => Err(format!("unknown roster command: {other}\n\n{ROSTER_HELP}").into()),
    }
}

/// Ensure the durable roster exists, seeding from committed identity data on
/// first use. A no-op once the durable file is present (so an intentionally
/// emptied roster is never re-seeded).
fn ensure_seeded(identity: &str) -> Result<(), Box<dyn Error>> {
    if identity_curated_state::load(STEWARDED_REPOS_COLLECTION, identity, None)?.is_none() {
        // Routes through `load_or_seed`, which persists the committed seed.
        load_stewarded_roster_from_env()?;
    }
    Ok(())
}

fn handle_list() -> Result<(), Box<dyn Error>> {
    let identity = identity_curated_state::active_identity();
    ensure_seeded(&identity)?;
    let collection = identity_curated_state::load(STEWARDED_REPOS_COLLECTION, &identity, None)?
        .unwrap_or_default();

    if collection.items.is_empty() {
        println!("(stewarded roster is empty for identity '{identity}')");
        return Ok(());
    }

    println!(
        "Stewarded roster for identity '{identity}' ({} repos):",
        collection.items.len()
    );
    for item in &collection.items {
        // Flag any persisted-but-malformed slug so an operator can see why the
        // observer would skip it (it never reaches `gh`).
        let flag = if is_valid_slug(item.key.trim()) {
            " "
        } else {
            "!"
        };
        if item.note.trim().is_empty() {
            println!("  {flag} {}", item.key);
        } else {
            println!("  {flag} {} — {}", item.key, item.note);
        }
    }
    Ok(())
}

fn handle_add(slug: &str, note: &str) -> Result<(), Box<dyn Error>> {
    let slug = slug.trim();
    if !is_valid_slug(slug) {
        return Err(format!(
            "invalid repo slug {slug:?}: expected a clean 'owner/name' \
             (only [A-Za-z0-9._-] per segment, exactly one '/', no '..', no leading '-')"
        )
        .into());
    }
    let identity = identity_curated_state::active_identity();
    ensure_seeded(&identity)?;
    let updated = identity_curated_state::add_item(
        STEWARDED_REPOS_COLLECTION,
        &identity,
        CuratedItem::new(slug, note),
        None,
    )?;
    println!(
        "roster: stewarding {slug} (identity '{identity}', {} repos total)",
        updated.items.len()
    );
    Ok(())
}

fn handle_remove(slug: &str) -> Result<(), Box<dyn Error>> {
    let slug = slug.trim();
    let identity = identity_curated_state::active_identity();
    ensure_seeded(&identity)?;
    let updated =
        identity_curated_state::remove_item(STEWARDED_REPOS_COLLECTION, &identity, slug, None)?;
    println!(
        "roster: no longer stewarding {slug} (identity '{identity}', {} repos total)",
        updated.items.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch(argv: &[&str]) -> Result<(), Box<dyn Error>> {
        dispatch_roster_command(argv.iter().map(|s| s.to_string()))
    }

    #[test]
    fn help_is_accepted_and_succeeds() {
        for flag in ["--help", "-h", "help"] {
            assert!(dispatch(&[flag]).is_ok(), "roster {flag} should succeed");
        }
    }

    #[test]
    fn missing_subcommand_errors() {
        let err = dispatch(&[]).unwrap_err().to_string();
        assert!(err.contains("expected roster command"), "got: {err}");
    }

    #[test]
    fn unknown_subcommand_errors_with_help() {
        let err = dispatch(&["frobnicate"]).unwrap_err().to_string();
        assert!(
            err.contains("unknown roster command: frobnicate"),
            "got: {err}"
        );
        assert!(
            err.contains("Usage: simard roster"),
            "help should be echoed: {err}"
        );
    }

    #[test]
    fn add_requires_a_slug() {
        let err = dispatch(&["add"]).unwrap_err().to_string();
        assert!(err.contains("expected repo slug"), "got: {err}");
    }

    #[test]
    fn remove_requires_a_slug() {
        let err = dispatch(&["remove"]).unwrap_err().to_string();
        assert!(err.contains("expected repo slug"), "got: {err}");
    }

    // Slug validation runs BEFORE any state I/O, so these are hermetic — they
    // never touch SIMARD_STATE_ROOT and cannot race a parallel test.
    #[test]
    fn add_rejects_a_malformed_slug_before_touching_state() {
        for bad in [
            "noslash",
            "owner/name/extra",
            "owner/",
            "/name",
            "a b/c",
            "../etc/passwd",
        ] {
            let err = dispatch(&["add", bad]).unwrap_err().to_string();
            assert!(
                err.contains("invalid repo slug"),
                "slug {bad:?} should be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn list_rejects_trailing_arguments() {
        let err = dispatch(&["list", "extra"]).unwrap_err().to_string();
        assert!(err.contains("unexpected trailing arguments"), "got: {err}");
    }
}
