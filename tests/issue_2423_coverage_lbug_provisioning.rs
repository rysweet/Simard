//! TDD red-phase acceptance test for issue #2423 — coverage CI must provision
//! the `lbug` native static library the same way the (now-fixed) pre-commit
//! clippy gate and `verify.yml` do (#2426 / #2427).
//!
//! # The failure this pins
//!
//! `lbug 0.17.1` caches its prebuilt `liblbug.a` inside the cargo
//! registry-source tree. CI's cargo cache evicts that archive while keeping
//! the build-script output that references it, so a fresh build fails with
//! "could not find native static library `lbug`". The fix (#2427) provisions a
//! stable copy via `scripts/provision-lbug-prebuilt.sh` and pins the link path
//! through `LBUG_LIBRARY_DIR` / `LBUG_INCLUDE_DIR`.
//!
//! `.github/workflows/coverage.yml` runs a fresh `cargo +nightly llvm-cov
//! --workspace` build and restores the same Swatinem cache, so it shares the
//! exact eviction failure mode — yet it currently lacks the provisioning step.
//! These assertions are RED until that step is added to the `coverage` job,
//! then GREEN, encoding the directed parity with `verify.yml`.
//!
//! Implemented as raw-text contract checks (rg-shaped) so an operator grepping
//! the workflow gets the same answer, and so the guard does not depend on the
//! heavy `simard`/`lbug` build to run.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {} ({e})", path.display()))
}

fn coverage_workflow() -> String {
    read_repo_file(".github/workflows/coverage.yml")
}

// ── Provisioning is wired into the coverage job ────────────────────────────

#[test]
fn coverage_workflow_invokes_lbug_provision_script() {
    let wf = coverage_workflow();
    assert!(
        wf.contains("scripts/provision-lbug-prebuilt.sh"),
        "coverage.yml must invoke scripts/provision-lbug-prebuilt.sh in the \
         coverage job (mirroring verify.yml's install-real step) so the \
         `cargo +nightly llvm-cov` build links liblbug.a deterministically \
         (#2423)."
    );
}

#[test]
fn coverage_workflow_exports_lbug_link_env() {
    let wf = coverage_workflow();
    assert!(
        wf.contains("LBUG_LIBRARY_DIR=") && wf.contains("LBUG_INCLUDE_DIR="),
        "coverage.yml must export LBUG_LIBRARY_DIR and LBUG_INCLUDE_DIR (#2423) \
         so lbug resolves the stable prebuilt link path."
    );
    assert!(
        wf.contains("GITHUB_ENV"),
        "the lbug link-path env vars must be written to $GITHUB_ENV so they \
         apply to the subsequent coverage build step, exactly like verify.yml."
    );
}

#[test]
fn coverage_workflow_evicts_stale_lbug_build_artifacts() {
    let wf = coverage_workflow();
    assert!(
        wf.contains("target/*/build/lbug-*") && wf.contains("target/*/.fingerprint/lbug-*"),
        "coverage.yml must drop stale `target/*/build/lbug-*` and \
         `target/*/.fingerprint/lbug-*` outputs before the coverage build so \
         the cached build-script output cannot reference an evicted archive \
         (#2423). Eviction must stay workspace-relative — never widen to $HOME."
    );
}

// ── Persistence: cache the prebuilt dir like verify.yml does ───────────────

#[test]
fn coverage_workflow_caches_lbug_prebuilt_dir() {
    let wf = coverage_workflow();
    assert!(
        wf.contains("simard-lbug-precommit"),
        "coverage.yml's rust-cache step should list the \
         `~/.cache/simard-lbug-precommit` prebuilt dir under cache-directories \
         (parity with verify.yml) so the provisioning is cached where the \
         cache persists."
    );
}

// ── Ordering: provision AFTER cache restore, BEFORE the coverage build ──────

#[test]
fn lbug_provision_runs_after_cache_restore_and_before_coverage_build() {
    let wf = coverage_workflow();

    let provision_at = wf
        .find("scripts/provision-lbug-prebuilt.sh")
        .expect("coverage.yml must invoke the lbug provision script (#2423)");
    let cache_at = wf.find("simard-coverage").expect(
        "coverage.yml must keep its Swatinem rust-cache step \
                 (shared-key: simard-coverage)",
    );
    // Anchor the build position on the `Run coverage` step name, not on a bare
    // `llvm-cov` substring: the workflow also has an earlier `Install
    // cargo-llvm-cov` step (which legitimately precedes provisioning), so a
    // loose `llvm-cov` match would point at the install step and mis-order the
    // check. The `Run coverage` step is the actual `cargo +nightly llvm-cov`
    // build that must see the exported LBUG_* env.
    let build_at = wf.find("Run coverage").expect(
        "coverage.yml must keep the `cargo +nightly llvm-cov` coverage \
                 build step (named `Run coverage`)",
    );

    assert!(
        cache_at < provision_at,
        "the lbug provision step must run AFTER the cargo cache restore so it \
         can evict stale cached lbug-* outputs (#2423)."
    );
    assert!(
        provision_at < build_at,
        "the lbug provision step must run BEFORE `cargo +nightly llvm-cov` so \
         LBUG_LIBRARY_DIR/LBUG_INCLUDE_DIR are exported for the build (#2423)."
    );
}

// ── Parity: coverage uses the same single-source provisioner as verify.yml ──

#[test]
fn coverage_provisioning_matches_verify_workflow_mechanism() {
    let coverage = coverage_workflow();
    let verify = read_repo_file(".github/workflows/verify.yml");

    for token in [
        "scripts/provision-lbug-prebuilt.sh",
        "LBUG_LIBRARY_DIR=",
        "LBUG_INCLUDE_DIR=",
        "target/*/build/lbug-*",
        "target/*/.fingerprint/lbug-*",
    ] {
        assert!(
            verify.contains(token),
            "verify.yml regressed: lost `{token}` from its lbug provisioning. \
             coverage.yml is supposed to mirror it (#2423/#2427)."
        );
        assert!(
            coverage.contains(token),
            "coverage.yml must mirror verify.yml's lbug provisioning but is \
             missing `{token}` (#2423)."
        );
    }
}

// ── Security: provisioning must not loosen the workflow's locked-down posture ─

#[test]
fn coverage_workflow_keeps_least_privilege_posture() {
    let wf = coverage_workflow();
    assert!(
        wf.contains("permissions: {}"),
        "coverage.yml must keep its top-level `permissions: {{}}` — the #2423 \
         provisioning step adds no new permissions or token access."
    );
    assert!(
        !wf.contains("releases/latest"),
        "lbug provisioning must stay version-pinned (Cargo.toml-derived), \
         never `releases/latest`."
    );
    assert!(
        !wf.contains("secrets."),
        "the lbug provisioning step reads only the repo and $GITHUB_ENV — it \
         must not reference any `secrets.*`."
    );
}
