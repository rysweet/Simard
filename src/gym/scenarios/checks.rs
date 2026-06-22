//! Class-specific check builders for the V1 `BenchmarkScenario` classes.
//!
//! Per `Specs/ProductArchitecture.md` (issue #2087) the gym scores only the
//! four sanctioned benchmark classes, so this module dispatches and implements
//! exactly those four check families.

use super::super::types::{BenchmarkCheckResult, BenchmarkClass, BenchmarkScenario};
use crate::handoff::RuntimeHandoffSnapshot;

pub(crate) fn class_specific_checks(
    scenario: &BenchmarkScenario,
    outcome: &crate::runtime::SessionOutcome,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let summary = outcome.execution_summary.to_lowercase();
    let plan = outcome.plan.to_lowercase();
    let reflection = outcome.reflection.summary.to_lowercase();
    let combined = format!("{summary} {plan} {reflection}");

    match scenario.class {
        BenchmarkClass::RepoExploration => checks_for_repo_exploration(&combined),
        BenchmarkClass::Documentation => checks_for_documentation(&combined),
        BenchmarkClass::SafeCodeChange => checks_for_safe_code_change(&combined),
        BenchmarkClass::SessionQuality => checks_for_session_quality(outcome, exported),
    }
}

fn checks_for_repo_exploration(combined: &str) -> Vec<BenchmarkCheckResult> {
    let structure_mentioned = combined.contains("src/")
        || combined.contains("directory")
        || combined.contains("structure")
        || combined.contains("module");
    let deps_mentioned = combined.contains("cargo.toml")
        || combined.contains("dependenc")
        || combined.contains("crate");
    let entry_mentioned = combined.contains("main.rs")
        || combined.contains("lib.rs")
        || combined.contains("entry point")
        || combined.contains("entry-point");
    vec![
        BenchmarkCheckResult {
            id: "repo-structure-discovered".to_string(),
            passed: structure_mentioned,
            detail: format!(
                "execution output {} project structure references",
                if structure_mentioned {
                    "contains"
                } else {
                    "lacks"
                }
            ),
        },
        BenchmarkCheckResult {
            id: "repo-dependencies-identified".to_string(),
            passed: deps_mentioned,
            detail: format!(
                "execution output {} dependency references",
                if deps_mentioned { "contains" } else { "lacks" }
            ),
        },
        BenchmarkCheckResult {
            id: "repo-entry-points-found".to_string(),
            passed: entry_mentioned,
            detail: format!(
                "execution output {} entry point references",
                if entry_mentioned { "contains" } else { "lacks" }
            ),
        },
    ]
}

fn checks_for_documentation(combined: &str) -> Vec<BenchmarkCheckResult> {
    let has_doc_syntax = combined.contains("///")
        || combined.contains("doc comment")
        || combined.contains("rustdoc")
        || combined.contains("documentation");
    let mentions_params = combined.contains("param")
        || combined.contains("argument")
        || combined.contains("return")
        || combined.contains("-> ");
    vec![
        BenchmarkCheckResult {
            id: "doc-comment-syntax-valid".to_string(),
            passed: has_doc_syntax,
            detail: format!(
                "execution output {} doc comment syntax",
                if has_doc_syntax {
                    "references"
                } else {
                    "lacks"
                }
            ),
        },
        BenchmarkCheckResult {
            id: "doc-params-return-covered".to_string(),
            passed: mentions_params,
            detail: format!(
                "execution output {} parameter/return documentation",
                if mentions_params { "includes" } else { "lacks" }
            ),
        },
    ]
}

fn checks_for_safe_code_change(combined: &str) -> Vec<BenchmarkCheckResult> {
    let compilation_evidence = combined.contains("compil")
        || combined.contains("cargo build")
        || combined.contains("cargo check")
        || combined.contains("build succeed")
        || combined.contains("no errors");
    let change_described = combined.contains("derive")
        || combined.contains("change")
        || combined.contains("modif")
        || combined.contains("diff");
    vec![
        BenchmarkCheckResult {
            id: "code-change-compilation-checked".to_string(),
            passed: compilation_evidence,
            detail: format!(
                "execution output {} compilation verification",
                if compilation_evidence {
                    "includes"
                } else {
                    "lacks"
                }
            ),
        },
        BenchmarkCheckResult {
            id: "code-change-described".to_string(),
            passed: change_described,
            detail: format!(
                "execution output {} change description",
                if change_described {
                    "includes"
                } else {
                    "lacks"
                }
            ),
        },
    ]
}

fn checks_for_session_quality(
    outcome: &crate::runtime::SessionOutcome,
    exported: &RuntimeHandoffSnapshot,
) -> Vec<BenchmarkCheckResult> {
    let session_summary_present =
        !outcome.execution_summary.trim().is_empty() && exported.memory_records.len() >= 2;
    vec![BenchmarkCheckResult {
        id: "session-quality-summary-adequate".to_string(),
        passed: session_summary_present,
        detail: format!(
            "session produced {} memory records with {} execution summary",
            exported.memory_records.len(),
            if outcome.execution_summary.trim().is_empty() {
                "empty"
            } else {
                "non-empty"
            }
        ),
    }]
}
