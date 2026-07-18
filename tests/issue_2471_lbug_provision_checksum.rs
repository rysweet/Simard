//! Regression guard for issue #2471 — `scripts/provision-lbug-prebuilt.sh` must
//! verify a pinned SHA-256 for the prebuilt `liblbug` static archive **before**
//! it is extracted and linked into every CI build.
//!
//! # The gap this pins
//!
//! The provisioner downloads a version-pinned `liblbug-static-*.tar.gz` release
//! asset and statically links it. TLS + version/repo pinning authenticate the
//! transport and the URL, but nothing verified the *content*: a release asset
//! tampered with at rest would be linked unnoticed (Security follow-up filed
//! from PR #2469). The fix pins each asset's SHA-256 in the code-reviewed
//! manifest `scripts/lbug-prebuilt.sha256` and refuses (fail-closed) to extract
//! any tarball whose hash does not match — an unknown version/asset is rejected,
//! never trusted.
//!
//! These are raw-text contract checks (parity with
//! `tests/issue_2423_coverage_lbug_provisioning.rs`) plus a behavioural check
//! that sources the script and exercises the real `verify_sha256` gate with a
//! throwaway manifest (no network).

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {} ({e})", path.display()))
}

fn provision_script() -> String {
    read_repo_file("scripts/provision-lbug-prebuilt.sh")
}

fn checksum_manifest() -> String {
    read_repo_file("scripts/lbug-prebuilt.sha256")
}

/// Resolve the `lbug` version the provisioner will actually fetch, the same way
/// the script does: from Cargo.lock's resolved `[[package]] name = "lbug"`.
fn resolved_lbug_version() -> String {
    let lock = read_repo_file("Cargo.lock");
    let mut in_lbug = false;
    for block in lock.split("[[package]]") {
        if block.contains("name = \"lbug\"") {
            in_lbug = true;
            for line in block.lines() {
                if let Some(v) = line
                    .trim()
                    .strip_prefix("version = \"")
                    .and_then(|rest| rest.strip_suffix('"'))
                {
                    return v.to_string();
                }
            }
        }
    }
    assert!(
        in_lbug,
        "Cargo.lock has no `lbug` package to resolve a version"
    );
    panic!("Cargo.lock `lbug` package has no version line");
}

// ── The provisioner verifies a pinned checksum before extraction ────────────

#[test]
fn provision_script_verifies_sha256_against_pinned_manifest() {
    let s = provision_script();
    assert!(
        s.contains("sha256sum"),
        "provision-lbug-prebuilt.sh must compute a SHA-256 of the downloaded \
         tarball (via `sha256sum`) so the asset content is verified, not just \
         the transport (#2471)."
    );
    assert!(
        s.contains("lbug-prebuilt.sha256") || s.contains("CHECKSUM_MANIFEST"),
        "provision-lbug-prebuilt.sh must compare against the code-reviewed \
         checksum manifest (scripts/lbug-prebuilt.sha256) — the trusted, \
         in-repo content-integrity anchor (#2471)."
    );
}

#[test]
fn checksum_verification_runs_before_extraction() {
    let s = provision_script();
    let verify_at = s
        .find("verify_sha256 \"$tmp/$asset\"")
        .expect("download_prebuilt must call verify_sha256 on the downloaded asset (#2471)");
    let extract_at = s
        .find("tar xzf \"$tmp/$asset\"")
        .expect("download_prebuilt must still extract the asset with `tar xzf`");
    assert!(
        verify_at < extract_at,
        "the SHA-256 verification must run BEFORE `tar xzf` so a tampered/corrupt \
         archive is rejected before it is ever extracted or linked (#2471)."
    );
}

#[test]
fn checksum_verification_is_fail_closed_on_missing_pin() {
    let s = provision_script();
    // The `[ -z "$want" ]` branch (no pinned digest found) must refuse, not
    // silently proceed — an unknown version/asset is never implicitly trusted.
    assert!(
        s.contains("no pinned SHA-256 for") && s.contains("return 1"),
        "verify_sha256 must fail-closed: when the manifest has no digest for the \
         resolved (version, asset) it must refuse (return 1), not link an \
         unverified prebuilt (#2471)."
    );
}

#[test]
fn checksum_verification_stays_version_pinned() {
    let s = provision_script();
    // The download URL must stay pinned to an explicit `v$version` release tag
    // (Cargo.lock-derived), so a given checkout always fetches the exact asset
    // its pinned checksum was recorded for. (A bare `releases/latest` fetch,
    // which the script's header explicitly rejects, would break that contract.)
    assert!(
        s.contains("releases/download/v$version/$asset"),
        "the prebuilt download URL must stay version-pinned \
         (`releases/download/v$version/$asset`) so the pinned SHA-256 always \
         corresponds to the fetched asset (#2471)."
    );
}

// ── The manifest pins the asset CI actually consumes ────────────────────────

#[test]
fn manifest_pins_resolved_version_ci_asset() {
    let manifest = checksum_manifest();
    let version = resolved_lbug_version();
    // ubuntu-latest x86_64 with the default `compat` variant — the asset the
    // `verify`/`coverage` jobs download and link.
    let asset = "liblbug-static-linux-x86_64-compat.tar.gz";

    let row = manifest
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .find(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            cols.len() >= 3 && cols[1] == version && cols[2] == asset
        })
        .unwrap_or_else(|| {
            panic!(
                "scripts/lbug-prebuilt.sha256 must pin a SHA-256 for the CI-used \
                 asset `{asset}` at the Cargo.lock-resolved lbug version \
                 {version} (#2471); regenerate the manifest on a version bump."
            )
        });

    let digest = row.split_whitespace().next().unwrap();
    assert_eq!(
        digest.len(),
        64,
        "pinned digest for {asset}@{version} must be a 64-hex SHA-256, got `{digest}`"
    );
    assert!(
        digest.chars().all(|c| c.is_ascii_hexdigit()),
        "pinned digest for {asset}@{version} must be hex, got `{digest}`"
    );
}

// ── Behavioural: the real verify gate accepts a match, refuses otherwise ────

#[test]
fn verify_sha256_gate_accepts_match_and_refuses_mismatch_and_missing() {
    let dir = std::env::temp_dir().join(format!("lbug-2471-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("mktemp dir");
    let asset = dir.join("asset.tar.gz");
    fs::write(&asset, b"pretend-tarball-bytes\n").expect("write asset");

    // Compute the real digest and pin it in a throwaway manifest.
    let out = Command::new("sha256sum")
        .arg(&asset)
        .output()
        .expect("run sha256sum");
    assert!(out.status.success(), "sha256sum failed");
    let digest = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    let manifest = dir.join("manifest.sha256");
    fs::write(&manifest, format!("{digest}  9.9.9  asset.tar.gz\n")).expect("write manifest");

    let script = repo_root().join("scripts/provision-lbug-prebuilt.sh");

    let run = |version: &str, name: &str| -> bool {
        Command::new("bash")
            .arg("-c")
            .arg(format!(
                "set -euo pipefail; export LBUG_CHECKSUM_MANIFEST={m}; \
                 source {s}; verify_sha256 {a} {v} {n}",
                m = manifest.display(),
                s = script.display(),
                a = asset.display(),
                v = version,
                n = name,
            ))
            .status()
            .expect("run bash verify_sha256")
            .success()
    };

    assert!(
        run("9.9.9", "asset.tar.gz"),
        "verify_sha256 must accept an asset whose hash matches the pinned digest"
    );

    // Mismatch: tamper the asset after pinning.
    fs::write(&asset, b"tampered-bytes\n").expect("tamper asset");
    assert!(
        !run("9.9.9", "asset.tar.gz"),
        "verify_sha256 must refuse an asset whose hash no longer matches the pin"
    );

    // Missing pin: unknown (version, asset) is fail-closed.
    assert!(
        !run("0.0.0", "unknown.tar.gz"),
        "verify_sha256 must fail-closed when the manifest has no matching pin"
    );

    let _ = fs::remove_dir_all(&dir);
}
