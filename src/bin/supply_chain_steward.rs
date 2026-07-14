//! `supply-chain-steward` — the scheduled advisory-remediation driver (#2741).
//!
//! Thin entrypoint over `simard::supply_chain_steward`: it parses
//! `cargo audit --json`, resolves the pre-decision [`RemediationContext`] facts
//! (patch resolvability, git-dep boundary, existing ignore), calls the pure
//! `decide()`, and drives `execute()`.
//!
//! ```text
//! supply-chain-steward <SUBCOMMAND>
//!   scan          Run cargo audit against DB HEAD on the default branch and,
//!                 for each new vulnerability: decide → file issue → open a
//!                 bump PR / justified-ignore / escalate. When the DB is clean,
//!                 log the advanceable advisory-db.sha pin SHA. Self-merges only
//!                 its own green-CI PRs.
//!   decide-only   Parse `cargo audit --json` from stdin and print the decision
//!                 for each advisory as JSON. No side effects — for inspection.
//!   init-journal  Initialize durable mutation state only. Performs no scan or
//!                 GitHub mutation; operator reconciliation must happen first.
//! ```
//!
//! Logs go to **stderr** via `tracing`; the only **stdout** output is the
//! `decide-only` structured JSON (the command's actual result) — there are no
//! stray `println!`/`eprintln!` in the production path.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use simard::error::{SimardError, SimardResult};
use simard::stewardship::mutation_guard::MutationGuard;
use simard::stewardship::{CycleId, IssueMutationLimit};
use simard::supply_chain_steward::{
    Advisory, Decision, IgnoreFiles, PatchStatus, RealSupplyChainGh, RemediationContext, decide,
    execute, parse_audit_json,
};
use tracing::{error, info, warn};

fn main() -> ExitCode {
    init_tracing();

    let subcommand = std::env::args().nth(1);
    let result = match subcommand.as_deref() {
        Some("scan") => scan(),
        Some("decide-only") => decide_only(),
        Some("init-journal") => init_journal(),
        other => {
            error!(
                subcommand = ?other,
                "usage: supply-chain-steward <scan|decide-only|init-journal>"
            );
            return ExitCode::from(2);
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!(error = %e, "supply-chain-steward failed");
            ExitCode::from(1)
        }
    }
}

fn init_journal() -> SimardResult<()> {
    simard::stewardship::mutation_store::MutationStore::new(
        simard::stewardship::mutation_store::MutationStore::default_path(),
    )
    .initialize_empty()
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,simard::supply_chain_steward=info"));
    // Logs to stderr so stdout stays reserved for `decide-only` JSON output.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// `scan`: fetch DB HEAD, decide + remediate each new vulnerability.
fn scan() -> SimardResult<()> {
    let root = repo_root()?;
    let json = run_cargo_audit_json()?;
    let advisories = parse_audit_json(&json)?;
    let files = IgnoreFiles::at_root(&root);
    let gh = RealSupplyChainGh::from_env();
    let cycle_id = CycleId::scheduled("supply-chain-scan")?;
    let mut mutation_guard = MutationGuard::from_default_store();
    mutation_guard.begin_cycle(cycle_id.clone(), IssueMutationLimit::configured()?)?;

    if advisories.is_empty() {
        info!("no lockfile-affecting vulnerabilities against advisory-DB HEAD");
        if let Err(e) = advance_pin_best_effort(&root) {
            warn!(error = %e, "advisory-db pin advance skipped");
        }
        return Ok(());
    }

    info!(
        count = advisories.len(),
        "vulnerabilities reported against DB HEAD"
    );
    // Parse Cargo.lock's git-sourced packages ONCE for the whole sweep instead
    // of re-reading and re-parsing the lockfile inside every advisory's context
    // build (see `git_sourced_packages`).
    let git_pkgs = git_sourced_packages(&root);
    for adv in &advisories {
        let ctx = build_context(adv, &git_pkgs, &files);
        let decision = decide(adv, &ctx);
        info!(advisory = %adv.id, crate_name = %adv.crate_name, decision = ?decision, "remediation decision");
        let outcome = execute(decision, adv, &files, &gh, &mut mutation_guard, &cycle_id)?;
        info!(advisory = %adv.id, outcome = ?outcome, "remediation outcome");
    }
    Ok(())
}

/// `decide-only`: read `cargo audit --json` from stdin, print decisions as JSON.
/// No side effects; context is a best-effort, I/O-free approximation.
fn decide_only() -> SimardResult<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| {
        SimardError::SupplyChainAuditParseFailed {
            reason: format!("failed to read stdin: {e}"),
        }
    })?;
    let advisories = parse_audit_json(&input)?;

    let decisions: Vec<serde_json::Value> = advisories
        .iter()
        .map(|adv| {
            let ctx = RemediationContext {
                resolvable_patch: match &adv.patched {
                    PatchStatus::Fixed { requirement } => lowest_version_token(requirement),
                    PatchStatus::None => None,
                },
                behind_git_dep: false,
                already_ignored: false,
            };
            decision_to_json(adv, &decide(adv, &ctx))
        })
        .collect();

    let rendered = serde_json::to_string_pretty(&decisions).map_err(|e| {
        SimardError::SupplyChainAuditParseFailed {
            reason: format!("failed to render decisions: {e}"),
        }
    })?;
    // The command's structured result — written to stdout via an explicit
    // writer, not a stray print.
    let mut out = std::io::stdout().lock();
    writeln!(out, "{rendered}").map_err(|e| SimardError::SupplyChainAuditParseFailed {
        reason: format!("failed to write stdout: {e}"),
    })?;
    Ok(())
}

// ─────────────────────── context resolution (I/O glue) ───────────────────────

/// Resolve the pre-decision facts `decide()` needs. Best-effort: any lookup
/// failure degrades to the conservative value (not resolvable / not ignored),
/// which routes the advisory to `Escalate`/`JustifiedIgnore` rather than a wrong
/// bump.
fn build_context(
    adv: &Advisory,
    git_pkgs: &[(String, String)],
    files: &IgnoreFiles,
) -> RemediationContext {
    let behind_git_dep = is_git_sourced(git_pkgs, &adv.crate_name, &adv.installed);
    let already_ignored = files.is_ignored(&adv.id).unwrap_or(false);
    let resolvable_patch = match &adv.patched {
        PatchStatus::None => None,
        // A crate pinned by an exact git rev cannot be `--precise`-bumped in
        // Simard's lockfile — the fix belongs upstream (Escalate), so don't
        // even try to resolve a registry version for it.
        PatchStatus::Fixed { .. } if behind_git_dep => None,
        PatchStatus::Fixed { requirement } => resolve_patch(&adv.crate_name, requirement),
    };
    RemediationContext {
        resolvable_patch,
        behind_git_dep,
        already_ignored,
    }
}

/// The `(name, version)` pairs pinned by a `git+` source in `Cargo.lock`.
///
/// Parsed **once per scan**: a package's git-vs-registry origin is stable across
/// a single sweep (a `--precise` bump changes versions, it never turns a
/// registry crate into a git one), so the per-advisory git-dep check becomes an
/// allocation-free lookup over this small set instead of re-reading and
/// re-parsing the whole lockfile — and building a full generic `toml::Value`
/// DOM — for every advisory. A read/parse failure degrades to an empty set
/// (nothing treated as git-sourced), matching the previous best-effort default.
fn git_sourced_packages(root: &Path) -> Vec<(String, String)> {
    #[derive(serde::Deserialize, Default)]
    struct Lock {
        #[serde(default)]
        package: Vec<LockPackage>,
    }
    #[derive(serde::Deserialize)]
    struct LockPackage {
        name: String,
        version: String,
        #[serde(default)]
        source: Option<String>,
    }
    let Ok(raw) = std::fs::read_to_string(root.join("Cargo.lock")) else {
        return Vec::new();
    };
    let Ok(lock) = toml::from_str::<Lock>(&raw) else {
        return Vec::new();
    };
    lock.package
        .into_iter()
        .filter(|p| p.source.as_deref().is_some_and(|s| s.starts_with("git+")))
        .map(|p| (p.name, p.version))
        .collect()
}

/// True when `(crate_name, version)` is among the (typically few) git-pinned
/// packages — a bump for it belongs in that upstream repo, not Simard's lock.
fn is_git_sourced(git_pkgs: &[(String, String)], crate_name: &str, version: &str) -> bool {
    git_pkgs
        .iter()
        .any(|(name, ver)| name == crate_name && ver == version)
}

/// Best-effort "lowest patched version that resolves against Cargo.lock": parse
/// the lower-bound version from the requirement and verify it with a
/// non-mutating `cargo update --dry-run`. Returns `None` (→ Escalate) when no
/// candidate resolves.
fn resolve_patch(crate_name: &str, requirement: &str) -> Option<String> {
    let candidate = lowest_version_token(requirement)?;
    let ok = Command::new("cargo")
        .args([
            "update",
            "-p",
            crate_name,
            "--precise",
            &candidate,
            "--dry-run",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    ok.then_some(candidate)
}

/// Extract the lowest version-like token from a patched requirement string,
/// e.g. `">= 0.9.20"` → `Some("0.9.20")`, `">= 0.7.4, < 0.8.0"` → `Some("0.7.4")`.
fn lowest_version_token(requirement: &str) -> Option<String> {
    requirement
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_digit() && c != '.'))
        .find(|t| !t.is_empty() && semver::Version::parse(t).is_ok())
        .map(str::to_string)
}

// ─────────────────────────── cargo audit runner ───────────────────────────

/// Run `cargo audit --json`, returning its stdout. `cargo audit` exits non-zero
/// when it finds vulnerabilities, so a non-zero exit with valid JSON on stdout
/// is the expected success case here.
fn run_cargo_audit_json() -> SimardResult<String> {
    let output = Command::new("cargo")
        .args(["audit", "--json"])
        .output()
        .map_err(|e| SimardError::SupplyChainRemediationFailed {
            reason: format!("failed to spawn `cargo audit`: {e}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(SimardError::SupplyChainRemediationFailed {
            reason: format!(
                "`cargo audit --json` produced no JSON (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(stdout)
}

// ─────────────────────────── advisory-db pin advance ───────────────────────────

/// When DB HEAD is clean, detect the advanceable `.github/advisory-db.sha`
/// revision and **log** it. Advancing the pin — writing the SHA and opening the
/// `chore(deps): bump advisory-db pin` PR — is a separate, deliberate step (the
/// logged SHA is what that PR records); this driver stays side-effect-light and
/// never force-pushes a branch or opens a PR from the scheduled default-branch
/// checkout itself. Best-effort: any failure is a non-fatal `warn`, since the
/// scan's primary job (remediating vulnerabilities) has already succeeded when
/// this runs.
fn advance_pin_best_effort(root: &Path) -> SimardResult<()> {
    let head = advisory_db_head()?;
    let pin_path = root.join(".github").join("advisory-db.sha");
    let current = std::fs::read_to_string(&pin_path).unwrap_or_default();
    if current.contains(&head) {
        info!(sha = %head, "advisory-db pin already at DB HEAD");
        return Ok(());
    }
    info!(
        sha = %head,
        "advisory-db HEAD is clean and ahead of the pin; advance \
         .github/advisory-db.sha to this SHA via a `chore(deps): bump advisory-db pin` PR"
    );
    Ok(())
}

/// Resolve the current `rustsec/advisory-db` HEAD commit SHA via `git ls-remote`.
fn advisory_db_head() -> SimardResult<String> {
    let output = Command::new("git")
        .args([
            "ls-remote",
            "https://github.com/rustsec/advisory-db",
            "HEAD",
        ])
        .output()
        .map_err(|e| SimardError::SupplyChainRemediationFailed {
            reason: format!("failed to spawn `git ls-remote`: {e}"),
        })?;
    if !output.status.success() {
        return Err(SimardError::SupplyChainRemediationFailed {
            reason: format!(
                "`git ls-remote advisory-db` exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| SimardError::SupplyChainRemediationFailed {
            reason: "`git ls-remote advisory-db HEAD` returned no SHA".to_string(),
        })
}

// ─────────────────────────── helpers ───────────────────────────

fn repo_root() -> SimardResult<PathBuf> {
    std::env::current_dir().map_err(|e| SimardError::SupplyChainRemediationFailed {
        reason: format!("cannot determine current directory: {e}"),
    })
}

/// Render one decision as JSON for `decide-only` output.
fn decision_to_json(adv: &Advisory, decision: &Decision) -> serde_json::Value {
    use serde_json::json;
    let detail = match decision {
        Decision::Bump {
            crate_name,
            from,
            to,
        } => json!({ "action": "bump", "crate": crate_name, "from": from, "to": to }),
        Decision::JustifiedIgnore {
            advisory_id,
            crate_name,
            reason,
        } => json!({
            "action": "justified-ignore",
            "advisory": advisory_id,
            "crate": crate_name,
            "reason": reason,
        }),
        Decision::Escalate {
            advisory_id,
            reason,
        } => json!({ "action": "escalate", "advisory": advisory_id, "reason": reason }),
        Decision::NoAction => json!({ "action": "no-action" }),
    };
    json!({ "advisory": adv.id, "crate": adv.crate_name, "decision": detail })
}

// ─────────────────────────────── tests ───────────────────────────────
//
// The pure I/O-glue helpers this driver layers over the (already exhaustively
// tested) `decide()` reasoner. `lowest_version_token` picks the bump target, so
// a regression there would mis-target a bump — and `decide-only` prints it with
// no `cargo update --dry-run` guard — which is exactly why it is pinned here.

#[cfg(test)]
mod tests {
    use super::*;

    // ── lowest_version_token ──────────────────────────────────────────────

    #[test]
    fn lowest_version_token_reads_simple_lower_bound() {
        assert_eq!(lowest_version_token(">= 0.9.20").as_deref(), Some("0.9.20"));
        assert_eq!(lowest_version_token(">= 1.2.5").as_deref(), Some("1.2.5"));
    }

    #[test]
    fn lowest_version_token_takes_the_floor_of_a_compound_requirement() {
        // The lower bound (the minimal fixed version) is the first version-like
        // token in a `>= a, < b` patched requirement.
        assert_eq!(
            lowest_version_token(">= 0.7.4, < 0.8.0").as_deref(),
            Some("0.7.4")
        );
    }

    #[test]
    fn lowest_version_token_reads_a_bare_version() {
        assert_eq!(lowest_version_token("1.0.0").as_deref(), Some("1.0.0"));
    }

    #[test]
    fn lowest_version_token_is_none_for_no_concrete_version() {
        assert_eq!(lowest_version_token(""), None);
        assert_eq!(lowest_version_token("*"), None);
        assert_eq!(lowest_version_token(">= "), None);
    }

    // ── is_git_sourced ────────────────────────────────────────────────────

    #[test]
    fn is_git_sourced_matches_only_exact_name_and_version() {
        let pkgs = vec![("moka".to_string(), "0.12.0".to_string())];
        assert!(is_git_sourced(&pkgs, "moka", "0.12.0"));
        // Same crate, different version → not the git-pinned one.
        assert!(!is_git_sourced(&pkgs, "moka", "0.11.0"));
        // Different crate → no match.
        assert!(!is_git_sourced(&pkgs, "other", "0.12.0"));
        // Empty set → nothing is git-sourced.
        assert!(!is_git_sourced(&[], "moka", "0.12.0"));
    }

    // ── git_sourced_packages ──────────────────────────────────────────────

    #[test]
    fn git_sourced_packages_returns_only_git_plus_sources() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.lock"),
            "version = 3\n\n\
             [[package]]\n\
             name = \"gitcrate\"\n\
             version = \"1.0.0\"\n\
             source = \"git+https://github.com/example/gitcrate?rev=abc#abc\"\n\n\
             [[package]]\n\
             name = \"regcrate\"\n\
             version = \"2.0.0\"\n\
             source = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n\
             [[package]]\n\
             name = \"localcrate\"\n\
             version = \"0.1.0\"\n",
        )
        .unwrap();

        let pkgs = git_sourced_packages(dir.path());
        assert_eq!(pkgs, vec![("gitcrate".to_string(), "1.0.0".to_string())]);
    }

    #[test]
    fn git_sourced_packages_degrades_to_empty_when_lockfile_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(git_sourced_packages(dir.path()).is_empty());
    }

    // ── decision_to_json ──────────────────────────────────────────────────

    fn advisory(id: &str, krate: &str) -> Advisory {
        Advisory {
            id: id.to_string(),
            crate_name: krate.to_string(),
            installed: "1.0.0".to_string(),
            patched: PatchStatus::None,
            title: "t".to_string(),
            url: format!("https://rustsec.org/advisories/{id}"),
        }
    }

    #[test]
    fn decision_to_json_renders_each_variant() {
        let adv = advisory("RUSTSEC-2026-0204", "crossbeam-epoch");

        let bump = decision_to_json(
            &adv,
            &Decision::Bump {
                crate_name: "crossbeam-epoch".to_string(),
                from: "0.9.18".to_string(),
                to: "0.9.20".to_string(),
            },
        );
        assert_eq!(bump["decision"]["action"], "bump");
        assert_eq!(bump["decision"]["to"], "0.9.20");
        assert_eq!(bump["advisory"], "RUSTSEC-2026-0204");

        let ignore = decision_to_json(
            &adv,
            &Decision::JustifiedIgnore {
                advisory_id: "RUSTSEC-2023-0071".to_string(),
                crate_name: "rsa".to_string(),
                reason: "no fix".to_string(),
            },
        );
        assert_eq!(ignore["decision"]["action"], "justified-ignore");
        assert_eq!(ignore["decision"]["advisory"], "RUSTSEC-2023-0071");

        let escalate = decision_to_json(
            &adv,
            &Decision::Escalate {
                advisory_id: "RUSTSEC-2025-0009".to_string(),
                reason: "unresolvable".to_string(),
            },
        );
        assert_eq!(escalate["decision"]["action"], "escalate");

        let none = decision_to_json(&adv, &Decision::NoAction);
        assert_eq!(none["decision"]["action"], "no-action");
    }
}
