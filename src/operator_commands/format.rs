use crate::goals::{GoalRecord, GoalStatus};
use crate::sanitization::sanitize_terminal_text;
use crate::terminal_engineer_bridge::{TERMINAL_MODE_BOUNDARY, TerminalBridgeContext};

pub(crate) fn print_text(label: &str, value: impl AsRef<str>) {
    println!("{label}: {}", sanitize_terminal_text(value.as_ref()));
}

pub(crate) fn print_display(label: &str, value: impl std::fmt::Display) {
    println!("{label}: {}", sanitize_terminal_text(&value.to_string()));
}

pub(crate) fn print_terminal_bridge_section(
    terminal_bridge_context: Option<&TerminalBridgeContext>,
    default_source: &str,
) {
    match terminal_bridge_context {
        Some(context) => {
            print_text("Mode boundary", TERMINAL_MODE_BOUNDARY);
            print_text("Terminal continuity available", "yes");
            print_text("Terminal continuity source", &context.continuity_source);
            print_text("Terminal continuity handoff", &context.handoff_file_name);
            print_text(
                "Terminal continuity working directory",
                &context.working_directory,
            );
            print_text("Terminal continuity command count", &context.command_count);
            print_text("Terminal continuity wait count", &context.wait_count);
            if let Some(last_output_line) = &context.last_output_line {
                print_text("Terminal continuity last output line", last_output_line);
            } else {
                print_text("Terminal continuity last output line", "<none>");
            }
        }
        None => {
            print_text("Terminal continuity available", "no");
            print_text("Terminal continuity source", default_source);
        }
    }
}

pub(crate) fn print_string_section(label: &str, values: &[String]) {
    println!("{label} count: {}", values.len());
    if values.is_empty() {
        println!("{label}: <none>");
        return;
    }

    let singular = label.strip_suffix('s').unwrap_or(label);
    for (index, value) in values.iter().enumerate() {
        print_text(&format!("{singular} {}", index + 1), value);
    }
}

pub(crate) fn print_meeting_goal_section(goals: &[crate::PersistedMeetingGoalUpdate]) {
    println!("Goal updates count: {}", goals.len());
    if goals.is_empty() {
        println!("Goal updates: <none>");
        return;
    }

    for (index, goal) in goals.iter().enumerate() {
        print_text(&format!("Goal update {}", index + 1), goal.concise_label());
    }
}

pub(crate) fn print_goal_section(
    records: &[GoalRecord],
    status: GoalStatus,
    heading: &'static str,
) {
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
    println!("{} goals count: {}", heading, matching.len());
    if matching.is_empty() {
        println!("{} goals: <none>", heading);
        return;
    }

    for (index, goal) in matching.iter().enumerate() {
        print_text(
            &format!("{heading} goal {}", index + 1),
            goal.concise_label(),
        );
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
            slug: title.to_lowercase().replace(' ', "-"),
            title: s(title),
            rationale: s("test rationale"),
            status,
            priority,
            owner_identity: s("test-identity"),
            source_session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
            updated_in: SessionPhase::Execution,
        }
    }

    #[test]
    fn print_text_does_not_panic() {
        print_text("label", "value");
    }

    #[test]
    fn print_display_does_not_panic() {
        print_display("label", 42);
    }

    #[test]
    fn print_string_section_empty_does_not_panic() {
        print_string_section("Items", &[]);
    }

    #[test]
    fn print_string_section_with_values_does_not_panic() {
        print_string_section("Items", &["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn print_meeting_goal_section_empty_does_not_panic() {
        print_meeting_goal_section(&[]);
    }

    #[test]
    fn print_goal_section_empty_does_not_panic() {
        print_goal_section(&[], GoalStatus::Active, "Active");
    }

    #[test]
    fn print_goal_section_with_matching_goals_does_not_panic() {
        let goals = vec![
            make_goal("X", GoalStatus::Active, 1),
            make_goal("Y", GoalStatus::Active, 2),
        ];
        print_goal_section(&goals, GoalStatus::Active, "Active");
    }

    #[test]
    fn print_goal_section_with_no_matching_status() {
        let goals = vec![make_goal("X", GoalStatus::Active, 1)];
        print_goal_section(&goals, GoalStatus::Completed, "Completed");
    }

    #[test]
    fn print_terminal_bridge_section_none_does_not_panic() {
        print_terminal_bridge_section(None, "default-source");
    }
}
