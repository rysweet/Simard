//! src/journal/tests_secrets.rs
//!
//! Tests (issue #2606): `scrub_secrets` redacts token / key / PEM-shaped
//! substrings while leaving ordinary prose intact, and
//! `JournalGenerator::generate` applies it as an **unconditional post-pass**
//! over the reviewed narrative — so a secret survives neither the offline
//! reviewer nor a passthrough reviewer (standing in for a language-model
//! reviewer whose output is never trusted to be secret-free on its own).
//!
//! Specifies the TARGET behaviour: `scrub_secrets` does not exist in the pre-fix
//! #2618 build and `generate` performs no secret redaction.
//!
//! The fixtures use the repository's clearly-fake credential shapes (an
//! `EXAMPLE_FAKE_..._do_not_use` token and a PEM whose markers are assembled
//! from fragments at runtime) so no literal credential is ever committed.

use super::test_support::day;
use crate::journal::generate::{JournalDrafter, JournalGenerator, JournalReviewer};
use crate::journal::jargon::scrub_secrets;
use crate::journal::types::DayContext;

/// A GitHub-personal-access-token-shaped, obviously-fake secret (`ghp_` prefix
/// plus a 32-character `EXAMPLE_FAKE_..._do_not_use` body).
const GH_TOKEN: &str = "ghp_EXAMPLE_FAKE_TOKEN_do_not_use_00";

#[test]
fn scrub_secrets_redacts_github_token() {
    let out = scrub_secrets(&format!("Simard authenticated with {GH_TOKEN} today."));
    assert!(!out.contains(GH_TOKEN), "the token must not survive: {out}");
    assert!(
        out.contains("Simard authenticated with"),
        "surrounding prose is preserved: {out}"
    );
}

#[test]
fn scrub_secrets_redacts_pem_private_key() {
    // Assemble the PEM markers from fragments at runtime so the source file
    // carries no literal `-----BEGIN … PRIVATE KEY-----` marker of its own.
    let begin = format!("-----BEGIN {} PRIVATE KEY-----", "OPENSSH");
    let end = format!("-----END {} PRIVATE KEY-----", "OPENSSH");
    let body = "EXAMPLEfakeKEYbodyDoNotUse000000000000";
    let pem = format!("{begin}\n{body}\n{end}");

    let out = scrub_secrets(&format!("A leaked key:\n{pem}\nend of leak."));
    assert!(!out.contains(body), "the key body must not survive: {out}");
    assert!(
        !out.contains(&begin),
        "the PEM block is redacted whole: {out}"
    );
    assert!(
        out.contains("end of leak."),
        "surrounding prose is preserved: {out}"
    );
}

#[test]
fn scrub_secrets_leaves_ordinary_prose_unchanged() {
    // No false-positive redaction of plain prose (even words like "key").
    let prose = "Simard shipped an update to the live system and reviewed a key change.";
    assert_eq!(
        scrub_secrets(prose),
        prose,
        "ordinary prose must pass through untouched"
    );
}

/// A drafter that leaks a secret.
struct LeakyDrafter;
impl JournalDrafter for LeakyDrafter {
    fn draft(&self, _day: &DayContext) -> String {
        format!("Overview. The update used the token {GH_TOKEN} to authenticate.")
    }
}

/// A reviewer that passes text straight through — stands in for a language-model
/// reviewer that might not redact secrets on its own.
struct PassthroughReviewer;
impl JournalReviewer for PassthroughReviewer {
    fn review(&self, draft: &str) -> String {
        draft.to_string()
    }
}

#[test]
fn generate_scrubs_secrets_even_when_the_reviewer_passes_them_through() {
    let generator = JournalGenerator::new(Box::new(LeakyDrafter), Box::new(PassthroughReviewer));
    let entry = generator.generate(&DayContext::new(day()));

    assert!(
        entry.draft.contains(GH_TOKEN),
        "the raw draft still carries the secret (provenance): {}",
        entry.draft
    );
    assert!(
        !entry.narrative.contains(GH_TOKEN),
        "the stored narrative must be secret-free after the unconditional post-pass: {}",
        entry.narrative
    );
}
