//! TDD contract for the behavior-preserving "unified Brain" terminology cleanup
//! (issue #2419). These tests are the executable form of the old→new map in
//! `docs/reference/brain-terminology-migration.md` and mirror the CI gate in
//! `scripts/ci/check-terminology-drift.sh`.
//!
//! They are pure filesystem scans (no `simard::` symbols), so the crate compiles
//! regardless of rename state and the tests FAIL on assertions until the rename
//! is complete, then PASS. Behavior is never asserted here — only naming.
//!
//! The terminology law (target state):
//!   1. "Brain" = the WHOLE cognition (scheduler + threads + memory). LEGAL.
//!   2. A single OODA phase is a "reasoner" — never a phase-level "brain".
//!   3. Nothing is named "Bridge" (any case); the sole survivor is the frozen
//!      JSON-RPC wire method literal "bridge.health".
//!   4. The scheduler executive is `Brain`, never `Mind`. "Hive Mind" (the
//!      distinct shared-memory concept in `memory_hive.rs`) is untouched.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

// ── curated retired identifiers ──────────────────────────────────────────────

/// Phase-level "brain" identifiers (retired → reasoner). "brain" for the WHOLE
/// cognition stays legal (`Brain`, `BrainIntrospection*`, `brain_introspection`,
/// the brain-model/executive/terminology docs, and the KEPT `ooda-brain-*.md` /
/// `recipe-brain-*.md` filenames whose content is reframed to "reasoner").
const PHASE_BRAIN_IDENTS: &[&str] = &[
    "OodaBrain",
    "OodaOrientBrain",
    "OodaDecideBrain",
    "RustyClawdBrain",
    "RustyClawdDecideBrain",
    "RustyClawdOrientBrain",
    "DeterministicFallbackBrain",
    "DeterministicLifecycleBrain",
    "DeterministicDecideBrain",
    "DeterministicFallbackDecideBrain",
    "DeterministicOrientBrain",
    "DeterministicFallbackOrientBrain",
    "RecipeBrain",
    "RecipeEngineerLifecycleBrain",
    "BrainPhase",
    "BrainParseSource",
    "BrainResponseUnparseable",
    "BrainJudgmentRecord",
    "BrainsLlmBackedProbe",
    "decide_brain",
    "orient_brain",
    "act_brain",
    "build_act_brain",
    "build_decide_brain",
    "build_orient_brain",
    "build_rustyclawd_brain",
    "build_rustyclawd_orient_brain",
    "fallback_brain_count",
    "FALLBACK_BRAIN_COUNT",
    "clear_brain_judgments",
    "take_brain_judgments",
];

/// Retired lowercase doc-link slugs (renamed docs).
const RETIRED_DOC_SLUGS: &[&str] = &[
    "bridge-pattern",
    "bridge-wire-protocol",
    "cognitive-memory-bridge-helpers",
];

/// Canonical NEW identifiers that must EXIST once the rename is complete.
const REQUIRED_NEW_IDENTS: &[&str] = &[
    "OrientReasoner",
    "DecideReasoner",
    "ActReasoner",
    "OodaContext",
    "ServerTransport",
    "CognitiveMemoryAdapter",
    "GymClient",
    "KnowledgeClient",
    "ServerSpawnFailed",
    "HEALTH_METHOD",
];

/// Code files that carry the denylist as data and are therefore exempt (the
/// analogue of the migration doc being allow-listed).
const ENFORCEMENT_FILES: &[&str] = &["terminology_drift.rs", "frozen_wire_values.rs"];

/// Docs permitted to spell retired identifiers (a migration map and a changelog must).
const DOC_ALLOWLIST: &[&str] = &["brain-terminology-migration.md", "whats-changed.md"];

// ── filesystem helpers ───────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn walk(root: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, exts, out);
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| exts.contains(&ext))
        {
            out.push(path);
        }
    }
}

fn is_excluded(path: &Path, names: &[&str]) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| names.contains(&n))
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// True if `needle` appears in `text` bounded by non-identifier chars on both sides.
fn contains_word(text: &str, needle: &str) -> bool {
    let hay: Vec<char> = text.chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    if pat.is_empty() || hay.len() < pat.len() {
        return false;
    }
    let mut i = 0;
    while i + pat.len() <= hay.len() {
        if hay[i..i + pat.len()] == pat[..] {
            let before_ok = i == 0 || !is_ident_char(hay[i - 1]);
            let after_idx = i + pat.len();
            let after_ok = after_idx >= hay.len() || !is_ident_char(hay[after_idx]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

// ── retired-token detectors ──────────────────────────────────────────────────

/// CODE rule: any "bridge" (case-insensitive) is retired except the frozen wire
/// literal `bridge.health` and lines annotated `FROZEN WIRE VALUE`.
fn code_has_bridge(line: &str) -> bool {
    if line.contains("FROZEN WIRE VALUE") {
        return false;
    }
    let stripped = line.replace("bridge.health", "");
    stripped.to_ascii_lowercase().contains("bridge")
}

/// The scheduler *type* `Mind` (capital-M word token). Case-sensitive on purpose:
/// the Rust type is always `Mind`, while lowercase "mind" is legitimate English
/// prose ("keep in mind", "on your mind"). The "Hive Mind" concept (distinct
/// shared memory) and the all-caps env literal `SIMARD_MIND_*` are both spared.
fn has_scheduler_mind(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let pat = ['M', 'i', 'n', 'd'];
    let n = chars.len();
    let mut i = 0;
    while i + 4 <= n {
        if chars[i..i + 4] == pat {
            let before_ok = i == 0 || !is_ident_char(chars[i - 1]);
            let after_ok = i + 4 >= n || !is_ident_char(chars[i + 4]);
            if before_ok && after_ok && !preceded_by_hive(&chars, i) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// True when the `Mind` token at `i` is the tail of "Hive Mind" / "hive-mind".
fn preceded_by_hive(chars: &[char], i: usize) -> bool {
    if i < 5 {
        return false;
    }
    let prev: String = chars[i - 5..i - 1]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();
    prev == "hive"
}

/// True if `needle` appears in `line` (case-sensitive) adjacent to an identifier
/// char on either side — i.e. it is part of a larger identifier token.
fn has_adjacent_ident(line: &str, needle: &str) -> bool {
    let hay: Vec<char> = line.chars().collect();
    let pat: Vec<char> = needle.chars().collect();
    let (n, m) = (hay.len(), pat.len());
    let mut i = 0;
    while i + m <= n {
        if hay[i..i + m] == pat[..] {
            let before_adj = i > 0 && is_ident_char(hay[i - 1]);
            let after_adj = i + m < n && is_ident_char(hay[i + m]);
            if before_adj || after_adj {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// DOCS rule: `Bridge` (CamelCase) or `bridge` (snake) only when part of an
/// identifier token. Case-sensitive so the all-caps frozen `BRIDGE_ERROR_*` code
/// names survive, and so `"Bridge"`, `` `Bridge` ``, `bridge.health`, and prose
/// are all spared.
fn doc_has_bridge_ident(line: &str) -> bool {
    has_adjacent_ident(line, "Bridge") || has_adjacent_ident(line, "bridge")
}

fn first_phase_brain_ident(line: &str) -> Option<&'static str> {
    PHASE_BRAIN_IDENTS
        .iter()
        .copied()
        .find(|id| line.contains(id))
}

fn first_retired_slug(line: &str) -> Option<&'static str> {
    RETIRED_DOC_SLUGS.iter().copied().find(|s| line.contains(s))
}

// ── scans ────────────────────────────────────────────────────────────────────

struct Violation {
    file: String,
    line_no: usize,
    kind: &'static str,
    text: String,
}

fn record(v: &mut Vec<Violation>, path: &Path, line_no: usize, kind: &'static str, line: &str) {
    let file = path
        .strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string();
    v.push(Violation {
        file,
        line_no,
        kind,
        text: line.trim().chars().take(160).collect(),
    });
}

fn scan_code(root: &str) -> Vec<Violation> {
    let base = repo_root().join(root);
    let mut files = Vec::new();
    walk(&base, &["rs", "py"], &mut files);
    let mut violations = Vec::new();
    for path in files {
        if is_excluded(&path, ENFORCEMENT_FILES) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            // A line may opt out of every code rule by carrying the frozen-value
            // annotation the anti-drift gate keys on (mirrors the CI script).
            if line.contains("FROZEN WIRE VALUE") {
                continue;
            }
            if code_has_bridge(line) {
                record(&mut violations, &path, idx + 1, "code:Bridge", line);
            }
            if has_scheduler_mind(line) {
                record(&mut violations, &path, idx + 1, "code:Mind", line);
            }
            if let Some(id) = first_phase_brain_ident(line) {
                record(&mut violations, &path, idx + 1, id, line);
            }
        }
    }
    violations
}

fn scan_docs() -> Vec<Violation> {
    let mut files = Vec::new();
    walk(&repo_root().join("docs"), &["md"], &mut files);
    files.push(repo_root().join("mkdocs.yml"));
    let mut violations = Vec::new();
    for path in files {
        if is_excluded(&path, DOC_ALLOWLIST) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            if doc_has_bridge_ident(line) {
                record(&mut violations, &path, idx + 1, "docs:Bridge-ident", line);
            }
            if let Some(slug) = first_retired_slug(line) {
                record(&mut violations, &path, idx + 1, slug, line);
            }
            if has_scheduler_mind(line) {
                record(&mut violations, &path, idx + 1, "docs:Mind", line);
            }
            if let Some(id) = first_phase_brain_ident(line) {
                record(&mut violations, &path, idx + 1, id, line);
            }
        }
    }
    violations
}

fn report(violations: &[Violation]) -> String {
    let kinds: BTreeSet<&str> = violations.iter().map(|v| v.kind).collect();
    let mut out = format!(
        "{} retired-terminology violations across {} distinct kinds:\n",
        violations.len(),
        kinds.len()
    );
    for v in violations.iter().take(40) {
        out.push_str(&format!(
            "  {}:{}  [{}]  {}\n",
            v.file, v.line_no, v.kind, v.text
        ));
    }
    if violations.len() > 40 {
        out.push_str(&format!("  … and {} more\n", violations.len() - 40));
    }
    out
}

// ── tests ────────────────────────────────────────────────────────────────────

#[test]
fn no_retired_identifiers_in_code() {
    let mut v = scan_code("src");
    v.extend(scan_code("tests"));
    assert!(
        v.is_empty(),
        "Code still spells retired terminology (Bridge / Mind / phase-brain).\n\
         The rename must be behavior-preserving and complete — no dangling old names.\n{}",
        report(&v)
    );
}

#[test]
fn no_retired_identifiers_in_docs() {
    let v = scan_docs();
    assert!(
        v.is_empty(),
        "Docs still spell retired identifier tokens (Bridge / Mind / phase-brain).\n\
         Only docs/reference/brain-terminology-migration.md and docs/whats-changed.md may.\n{}",
        report(&v)
    );
}

#[test]
fn required_new_identifiers_present() {
    let mut files = Vec::new();
    walk(&repo_root().join("src"), &["rs"], &mut files);
    let corpus: String = files
        .iter()
        .filter(|p| !is_excluded(p, ENFORCEMENT_FILES))
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect();

    let mut missing: Vec<&str> = REQUIRED_NEW_IDENTS
        .iter()
        .copied()
        .filter(|id| !corpus.contains(id))
        .collect();

    // `Brain` (the scheduler executive) must exist as a real struct declaration,
    // not merely as a substring of BrainIntrospectionReport / BrainJudgmentRecord.
    if !contains_word(&corpus, "struct Brain") {
        missing.push("struct Brain (scheduler executive)");
    }

    assert!(
        missing.is_empty(),
        "The unified-Brain rename has not introduced the canonical new names: {missing:?}\n\
         See docs/reference/brain-terminology-migration.md for the old→new map."
    );
}

#[test]
fn health_log_strings_reframed_to_reasoners() {
    let mut files = Vec::new();
    walk(&repo_root().join("src"), &["rs"], &mut files);
    let corpus: String = files
        .iter()
        .filter(|p| !is_excluded(p, ENFORCEMENT_FILES))
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect();

    // Retired daemon/health-probe wording that names phase "brains" must be gone.
    for retired in ["brains LLM-backed", "3 brains", "brains fell back"] {
        assert!(
            !corpus.contains(retired),
            "Retired health/log wording still present: {retired:?} — reframe to \"reasoners\"."
        );
    }

    // The new unified wording must be present.
    assert!(
        corpus.contains("reasoners LLM-backed"),
        "Expected the reframed health wording \"… reasoners LLM-backed …\" \
         (e.g. \"OODA daemon: brain online — orient/decide/act reasoners LLM-backed (no fallback)\")."
    );
}

#[test]
fn ci_terminology_gate_script_passes() {
    let script = repo_root().join("scripts/ci/check-terminology-drift.sh");
    assert!(
        script.exists(),
        "missing CI gate script: {}",
        script.display()
    );

    let output = match std::process::Command::new("bash").arg(&script).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skipping gate execution (bash unavailable): {e}");
            return;
        }
    };
    assert!(
        output.status.success(),
        "scripts/ci/check-terminology-drift.sh reported terminology drift:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
