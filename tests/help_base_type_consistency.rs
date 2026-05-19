//! Regression test for issue #1907 (base-type registry inconsistency).
//!
//! Every base-type identifier advertised in any user-facing `--help`
//! example string must round-trip successfully through the built-in
//! base-type registry that `goal-curation read`, `improvement-curation
//! read`, and `meeting run/read` validate against. Otherwise an operator
//! who copy-pastes the example will hit a hard
//! `AdapterNotRegistered` error (Pillar 11 violation).
//!
//! The original drift: `simard meeting --help` shipped `gpt-5` in its
//! examples but `gpt-5` was never wired through `register_builtin_base_type`,
//! so adjacent surfaces rejected it. This test ensures the help text
//! and the registry stay synchronized regardless of which one drifts in
//! the future.

use simard::{all_operator_help_texts, known_builtin_base_type_ids};

/// Commands whose `run`/`read` subcommand takes a base-type identifier
/// as a positional argument, paired with the index offset where the
/// base-type token appears after the verb (0 = the very next token).
///
/// `bootstrap run` is intentionally offset 1 because its signature is
/// `bootstrap run <identity> <base-type> <topology> <objective>`.
const BASE_TYPE_BEARING_COMMANDS: &[(&str, &str, usize)] = &[
    ("meeting", "run", 0),
    ("meeting", "read", 0),
    ("goal-curation", "run", 0),
    ("goal-curation", "read", 0),
    ("improvement-curation", "run", 0),
    ("improvement-curation", "read", 0),
    ("review", "run", 0),
    ("review", "read", 0),
    ("bootstrap", "run", 1),
];

/// Walk a help-text block and return every concrete base-type
/// identifier it advertises in an `Examples:` line (i.e. tokens that are
/// not angle-bracket placeholders like `<base-type>`).
fn extract_help_base_type_examples(help_text: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for (lineno, line) in help_text.lines().enumerate() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(simard_pos) = words.iter().position(|w| *w == "simard") else {
            continue;
        };
        for (cmd, verb, offset) in BASE_TYPE_BEARING_COMMANDS {
            let cmd_pos = simard_pos + 1;
            let verb_pos = simard_pos + 2;
            let token_pos = verb_pos + 1 + offset;
            if words.get(cmd_pos).copied() != Some(cmd) {
                continue;
            }
            if words.get(verb_pos).copied() != Some(verb) {
                continue;
            }
            let Some(raw_token) = words.get(token_pos) else {
                continue;
            };
            let token = raw_token.trim_matches(|c: char| c == '"' || c == '\\');
            if token.is_empty() || token.starts_with('<') {
                continue;
            }
            hits.push((lineno + 1, token.to_string()));
        }
    }
    hits
}

#[test]
fn every_help_example_base_type_is_registered() {
    let registered: Vec<&str> = known_builtin_base_type_ids().to_vec();
    let mut violations: Vec<String> = Vec::new();
    let mut total_examples = 0usize;

    for (label, help_text) in all_operator_help_texts() {
        for (lineno, token) in extract_help_base_type_examples(help_text) {
            total_examples += 1;
            if !registered.contains(&token.as_str()) {
                violations.push(format!(
                    "help block '{label}' line {lineno}: example uses '{token}', \
                     which is not in the built-in base-type registry \
                     {registered:?}. Either register the adapter for '{token}' \
                     via `register_builtin_base_type` and add it to \
                     `KNOWN_BUILTIN_BASE_TYPE_IDS`, or change the help example \
                     to use a registered identifier."
                ));
            }
        }
    }

    assert!(
        total_examples > 0,
        "extractor found zero base-type example tokens across all help \
         blocks; this most likely means the parser regressed (or the help \
         text format changed). Help blocks scanned: {:?}",
        all_operator_help_texts()
            .iter()
            .map(|(label, _)| *label)
            .collect::<Vec<_>>()
    );

    assert!(
        violations.is_empty(),
        "help <-> base-type registry drift detected (issue #1907):\n{}",
        violations.join("\n")
    );
}

#[test]
fn registry_canonical_list_matches_spec() {
    // Specs/ProductArchitecture.md §"Agent Base Type" enumerates the
    // builtin manifest-advertised base types. If a new builtin adapter
    // ships, both this list and the spec table need to be updated in
    // lockstep.
    let ids = known_builtin_base_type_ids();
    let must_have = [
        "local-harness",
        "terminal-shell",
        "rusty-clawd",
        "copilot-sdk",
    ];
    for id in must_have {
        assert!(
            ids.contains(&id),
            "spec-required base type '{id}' missing from \
             KNOWN_BUILTIN_BASE_TYPE_IDS = {ids:?}"
        );
    }
}

// ── parser self-tests: make sure the extractor itself is correct so the
// integration test above can't silently pass with zero matches ──

#[test]
fn extractor_returns_literal_base_type_examples() {
    let help = "\
Examples:
  simard meeting run local-harness ring \"design review\"
  simard meeting read local-harness ring
  simard meeting --help
  simard meeting repl \"weekly sync\"
";
    let hits = extract_help_base_type_examples(help);
    let tokens: Vec<&str> = hits.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(
        tokens,
        vec!["local-harness", "local-harness"],
        "extractor should pick up both literal base-type tokens"
    );
}

#[test]
fn extractor_skips_angle_bracket_placeholders() {
    let help = "\
  meeting run <base-type> <topology> <objective>
  meeting read <base-type> <topology>
";
    let hits = extract_help_base_type_examples(help);
    assert!(
        hits.is_empty(),
        "extractor must skip <placeholder> tokens, but found: {hits:?}"
    );
}

#[test]
fn extractor_handles_bootstrap_run_offset() {
    // `bootstrap run <identity> <base-type> ...` — base-type is the
    // 2nd token after the verb, not the 1st.
    let help = "  simard bootstrap run simard-engineer terminal-shell single-process \"obj\"";
    let hits = extract_help_base_type_examples(help);
    let tokens: Vec<&str> = hits.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(tokens, vec!["terminal-shell"]);
}

#[test]
fn extractor_skips_unrelated_run_commands() {
    // `gym run <scenario>` and `engineer run <topology>` are not
    // base-type-leading — the extractor must not flag their first
    // positional as a base type.
    let help = "\
  simard gym run dep-analysis-cargo-audit
  simard engineer run single-process /tmp/ws \"obj\"
  simard ooda run --cycles=3
";
    let hits = extract_help_base_type_examples(help);
    assert!(
        hits.is_empty(),
        "extractor must not flag positional args of unrelated commands, \
         but found: {hits:?}"
    );
}

#[test]
fn extractor_flags_gpt5_in_pre_fix_meeting_help() {
    // This is the exact pre-fix MEETING_HELP example block. The test
    // pins the behavior: if anyone re-introduces 'gpt-5' (or any other
    // unregistered identifier) into a meeting --help example, the
    // integration test `every_help_example_base_type_is_registered`
    // will catch it because the extractor reliably finds the token.
    let pre_fix_help = "\
Examples:
  simard meeting --help
  simard meeting repl \"weekly sync\"
  simard meeting run gpt-5 ring \"design review\"
  simard meeting read gpt-5 ring
";
    let hits = extract_help_base_type_examples(pre_fix_help);
    let tokens: Vec<String> = hits.into_iter().map(|(_, t)| t).collect();
    assert!(
        tokens.iter().any(|t| t == "gpt-5"),
        "extractor must surface the regressed token 'gpt-5'; \
         found tokens = {tokens:?}"
    );
}
