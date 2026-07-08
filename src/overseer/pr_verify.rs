//! M2 **hard gate** — the pr-verify safety diff-scans (design doc §pr-verify
//! checklist, items 3–6). These are the NEW, additive checks that did not exist
//! before this milestone; **no merge capability ships before they do and are
//! unit-tested** (crusty review risk #2 / operator hard-gate #5).
//!
//! Every scan is a **pure** function over a unified diff (`gh pr diff` output),
//! so the whole merge-safety surface is testable on fixture diffs with zero
//! network. The scans:
//!
//! | # | Check | This module |
//! |---|-------|-------------|
//! | 3 | No `Bridge` naming in **added** lines | [`scan_no_bridge_naming`] |
//! | 4 | No stray `print!`/`println!`/`eprintln!`/`eprint!` in added `src/**` | [`scan_no_stray_prints`] |
//! | 5 | Additive / non-breaking — no **removed** `pub` items | [`scan_additive_no_removed_pub`] |
//! | 6 | PRD (`Specs/ProductArchitecture.md`) preserved — no removed lines | [`scan_prd_preserved`] |
//! | 8 | No **added** point-in-time report doc (G4 durable-docs policy) | [`scan_no_point_in_time_report_docs`] |
//!
//! Items 1–2 (CI-green / mergeable / base-allowlist) reuse
//! `stewardship::merge_authority::evaluate_objective_gates`; item 7 reuses
//! `review_pipeline::should_commit`. Those are wired in
//! [`merge_ops`](crate::overseer::merge_ops), not here.

use crate::overseer::capabilities::CheckItem;
use std::collections::{HashMap, HashSet};

/// The PRD file whose content must be preserved (check #6).
pub const PRD_PATH: &str = "Specs/ProductArchitecture.md";

/// One offending location found by a diff scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFinding {
    /// Path (new-side) the finding is in.
    pub file: String,
    /// Best-effort new-file line number (`None` for removed-line findings).
    pub line: Option<usize>,
    /// The offending source text (trimmed).
    pub text: String,
}

impl DiffFinding {
    fn describe(&self) -> String {
        match self.line {
            Some(n) => format!("{}:{} — {}", self.file, n, self.text),
            None => format!("{} — {}", self.file, self.text),
        }
    }
}

/// One parsed line of a unified diff, classified with its new-side line number.
struct DiffLine<'a> {
    file: &'a str,
    kind: LineKind,
    /// New-file line number for added/context lines; `None` for removed.
    new_line: Option<usize>,
    /// The content after the leading +/-/space marker.
    content: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Added,
    Removed,
    Context,
}

/// Walk a unified diff, invoking `f` for every added/removed/context content
/// line with its current file and new-side line number. A single tiny parser
/// shared by all four scans (one place to get header/hunk handling right).
fn for_each_diff_line<'a>(diff: &'a str, mut f: impl FnMut(DiffLine<'a>)) {
    let mut current_file: &str = "";
    let mut new_line: usize = 0;

    for raw in diff.lines() {
        // File header: `+++ b/<path>` names the new-side file. `/dev/null`
        // (deletion) yields an empty file so src/PRD scans simply skip it.
        if let Some(rest) = raw.strip_prefix("+++ ") {
            current_file = normalize_diff_path(rest);
            continue;
        }
        if raw.starts_with("--- ") {
            // Old-side header — ignore (new-side is authoritative for paths).
            continue;
        }
        if raw.starts_with("diff --git") || raw.starts_with("index ") {
            continue;
        }
        // Hunk header: `@@ -old,cnt +new,cnt @@` resets the new-side counter.
        if let Some(start) = parse_hunk_new_start(raw) {
            new_line = start;
            continue;
        }

        if let Some(content) = raw.strip_prefix('+') {
            // `+++` already handled above.
            f(DiffLine {
                file: current_file,
                kind: LineKind::Added,
                new_line: Some(new_line),
                content,
            });
            new_line += 1;
        } else if let Some(content) = raw.strip_prefix('-') {
            f(DiffLine {
                file: current_file,
                kind: LineKind::Removed,
                new_line: None,
                content,
            });
            // Removed lines do not advance the new-side counter.
        } else if let Some(content) = raw.strip_prefix(' ') {
            f(DiffLine {
                file: current_file,
                kind: LineKind::Context,
                new_line: Some(new_line),
                content,
            });
            new_line += 1;
        }
        // Any other line (e.g. "\ No newline at end of file") is ignored.
    }
}

/// Strip the `a/` or `b/` prefix from a diff path and any trailing tab-decorated
/// metadata (`+++ b/foo.rs\t2026-...`). `/dev/null` maps to an empty path.
fn normalize_diff_path(rest: &str) -> &str {
    let path = rest.split('\t').next().unwrap_or(rest).trim();
    if path == "/dev/null" {
        return "";
    }
    path.strip_prefix("b/")
        .or_else(|| path.strip_prefix("a/"))
        .unwrap_or(path)
}

/// Parse the new-side start line from a hunk header `@@ -a,b +c,d @@`.
fn parse_hunk_new_start(line: &str) -> Option<usize> {
    let rest = line.strip_prefix("@@ ")?;
    // Find the `+c,d` token.
    let plus = rest.split_whitespace().find(|t| t.starts_with('+'))?;
    let digits = &plus[1..];
    let start = digits.split(',').next().unwrap_or(digits);
    start.parse::<usize>().ok()
}

// ─────────────────────────── scan #3: no Bridge ────────────────────────────

/// Check #3: no `Bridge` naming introduced in **added** lines. Scoped to added
/// lines only, so pre-existing `terminal_engineer_bridge` / `OodaBridges` code
/// (which predates the operator's no-`Bridge` preference) is untouched. Uses a
/// case-sensitive `Bridge` substring so CamelCase identifiers (`HttpBridge`,
/// `FooBridge`) are caught while the lowercase word (`abridged`, `cambridge`)
/// is not.
pub fn scan_no_bridge_naming(diff: &str) -> Vec<DiffFinding> {
    let mut out = Vec::new();
    for_each_diff_line(diff, |dl| {
        if dl.kind == LineKind::Added && dl.content.contains("Bridge") {
            out.push(DiffFinding {
                file: dl.file.to_string(),
                line: dl.new_line,
                text: dl.content.trim().to_string(),
            });
        }
    });
    out
}

// ─────────────────────────── scan #4: no stray prints ──────────────────────

const PRINT_MACROS: &[&str] = &["println!", "print!", "eprintln!", "eprint!"];

/// Check #4: no stray `print!`/`println!`/`eprintln!`/`eprint!` in added lines
/// of files under `src/`. Test files, examples, and build scripts are out of
/// scope (they legitimately print).
pub fn scan_no_stray_prints(diff: &str) -> Vec<DiffFinding> {
    let mut out = Vec::new();
    for_each_diff_line(diff, |dl| {
        if dl.kind != LineKind::Added || !is_src_rust(dl.file) {
            return;
        }
        if PRINT_MACROS.iter().any(|m| dl.content.contains(m)) {
            out.push(DiffFinding {
                file: dl.file.to_string(),
                line: dl.new_line,
                text: dl.content.trim().to_string(),
            });
        }
    });
    out
}

fn is_src_rust(path: &str) -> bool {
    path.starts_with("src/") && path.ends_with(".rs")
}

// ─────────────────────────── scan #5: additive only ────────────────────────

const PUB_PREFIXES: &[&str] = &[
    "pub fn ",
    "pub struct ",
    "pub enum ",
    "pub trait ",
    "pub const ",
    "pub static ",
    "pub type ",
    "pub mod ",
    "pub use ",
    "pub(crate) fn ",
    "pub async fn ",
];

/// Check #5: additive / non-breaking — no **removed** `pub` item declarations.
/// A removed public item is a potential breaking API change; the Overseer only
/// merges additive PRs. Heuristic and conservative: a `pub` item that is merely
/// moved will flag, which is the safe direction (a human reviews the escalation).
pub fn scan_additive_no_removed_pub(diff: &str) -> Vec<DiffFinding> {
    let mut out = Vec::new();
    for_each_diff_line(diff, |dl| {
        if dl.kind != LineKind::Removed {
            return;
        }
        let trimmed = dl.content.trim_start();
        if PUB_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            out.push(DiffFinding {
                file: dl.file.to_string(),
                line: None,
                text: dl.content.trim().to_string(),
            });
        }
    });
    out
}

// ─────────────────────────── scan #6: PRD preserved ────────────────────────

/// Check #6: the PRD (`Specs/ProductArchitecture.md`) is preserved — the diff
/// removes no lines from it. Additions (new sections) are fine; deletions /
/// rewrites are not.
pub fn scan_prd_preserved(diff: &str) -> Vec<DiffFinding> {
    let mut out = Vec::new();
    for_each_diff_line(diff, |dl| {
        if dl.kind == LineKind::Removed && dl.file == PRD_PATH {
            out.push(DiffFinding {
                file: dl.file.to_string(),
                line: None,
                text: dl.content.trim().to_string(),
            });
        }
    });
    out
}

// ─────────────── scan #8: no point-in-time report docs (G4) ────────────────

/// Reserved directories whose newly-added `.md` files are, by filing location
/// alone, point-in-time report docs (G4). A doc that lives here is a report by
/// convention regardless of its title.
const REPORT_DIRS: &[&str] = &["docs/investigation/", "docs/reports/", "docs/runs/"];

/// Report-KIND title markers, matched against a newly-added doc's **title
/// surface only** — its front-matter `type:` / `doc_type:` / `title:` values and
/// its first `# ` H1 — never its body prose. They are deliberately SPECIFIC
/// multi-word report kinds ("investigation report", not bare "investigation" or
/// "report"), so a durable doc that merely mentions reports in its title (e.g. a
/// reference doc titled "No point-in-time report docs") does not self-flag. This
/// is the doc-TYPE-not-topic rule: the same subsystem can host a good durable
/// design doc and a banned report doc; only the latter names a report kind.
const REPORT_TITLE_MARKERS: &[&str] = &[
    "investigation report",
    "testing report",
    "test report",
    "diagnosis report",
    "diagnostic report",
    "diagnostics report",
    "recurrence report",
    "blockage report",
    "benchmark snapshot",
    "measured-rate",
    "measured rate",
    "snapshot findings",
    "postmortem",
    "post-mortem",
];

/// The guidance attached to a G4 finding: where the content belongs instead.
const POINT_IN_TIME_GUIDANCE: &str = "point-in-time report doc — record the finding in a GitHub issue \
(consolidate recurrences into one tracking issue) and/or memory, not a committed repo doc; \
durable feature/architecture docs are encouraged";

/// The set of new-side file paths a diff ADDS (creates). A file counts as added
/// only when its old side is `/dev/null` — the deterministic, diff-local signal
/// git emits for a creation (`--- /dev/null`, alongside `new file mode`). Edits
/// and deletions are excluded, which is exactly what scopes the G4 scan to
/// added-only so pre-existing docs and durable-doc edits are never touched.
pub fn newly_added_files(diff: &str) -> HashSet<String> {
    let mut added = HashSet::new();
    let mut old_is_dev_null = false;
    for raw in diff.lines() {
        if let Some(rest) = raw.strip_prefix("--- ") {
            old_is_dev_null = normalize_diff_path(rest).is_empty();
        } else if let Some(rest) = raw.strip_prefix("+++ ") {
            let new_path = normalize_diff_path(rest);
            if old_is_dev_null && !new_path.is_empty() {
                added.insert(new_path.to_string());
            }
            old_is_dev_null = false;
        }
    }
    added
}

fn is_markdown(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

/// Extract the trimmed value of a YAML front-matter `key: value` line, or `None`
/// when the line is not that field.
fn front_matter_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim().strip_prefix(key)?.strip_prefix(':')?;
    Some(rest.trim())
}

/// True when a newly-added doc's TITLE SURFACE names a report KIND. Only the
/// optional YAML front-matter (`type` / `doc_type` / `title`) and the first `# `
/// H1 are consulted — body prose is never read, so a durable doc that merely
/// discusses an investigation in its body cannot flag.
fn title_names_a_report(added_lines: &[&str]) -> bool {
    let mut title_texts: Vec<String> = Vec::new();
    let mut idx = 0;

    // Optional YAML front-matter block delimited by `---` fences.
    if added_lines.first().map(|l| l.trim()) == Some("---") {
        idx = 1;
        while idx < added_lines.len() && added_lines[idx].trim() != "---" {
            for key in ["type", "doc_type", "title"] {
                if let Some(value) = front_matter_value(added_lines[idx], key) {
                    title_texts.push(value.to_lowercase());
                    break;
                }
            }
            idx += 1;
        }
        if idx < added_lines.len() {
            idx += 1; // step past the closing `---`
        }
    }

    // First markdown H1 after any front-matter.
    for line in &added_lines[idx.min(added_lines.len())..] {
        if let Some(h1) = line.trim_start().strip_prefix("# ") {
            title_texts.push(h1.to_lowercase());
            break;
        }
    }

    title_texts
        .iter()
        .any(|t| REPORT_TITLE_MARKERS.iter().any(|m| t.contains(m)))
}

/// Check #8 (G4): flag a PR that ADDS a point-in-time report doc. Added-only,
/// `.md`-only, and report-TYPED (by reserved directory or by title kind), so
/// pre-existing docs, durable feature/architecture docs, and edits to any doc
/// are never flagged. Each finding routes the author to a GitHub issue/memory.
pub fn scan_no_point_in_time_report_docs(diff: &str) -> Vec<DiffFinding> {
    let added = newly_added_files(diff);

    // Collect each added `.md` file's added content lines, in diff order.
    let mut per_file: HashMap<String, Vec<&str>> = HashMap::new();
    for_each_diff_line(diff, |dl| {
        if dl.kind == LineKind::Added && is_markdown(dl.file) && added.contains(dl.file) {
            per_file
                .entry(dl.file.to_string())
                .or_default()
                .push(dl.content);
        }
    });

    // Deterministic iteration over the added `.md` files.
    let mut md_files: Vec<&String> = added.iter().filter(|f| is_markdown(f)).collect();
    md_files.sort();

    let empty: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for file in md_files {
        let by_dir = REPORT_DIRS.iter().any(|d| file.starts_with(d));
        let by_title = !by_dir && title_names_a_report(per_file.get(file).unwrap_or(&empty));
        if by_dir || by_title {
            out.push(DiffFinding {
                file: file.clone(),
                line: None,
                text: POINT_IN_TIME_GUIDANCE.to_string(),
            });
        }
    }
    out
}

// ─────────────────────────── checklist assembly ────────────────────────────

/// Run all additive diff-scans and return one [`CheckItem`] per scan. A scan
/// passes when it finds no violations; the note names the offenders.
pub fn run_diff_scans(diff: &str) -> Vec<CheckItem> {
    vec![
        finding_check(
            "no-Bridge-naming (added lines)",
            scan_no_bridge_naming(diff),
        ),
        finding_check("no-stray-print (added src/**)", scan_no_stray_prints(diff)),
        finding_check(
            "additive (no removed pub items)",
            scan_additive_no_removed_pub(diff),
        ),
        finding_check("PRD preserved", scan_prd_preserved(diff)),
        finding_check(
            "no-point-in-time-report-docs (added .md)",
            scan_no_point_in_time_report_docs(diff),
        ),
    ]
}

fn finding_check(name: &str, findings: Vec<DiffFinding>) -> CheckItem {
    if findings.is_empty() {
        CheckItem {
            name: name.to_string(),
            passed: true,
            note: "ok".to_string(),
        }
    } else {
        let note = findings
            .iter()
            .take(5)
            .map(DiffFinding::describe)
            .collect::<Vec<_>>()
            .join("; ");
        CheckItem {
            name: name.to_string(),
            passed: false,
            note: format!("{} violation(s): {note}", findings.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── #3: Bridge ───────────────────────────────────────────────────────────

    #[test]
    fn bridge_flags_added_camelcase_lines_only() {
        let diff = "\
diff --git a/src/foo.rs b/src/foo.rs
+++ b/src/foo.rs
@@ -1,2 +1,4 @@
 fn keep() {}
+struct PaymentBridge; // new camelcase type — must flag
+let abridged = 1; // lowercase word, must NOT flag
-struct OldBridge; // removed line — not scanned by #3
";
        let f = scan_no_bridge_naming(diff);
        assert_eq!(f.len(), 1, "only the added CamelCase Bridge flags");
        assert_eq!(f[0].file, "src/foo.rs");
        assert!(f[0].text.contains("PaymentBridge"));
    }

    #[test]
    fn bridge_clean_diff_passes() {
        let diff = "\
+++ b/src/foo.rs
@@ -1,1 +1,2 @@
 fn keep() {}
+fn reasoner_orient() {}
";
        assert!(scan_no_bridge_naming(diff).is_empty());
    }

    // ── #4: prints ───────────────────────────────────────────────────────────

    #[test]
    fn stray_print_flags_added_src_only() {
        let diff = "\
+++ b/src/foo.rs
@@ -1,1 +1,3 @@
 fn keep() {}
+    println!(\"debug\");
+    tracing::info!(\"ok\");
+++ b/tests/bar.rs
@@ -1,1 +1,2 @@
 fn t() {}
+    println!(\"tests may print\");
";
        let f = scan_no_stray_prints(diff);
        assert_eq!(f.len(), 1, "only the src/ println! flags, not tests/");
        assert_eq!(f[0].file, "src/foo.rs");
    }

    #[test]
    fn all_print_macros_detected() {
        let diff = "\
+++ b/src/foo.rs
@@ -1,1 +1,5 @@
 fn keep() {}
+print!(\"a\");
+println!(\"b\");
+eprint!(\"c\");
+eprintln!(\"d\");
";
        assert_eq!(scan_no_stray_prints(diff).len(), 4);
    }

    // ── #5: additive ─────────────────────────────────────────────────────────

    #[test]
    fn removed_pub_item_flags() {
        let diff = "\
+++ b/src/foo.rs
@@ -1,4 +1,2 @@
-pub fn removed_api() {}
-    pub struct RemovedType;
 fn private_keep() {}
-fn removed_private() {} // not pub — ok
+pub fn added_api() {}
";
        let f = scan_additive_no_removed_pub(diff);
        assert_eq!(f.len(), 2, "two removed pub items; private removal is fine");
    }

    #[test]
    fn purely_additive_diff_passes_additive_scan() {
        let diff = "\
+++ b/src/foo.rs
@@ -1,1 +1,3 @@
 fn keep() {}
+pub fn added_api() {}
+pub struct AddedType;
";
        assert!(scan_additive_no_removed_pub(diff).is_empty());
    }

    // ── #6: PRD ──────────────────────────────────────────────────────────────

    #[test]
    fn prd_removed_line_flags_but_addition_is_fine() {
        let diff = format!(
            "\
+++ b/{PRD_PATH}
@@ -1,3 +1,3 @@
 # Product Architecture
-A load-bearing invariant that must be preserved.
+A rewritten invariant.
+A brand-new additive section.
"
        );
        let f = scan_prd_preserved(&diff);
        assert_eq!(f.len(), 1, "only the removed PRD line flags");
    }

    #[test]
    fn prd_addition_only_passes() {
        let diff = format!(
            "\
+++ b/{PRD_PATH}
@@ -1,1 +1,2 @@
 # Product Architecture
+## A new additive section
"
        );
        assert!(scan_prd_preserved(&diff).is_empty());
    }

    #[test]
    fn removed_lines_in_other_files_do_not_touch_prd_scan() {
        let diff = "\
+++ b/src/foo.rs
@@ -1,2 +1,1 @@
-let x = 1;
 fn keep() {}
";
        assert!(scan_prd_preserved(diff).is_empty());
    }

    // ── checklist assembly ───────────────────────────────────────────────────

    #[test]
    fn clean_additive_diff_passes_every_scan() {
        let diff = "\
+++ b/src/overseer/new_thing.rs
@@ -0,0 +1,3 @@
+pub fn reasoner_step() {}
+pub struct Observation;
+// orient-decide-act, no forbidden tokens
";
        let checks = run_diff_scans(diff);
        assert_eq!(checks.len(), 5);
        assert!(
            checks.iter().all(|c| c.passed),
            "a clean additive diff passes all five scans: {checks:?}"
        );
    }

    #[test]
    fn dirty_diff_fails_the_right_scans() {
        let diff = format!(
            "\
+++ b/src/foo.rs
@@ -1,2 +1,3 @@
-pub fn removed_api() {{}}
+struct HttpBridge;
+    println!(\"noise\");
+++ b/{PRD_PATH}
@@ -1,1 +1,1 @@
-Invariant.
"
        );
        let checks = run_diff_scans(&diff);
        let failed: Vec<&str> = checks
            .iter()
            .filter(|c| !c.passed)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(failed.len(), 4, "all four scans should fail: {checks:?}");
    }

    #[test]
    fn hunk_line_numbers_are_tracked() {
        let diff = "\
+++ b/src/foo.rs
@@ -10,2 +10,3 @@
 context
+struct WsBridge;
";
        let f = scan_no_bridge_naming(diff);
        assert_eq!(
            f[0].line,
            Some(11),
            "added line after one context line at 10"
        );
    }

    // ── #8: no point-in-time report docs (G4) ────────────────────────────────
    //
    // TDD (Step 7 — write tests first): these reference `newly_added_files` and
    // `scan_no_point_in_time_report_docs`, which the implementation step (Step 8)
    // adds. Until then this module fails to COMPILE — the intended RED state for
    // the durable-documentation enforcement rail. The contract is deliberately
    // narrow: added-only, `.md`-only, report-TYPED (path or title), never topic.

    /// A newly-added `.md` under a reserved report directory flags on the path
    /// alone (path rail), and the finding text points the author at issue/memory.
    #[test]
    fn g4_flags_added_doc_under_reserved_report_dir() {
        let diff = "\
diff --git a/docs/investigation/run-42.md b/docs/investigation/run-42.md
new file mode 100644
--- /dev/null
+++ b/docs/investigation/run-42.md
@@ -0,0 +1,3 @@
+# kgpacks-rs run 42
+
+The build was blocked on X as of today.
";
        let f = scan_no_point_in_time_report_docs(diff);
        assert_eq!(f.len(), 1, "one path-rail report doc flags: {f:?}");
        assert_eq!(f[0].file, "docs/investigation/run-42.md");
        assert!(
            f[0].text.to_lowercase().contains("issue"),
            "the finding guides the author to a GitHub issue/memory: {:?}",
            f[0].text
        );
    }

    /// All three reserved report directories flag an added `.md` on path alone.
    #[test]
    fn g4_all_reserved_report_dirs_flag() {
        for dir in ["docs/investigation", "docs/reports", "docs/runs"] {
            let path = format!("{dir}/x.md");
            let diff = format!(
                "\
diff --git a/{path} b/{path}
new file mode 100644
--- /dev/null
+++ b/{path}
@@ -0,0 +1,1 @@
+# notes
"
            );
            assert_eq!(
                scan_no_point_in_time_report_docs(&diff).len(),
                1,
                "reserved report dir {dir} must flag an added .md"
            );
        }
    }

    /// Doc-TYPE-not-topic: a durable design doc under `docs/design/` with a
    /// durable H1, whose report vocabulary ("investigation") appears only in BODY
    /// prose, must NOT flag.
    #[test]
    fn g4_does_not_flag_durable_doc_with_report_words_in_body() {
        let diff = "\
diff --git a/docs/design/kgpacks-parity.md b/docs/design/kgpacks-parity.md
new file mode 100644
--- /dev/null
+++ b/docs/design/kgpacks-parity.md
@@ -0,0 +1,4 @@
+# kgpacks-rs parity design
+
+This design explains the parity architecture. A prior investigation
+informed it, but this doc describes durable behavior.
";
        assert!(
            scan_no_point_in_time_report_docs(diff).is_empty(),
            "a durable design doc with report words only in body prose must NOT flag"
        );
    }

    /// Title rail: an added `.md` OUTSIDE the reserved dirs flags when its H1 is a
    /// report-typed title.
    #[test]
    fn g4_flags_report_typed_h1_outside_reserved_dirs() {
        let diff = "\
diff --git a/docs/notes/x.md b/docs/notes/x.md
new file mode 100644
--- /dev/null
+++ b/docs/notes/x.md
@@ -0,0 +1,2 @@
+# Investigation Report: kgpacks-rs
+details
";
        let f = scan_no_point_in_time_report_docs(diff);
        assert_eq!(f.len(), 1, "title-rail report doc flags: {f:?}");
        assert_eq!(f[0].file, "docs/notes/x.md");
    }

    /// Title rail via front-matter `type:` fires even under a DURABLE directory —
    /// a report-typed title trips regardless of where the file lives.
    #[test]
    fn g4_flags_report_typed_frontmatter_even_under_durable_dir() {
        let diff = "\
diff --git a/docs/design/y.md b/docs/design/y.md
new file mode 100644
--- /dev/null
+++ b/docs/design/y.md
@@ -0,0 +1,5 @@
+---
+type: diagnosis report
+---
+# Y
+body
";
        assert_eq!(
            scan_no_point_in_time_report_docs(diff).len(),
            1,
            "a report-typed front-matter trips the title rail even under docs/design/"
        );
    }

    /// Added-only: EDITING a pre-existing report doc is never flagged.
    #[test]
    fn g4_ignores_edits_to_preexisting_report_doc() {
        let diff = "\
diff --git a/docs/investigation/old.md b/docs/investigation/old.md
--- a/docs/investigation/old.md
+++ b/docs/investigation/old.md
@@ -1,2 +1,3 @@
 # Old investigation
 existing line
+an appended line
";
        assert!(
            scan_no_point_in_time_report_docs(diff).is_empty(),
            "an EDIT to a pre-existing report doc is never flagged (added-only)"
        );
    }

    /// Only `.md` is in scope; an added `.rs` (even under a report dir) is ignored.
    #[test]
    fn g4_ignores_non_markdown_added_files() {
        let diff = "\
diff --git a/docs/investigation/run.rs b/docs/investigation/run.rs
new file mode 100644
--- /dev/null
+++ b/docs/investigation/run.rs
@@ -0,0 +1,1 @@
+fn main() {}
";
        assert!(
            scan_no_point_in_time_report_docs(diff).is_empty(),
            "only .md files are in scope; an added .rs is ignored"
        );
    }

    /// Self-non-flag: the enforcement PR only EDITS CONTRIBUTING.md and prompt
    /// `.md` files (never ADDS a report doc), so it must not flag itself.
    #[test]
    fn g4_does_not_flag_the_enforcement_prs_own_edit_shape() {
        let diff = "\
diff --git a/CONTRIBUTING.md b/CONTRIBUTING.md
--- a/CONTRIBUTING.md
+++ b/CONTRIBUTING.md
@@ -1,1 +1,2 @@
 # Contributing
+G4 — never commit a point-in-time investigation report doc.
diff --git a/prompt_assets/simard/engineer_system.md b/prompt_assets/simard/engineer_system.md
--- a/prompt_assets/simard/engineer_system.md
+++ b/prompt_assets/simard/engineer_system.md
@@ -1,1 +1,2 @@
 # Engineer
+Record investigation/testing findings as an issue, not a repo doc.
";
        assert!(
            scan_no_point_in_time_report_docs(diff).is_empty(),
            "the enforcement PR only edits docs/prompts; it must not self-flag"
        );
    }

    /// Self-non-flag (hard case): the enforcement PR ADDS three DURABLE docs whose
    /// titles/bodies necessarily discuss reports (including a reference doc whose
    /// title literally contains "point-in-time report docs"). They sit under
    /// durable directories with durable `doc_type`s, so the scan must NOT flag
    /// them — otherwise the policy PR blocks itself. This pins doc-TYPE-not-topic
    /// hard and forces SPECIFIC report-kind title markers, not bare "report".
    #[test]
    fn g4_does_not_flag_this_features_own_durable_docs() {
        let diff = "\
diff --git a/docs/reference/no-point-in-time-docs-scan.md b/docs/reference/no-point-in-time-docs-scan.md
new file mode 100644
--- /dev/null
+++ b/docs/reference/no-point-in-time-docs-scan.md
@@ -0,0 +1,6 @@
+---
+title: \"No point-in-time report docs — pr-verify scan\"
+doc_type: reference
+---
+# No point-in-time report docs — pr-verify scan
+It flags a PR that adds an investigation report doc.
diff --git a/docs/howto/record-an-investigation-finding.md b/docs/howto/record-an-investigation-finding.md
new file mode 100644
--- /dev/null
+++ b/docs/howto/record-an-investigation-finding.md
@@ -0,0 +1,5 @@
+---
+title: \"Record an investigation finding (issue/memory, not a repo doc)\"
+doc_type: howto
+---
+# Record an investigation finding (issue/memory, not a repo doc)
diff --git a/docs/concepts/durable-documentation-policy.md b/docs/concepts/durable-documentation-policy.md
new file mode 100644
--- /dev/null
+++ b/docs/concepts/durable-documentation-policy.md
@@ -0,0 +1,6 @@
+---
+title: \"Durable-Documentation Policy (G4)\"
+doc_type: concept
+---
+# Durable-Documentation Policy (G4)
+A diagnosis report belongs in an issue, not the repo.
";
        let f = scan_no_point_in_time_report_docs(diff);
        assert!(
            f.is_empty(),
            "the feature's own durable concept/howto/reference docs must not self-flag: {f:?}"
        );
    }

    /// `newly_added_files` distinguishes an add (new file mode / --- /dev/null)
    /// from a plain edit and from a deletion.
    #[test]
    fn newly_added_files_distinguishes_add_edit_and_delete() {
        let diff = "\
diff --git a/added.md b/added.md
new file mode 100644
--- /dev/null
+++ b/added.md
@@ -0,0 +1,1 @@
+new
diff --git a/edited.md b/edited.md
--- a/edited.md
+++ b/edited.md
@@ -1,1 +1,2 @@
 keep
+more
diff --git a/deleted.md b/deleted.md
deleted file mode 100644
--- a/deleted.md
+++ /dev/null
@@ -1,1 +0,0 @@
-gone
";
        let added = newly_added_files(diff);
        assert!(
            added.contains("added.md"),
            "the added file is detected: {added:?}"
        );
        assert!(!added.contains("edited.md"), "an edit is not an add");
        assert!(!added.contains("deleted.md"), "a deletion is not an add");
        assert_eq!(added.len(), 1, "exactly one added file: {added:?}");
    }

    /// A `--- /dev/null` old side ALONE (no `new file mode` header, as some tools
    /// emit) is sufficient proof that a file is added.
    #[test]
    fn newly_added_files_accepts_dev_null_old_side_without_new_file_mode() {
        let diff = "\
--- /dev/null
+++ b/docs/investigation/x.md
@@ -0,0 +1,1 @@
+# x
";
        let added = newly_added_files(diff);
        assert!(
            added.contains("docs/investigation/x.md"),
            "a /dev/null old side alone proves an add: {added:?}"
        );
    }

    /// The G4 scan is registered as a fifth `run_diff_scans` check and fails on a
    /// diff that adds a report doc.
    #[test]
    fn g4_is_registered_as_a_diff_scan() {
        let diff = "\
diff --git a/docs/investigation/run.md b/docs/investigation/run.md
new file mode 100644
--- /dev/null
+++ b/docs/investigation/run.md
@@ -0,0 +1,1 @@
+# Investigation Report
";
        let checks = run_diff_scans(diff);
        assert_eq!(checks.len(), 5, "the G4 scan is registered as a 5th check");
        let g4 = checks
            .iter()
            .find(|c| c.name.to_lowercase().contains("point-in-time"))
            .expect("a check named for the point-in-time-docs policy is registered");
        assert!(
            !g4.passed,
            "the G4 check fails on an added report doc: {g4:?}"
        );
    }

    /// Sibling isolation: adding a report doc must not perturb the pre-existing
    /// Bridge / print / additive / PRD scans.
    #[test]
    fn g4_sibling_scans_are_unchanged_by_a_report_doc_add() {
        let diff = "\
diff --git a/docs/investigation/run.md b/docs/investigation/run.md
new file mode 100644
--- /dev/null
+++ b/docs/investigation/run.md
@@ -0,0 +1,1 @@
+# Investigation Report
";
        assert!(
            scan_no_bridge_naming(diff).is_empty(),
            "no-Bridge scan unchanged"
        );
        assert!(
            scan_no_stray_prints(diff).is_empty(),
            "no-stray-print scan unchanged"
        );
        assert!(
            scan_additive_no_removed_pub(diff).is_empty(),
            "additive scan unchanged"
        );
        assert!(scan_prd_preserved(diff).is_empty(), "PRD scan unchanged");
    }
}
