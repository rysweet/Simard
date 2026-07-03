//! Behavior-preservation golden test for the "unified Brain" terminology cleanup
//! (issue #2419). The rename is mechanical and MUST NOT change any value that
//! crosses a trust boundary. This pins the frozen wire values from the
//! "frozen-value allow-list" in `docs/reference/brain-terminology-migration.md`:
//! the identifier is renamed, the on-the-wire VALUE is frozen.
//!
//! Pure filesystem scan (no `simard::` symbols) so the crate compiles regardless
//! of rename state; assertions fail until the rename lands, then pass.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn walk(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Whole `src/` as one string (line boundaries preserved) plus the line list.
fn src_corpus() -> (String, Vec<String>) {
    let mut files = Vec::new();
    walk(&repo_root().join("src"), &mut files);
    let mut corpus = String::new();
    let mut lines = Vec::new();
    for path in files {
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                lines.push(line.to_string());
            }
            corpus.push_str(&text);
            corpus.push('\n');
        }
    }
    (corpus, lines)
}

fn any_line_has_all(lines: &[String], needles: &[&str]) -> bool {
    lines.iter().any(|l| needles.iter().all(|n| l.contains(n)))
}

// ── frozen values (behavior-preserving — must survive the rename) ────────────

#[test]
fn frozen_wire_literals_preserved() {
    let (corpus, _) = src_corpus();

    // JSON-RPC method literal on the wire.
    assert!(
        corpus.contains("bridge.health"),
        "frozen wire method literal \"bridge.health\" must be preserved verbatim."
    );

    // Numeric JSON-RPC error codes (values frozen regardless of const rename).
    for code in ["-32601", "-32603", "-32000", "-32001"] {
        assert!(
            corpus.contains(code),
            "frozen JSON-RPC error code {code} must be preserved."
        );
    }

    // Operator-set environment variable literal.
    assert!(
        corpus.contains("SIMARD_MIND_MAX_NONCRITICAL_PER_TICK"),
        "frozen env literal SIMARD_MIND_MAX_NONCRITICAL_PER_TICK must be preserved \
         (the identifier renames to BRAIN_NONCRITICAL_BUDGET_ENV, the value is frozen)."
    );

    // Persisted CycleReport JSON key.
    assert!(
        corpus.contains("brain_judgments"),
        "frozen persisted serde key \"brain_judgments\" must be preserved."
    );
}

// ── renamed carriers of the frozen values (must appear post-rename) ──────────

#[test]
fn renamed_wire_constants_present() {
    let (corpus, _) = src_corpus();
    let required = [
        "SERVER_ERROR_METHOD_NOT_FOUND",
        "SERVER_ERROR_INTERNAL",
        "SERVER_ERROR_TIMEOUT",
        "SERVER_ERROR_TRANSPORT",
        "HEALTH_METHOD",
        "BRAIN_NONCRITICAL_BUDGET_ENV",
    ];
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|id| !corpus.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "the de-Bridged constants that carry the frozen values are missing: {missing:?}\n\
         (BRIDGE_ERROR_* → SERVER_ERROR_*, the bridge.health method → HEALTH_METHOD, \
         BUDGET_ENV → BRAIN_NONCRITICAL_BUDGET_ENV)."
    );
}

#[test]
fn error_codes_frozen_on_renamed_consts() {
    let (_, lines) = src_corpus();
    let bindings = [
        ("SERVER_ERROR_METHOD_NOT_FOUND", "-32601"),
        ("SERVER_ERROR_INTERNAL", "-32603"),
        ("SERVER_ERROR_TIMEOUT", "-32000"),
        ("SERVER_ERROR_TRANSPORT", "-32001"),
    ];
    for (name, value) in bindings {
        assert!(
            any_line_has_all(&lines, &[name, value]),
            "renamed const {name} must keep its frozen numeric value {value} on one line."
        );
    }
}

#[test]
fn health_method_binds_frozen_literal() {
    let (_, lines) = src_corpus();
    assert!(
        any_line_has_all(&lines, &["HEALTH_METHOD", "bridge.health"]),
        "HEALTH_METHOD must bind the frozen wire literal, e.g.\n\
         `// FROZEN WIRE VALUE: JSON-RPC method name`\n\
         `const HEALTH_METHOD: &str = \"bridge.health\";`"
    );
}

#[test]
fn budget_env_binds_frozen_literal() {
    let (_, lines) = src_corpus();
    assert!(
        any_line_has_all(
            &lines,
            &[
                "BRAIN_NONCRITICAL_BUDGET_ENV",
                "SIMARD_MIND_MAX_NONCRITICAL_PER_TICK"
            ]
        ),
        "the renamed budget-env const must still read the frozen operator env literal:\n\
         `const BRAIN_NONCRITICAL_BUDGET_ENV: &str = \"SIMARD_MIND_MAX_NONCRITICAL_PER_TICK\";`"
    );
}

#[test]
fn cycle_report_serde_key_frozen() {
    let (corpus, lines) = src_corpus();

    // The Rust field is renamed off the phase-brain word …
    assert!(
        corpus.contains("reasoner_judgments"),
        "the CycleReport judgment field must be renamed to `reasoner_judgments`."
    );
    // … while the persisted JSON key is frozen via an explicit serde rename.
    assert!(
        any_line_has_all(&lines, &["rename", "brain_judgments"]),
        "the persisted key must be frozen with `#[serde(rename = \"brain_judgments\")]` \
         so existing CycleReport JSON round-trips unchanged."
    );
}

#[test]
fn frozen_values_are_annotated() {
    let (corpus, _) = src_corpus();
    let count = corpus.matches("FROZEN WIRE VALUE").count();
    assert!(
        count >= 4,
        "each surviving frozen literal must be annotated `// FROZEN WIRE VALUE:` so the \
         anti-drift gate can distinguish a frozen value from a missed rename; found {count}, \
         expected at least 4 (bridge.health, the SERVER_ERROR_* codes, the env literal, \
         and the brain_judgments serde key)."
    );
}
