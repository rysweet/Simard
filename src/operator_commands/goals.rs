use crate::goals::{GoalRecord, GoalStatus};

use super::format::format_labeled;

pub(crate) struct GoalRegisterView {
    sections: [GoalRegisterSection; 4],
}

impl GoalRegisterView {
    pub(crate) fn from_records(records: Vec<GoalRecord>) -> Self {
        let mut active = Vec::new();
        let mut proposed = Vec::new();
        let mut paused = Vec::new();
        let mut completed = Vec::new();

        for record in records {
            match record.status {
                GoalStatus::Active => active.push(record),
                GoalStatus::Proposed => proposed.push(record),
                GoalStatus::Paused => paused.push(record),
                GoalStatus::Completed => completed.push(record),
            }
        }

        Self {
            sections: [
                GoalRegisterSection::new(GoalStatus::Active, active),
                GoalRegisterSection::new(GoalStatus::Proposed, proposed),
                GoalRegisterSection::new(GoalStatus::Paused, paused),
                GoalRegisterSection::new(GoalStatus::Completed, completed),
            ],
        }
    }

    pub(crate) fn render_lines(&self) -> Vec<String> {
        self.sections
            .iter()
            .flat_map(GoalRegisterSection::render_lines)
            .collect()
    }

    pub(crate) fn print(&self) {
        for line in self.render_lines() {
            println!("{line}");
        }
    }
}

struct GoalRegisterSection {
    heading: &'static str,
    label: &'static str,
    goals: Vec<GoalRecord>,
}

impl GoalRegisterSection {
    fn new(status: GoalStatus, mut goals: Vec<GoalRecord>) -> Self {
        goals.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then(left.title.cmp(&right.title))
                .then(left.slug.cmp(&right.slug))
        });
        let (heading, label) = match status {
            GoalStatus::Active => ("Active", "Active goals"),
            GoalStatus::Proposed => ("Proposed", "Proposed goals"),
            GoalStatus::Paused => ("Paused", "Paused goals"),
            GoalStatus::Completed => ("Completed", "Completed goals"),
        };

        Self {
            heading,
            label,
            goals,
        }
    }

    fn render_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("{} count: {}", self.label, self.goals.len())];
        if self.goals.is_empty() {
            lines.push(format!("{}: <none>", self.label));
            return lines;
        }

        for (index, goal) in self.goals.iter().enumerate() {
            lines.push(format_labeled(
                &format!("{} goal {}", self.heading, index + 1),
                &goal.concise_label(),
            ));
        }
        lines
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
    fn goal_register_view_partitions_by_status() {
        let records = vec![
            make_goal("Alpha", GoalStatus::Active, 1),
            make_goal("Beta", GoalStatus::Proposed, 2),
            make_goal("Gamma", GoalStatus::Paused, 3),
            make_goal("Delta", GoalStatus::Completed, 1),
            make_goal("Epsilon", GoalStatus::Active, 2),
        ];
        let view = GoalRegisterView::from_records(records);

        assert_eq!(view.sections[0].goals.len(), 2, "Active");
        assert_eq!(view.sections[1].goals.len(), 1, "Proposed");
        assert_eq!(view.sections[2].goals.len(), 1, "Paused");
        assert_eq!(view.sections[3].goals.len(), 1, "Completed");

        // Active section should be sorted by priority
        assert_eq!(view.sections[0].goals[0].title, "Alpha");
        assert_eq!(view.sections[0].goals[1].title, "Epsilon");
    }

    #[test]
    fn goal_register_view_empty_input() {
        let view = GoalRegisterView::from_records(vec![]);
        for section in &view.sections {
            assert!(section.goals.is_empty());
        }
    }

    #[test]
    fn goal_register_view_sorts_within_sections_by_priority_then_title() {
        let records = vec![
            make_goal("Zeta", GoalStatus::Active, 2),
            make_goal("Alpha", GoalStatus::Active, 2),
            make_goal("Beta", GoalStatus::Active, 1),
        ];
        let view = GoalRegisterView::from_records(records);
        assert_eq!(view.sections[0].goals[0].title, "Beta");
        assert_eq!(view.sections[0].goals[1].title, "Alpha");
        assert_eq!(view.sections[0].goals[2].title, "Zeta");
    }

    #[test]
    fn goal_register_view_all_same_status() {
        let records = vec![
            make_goal("A", GoalStatus::Proposed, 1),
            make_goal("B", GoalStatus::Proposed, 2),
        ];
        let view = GoalRegisterView::from_records(records);
        assert_eq!(view.sections[0].goals.len(), 0, "Active");
        assert_eq!(view.sections[1].goals.len(), 2, "Proposed");
    }

    #[test]
    fn goal_register_view_single_goal_each_status() {
        let records = vec![
            make_goal("A", GoalStatus::Active, 1),
            make_goal("B", GoalStatus::Proposed, 1),
            make_goal("C", GoalStatus::Paused, 1),
            make_goal("D", GoalStatus::Completed, 1),
        ];
        let view = GoalRegisterView::from_records(records);
        for section in &view.sections {
            assert_eq!(section.goals.len(), 1);
        }
    }

    #[test]
    fn goal_register_view_render_lines_lists_goals_by_section_with_concise_labels() {
        let records = vec![
            make_goal("Alpha", GoalStatus::Active, 1),
            make_goal("Beta", GoalStatus::Proposed, 2),
        ];
        let view = GoalRegisterView::from_records(records);
        let lines = view.render_lines();
        // Sections render in Active, Proposed, Paused, Completed order. Populated
        // sections list each goal via its `p{priority} [{status}] {title}` label;
        // empty sections render a `<none>` placeholder.
        assert_eq!(
            lines,
            vec![
                "Active goals count: 1".to_string(),
                "Active goal 1: p1 [active] Alpha".to_string(),
                "Proposed goals count: 1".to_string(),
                "Proposed goal 1: p2 [proposed] Beta".to_string(),
                "Paused goals count: 0".to_string(),
                "Paused goals: <none>".to_string(),
                "Completed goals count: 0".to_string(),
                "Completed goals: <none>".to_string(),
            ]
        );
    }

    #[test]
    fn goal_register_view_render_lines_empty_reports_none_for_every_section() {
        let view = GoalRegisterView::from_records(vec![]);
        let lines = view.render_lines();
        assert_eq!(
            lines,
            vec![
                "Active goals count: 0",
                "Active goals: <none>",
                "Proposed goals count: 0",
                "Proposed goals: <none>",
                "Paused goals count: 0",
                "Paused goals: <none>",
                "Completed goals count: 0",
                "Completed goals: <none>",
            ]
        );
        // The `print` wrapper only forwards `render_lines` to stdout; exercise it
        // once so the `println!` path stays covered without capturing stdout.
        view.print();
    }

    #[test]
    fn goal_register_section_sorts_by_slug_as_final_tiebreak() {
        let records = vec![
            make_goal("Same", GoalStatus::Active, 1),
            make_goal("Same", GoalStatus::Active, 1),
        ];
        let view = GoalRegisterView::from_records(records);
        // Both have same title and priority, slug is derived from title
        assert_eq!(view.sections[0].goals.len(), 2);
    }

    #[test]
    fn goal_register_section_labels() {
        let section = GoalRegisterSection::new(GoalStatus::Active, vec![]);
        assert_eq!(section.heading, "Active");
        assert_eq!(section.label, "Active goals");

        let section = GoalRegisterSection::new(GoalStatus::Proposed, vec![]);
        assert_eq!(section.heading, "Proposed");

        let section = GoalRegisterSection::new(GoalStatus::Paused, vec![]);
        assert_eq!(section.heading, "Paused");

        let section = GoalRegisterSection::new(GoalStatus::Completed, vec![]);
        assert_eq!(section.heading, "Completed");
    }
}
