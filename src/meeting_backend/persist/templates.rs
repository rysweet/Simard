//! Built-in meeting templates and lookup.


/// Meeting template content (agenda and prompts) for common meeting types.
pub struct MeetingTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub agenda: &'static str,
}

/// All available meeting templates.
pub const TEMPLATES: &[MeetingTemplate] = &[
    MeetingTemplate {
        name: "standup",
        description: "Daily standup / sync",
        agenda: "\
## Daily Standup

1. **What did you accomplish since last standup?**
2. **What are you working on today?**
3. **Any blockers or impediments?**

_Tip: Keep updates brief — flag blockers for offline follow-up._",
    },
    MeetingTemplate {
        name: "1on1",
        description: "One-on-one check-in",
        agenda: "\
## 1:1 Check-in

1. **How are things going?** (personal/professional)
2. **Progress on current goals**
3. **Feedback** — anything to share in either direction?
4. **Growth & development** — skills, interests, opportunities
5. **Action items from last time**

_Tip: This is their meeting — let them drive the agenda._",
    },
    MeetingTemplate {
        name: "retro",
        description: "Sprint retrospective",
        agenda: "\
## Retrospective

1. **What went well?** 🟢
2. **What didn't go well?** 🔴
3. **What can we improve?** 🔧
4. **Action items** — concrete, assigned, time-boxed

_Tip: Celebrate wins before diving into problems._",
    },
    MeetingTemplate {
        name: "planning",
        description: "Sprint / iteration planning",
        agenda: "\
## Planning Session

1. **Review previous sprint** — what carried over and why?
2. **Capacity check** — who's available, any PTO or conflicts?
3. **Backlog review** — prioritize items for this sprint
4. **Estimation** — size and assign selected items
5. **Sprint goal** — one sentence capturing the sprint's purpose
6. **Risks & dependencies** — anything that could block progress?

_Tip: Timebox estimation discussions — if it takes >2 min, take it offline._",
    },
];

/// Look up a template by name. Returns `None` if not found.
pub fn find_template(name: &str) -> Option<&'static MeetingTemplate> {
    TEMPLATES.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}
