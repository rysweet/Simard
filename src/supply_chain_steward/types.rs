//! Data types for the supply-chain advisory remediation reasoner (issue #2741).
//!
//! These describe a single security-vulnerability advisory (as reported by
//! `cargo audit --json` against `Cargo.lock`), the pre-resolved context the
//! pure [`decide`](super::decide) function needs, and the [`Decision`] it
//! returns. Keeping the decision *inputs* and *outputs* as plain data — with
//! all I/O resolved before `decide` is called — is what makes the deterministic
//! rail unit-testable.
//!
//! See `docs/reference/supply-chain-advisory-stewardship.md` § The remediation
//! reasoner.

/// One security vulnerability reported by `cargo audit --json` against
/// `Cargo.lock`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    /// Advisory identifier, e.g. `"RUSTSEC-2026-0204"`.
    pub id: String,
    /// Affected crate name.
    pub crate_name: String,
    /// Version currently pinned in `Cargo.lock`.
    pub installed: String,
    /// Parsed `versions.patched` requirement.
    pub patched: PatchStatus,
    /// Human-readable advisory title.
    pub title: String,
    /// Canonical advisory URL (`https://rustsec.org/advisories/<id>`).
    pub url: String,
}

/// The `versions.patched` field of an advisory, parsed from cargo-audit JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchStatus {
    /// No fixed release exists (empty `patched` requirement).
    None,
    /// A patched-version requirement exists, e.g. `">= 0.9.20"`.
    Fixed {
        /// The raw semver requirement string from the advisory.
        requirement: String,
    },
}

/// Facts the pure decision needs beyond the advisory itself. The execution
/// layer resolves these (registry / lockfile lookups) *before* calling
/// [`decide`](super::decide), so `decide` stays pure and total.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RemediationContext {
    /// Lowest released version satisfying `patched` that resolves against
    /// `Cargo.lock`, if one exists. `None` when no fix is resolvable.
    pub resolvable_patch: Option<String>,
    /// True when the affected crate is reached only behind a first-party git
    /// dependency (a bump belongs in that upstream repo, not Simard's
    /// `Cargo.lock`).
    pub behind_git_dep: bool,
    /// True when a justified ignore for this advisory already exists in *both*
    /// `deny.toml` and `.cargo/audit.toml`.
    ///
    /// Interpreted together with patch status: an ignore with no upstream fix
    /// is honoured (→ [`Decision::NoAction`]), while an ignore whose fix has
    /// since shipped is *stale* and is corrected (→ [`Decision::Bump`] /
    /// [`Decision::Escalate`], with the stale entries removed downstream).
    pub already_ignored: bool,
}

/// The single remediation action chosen by [`decide`](super::decide).
///
/// The deterministic rail: the mapping from (patch status, resolvability,
/// git-dep, existing-ignore) to outcome is fixed and unit-tested. The one
/// safety property that matters most — a *fixable* advisory can never be
/// silently suppressed — is structural: [`Decision::JustifiedIgnore`] is only
/// ever produced from the no-patched-version branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A patched version exists AND resolves against `Cargo.lock` — do the
    /// minimal `cargo update -p <crate> --precise <to>`. If the advisory was
    /// previously (mis)ignored as "no fix", the bump additionally removes the
    /// now-stale ignore from both files (done by the execution layer).
    Bump {
        /// Affected crate.
        crate_name: String,
        /// Version currently in `Cargo.lock`.
        from: String,
        /// Version to pin via `cargo update --precise`.
        to: String,
    },

    /// No patched version exists AND the advisory is not exploitable in
    /// Simard's usage — file a tracking issue, THEN add a justified ignore that
    /// embeds the issue URL, to BOTH `deny.toml` and `.cargo/audit.toml`.
    ///
    /// Produced ONLY from the no-patched-version branch (the hard rail).
    JustifiedIgnore {
        /// Advisory identifier being ignored.
        advisory_id: String,
        /// Affected crate.
        crate_name: String,
        /// Deterministic base justification (the execution layer appends the
        /// tracking-issue URL before writing it to the ignore lists).
        reason: String,
    },

    /// A fix exists but cannot be applied here (semver-incompatible / not
    /// resolvable against `Cargo.lock`, or the crate is behind a first-party
    /// git dep) — file a tracking issue, open NO auto-PR, write NO ignore.
    Escalate {
        /// Advisory identifier being escalated.
        advisory_id: String,
        /// Why the existing fix cannot be auto-applied here.
        reason: String,
    },

    /// Already mitigated: an existing justified ignore for an advisory that
    /// STILL has no upstream fix — nothing to do. (An ignore whose fix has
    /// since shipped is NOT `NoAction`; it is corrected to a [`Decision::Bump`]
    /// or [`Decision::Escalate`].)
    NoAction,
}
