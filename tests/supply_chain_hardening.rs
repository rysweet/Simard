//! Failing TDD acceptance tests for supply-chain hardening
//! (issues #2260, #2261, #2262 — Step 7).
//!
//! These tests encode the acceptance criteria of the three supply-chain
//! issues as machine-checkable assertions over the repository's own files.
//! They are intentionally written **before** the enforcement artifacts exist,
//! so the suite starts RED and turns GREEN as each increment lands:
//!
//!   * #2260 (PR-A): `deny.toml` policy + `cargo-deny` CI job + build-script /
//!     proc-macro audit doc.
//!   * #2262 (PR-B): `cargo vet` baseline under `supply-chain/` + `cargo-vet`
//!     CI job + dependency-trust policy doc.
//!   * #2261 (PR-C): release-flow SBOM (CycloneDX) + cosign keyless signing +
//!     release-integrity doc + `SECURITY.md`.
//!
//! The checks are deliberately file-shaped (no network, no toolchain install)
//! so an operator running the equivalent `grep`/`cat` gets the same answer the
//! CI does. They assert *policy presence and shape*, not exact formatting, so
//! any reasonable implementation of the three PRs satisfies them.

use std::fs;
use std::path::PathBuf;

// ── Path / IO helpers ────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn path_exists(rel: &str) -> bool {
    repo_root().join(rel).exists()
}

/// Read a file that the supply-chain deliverable is required to provide.
/// Panics with an actionable message (naming the owning issue) when missing,
/// which is exactly the RED state these tests start in.
fn read_required(rel: &str, issue: &str) -> String {
    let p = repo_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "supply-chain deliverable `{rel}` ({issue}) is missing or unreadable: {e}\n\
             This file is part of the supply-chain hardening acceptance criteria; \
             create it as part of {issue}."
        )
    })
}

// ── Tiny structural matchers (std-only, comment-aware) ───────────────────────

/// True when the TOML text declares a top-level table header `[name]`
/// (ignoring leading/trailing whitespace and commented-out lines).
fn toml_has_table(contents: &str, name: &str) -> bool {
    let header = format!("[{name}]");
    contents.lines().any(|line| {
        let l = line.trim();
        !l.starts_with('#') && l == header
    })
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// True when a GitHub Actions workflow defines a top-level job `name:`
/// (jobs are two-space-indented keys under `jobs:`).
fn workflow_defines_job(contents: &str, name: &str) -> bool {
    let header = format!("  {name}:");
    contents.lines().any(|line| line.trim_end() == header)
}

fn deny_toml() -> String {
    read_required("deny.toml", "#2260")
}

fn verify_yml() -> String {
    read_required(".github/workflows/verify.yml", "#2260/#2262")
}

fn release_yml() -> String {
    read_required(".github/workflows/release.yml", "#2261")
}

fn mkdocs_yml() -> String {
    read_required("mkdocs.yml", "#2260/#2261/#2262")
}

// ─────────────────────────────────────────────────────────────────────────────
// #2260 — Build-script / proc-macro audit + cargo-deny guardrail
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn deny_toml_declares_all_required_policy_tables() {
    let deny = deny_toml();
    for table in ["advisories", "licenses", "bans", "sources"] {
        assert!(
            toml_has_table(&deny, table),
            "deny.toml (#2260) must declare a [{table}] table so cargo-deny enforces \
             advisories + licenses + bans + sources. Missing [{table}]."
        );
    }
}

#[test]
fn deny_toml_sources_allowlist_covers_every_git_dependency() {
    // Cargo.lock pulls three first-party crates over git; cargo-deny's
    // [sources] gate denies unknown git sources by default, so each must be
    // explicitly allow-listed or the guardrail fails closed on a clean tree.
    let deny = deny_toml();
    for repo in [
        "rysweet/RustyClawd",
        "rysweet/amplihack-memory-lib",
        "rysweet/amplihack-rs",
    ] {
        assert!(
            contains_ci(&deny, repo),
            "deny.toml [sources] (#2260) must allow-list the git source \
             `https://github.com/{repo}` — it is a transitive git dependency in \
             Cargo.lock and cargo-deny denies unknown git sources by default."
        );
    }
}

#[test]
fn deny_toml_ignores_the_unfixable_rsa_advisory_with_justification() {
    // RUSTSEC-2023-0071 (Marvin timing side-channel in `rsa`) has no fixed
    // upstream release and arrives transitively. The policy must ignore it so
    // `cargo deny check advisories` is green, and the ignore must be justified
    // (the same exemption already lives in .cargo/audit.toml).
    let deny = deny_toml();
    assert!(
        contains_ci(&deny, "RUSTSEC-2023-0071"),
        "deny.toml (#2260) must ignore RUSTSEC-2023-0071 (rsa, no upstream fix) so \
         `cargo deny check advisories` passes; this mirrors .cargo/audit.toml."
    );
    let rsa_line = deny
        .lines()
        .find(|l| l.contains("RUSTSEC-2023-0071"))
        .unwrap_or_default();
    let idx = deny.find("RUSTSEC-2023-0071").unwrap_or(0);
    let window = &deny[idx.saturating_sub(400)..idx];
    assert!(
        rsa_line.contains('#') || window.contains('#'),
        "the RUSTSEC-2023-0071 ignore in deny.toml (#2260) must carry a justification \
         comment (why it is unfixable / where it is tracked)."
    );
}

#[test]
fn verify_workflow_runs_cargo_deny_in_a_dedicated_job() {
    let verify = verify_yml();
    assert!(
        workflow_defines_job(&verify, "cargo-deny"),
        "verify.yml (#2260) must define a dedicated `cargo-deny` job (separate, \
         lockfile-only; not folded into the memory-sensitive pre-commit job)."
    );
    assert!(
        contains_ci(&verify, "cargo deny check"),
        "the cargo-deny job in verify.yml (#2260) must run `cargo deny check`."
    );
    assert!(
        contains_ci(&verify, "taiki-e/install-action"),
        "the cargo-deny job in verify.yml (#2260) should install cargo-deny via the \
         SHA-pinned taiki-e/install-action, matching the existing cargo-audit job."
    );
}

#[test]
fn supply_chain_audit_doc_enumerates_build_script_and_proc_macro_surface() {
    let doc = read_required("docs/reference/supply-chain-audit.md", "#2260");
    for anchor in ["build.rs", "proc-macro", "cargo-deny"] {
        assert!(
            contains_ci(&doc, anchor),
            "docs/reference/supply-chain-audit.md (#2260) must document the `{anchor}` \
             supply-chain surface."
        );
    }
    // The audit must call out the C-compiling build scripts (highest-risk
    // build-time code) by name.
    let names_present = ["libsqlite3-sys", "openssl-sys", "ring", "cc"]
        .iter()
        .filter(|n| contains_ci(&doc, n))
        .count();
    assert!(
        names_present >= 2,
        "docs/reference/supply-chain-audit.md (#2260) must name the high-risk \
         C-compiling build-script crates (e.g. libsqlite3-sys, openssl-sys, ring, cc)."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// #2262 — cargo-vet transitive-trust baseline + CI job
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cargo_vet_baseline_files_are_committed() {
    for f in [
        "supply-chain/config.toml",
        "supply-chain/audits.toml",
        "supply-chain/imports.lock",
    ] {
        assert!(
            path_exists(f),
            "{f} (#2262) must be committed — the `cargo vet init` baseline that records \
             the current dependency graph as exemptions and pins trusted imports."
        );
    }
}

#[test]
fn verify_workflow_runs_cargo_vet_in_a_dedicated_job() {
    let verify = verify_yml();
    assert!(
        workflow_defines_job(&verify, "cargo-vet"),
        "verify.yml (#2262) must define a dedicated `cargo-vet` job (separate, \
         lockfile-only)."
    );
    assert!(
        contains_ci(&verify, "cargo vet"),
        "the cargo-vet job in verify.yml (#2262) must run `cargo vet`."
    );
}

#[test]
fn dependency_trust_policy_doc_defines_exemption_criteria() {
    let doc = read_required("docs/reference/dependency-trust-policy.md", "#2262");
    for anchor in ["cargo-vet", "exemption", "trusted"] {
        assert!(
            contains_ci(&doc, anchor),
            "docs/reference/dependency-trust-policy.md (#2262) must document the \
             `{anchor}` policy (trusted-crate criteria + exemption process)."
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// #2261 — Release SBOM + cosign keyless signing + reproducibility docs
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn release_workflow_requests_oidc_id_token_permission() {
    let release = release_yml();
    assert!(
        contains_ci(&release, "id-token: write"),
        "release.yml (#2261) must grant `id-token: write` so cosign keyless \
         signing can obtain a Fulcio certificate via GitHub OIDC."
    );
}

#[test]
fn release_workflow_generates_a_cyclonedx_sbom() {
    let release = release_yml();
    assert!(
        contains_ci(&release, "cyclonedx"),
        "release.yml (#2261) must generate a CycloneDX SBOM (e.g. via cargo-cyclonedx)."
    );
    assert!(
        contains_ci(&release, ".cdx"),
        "release.yml (#2261) must attach the CycloneDX SBOM artifact \
         (e.g. simard-<version>.cdx.json) to the release."
    );
}

#[test]
fn release_workflow_signs_artifacts_with_cosign_keyless() {
    let release = release_yml();
    assert!(
        contains_ci(&release, "cosign"),
        "release.yml (#2261) must sign release artifacts with cosign."
    );
    assert!(
        contains_ci(&release, "sign-blob"),
        "release.yml (#2261) must use `cosign sign-blob` for detached keyless \
         signing of the release tarball."
    );
    assert!(
        contains_ci(&release, ".sig"),
        "release.yml (#2261) must publish the detached cosign signature (.sig) \
         alongside the release tarball."
    );
}

#[test]
fn release_workflow_signs_the_sbom_not_only_the_tarball() {
    // The SBOM is the dependency inventory consumers rely on to spot a malicious
    // or vulnerable crate. Publishing it unsigned alongside a signed tarball
    // leaves the one security-relevant artifact tamper-able, so the release must
    // sign the SBOM with the same cosign keyless flow and publish its detached
    // signature + certificate (#2261).
    let release = release_yml();
    assert!(
        contains_ci(&release, ".cdx.json.sig"),
        "release.yml (#2261) must publish a cosign signature for the SBOM \
         (simard-<version>.cdx.json.sig), not only for the tarball — otherwise \
         the dependency inventory ships with no integrity protection."
    );
    assert!(
        contains_ci(&release, ".cdx.json.pem"),
        "release.yml (#2261) must publish the SBOM signing certificate \
         (simard-<version>.cdx.json.pem) so `cosign verify-blob` can check the \
         SBOM the same way it checks the tarball."
    );
}

#[test]
fn release_integrity_doc_documents_keyless_verification() {
    let doc = read_required("docs/reference/release-integrity.md", "#2261");
    for anchor in [
        "verify-blob",
        "certificate-identity",
        "certificate-oidc-issuer",
    ] {
        assert!(
            contains_ci(&doc, anchor),
            "docs/reference/release-integrity.md (#2261) must give copy-pasteable \
             `cosign verify-blob` steps pinning `--{anchor}`."
        );
    }
}

#[test]
fn security_policy_exists_and_points_to_release_verification() {
    let security = read_required("SECURITY.md", "#2261");
    assert!(
        contains_ci(&security, "report"),
        "SECURITY.md (#2261) must explain how to report a vulnerability."
    );
    assert!(
        contains_ci(&security, "supply-chain") || contains_ci(&security, "supply chain"),
        "SECURITY.md (#2261) must summarize the supply-chain guardrails."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-cutting — docs are discoverable via mkdocs nav
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn mkdocs_nav_links_every_supply_chain_reference_page() {
    let nav = mkdocs_yml();
    for page in [
        "reference/supply-chain-audit.md",
        "reference/dependency-trust-policy.md",
        "reference/release-integrity.md",
    ] {
        assert!(
            contains_ci(&nav, page),
            "mkdocs.yml must link `{page}` from the nav so the supply-chain docs \
             are discoverable and pass `mkdocs build --strict`."
        );
    }
}
