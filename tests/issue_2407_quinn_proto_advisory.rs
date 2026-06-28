//! TDD acceptance guards for issue #2407 — RUSTSEC-2026-0185.
//!
//! Advisory: `quinn-proto` < 0.11.15 is vulnerable to remote memory
//! exhaustion (RUSTSEC-2026-0185). The directed fix is to bump the dependency
//! *out of the vulnerable range* via the lockfile — NOT to add a blanket
//! `cargo audit` ignore.
//!
//! # What these guards encode (verify-and-close contract)
//!
//! #2407 is a **verify-and-close** item: `main`'s `Cargo.lock` already pins
//! `quinn-proto 0.11.15` (the first fixed release), so these guards are
//! expected to be GREEN on the fixed tree. They are written as regression
//! guards so that:
//!
//!   1. A future `cargo update` that re-introduces a `quinn-proto < 0.11.15`
//!      transitively turns the lockfile red here (not just in nightly
//!      `cargo audit`), and
//!   2. Nobody "resolves" the advisory by silencing it: RUSTSEC-2026-0185 must
//!      never appear in `.cargo/audit.toml`'s ignore list.
//!
//! These read the raw manifests (rg-shaped) so an operator running the same
//! `grep` gets the same answer. They do not import the crate, so they stay
//! decoupled from the heavy `simard` build.

use std::fs;
use std::path::PathBuf;

/// First `quinn-proto` release that is OUT of the RUSTSEC-2026-0185 vulnerable
/// range (`>= 0.11.15`).
const QUINN_PROTO_FIXED: (u64, u64, u64) = (0, 11, 15);

/// The advisory id that must be resolved by the lockfile pin, never silenced.
const ADVISORY_ID: &str = "RUSTSEC-2026-0185";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {} ({e})", path.display()))
}

/// Parse an `x.y.z` semver core into a comparable tuple. Pre-release/build
/// metadata (after `-`/`+`) is ignored — sufficient for advisory range checks.
fn parse_semver(v: &str) -> (u64, u64, u64) {
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let mut parts = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Return every locked `version` for a given `[[package]]` `name` in
/// `Cargo.lock`. A crate may legitimately appear more than once (multiple
/// major lines); ALL occurrences must be out of the vulnerable range.
fn locked_versions(lockfile: &str, crate_name: &str) -> Vec<String> {
    let needle = format!("name = \"{crate_name}\"");
    let mut versions = Vec::new();
    let mut lines = lockfile.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != needle {
            continue;
        }
        // The `version = "..."` line follows `name = "..."` within the same
        // `[[package]]` table. Scan forward until we hit it or the next table.
        for following in lines.by_ref() {
            let t = following.trim();
            if let Some(rest) = t.strip_prefix("version = \"") {
                if let Some(end) = rest.find('"') {
                    versions.push(rest[..end].to_string());
                }
                break;
            }
            if t.starts_with("[[package]]") {
                break; // malformed entry without a version; stop scanning.
            }
        }
    }
    versions
}

// ── #2407 primary contract: lockfile pins quinn-proto out of range ─────────

#[test]
fn quinn_proto_locked_out_of_rustsec_2026_0185_range() {
    let lock = read_repo_file("Cargo.lock");
    let versions = locked_versions(&lock, "quinn-proto");

    assert!(
        !versions.is_empty(),
        "quinn-proto not found in Cargo.lock — update this guard if the dep \
         was removed entirely (which also resolves RUSTSEC-2026-0185)"
    );

    for v in &versions {
        let parsed = parse_semver(v);
        assert!(
            parsed >= QUINN_PROTO_FIXED,
            "quinn-proto {v} is inside the RUSTSEC-2026-0185 vulnerable range \
             (< {}.{}.{}). Bump it out of range via the lockfile \
             (`cargo update -p quinn-proto`) — do NOT add an audit ignore.",
            QUINN_PROTO_FIXED.0,
            QUINN_PROTO_FIXED.1,
            QUINN_PROTO_FIXED.2,
        );
    }
}

// ── #2407 anti-suppression contract: never silence the advisory ────────────

#[test]
fn rustsec_2026_0185_is_not_blanket_ignored() {
    // The audit config is optional; absence trivially satisfies the contract.
    let path = repo_root().join(".cargo/audit.toml");
    let Ok(contents) = fs::read_to_string(&path) else {
        return;
    };

    assert!(
        !contents.contains(ADVISORY_ID),
        "{ADVISORY_ID} must not appear in .cargo/audit.toml — #2407 requires \
         resolving the advisory by bumping quinn-proto out of the vulnerable \
         range, not by suppressing `cargo audit`."
    );
}

#[test]
fn cargo_audit_workflow_runs_without_ignore_flag() {
    // The `cargo-audit` CI job must run a bare `cargo audit` (config-driven),
    // not paper over RUSTSEC-2026-0185 with an inline `--ignore` flag.
    let verify = read_repo_file(".github/workflows/verify.yml");
    assert!(
        verify.contains("cargo audit"),
        "verify.yml lost its `cargo audit` job — the advisory gate for #2407 \
         must stay wired."
    );
    assert!(
        !verify.contains(&format!("--ignore {ADVISORY_ID}"))
            && !verify.contains("--ignore RUSTSEC"),
        "cargo audit must not be invoked with an inline `--ignore` flag in \
         verify.yml — resolution for #2407 is the lockfile pin, not a CLI \
         suppression."
    );
}
