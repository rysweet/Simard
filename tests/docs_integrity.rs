//! Native (Python-free) documentation-integrity gate — issue #3181.
//!
//! Replaces the former `mkdocs build --strict` docs CI job (`.github/workflows/
//! docs.yml`), which required a Python `mkdocs` toolchain. Simard is a pure-Rust
//! daemon, so the link/nav integrity the strict build enforced now runs here
//! under the main `cargo test` gate — with no Python:
//!
//!   1. `mkdocs_nav_entries_resolve_to_existing_files` — every `*.md` entry in
//!      the `mkdocs.yml` nav points at a file that exists under `docs/`
//!      (mirrors mkdocs' broken-nav / `nav.omitted_files` integrity).
//!   2. `no_dead_intrarepo_markdown_links` — every relative Markdown link to a
//!      `*.md` file resolves to an existing file (mirrors mkdocs'
//!      `links.not_found` integrity).
//!
//! `mkdocs.yml` is retained purely as the human-maintained navigation manifest
//! (YAML data, not code); nothing invokes the Python `mkdocs` tool anymore. The
//! checks are std-only and shaped like the `grep`/`find` an operator would run
//! by hand, so a human running the equivalent gets the same answer.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn docs_dir() -> PathBuf {
    repo_root().join("docs")
}

/// Recursively collect every `*.md` file under `dir`.
fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

/// Extract every `.md` token from `mkdocs.yml`. A token is a maximal run of
/// `[A-Za-z0-9_./-]` that ends in `.md` — the shape of a nav path
/// (`architecture/cognitive-memory.md`, `ROADMAP.md`, ...).
fn mkdocs_md_tokens(contents: &str) -> Vec<String> {
    let is_tok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-');
    let chars: Vec<char> = contents.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if is_tok(chars[i]) {
            let start = i;
            while i < chars.len() && is_tok(chars[i]) {
                i += 1;
            }
            let tok: String = chars[start..i].iter().collect();
            // A nav path ends in `.md` AND has a non-empty filename stem, so a
            // glob or comment fragment like `*.md` / `docs/*.md` (which the
            // scanner sees as the bare token `.md`) is not mistaken for a page.
            if let Some(stem) = tok.strip_suffix(".md")
                && !stem.rsplit('/').next().unwrap_or("").is_empty()
            {
                tokens.push(tok);
            }
        } else {
            i += 1;
        }
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

/// Extract the raw target of every inline Markdown link `](target)` in `content`
/// (the target is everything between `](` and the next `)` or end-of-line).
fn inline_link_targets(content: &str) -> Vec<String> {
    let bytes = content.as_bytes();
    let mut targets = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b'(' {
            let start = i + 2;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b')' && bytes[j] != b'\n' {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b')' {
                targets.push(content[start..j].to_string());
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    targets
}

/// The `*.md` file path a link points at, if it is an intra-repo relative link.
/// Returns `None` for external links (`http(s)://`, `mailto:`), pure anchors
/// (`#section`), absolute paths (`/...`), and non-`.md` targets (directory or
/// asset links, which mkdocs resolves differently and are out of scope here).
fn relative_md_target(raw: &str) -> Option<String> {
    // Strip an optional link title: `path "Title"` / `path 'Title'`.
    let target = raw.split_whitespace().next().unwrap_or("").trim();
    if target.is_empty()
        || target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with('#')
        || target.starts_with('/')
    {
        return None;
    }
    // Drop the anchor fragment: `foo.md#section` -> `foo.md`.
    let base = target.split('#').next().unwrap_or("");
    if base.ends_with(".md") {
        Some(base.to_string())
    } else {
        None
    }
}

#[test]
fn mkdocs_nav_entries_resolve_to_existing_files() {
    let mkdocs = repo_root().join("mkdocs.yml");
    let contents = fs::read_to_string(&mkdocs).unwrap_or_else(|e| {
        panic!(
            "[simard] docs-integrity: cannot read mkdocs.yml (the nav manifest) at {}: {e}",
            mkdocs.display()
        )
    });

    // Nav paths are relative to mkdocs' `docs_dir`, which defaults to `docs/`.
    let mut missing: Vec<String> = mkdocs_md_tokens(&contents)
        .into_iter()
        .filter(|tok| !docs_dir().join(tok).is_file())
        .collect();
    missing.sort();

    assert!(
        missing.is_empty(),
        "[simard] docs-integrity: {} mkdocs.yml nav entr(y/ies) point at a file that does not \
         exist under docs/. A broken nav entry would have red the old `mkdocs build --strict` \
         gate; it now reds `cargo test` (issue #3181). Fix the nav path or restore the page:\n{}",
        missing.len(),
        missing
            .iter()
            .map(|t| format!("  docs/{t}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn no_dead_intrarepo_markdown_links() {
    let mut md_files = Vec::new();
    collect_markdown(&docs_dir(), &mut md_files);
    assert!(
        !md_files.is_empty(),
        "[simard] docs-integrity: found no *.md files under docs/ — the walk is broken"
    );

    let mut dead: Vec<String> = Vec::new();
    for file in &md_files {
        let content = fs::read_to_string(file).unwrap_or_default();
        let dir = file.parent().unwrap_or_else(|| Path::new("."));
        for raw in inline_link_targets(&content) {
            let Some(base) = relative_md_target(&raw) else {
                continue;
            };
            // The OS resolves `..` during stat, so a lexical join is enough.
            if !dir.join(&base).is_file() {
                let rel = file
                    .strip_prefix(repo_root())
                    .unwrap_or(file.as_path())
                    .display();
                dead.push(format!("  {rel} -> {base}"));
            }
        }
    }
    dead.sort();
    dead.dedup();

    assert!(
        dead.is_empty(),
        "[simard] docs-integrity: {} dead intra-repo Markdown link(s) to a missing *.md file. \
         These would have red the old `mkdocs build --strict` gate; they now red `cargo test` \
         (issue #3181). Fix the link target or restore the page:\n{}",
        dead.len(),
        dead.join("\n")
    );
}

#[test]
fn mkdocs_token_extractor_boundary_logic() {
    // Pure-function pin so the extractor's boundary logic is verified
    // independently of the tree.
    let toks = mkdocs_md_tokens("  - Home: index.md\n  - A: architecture/cognitive-memory.md\n");
    assert!(toks.contains(&"index.md".to_string()));
    assert!(toks.contains(&"architecture/cognitive-memory.md".to_string()));
    // A bare word ending in a non-.md extension is not a nav path.
    assert!(mkdocs_md_tokens("site_name: Simard\nrepo: rysweet/Simard\n").is_empty());
    // A glob / comment fragment like `docs/*.md` is not a nav page.
    assert!(
        mkdocs_md_tokens("    omitted_files: warn  # every docs/*.md must appear in nav")
            .is_empty()
    );
}

#[test]
fn relative_md_target_classifier() {
    // Intra-repo relative .md links are validated…
    assert_eq!(
        relative_md_target("guide/foo.md"),
        Some("guide/foo.md".into())
    );
    assert_eq!(
        relative_md_target("../ops/bar.md#anchor"),
        Some("../ops/bar.md".into())
    );
    assert_eq!(
        relative_md_target("baz.md \"Title\""),
        Some("baz.md".into())
    );
    // …while external / anchor-only / absolute / non-.md targets are skipped.
    assert_eq!(relative_md_target("https://example.com/x.md"), None);
    assert_eq!(relative_md_target("mailto:a@b.md"), None);
    assert_eq!(relative_md_target("#section"), None);
    assert_eq!(relative_md_target("/abs/path.md"), None);
    assert_eq!(relative_md_target("images/diagram.png"), None);
    assert_eq!(relative_md_target("subdir/"), None);
}
