//! Predefined meeting templates with structured agendas.

/// A meeting template with a name, description, and opening prompt for the LLM.
pub struct MeetingTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub agenda_items: &'static [&'static str],
    pub opening_prompt: &'static str,
}

/// All built-in templates.
pub const TEMPLATES: &[MeetingTemplate] = &[
    MeetingTemplate {
        name: "standup",
        description: "Daily standup — quick sync on progress and blockers",
        agenda_items: &[
            "What I accomplished since last standup",
            "What I'm working on today",
            "Blockers or things I need help with",
        ],
        opening_prompt: "Let's run a standup. I'll go through what I did, what I'm doing next, and any blockers. Help me stay focused and flag anything that sounds risky.",
    },
    MeetingTemplate {
        name: "retro",
        description: "Retrospective — reflect on what worked and what to improve",
        agenda_items: &[
            "What went well",
            "What could be improved",
            "Action items for next cycle",
        ],
        opening_prompt: "Let's do a retrospective. I'll share what went well and what didn't. Help me identify patterns and suggest concrete action items.",
    },
    MeetingTemplate {
        name: "planning",
        description: "Planning session — priorities, dependencies, and assignments",
        agenda_items: &[
            "Priorities for this cycle",
            "Dependencies and risks",
            "Task assignments and ownership",
        ],
        opening_prompt: "Let's plan the upcoming work. Help me think through priorities, identify dependencies between tasks, and make sure nothing critical is missed.",
    },
    MeetingTemplate {
        name: "1on1",
        description: "One-on-one — wins, concerns, and feedback",
        agenda_items: &[
            "Recent wins and progress",
            "Concerns or challenges",
            "Feedback and growth areas",
        ],
        opening_prompt: "Let's have a 1:1 check-in. I'll share what's going well and any concerns. Help me think through challenges and identify opportunities for growth.",
    },
    MeetingTemplate {
        name: "bug-triage",
        description: "Bug triage — severity, repro, and owner assignment",
        agenda_items: &[
            "Severity assessment",
            "Reproduction steps",
            "Owner assignment and timeline",
        ],
        opening_prompt: "Let's triage bugs. For each one I'll describe the issue — help me assess severity, clarify reproduction steps, and decide on ownership.",
    },
];

/// Look up a template by name (case-insensitive, with common aliases).
pub fn find_template(name: &str) -> Option<&'static MeetingTemplate> {
    let lower = name.to_ascii_lowercase();
    // Support common aliases
    let canonical = match lower.as_str() {
        "sprint-planning" => "planning",
        "retrospective" => "retro",
        other => other,
    };
    TEMPLATES.iter().find(|t| t.name == canonical)
}

/// Format a template as a display string for the user.
pub fn format_template(template: &MeetingTemplate) -> String {
    let mut out = format!(
        "📋 Template: {} — {}\n\nAgenda:\n",
        template.name, template.description
    );
    for (i, item) in template.agenda_items.iter().enumerate() {
        out.push_str(&format!("  {}. {}\n", i + 1, item));
    }
    out
}

/// List all available templates as a display string.
pub fn list_templates() -> String {
    let mut out = String::from("Available meeting templates:\n");
    for t in TEMPLATES {
        out.push_str(&format!("  /template {}  — {}\n", t.name, t.description));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_known_templates() {
        assert!(find_template("standup").is_some());
        assert!(find_template("retro").is_some());
        assert!(find_template("planning").is_some());
        assert!(find_template("1on1").is_some());
        assert!(find_template("bug-triage").is_some());
    }

    #[test]
    fn find_case_insensitive() {
        assert!(find_template("STANDUP").is_some());
        assert!(find_template("Retro").is_some());
    }

    #[test]
    fn find_aliases() {
        assert!(find_template("sprint-planning").is_some());
        assert!(find_template("retrospective").is_some());
        assert_eq!(
            find_template("sprint-planning").unwrap().name,
            find_template("planning").unwrap().name
        );
        assert_eq!(
            find_template("retrospective").unwrap().name,
            find_template("retro").unwrap().name
        );
    }

    #[test]
    fn find_unknown_returns_none() {
        assert!(find_template("brainstorm").is_none());
    }

    #[test]
    fn list_templates_includes_all() {
        let listing = list_templates();
        assert!(listing.contains("standup"));
        assert!(listing.contains("retro"));
        assert!(listing.contains("planning"));
        assert!(listing.contains("1on1"));
        assert!(listing.contains("bug-triage"));
    }
}
