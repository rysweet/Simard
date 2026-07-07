//! Supply-chain advisory remediation reasoner (issue #2741).
//!
//! Simard proactively adopts new RUSTSEC / cargo-deny advisory requirements
//! *before* they can retroactively block unrelated PRs. A scheduled scan (see
//! `.github/workflows/advisory-scan.yml`) tracks the advisory-DB HEAD against
//! the default branch; when it detects a new lockfile-affecting vulnerability,
//! this module decides — behind a deterministic rail — whether to open a
//! minimal-bump PR, add a justified+tracked ignore (only when no fix exists),
//! or escalate.
//!
//! ## Layout
//!
//! - [`types`] — [`Advisory`], [`PatchStatus`], [`RemediationContext`],
//!   [`Decision`].
//! - [`decide`] — the pure, total decision function (the deterministic rail).
//! - [`parse`] — [`parse_audit_json`]: `cargo audit --json` → `Vec<Advisory>`.
//! - [`config`] — [`IgnoreFiles`]: read/write the `deny.toml` +
//!   `.cargo/audit.toml` ignore lists, kept in sync.
//! - [`gh`] — [`SupplyChainGh`] trait + [`RealSupplyChainGh`] (gh/cargo/git
//!   glue) and a test fake.
//! - [`execute`] — [`execute`]: drives a [`Decision`] to completion behind the
//!   traits, enforcing the hard-rail ordering.
//!
//! `supply_chain_steward` reuses `stewardship::dedup` (issue de-dup) and
//! `stewardship::merge_authority` (green-CI-only self-merge). It does **not**
//! depend on `engineer_loop` or `self_improve`.
//!
//! See `docs/reference/supply-chain-advisory-stewardship.md` for the full
//! design (pinned PR-gate DB, scheduled scan, reasoner, Dependabot).

pub mod config;
pub mod decide;
pub mod execute;
pub mod gh;
pub mod parse;
pub mod types;

#[cfg(test)]
mod tests;

pub use config::IgnoreFiles;
pub use decide::decide;
pub use execute::{RemediationOutcome, execute};
pub use gh::{OpenedPr, PrSpec, RealSupplyChainGh, SupplyChainGh};
pub use parse::parse_audit_json;
pub use types::{Advisory, Decision, PatchStatus, RemediationContext};

#[cfg(test)]
pub use gh::FakeSupplyChainGh;
