//! Native (Python-free) acceptance test for the code-derived metacognitive
//! atlas deliverable — issue #4982.
//!
//! This is the machine-checkable half of the atlas TDD contract. Where the
//! shell harness (`scripts/verify-metacognitive-atlas.sh`) checks presentation
//! and cross-linking, this test binds the *diagram content* to the *source of
//! truth* so the atlas cannot silently drift from the code it documents. It
//! runs under the ordinary `cargo test` gate with std only — no Python, no
//! Graphviz, no mkdocs toolchain.
//!
//! What it guards (each `#[test]` is one acceptance criterion):
//!   1. `thread_roster_matches_source` — the thirteen `snake_case` thread
//!      labels derived from `ThreadName::ALL` (declaration order) in
//!      `src/ooda_brain/thread_reasoning_record.rs` all appear in the
//!      thread-drilldown diagram AND in the atlas page. Adding/removing a
//!      thread in source without updating the diagrams fails this test.
//!   2. `overseer_is_framed_as_unwired_sketch` — the Overseer diagram is drawn
//!      dashed and labeled a design sketch, and the underlying module still
//!      carries `#![allow(dead_code)]` (so the "not wired" framing stays true).
//!   3. `metacognition_flow_has_canonical_path` — the representative data-flow
//!      diagram carries the full `recipe -> record -> rail -> summary` token
//!      chain.
//!   4. `all_dot_sources_are_wellformed_digraphs` — the five committed `.dot`
//!      sources exist, declare a `digraph`, and have balanced braces.
//!   5. `atlas_renders_five_mermaid_diagrams` — exactly five inline Mermaid
//!      fences, matching the five required diagrams.
//!   6. `cross_links_are_reciprocal` — atlas <-> model links exist in both
//!      body text and front-matter `related:` blocks, and the atlas is present
//!      in the mkdocs nav.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

fn exists(rel: &str) -> bool {
    repo_root().join(rel).exists()
}

const ATLAS: &str = "docs/architecture/metacognitive-atlas.md";
const MODEL: &str = "docs/architecture/metacognitive-model.md";
const ROSTER_SRC: &str = "src/ooda_brain/thread_reasoning_record.rs";
const DRILLDOWN_DOT: &str = "docs/architecture/diagrams/thread-drilldown.dot";
const OVERSEER_DOT: &str = "docs/architecture/diagrams/overseer-sketch.dot";
const FLOW_DOT: &str = "docs/architecture/diagrams/metacognition-flow.dot";

const DOT_SOURCES: [&str; 5] = [
    "docs/architecture/diagrams/system-map.dot",
    "docs/architecture/diagrams/thread-drilldown.dot",
    "docs/architecture/diagrams/ooda-loop.dot",
    "docs/architecture/diagrams/overseer-sketch.dot",
    "docs/architecture/diagrams/metacognition-flow.dot",
];

/// Extract the ordered variant identifiers inside the `ThreadName::ALL` array
/// literal, e.g. `Self::Metacognition` -> `Metacognition`.
fn all_variants_in_declaration_order(src: &str) -> Vec<String> {
    let start = src
        .find("pub const ALL")
        .expect("ThreadName::ALL declaration not found in source");
    // Anchor on the array-literal `= [`, not the `[ThreadName; 13]` type bracket.
    let eq = src[start..]
        .find("= [")
        .map(|i| start + i)
        .expect("no '= [' array literal after ThreadName::ALL");
    let open = src[eq..]
        .find('[')
        .map(|i| eq + i)
        .expect("no '[' in ThreadName::ALL array literal");
    let close = src[open..]
        .find(']')
        .map(|i| open + i)
        .expect("no ']' closing ThreadName::ALL");
    let body = &src[open + 1..close];
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("Self::"))
        .map(|rest| rest.trim_end_matches(',').trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Map each `Self::Variant => "label",` arm in `fn label(self)` to its
/// snake_case label so the diagram binding tracks the source of truth.
fn variant_to_label(src: &str, variant: &str) -> String {
    let needle = format!("Self::{variant} =>");
    let arm_start = src
        .find(&needle)
        .unwrap_or_else(|| panic!("no label() arm for variant Self::{variant}"));
    let after = &src[arm_start + needle.len()..];
    let q1 = after.find('"').expect("no opening quote in label arm");
    let q2 = after[q1 + 1..]
        .find('"')
        .expect("no closing quote in label arm");
    after[q1 + 1..q1 + 1 + q2].to_string()
}

fn thread_labels_from_source() -> Vec<String> {
    let src = read(ROSTER_SRC);
    let variants = all_variants_in_declaration_order(&src);
    assert_eq!(
        variants.len(),
        13,
        "ThreadName::ALL must declare exactly 13 threads; found {} ({:?})",
        variants.len(),
        variants
    );
    variants.iter().map(|v| variant_to_label(&src, v)).collect()
}

/// True when `haystack` contains `word` bounded by non-word characters, so
/// `narrative` does not spuriously match inside `narratives`/`prenarrative`.
fn contains_word(haystack: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(word) {
        let i = from + pos;
        let before_ok = i == 0 || !is_word(haystack[..i].chars().next_back().unwrap());
        let after_idx = i + word.len();
        let after_ok =
            after_idx >= haystack.len() || !is_word(haystack[after_idx..].chars().next().unwrap());
        if before_ok && after_ok {
            return true;
        }
        from = i + word.len();
    }
    false
}

#[test]
fn thread_roster_matches_source() {
    let labels = thread_labels_from_source();
    let drill = read(DRILLDOWN_DOT);
    let atlas = read(ATLAS);

    let mut missing_in_dot = Vec::new();
    let mut missing_in_atlas = Vec::new();
    for label in &labels {
        if !contains_word(&drill, label) {
            missing_in_dot.push(label.clone());
        }
        if !contains_word(&atlas, label) {
            missing_in_atlas.push(label.clone());
        }
    }
    assert!(
        missing_in_dot.is_empty(),
        "threads present in ThreadName::ALL but missing from thread-drilldown.dot: {missing_in_dot:?}"
    );
    assert!(
        missing_in_atlas.is_empty(),
        "threads present in ThreadName::ALL but missing from the atlas page: {missing_in_atlas:?}"
    );
}

#[test]
fn overseer_is_framed_as_unwired_sketch() {
    let dot = read(OVERSEER_DOT).to_lowercase();
    assert!(
        dot.contains("dashed"),
        "overseer-sketch.dot must render the design sketch with dashed styling"
    );
    assert!(
        dot.contains("design sketch") || dot.contains("not wired") || dot.contains("dead_code"),
        "overseer-sketch.dot must label itself an unwired design sketch"
    );

    let atlas = read(ATLAS).to_lowercase();
    assert!(
        atlas.contains("design sketch") && atlas.contains("not wired"),
        "atlas prose must frame the Overseer as an unwired design sketch"
    );

    // The framing must stay grounded in source: the module is dead code.
    let overseer_mod = read("src/overseer/mod.rs");
    assert!(
        overseer_mod.contains("#![allow(dead_code)]"),
        "src/overseer/mod.rs no longer carries #![allow(dead_code)]; \
         re-verify the atlas 'unwired sketch' framing"
    );
}

#[test]
fn metacognition_flow_has_canonical_path() {
    let flow = read(FLOW_DOT);
    for token in [
        "RecipeRunnerInvoker",
        "run_reflective_thread",
        "ThreadReasoningRecord",
        "thread-reasoning/v1",
        "rail",
        "ThreadOutcome",
    ] {
        assert!(
            flow.contains(token),
            "metacognition-flow.dot is missing canonical-path token '{token}'"
        );
    }
}

#[test]
fn all_dot_sources_are_wellformed_digraphs() {
    for rel in DOT_SOURCES {
        assert!(exists(rel), "missing committed Graphviz source: {rel}");
        let src = read(rel);
        assert!(
            src.contains("digraph"),
            "{rel} does not declare a 'digraph'"
        );
        let opens = src.matches('{').count();
        let closes = src.matches('}').count();
        assert_eq!(
            opens, closes,
            "{rel} has unbalanced braces (open={opens}, close={closes})"
        );
    }
}

#[test]
fn atlas_renders_five_mermaid_diagrams() {
    let atlas = read(ATLAS);

    // Walk fenced code blocks in document order, tracking open/close state so
    // that every ```mermaid block is verified to be individually closed by a
    // bare ``` fence before the next block opens. This is stricter than a
    // global even-count of fences, which could pass even if a mermaid block
    // were closed by another info-string fence (e.g. ```mermaid ... ```bash).
    let mut inside_fence = false;
    let mut mermaid_blocks = 0usize;
    for line in atlas.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("```") {
            continue;
        }
        let info = trimmed.trim_start_matches('`').trim();
        if inside_fence {
            assert_eq!(
                info, "",
                "code fence must be closed by a bare ``` line, found: {line:?}"
            );
            inside_fence = false;
        } else {
            if info == "mermaid" {
                mermaid_blocks += 1;
            }
            inside_fence = true;
        }
    }

    assert!(
        !inside_fence,
        "atlas has an unclosed code fence; Mermaid/code blocks will not render"
    );
    assert_eq!(
        mermaid_blocks, 5,
        "atlas must render exactly 5 inline Mermaid diagrams; found {mermaid_blocks}"
    );
}

#[test]
fn cross_links_are_reciprocal() {
    let atlas = read(ATLAS);
    let model = read(MODEL);

    assert!(
        atlas.contains("metacognitive-model.md"),
        "atlas must link to metacognitive-model.md"
    );
    assert!(
        model.contains("metacognitive-atlas.md"),
        "model doc must link back to metacognitive-atlas.md"
    );

    // Reciprocal links must also live in each front-matter `related:` block.
    assert!(
        frontmatter_related_mentions(&atlas, "metacognitive-model.md"),
        "atlas front-matter 'related:' must reference the model doc"
    );
    assert!(
        frontmatter_related_mentions(&model, "metacognitive-atlas.md"),
        "model front-matter 'related:' must reference the atlas"
    );

    // The atlas must be reachable from the mkdocs nav (no orphan page).
    let nav = read("mkdocs.yml");
    assert!(
        nav.contains("architecture/metacognitive-atlas.md"),
        "mkdocs.yml nav must reference the atlas page"
    );
}

/// True when the leading YAML front-matter block contains a `related:` list
/// entry mentioning `needle`.
fn frontmatter_related_mentions(doc: &str, needle: &str) -> bool {
    let fm = front_matter_block(doc);
    let mut in_related = false;
    for line in fm.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("related:") {
            in_related = true;
            if trimmed.contains(needle) {
                return true;
            }
            continue;
        }
        if in_related {
            // List items are indented `- ...`; a new top-level key ends the block.
            let indented = line.starts_with(char::is_whitespace);
            if !indented && !trimmed.is_empty() {
                in_related = false;
            } else if trimmed.starts_with('-') && trimmed.contains(needle) {
                return true;
            }
        }
    }
    false
}

fn front_matter_block(doc: &str) -> &str {
    if !doc.starts_with("---") {
        return "";
    }
    // Skip the first delimiter line, then find the closing `---`.
    let after_first = match doc.find('\n') {
        Some(i) => &doc[i + 1..],
        None => return "",
    };
    match after_first.find("\n---") {
        Some(i) => &after_first[..i],
        None => "",
    }
}
