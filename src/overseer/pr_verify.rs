//! M2 **hard gate** — the pr-verify safety diff-scans (design doc §pr-verify
//! checklist, items 3–6). These are the NEW, additive checks that did not exist
//! before this milestone; **no merge capability ships before they do and are
//! unit-tested** (crusty review risk #2 / operator hard-gate #5).
//!
//! Every scan is a **pure** function over a unified diff (`gh pr diff` output),
//! so the whole merge-safety surface is testable on fixture diffs with zero
//! network. The four scans:
//!
//! | # | Check | This module |
//! |---|-------|-------------|
//! | 3 | No `Bridge` naming in **added** lines | [`scan_no_bridge_naming`] |
//! | 4 | No stray `print!`/`println!`/`eprintln!`/`eprint!` in added `src/**` | [`scan_no_stray_prints`] |
//! | 5 | Additive / non-breaking — no **removed** `pub` items | [`scan_additive_no_removed_pub`] |
//! | 6 | PRD (`Specs/ProductArchitecture.md`) preserved — no removed lines | [`scan_prd_preserved`] |
//!
//! Items 1–2 (CI-green / mergeable / base-allowlist) reuse
//! `stewardship::merge_authority::evaluate_objective_gates`; item 7 reuses
//! `review_pipeline::should_commit`. Those are wired in
//! [`merge_ops`](crate::overseer::merge_ops), not here.

use crate::overseer::capabilities::CheckItem;

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

// ─────────────────────────── checklist assembly ────────────────────────────

/// Run all four additive diff-scans and return one [`CheckItem`] per scan. A
/// scan passes when it finds no violations; the note names the offenders.
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
        assert_eq!(checks.len(), 4);
        assert!(
            checks.iter().all(|c| c.passed),
            "a clean additive diff passes all four scans: {checks:?}"
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
}
