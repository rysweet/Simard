// tests/escalation_triage_docs.rs
//
// TDD contract tests for the "triage-and-course-correct a blocked goal before
// escalating to a human" feature (issue #17 worked example).
//
// The deliverable of this feature is DOCUMENTATION plus reliance on the existing
// `simard goal complete` CLI verb — no new runtime module. So the machine-checkable
// contract is the documentation itself: it must name the right target, prescribe
// the right (real, not fictional) completion command, cite machine-checkable
// evidence, wire into the site nav, keep every internal link live, and — the load-
// bearing safety property — NEVER leak an internal diagnostic marker into an
// operator-facing Signal message.
//
// These tests define that contract. Against the pre-feature tree (docs absent,
// `simard goal complete` undocumented) they FAIL; once the docs land and the CLI
// verb is documented + implemented, they pass and stand as regression guards.

use std::path::{Path, PathBuf};

// ---- fixtures ------------------------------------------------------------

const CONCEPT_DOC: &str = "docs/concepts/overseer-escalation-triage-course-correction.md";
const HOWTO_DOC: &str = "docs/howto/triage-and-course-correct-a-blocked-goal.md";
const REFERENCE_DOC: &str = "docs/reference/escalation-triage-api.md";
const CLI_REFERENCE: &str = "docs/reference/simard-cli.md";
const MKDOCS: &str = "mkdocs.yml";
const GOAL_SOURCE: &str = "src/operator_cli/goal.rs";

/// Raw internal markers that must be TRANSLATED to plain English and must never
/// appear verbatim in a message an operator receives.
const RAW_MARKERS: &[&str] = &[
    "OODA-SAFEGUARD",
    "UNCLEAR-CRITERIA",
    "GENUINELY-STUCK",
    "why=",
    "evidence=[",
    "health-review:stuck-goal",
    "\u{1f512}", // the 🔒 lock token
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "required doc {} is missing or unreadable: {e}",
            path.display()
        )
    })
}

/// Extract the operator-VOICED example messages. Throughout the docs, every
/// string quoted as something the operator actually reads is rendered as an
/// italic quote `*"..."*`. Translation tables and "never surface these"
/// instructions are NOT wrapped this way, so this scopes the leak check to
/// exactly the operator-facing surface.
fn italic_quotes(doc: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = doc;
    while let Some(start) = rest.find("*\"") {
        let after = &rest[start + 2..];
        match after.find("\"*") {
            Some(end) => {
                out.push(after[..end].to_string());
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    out
}

/// Extract relative Markdown links (`](./x.md)` / `](../y/z.md#anchor)`),
/// stripping any `#anchor` suffix. Absolute/http links are ignored.
fn relative_md_links(doc: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = doc;
    while let Some(i) = rest.find("](") {
        let after = &rest[i + 2..];
        match after.find(')') {
            Some(j) => {
                let target = &after[..j];
                let path = target.split('#').next().unwrap_or("");
                if (path.starts_with("./") || path.starts_with("../")) && path.ends_with(".md") {
                    out.push(path.to_string());
                }
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out
}

// ---- the three docs exist and are wired into the site --------------------

#[test]
fn all_three_feature_docs_exist_and_are_nonempty() {
    for doc in [CONCEPT_DOC, HOWTO_DOC, REFERENCE_DOC] {
        let body = read(doc);
        assert!(
            body.trim().len() > 200,
            "{doc} must be a substantive doc, not a stub"
        );
    }
}

#[test]
fn feature_docs_are_wired_into_mkdocs_nav() {
    let nav = read(MKDOCS);
    for doc in [CONCEPT_DOC, HOWTO_DOC, REFERENCE_DOC] {
        // mkdocs nav paths are relative to docs/, so strip the leading "docs/".
        let nav_path = doc.strip_prefix("docs/").unwrap();
        assert!(
            nav.contains(nav_path),
            "mkdocs.yml nav must reference {nav_path} so the doc is discoverable"
        );
    }
}

// ---- the completion path is the real `simard goal complete` verb ----------

#[test]
fn docs_prescribe_the_goal_complete_verb_as_completion_path() {
    // The `complete-delivered-goal` course-correction is executed via
    // `simard goal complete <goal-id>`. All three docs must name it.
    for doc in [CONCEPT_DOC, HOWTO_DOC, REFERENCE_DOC] {
        let body = read(doc);
        assert!(
            body.contains("goal complete"),
            "{doc} must prescribe `simard goal complete` as the completion path"
        );
    }
}

#[test]
fn cli_reference_documents_goal_complete_section() {
    // The verb the feature docs promise must be a real, documented CLI command
    // (the exact "is this command fictional?" risk). It must have its own
    // heading in the CLI reference so the deep-link anchor resolves.
    let cli = read(CLI_REFERENCE);
    assert!(
        cli.contains("### `simard goal complete"),
        "docs/reference/simard-cli.md must document `simard goal complete` under its own heading"
    );
}

#[test]
fn documented_goal_complete_verb_is_actually_implemented() {
    // Cross-check the docs against source: the `complete` verb must be routed
    // in the CLI dispatcher and advertised in the help text. This prevents the
    // docs from describing a command that does not exist.
    let src = read(GOAL_SOURCE);
    assert!(
        src.contains("\"complete\" =>"),
        "src/operator_cli/goal.rs must route the `complete` subcommand"
    );
    assert!(
        src.contains("fn handle_complete"),
        "src/operator_cli/goal.rs must implement handle_complete"
    );
}

// ---- target discipline: pin the -rs repo, reject the bare false lead ------

#[test]
fn docs_pin_the_rs_repo_and_reject_the_bare_repo_false_lead() {
    for doc in [CONCEPT_DOC, HOWTO_DOC] {
        let body = read(doc);
        assert!(
            body.contains("agent-kgpacks-rs"),
            "{doc} must pin the fully-qualified `agent-kgpacks-rs` repo"
        );
        assert!(
            body.contains("false lead"),
            "{doc} must warn that the bare `agent-kgpacks#17` is a false lead"
        );
    }
}

// ---- evidence gate: closed issue #17 + merged PR #40 ---------------------

#[test]
fn concept_doc_cites_machine_checkable_evidence() {
    let body = read(CONCEPT_DOC);
    assert!(
        body.contains("agent-kgpacks-rs/issues/17"),
        "the worked example must cite the closed issue #17 in the -rs repo"
    );
    assert!(
        body.contains("agent-kgpacks-rs/pull/40"),
        "the worked example must cite the merged PR #40 in the -rs repo"
    );
    assert!(
        body.to_lowercase().contains("merged"),
        "the evidence must describe PR #40 as merged"
    );
}

// ---- marker-translation contract ----------------------------------------

#[test]
fn reference_doc_maps_every_raw_marker_to_plain_english() {
    let body = read(REFERENCE_DOC);
    for marker in RAW_MARKERS {
        assert!(
            body.contains(marker),
            "the reference translation table must account for the raw marker {marker:?}"
        );
    }
}

#[test]
fn docs_instruct_that_markers_are_never_surfaced() {
    // Both the operator-facing howto and the concept doc must state the
    // non-leak rule explicitly.
    for doc in [CONCEPT_DOC, HOWTO_DOC] {
        let body = read(doc);
        let lc = body.to_lowercase();
        let states_the_rule = lc.contains("never see")
            || lc.contains("never sees")
            || lc.contains("never surface")
            || lc.contains("raw internal markers")
            || lc.contains("translates all of them");
        assert!(
            states_the_rule,
            "{doc} must state that internal markers are never surfaced to the operator"
        );
    }
}

// ---- THE safety property: no marker leaks into an operator message --------

#[test]
fn operator_facing_example_messages_contain_no_raw_markers() {
    let mut quotes = italic_quotes(&read(CONCEPT_DOC));
    quotes.extend(italic_quotes(&read(HOWTO_DOC)));

    assert!(
        quotes.len() >= 3,
        "expected to find the operator-voiced example messages ({} found)",
        quotes.len()
    );

    for quote in &quotes {
        for marker in RAW_MARKERS {
            assert!(
                !quote.contains(marker),
                "operator-facing example message leaks the internal marker {marker:?}: {quote:?}"
            );
        }
    }
}

// ---- link hygiene --------------------------------------------------------

#[test]
fn internal_relative_links_resolve() {
    for doc in [CONCEPT_DOC, HOWTO_DOC, REFERENCE_DOC] {
        let dir = repo_root().join(Path::new(doc).parent().unwrap());
        for link in relative_md_links(&read(doc)) {
            let target = dir.join(&link);
            assert!(
                target.exists(),
                "{doc} links to {link}, but {} does not exist",
                target.display()
            );
        }
    }
}
