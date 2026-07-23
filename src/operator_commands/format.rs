use crate::engineer_handoff::{EngineerHandoffContext, TERMINAL_MODE_BOUNDARY};
use crate::goals::{GoalRecord, GoalStatus};
use crate::sanitization::sanitize_terminal_text;

/// Render a single `"{label}: {value}"` line with the value passed through
/// [`sanitize_terminal_text`]. This is the single source of truth for the
/// labeled-line format so it can be asserted directly in tests instead of only
/// checking that the `println!` wrappers do not panic.
pub(crate) fn format_labeled(label: &str, value: &str) -> String {
    format!("{label}: {}", sanitize_terminal_text(value))
}

pub(crate) fn print_text(label: &str, value: impl AsRef<str>) {
    println!("{}", format_labeled(label, value.as_ref()));
}

pub(crate) fn print_display(label: &str, value: impl std::fmt::Display) {
    println!("{}", format_labeled(label, &value.to_string()));
}

pub(crate) fn terminal_handoff_section_lines(
    terminal_handoff_context: Option<&EngineerHandoffContext>,
    default_source: &str,
) -> Vec<String> {
    match terminal_handoff_context {
        Some(context) => {
            let last_output_line = context.last_output_line.as_deref().unwrap_or("<none>");
            vec![
                format_labeled("Mode boundary", TERMINAL_MODE_BOUNDARY),
                format_labeled("Terminal continuity available", "yes"),
                format_labeled("Terminal continuity source", &context.continuity_source),
                format_labeled("Terminal continuity handoff", &context.handoff_file_name),
                format_labeled(
                    "Terminal continuity working directory",
                    &context.working_directory,
                ),
                format_labeled("Terminal continuity command count", &context.command_count),
                format_labeled("Terminal continuity wait count", &context.wait_count),
                format_labeled("Terminal continuity last output line", last_output_line),
            ]
        }
        None => vec![
            format_labeled("Terminal continuity available", "no"),
            format_labeled("Terminal continuity source", default_source),
        ],
    }
}

pub(crate) fn print_terminal_handoff_section(
    terminal_handoff_context: Option<&EngineerHandoffContext>,
    default_source: &str,
) {
    for line in terminal_handoff_section_lines(terminal_handoff_context, default_source) {
        println!("{line}");
    }
}

pub(crate) fn string_section_lines(label: &str, values: &[String]) -> Vec<String> {
    let mut lines = vec![format!("{label} count: {}", values.len())];
    if values.is_empty() {
        lines.push(format!("{label}: <none>"));
        return lines;
    }

    let singular = label.strip_suffix('s').unwrap_or(label);
    for (index, value) in values.iter().enumerate() {
        lines.push(format_labeled(&format!("{singular} {}", index + 1), value));
    }
    lines
}

pub(crate) fn print_string_section(label: &str, values: &[String]) {
    for line in string_section_lines(label, values) {
        println!("{line}");
    }
}

pub(crate) fn meeting_goal_section_lines(
    goals: &[crate::PersistedMeetingGoalUpdate],
) -> Vec<String> {
    let mut lines = vec![format!("Goal updates count: {}", goals.len())];
    if goals.is_empty() {
        lines.push("Goal updates: <none>".to_string());
        return lines;
    }

    for (index, goal) in goals.iter().enumerate() {
        lines.push(format_labeled(
            &format!("Goal update {}", index + 1),
            &goal.concise_label(),
        ));
    }
    lines
}

pub(crate) fn print_meeting_goal_section(goals: &[crate::PersistedMeetingGoalUpdate]) {
    for line in meeting_goal_section_lines(goals) {
        println!("{line}");
    }
}

pub(crate) fn goal_section_lines(
    records: &[GoalRecord],
    status: GoalStatus,
    heading: &'static str,
) -> Vec<String> {
    let mut matching = records
        .iter()
        .filter(|record| record.status == status)
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then(left.title.cmp(&right.title))
            .then(left.slug.cmp(&right.slug))
    });
    let mut lines = vec![format!("{} goals count: {}", heading, matching.len())];
    if matching.is_empty() {
        lines.push(format!("{heading} goals: <none>"));
        return lines;
    }

    for (index, goal) in matching.iter().enumerate() {
        lines.push(format_labeled(
            &format!("{heading} goal {}", index + 1),
            &goal.concise_label(),
        ));
    }
    lines
}

pub(crate) fn print_goal_section(
    records: &[GoalRecord],
    status: GoalStatus,
    heading: &'static str,
) {
    for line in goal_section_lines(records, status, heading) {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goals::GoalStatus;
    use crate::session::{SessionId, SessionPhase};

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn make_goal(title: &str, status: GoalStatus, priority: u8) -> GoalRecord {
        GoalRecord {
            wip_refs: Vec::new(),
            labels: Vec::new(),
            slug: title.to_lowercase().replace(' ', "-"),
            title: s(title),
            rationale: s("test rationale"),
            status,
            priority,
            owner_identity: s("test-identity"),
            source_session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            updated_in: SessionPhase::Execution,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn format_labeled_strips_terminal_control_sequences() {
        // A raw value carrying an ANSI colour sequence must be sanitised so the
        // rendered line contains only the visible text, never the escape bytes.
        let line = format_labeled("Value", "\u{1b}[31mred\u{1b}[0m");
        assert_eq!(line, "Value: red");
    }

    #[test]
    fn print_display_renders_via_format_labeled() {
        assert_eq!(format_labeled("Count", &42.to_string()), "Count: 42");
    }

    #[test]
    fn string_section_empty_reports_zero_count_and_none_placeholder() {
        let lines = string_section_lines("Items", &[]);
        assert_eq!(lines, vec!["Items count: 0", "Items: <none>"]);
    }

    #[test]
    fn string_section_with_values_numbers_singularized_entries() {
        let lines = string_section_lines("Items", &["first".to_string(), "second".to_string()]);
        // "Items" is singularised to "Item"; entries are 1-indexed and sanitised.
        assert_eq!(
            lines,
            vec![
                "Items count: 2".to_string(),
                "Item 1: first".to_string(),
                "Item 2: second".to_string(),
            ]
        );
    }

    #[test]
    fn meeting_goal_section_empty_reports_none_placeholder() {
        let lines = meeting_goal_section_lines(&[]);
        assert_eq!(lines, vec!["Goal updates count: 0", "Goal updates: <none>"]);
    }

    #[test]
    fn goal_section_empty_reports_none_placeholder() {
        let lines = goal_section_lines(&[], GoalStatus::Active, "Active");
        assert_eq!(lines, vec!["Active goals count: 0", "Active goals: <none>"]);
    }

    #[test]
    fn goal_section_filters_by_status_sorts_by_priority_and_renders_concise_labels() {
        // Two Active goals (out of priority order) plus one Completed goal that
        // must be filtered out. Output must be priority-sorted and use the
        // `p{priority} [{status}] {title}` concise label form.
        let goals = vec![
            make_goal("Y", GoalStatus::Active, 2),
            make_goal("X", GoalStatus::Active, 1),
            make_goal("Z", GoalStatus::Completed, 1),
        ];
        let lines = goal_section_lines(&goals, GoalStatus::Active, "Active");
        assert_eq!(
            lines,
            vec![
                "Active goals count: 2".to_string(),
                "Active goal 1: p1 [active] X".to_string(),
                "Active goal 2: p2 [active] Y".to_string(),
            ]
        );
    }

    #[test]
    fn goal_section_with_no_matching_status_reports_empty() {
        let goals = vec![make_goal("X", GoalStatus::Active, 1)];
        let lines = goal_section_lines(&goals, GoalStatus::Completed, "Completed");
        assert_eq!(
            lines,
            vec!["Completed goals count: 0", "Completed goals: <none>"]
        );
    }

    #[test]
    fn terminal_handoff_section_none_reports_unavailable_with_default_source() {
        let lines = terminal_handoff_section_lines(None, "default-source");
        assert_eq!(
            lines,
            vec![
                "Terminal continuity available: no".to_string(),
                "Terminal continuity source: default-source".to_string(),
            ]
        );
    }

    #[test]
    fn terminal_handoff_section_some_substitutes_none_for_missing_last_output_line() {
        let context = EngineerHandoffContext {
            continuity_source: s("session-log"),
            handoff_file_name: s("handoff.json"),
            working_directory: s("/work/dir"),
            command_count: s("7"),
            wait_count: s("2"),
            last_output_line: None,
        };
        let lines = terminal_handoff_section_lines(Some(&context), "unused-default");
        assert_eq!(lines[0], format!("Mode boundary: {TERMINAL_MODE_BOUNDARY}"));
        assert_eq!(lines[1], "Terminal continuity available: yes");
        assert_eq!(lines[2], "Terminal continuity source: session-log");
        assert_eq!(lines[3], "Terminal continuity handoff: handoff.json");
        assert_eq!(lines[4], "Terminal continuity working directory: /work/dir");
        assert_eq!(lines[5], "Terminal continuity command count: 7");
        assert_eq!(lines[6], "Terminal continuity wait count: 2");
        assert_eq!(lines[7], "Terminal continuity last output line: <none>");
    }

    #[test]
    fn print_wrappers_do_not_panic() {
        // The `print_*` wrappers only forward the `*_lines` output to stdout;
        // the rendering itself is asserted above. Exercise the wrappers once so
        // the `println!` paths stay covered without a captured-stdout harness.
        print_text("label", "value");
        print_display("label", 42);
        print_string_section("Items", &["first".to_string()]);
        print_meeting_goal_section(&[]);
        print_goal_section(&[], GoalStatus::Active, "Active");
        print_terminal_handoff_section(None, "default-source");
    }
}
