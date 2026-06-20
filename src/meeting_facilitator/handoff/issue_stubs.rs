//! Ready-to-file GitHub issue **stubs** rendered from a meeting handoff.
//!
//! When a meeting closes, [`crate::meeting_facilitator::write_meeting_bundle`]
//! calls [`write_issue_stubs`] to render each [`MeetingDecision`] and each
//! [`ActionItem`] into an `issues/NN-slug.md` file inside the per-meeting
//! bundle directory. Each file is a reviewable, ready-to-edit GitHub issue
//! draft — the middle ground between re-typing decisions by hand and
//! `simard act-on-decisions`, which auto-files immediately.
//!
//! Design (issue #2309):
//! * [`plan_issue_stubs`] is a **pure** renderer — no I/O — so it can be unit
//!   tested and reused by the markdown report.
//! * [`write_issue_stubs`] is the I/O wrapper: it is a no-op (no `issues/`
//!   directory) when there are no decisions and no action items (mirrors the
//!   empty-handoff write guard, #2268), writes files `0o600`, and is
//!   idempotent — stale `*.md` stubs are cleared before regeneration.
//! * Filenames are filesystem-safe (`[a-z0-9-]`, no path traversal).

use std::fs;
use std::path::{Path, PathBuf};

use super::MeetingHandoff;
use crate::error::{SimardError, SimardResult};
use crate::meeting_facilitator::types::{ActionItem, MeetingDecision};

/// Name of the bundle subdirectory that holds generated issue stubs.
pub const ISSUES_SUBDIR: &str = "issues";

/// A single ready-to-file GitHub issue draft rendered from a meeting handoff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueStub {
    /// Filesystem-safe filename, relative to the `issues/` directory
    /// (e.g. `01-adopt-tdd.md`). Always `[a-z0-9-]` plus the `.md` suffix.
    pub filename: String,
    /// Rendered issue title (`Action: …` or `Decision: …`).
    pub title: String,
    /// Full markdown body written to the file.
    pub body: String,
}

/// Render every decision and action item in `handoff` into an ordered list of
/// [`IssueStub`]s. Pure — performs no I/O. Decisions come first, then action
/// items; the combined list is numbered `01`, `02`, … in that order.
///
/// Returns an empty vector when the handoff carries no decisions and no
/// action items.
pub fn plan_issue_stubs(handoff: &MeetingHandoff) -> Vec<IssueStub> {
    let mut stubs = Vec::with_capacity(handoff.decisions.len() + handoff.action_items.len());
    let goal_line = match handoff.goal.as_deref().map(str::trim) {
        Some(g) if !g.is_empty() => g.to_string(),
        _ => "_not set_".to_string(),
    };

    let mut index = 0usize;
    for decision in &handoff.decisions {
        index += 1;
        stubs.push(render_decision_stub(index, decision, handoff, &goal_line));
    }
    for action in &handoff.action_items {
        index += 1;
        stubs.push(render_action_stub(index, action, handoff, &goal_line));
    }
    stubs
}

/// Write the planned issue stubs into `bundle_dir/issues/`.
///
/// * No-op (returns `Ok(vec![])`, creates no `issues/` directory) when there
///   are no decisions and no action items.
/// * Idempotent: any pre-existing `*.md` stub files are removed before the
///   fresh set is written, so a regeneration with fewer items never leaves
///   stale stubs behind.
/// * Files are written `0o600` on unix.
///
/// Returns the paths of the files written, in stub order.
pub fn write_issue_stubs(
    bundle_dir: &Path,
    handoff: &MeetingHandoff,
) -> SimardResult<Vec<PathBuf>> {
    let stubs = plan_issue_stubs(handoff);
    // No-op for empty handoffs — mirrors the empty-handoff write guard (#2268).
    if stubs.is_empty() {
        return Ok(Vec::new());
    }

    let issues_dir = bundle_dir.join(ISSUES_SUBDIR);
    fs::create_dir_all(&issues_dir).map_err(|e| SimardError::ArtifactIo {
        path: issues_dir.clone(),
        reason: format!("creating issues dir: {e}"),
    })?;

    // Idempotent regeneration: clear stale `*.md` stubs before writing the
    // fresh set so a regeneration with fewer items leaves nothing behind.
    if let Ok(entries) = fs::read_dir(&issues_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let _ = fs::remove_file(&path);
            }
        }
    }

    let mut written = Vec::with_capacity(stubs.len());
    for stub in &stubs {
        let path = issues_dir.join(&stub.filename);
        fs::write(&path, &stub.body).map_err(|e| SimardError::ArtifactIo {
            path: path.clone(),
            reason: format!("writing issue stub: {e}"),
        })?;
        written.push(path);
    }

    // 0o600 on Unix — stubs may carry operator-private text.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        for path in &written {
            if let Err(e) = std::fs::set_permissions(path, perms.clone()) {
                tracing::warn!(path = %path.display(), error = %e, "failed to set 0o600 on issue stub");
            }
        }
    }

    Ok(written)
}

/// Build a filesystem-safe slug containing only `[a-z0-9-]`.
///
/// Non-alphanumeric runs collapse to a single `-`; leading/trailing dashes are
/// trimmed and the result is length-capped. Because the output is restricted
/// to `[a-z0-9-]`, it can never contain `/`, `\\`, or `.` and is therefore
/// immune to path traversal. Falls back to `item` when nothing survives.
fn sanitize_slug(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(48));
    let mut prev_dash = false;
    for c in input.chars() {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > 48 {
        out.truncate(48);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str("item");
    }
    out
}

/// Collapse a (possibly multi-line) description into a single-line title.
fn title_text(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Shared provenance + goal footer for every stub.
fn provenance_block(handoff: &MeetingHandoff) -> String {
    format!(
        "## Provenance\n\n\
         - **Meeting ID:** `{meeting_id}`\n\
         - **Topic:** {topic}\n\
         - **Closed at:** {closed_at}\n\n\
         _Generated by the Simard meeting handoff pipeline (issue #2309). \
         Review and edit before filing with `gh issue create`._\n",
        meeting_id = handoff.meeting_id,
        topic = handoff.topic,
        closed_at = handoff.closed_at,
    )
}

fn render_decision_stub(
    index: usize,
    decision: &MeetingDecision,
    handoff: &MeetingHandoff,
    goal_line: &str,
) -> IssueStub {
    let title_body = title_text(&decision.description);
    let title = format!("Decision: {title_body}");
    let filename = format!("{:02}-{}.md", index, sanitize_slug(&decision.description));

    let rationale = if decision.rationale.trim().is_empty() {
        "_None recorded._".to_string()
    } else {
        decision.rationale.clone()
    };
    let decided_by = if decision.participants.is_empty() {
        "_unknown_".to_string()
    } else {
        decision.participants.join(", ")
    };

    let body = format!(
        "# {title}\n\n\
         > Auto-generated issue stub from a Simard meeting handoff.\n\n\
         ## Context\n\n{description}\n\n\
         ### Rationale\n\n{rationale}\n\n\
         ## Details\n\n\
         - **Decided by:** {decided_by}\n\
         - **Priority:** _n/a (decision)_\n\
         - **Due:** _n/a (decision)_\n\
         - **Meeting goal:** {goal_line}\n\n\
         ## Acceptance criteria\n\n\
         - [ ] Decision \"{title_body}\" is reflected in code, config, or docs\n\
         - [ ] Affected owners and stakeholders are informed\n\
         - [ ] Any follow-up work is filed as its own issue\n\n\
         {provenance}",
        title = title,
        description = decision.description,
        rationale = rationale,
        decided_by = decided_by,
        goal_line = goal_line,
        title_body = title_body,
        provenance = provenance_block(handoff),
    );

    IssueStub {
        filename,
        title,
        body,
    }
}

fn render_action_stub(
    index: usize,
    action: &ActionItem,
    handoff: &MeetingHandoff,
    goal_line: &str,
) -> IssueStub {
    let title_body = title_text(&action.description);
    let title = format!("Action: {title_body}");
    let filename = format!("{:02}-{}.md", index, sanitize_slug(&action.description));

    let owner = if action.owner.trim().is_empty() {
        "_unassigned_".to_string()
    } else {
        action.owner.clone()
    };
    let due = match action.due_description.as_deref().map(str::trim) {
        Some(d) if !d.is_empty() => d.to_string(),
        _ => "_none_".to_string(),
    };

    let body = format!(
        "# {title}\n\n\
         > Auto-generated issue stub from a Simard meeting handoff.\n\n\
         ## Context\n\n{description}\n\n\
         ## Details\n\n\
         - **Owner:** {owner}\n\
         - **Priority:** {priority}\n\
         - **Due:** {due}\n\
         - **Meeting goal:** {goal_line}\n\n\
         ## Acceptance criteria\n\n\
         - [ ] {title_body} is implemented / completed\n\
         - [ ] Work is covered by tests where applicable\n\
         - [ ] Documentation is updated if user-facing behavior changes\n\n\
         {provenance}",
        title = title,
        description = action.description,
        owner = owner,
        priority = action.priority,
        due = due,
        goal_line = goal_line,
        title_body = title_body,
        provenance = provenance_block(handoff),
    );

    IssueStub {
        filename,
        title,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_facilitator::types::{ActionItem, MeetingDecision, OpenQuestion};

    fn base_handoff() -> MeetingHandoff {
        MeetingHandoff {
            schema_version: 2,
            meeting_id: "20260513T070000Z-sprint-planning".to_string(),
            topic: "Sprint planning".to_string(),
            started_at: "2026-05-13T07:00:00Z".to_string(),
            closed_at: "2026-05-13T07:30:00Z".to_string(),
            decisions: vec![],
            action_items: vec![],
            open_questions: vec![OpenQuestion {
                text: "ignored by stubs".to_string(),
                explicit: false,
            }],
            processed: false,
            duration_secs: Some(1800),
            transcript: vec![],
            transcript_path: None,
            participants: vec!["alice".to_string(), "bob".to_string()],
            themes: vec![],
            next_owner: None,
            artifacts: Vec::new(),
            goal: Some("Ship the richer-handoffs prong".to_string()),
            next_actor: None,
            applied_templates: Vec::new(),
            history_truncated_count: 0,
            partial_reason: None,
            risks: vec![],
            disagreements: vec![],
        }
    }

    fn decision(description: &str, rationale: &str) -> MeetingDecision {
        MeetingDecision {
            description: description.to_string(),
            rationale: rationale.to_string(),
            participants: vec!["alice".to_string(), "bob".to_string()],
        }
    }

    fn action(description: &str, owner: &str) -> ActionItem {
        ActionItem {
            description: description.to_string(),
            owner: owner.to_string(),
            priority: 1,
            due_description: Some("by friday".to_string()),
            linked_issue: None,
        }
    }

    fn temp_bundle(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{label}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── (a) empty handoff → no issues/ dir (no-op) ──────────────────────

    #[test]
    fn empty_handoff_plans_no_stubs() {
        let handoff = base_handoff();
        assert!(plan_issue_stubs(&handoff).is_empty());
    }

    #[test]
    fn empty_handoff_writes_no_issues_dir() {
        let bundle = temp_bundle("stubs-empty");
        let handoff = base_handoff();

        let written = write_issue_stubs(&bundle, &handoff).expect("write");

        assert!(written.is_empty(), "no files should be written");
        assert!(
            !bundle.join(ISSUES_SUBDIR).exists(),
            "issues/ dir must not be created for an empty handoff"
        );

        std::fs::remove_dir_all(&bundle).ok();
    }

    // ── (b) single action item → one stub ───────────────────────────────

    #[test]
    fn single_action_item_produces_one_stub() {
        let mut handoff = base_handoff();
        handoff.action_items = vec![action("Wire bundle writer into close flow", "bob")];

        let stubs = plan_issue_stubs(&handoff);
        assert_eq!(stubs.len(), 1);
        let stub = &stubs[0];
        assert!(stub.title.starts_with("Action:"), "title={}", stub.title);
        assert!(stub.filename.starts_with("01-"), "fn={}", stub.filename);
        assert!(stub.filename.ends_with(".md"));
        // Body content requirements.
        assert!(stub.body.contains("Wire bundle writer into close flow"));
        assert!(stub.body.contains("bob"), "owner missing");
        assert!(stub.body.contains("Priority"), "priority missing");
        assert!(stub.body.contains("by friday"), "due missing");
        assert!(
            stub.body.contains("Ship the richer-handoffs prong"),
            "meeting goal missing"
        );
        assert!(
            stub.body.contains("Acceptance criteria"),
            "acceptance criteria missing"
        );
        assert!(stub.body.contains("- [ ]"), "checklist missing");
        // Provenance.
        assert!(stub.body.contains("20260513T070000Z-sprint-planning"));
        assert!(stub.body.contains("Sprint planning"));
        assert!(stub.body.contains("2026-05-13T07:30:00Z"));
    }

    #[test]
    fn single_action_item_writes_one_file() {
        let bundle = temp_bundle("stubs-single");
        let mut handoff = base_handoff();
        handoff.action_items = vec![action("Do the thing", "bob")];

        let written = write_issue_stubs(&bundle, &handoff).expect("write");
        assert_eq!(written.len(), 1);
        assert!(written[0].is_file());
        assert_eq!(written[0].parent().unwrap(), bundle.join(ISSUES_SUBDIR));

        std::fs::remove_dir_all(&bundle).ok();
    }

    // ── (c) decision with rationale → stub includes rationale ───────────

    #[test]
    fn decision_with_rationale_stub_includes_rationale() {
        let mut handoff = base_handoff();
        handoff.decisions = vec![decision(
            "Adopt structured handoff bundles",
            "Downstream engineer loop needs a stable shape",
        )];

        let stubs = plan_issue_stubs(&handoff);
        assert_eq!(stubs.len(), 1);
        let stub = &stubs[0];
        assert!(stub.title.starts_with("Decision:"), "title={}", stub.title);
        assert!(stub.body.contains("Adopt structured handoff bundles"));
        assert!(
            stub.body
                .contains("Downstream engineer loop needs a stable shape"),
            "rationale missing from stub body"
        );
        assert!(
            stub.body.to_lowercase().contains("rationale"),
            "rationale heading missing"
        );
        // decided-by / participants
        assert!(stub.body.contains("alice"));
    }

    #[test]
    fn decisions_render_before_action_items() {
        let mut handoff = base_handoff();
        handoff.decisions = vec![decision("A decision", "because")];
        handoff.action_items = vec![action("An action", "bob")];

        let stubs = plan_issue_stubs(&handoff);
        assert_eq!(stubs.len(), 2);
        assert!(stubs[0].title.starts_with("Decision:"));
        assert!(stubs[0].filename.starts_with("01-"));
        assert!(stubs[1].title.starts_with("Action:"));
        assert!(stubs[1].filename.starts_with("02-"));
    }

    // ── (d) filename sanitization [a-z0-9-], no path traversal ──────────

    #[test]
    fn filenames_are_sanitized_and_traversal_safe() {
        let mut handoff = base_handoff();
        handoff.action_items = vec![action("../../etc/passwd: Do BAD!! things/now", "bob")];

        let stubs = plan_issue_stubs(&handoff);
        assert_eq!(stubs.len(), 1);
        let name = &stubs[0].filename;

        // Strip the NN- prefix and .md suffix; the slug body must be [a-z0-9-].
        let stem = name.strip_suffix(".md").expect("ends with .md");
        let slug = &stem[3..]; // after "01-"
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "slug has illegal chars: {slug}"
        );
        // No traversal sequences anywhere in the filename.
        assert!(!name.contains(".."), "filename contains '..': {name}");
        assert!(!name.contains('/'), "filename contains '/': {name}");
        assert!(!name.contains('\\'), "filename contains backslash: {name}");
    }

    #[test]
    fn malicious_description_stays_inside_issues_dir() {
        let bundle = temp_bundle("stubs-traversal");
        let mut handoff = base_handoff();
        handoff.action_items = vec![action("../../../tmp/evil", "x")];

        let written = write_issue_stubs(&bundle, &handoff).expect("write");
        assert_eq!(written.len(), 1);
        let path = &written[0];
        assert!(path.is_file());
        // The written file's parent must be exactly bundle/issues — no escape.
        assert_eq!(path.parent().unwrap(), bundle.join(ISSUES_SUBDIR));

        std::fs::remove_dir_all(&bundle).ok();
    }

    #[test]
    fn empty_description_yields_nonempty_slug() {
        let mut handoff = base_handoff();
        handoff.action_items = vec![action("!!!", "x")];
        let stubs = plan_issue_stubs(&handoff);
        let stem = stubs[0].filename.strip_suffix(".md").unwrap();
        assert!(
            stem.len() > 3,
            "slug must not be empty: {}",
            stubs[0].filename
        );
    }

    // ── (e) idempotent regeneration (stale *.md cleared first) ──────────

    #[test]
    fn regeneration_clears_stale_stubs() {
        let bundle = temp_bundle("stubs-idempotent");
        let mut handoff = base_handoff();
        handoff.decisions = vec![decision("D1", "r1")];
        handoff.action_items = vec![action("A1", "bob")];

        // First write: 2 stubs.
        let first = write_issue_stubs(&bundle, &handoff).expect("write 1");
        assert_eq!(first.len(), 2);

        // Plant a stale stub that the next regeneration must remove.
        let issues_dir = bundle.join(ISSUES_SUBDIR);
        let stale = issues_dir.join("99-stale.md");
        std::fs::write(&stale, "stale").unwrap();
        assert!(stale.is_file());

        // Regenerate with a single action item → exactly 1 stub, stale gone.
        handoff.decisions = vec![];
        handoff.action_items = vec![action("only one", "bob")];
        let second = write_issue_stubs(&bundle, &handoff).expect("write 2");
        assert_eq!(second.len(), 1);
        assert!(!stale.exists(), "stale stub should have been cleared");

        // Only the freshly written *.md files remain.
        let md_count = std::fs::read_dir(&issues_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .count();
        assert_eq!(md_count, 1, "exactly one stub should remain");

        std::fs::remove_dir_all(&bundle).ok();
    }

    #[test]
    fn rewrite_is_stable_no_accumulation() {
        let bundle = temp_bundle("stubs-stable");
        let mut handoff = base_handoff();
        handoff.action_items = vec![action("Stable item", "bob")];

        let first = write_issue_stubs(&bundle, &handoff).expect("write 1");
        let second = write_issue_stubs(&bundle, &handoff).expect("write 2");
        assert_eq!(first, second, "filenames must be deterministic");

        let issues_dir = bundle.join(ISSUES_SUBDIR);
        let md_count = std::fs::read_dir(&issues_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            .count();
        assert_eq!(md_count, 1);

        std::fs::remove_dir_all(&bundle).ok();
    }

    #[cfg(unix)]
    #[test]
    fn written_stubs_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let bundle = temp_bundle("stubs-perms");
        let mut handoff = base_handoff();
        handoff.action_items = vec![action("perm check", "bob")];

        let written = write_issue_stubs(&bundle, &handoff).expect("write");
        let mode = std::fs::metadata(&written[0]).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "stub must be 0o600");

        std::fs::remove_dir_all(&bundle).ok();
    }
}
