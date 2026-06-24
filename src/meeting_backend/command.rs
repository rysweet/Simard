//! Simple command parsing for the unified meeting backend.
//!
//! Only slash commands are special — everything else is natural conversation.

/// Parsed command from user input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MeetingCommand {
    Help,
    Close,
    Status,
    /// Show or apply a meeting template (standup, 1on1, retro, planning).
    /// Empty string means "list available templates".
    Template(String),
    /// Export the current meeting as markdown to ~/.simard/meetings/.
    Export,
    /// Record an explicit theme for the meeting (e.g. `/theme performance`).
    Theme(String),
    /// Show a color-coded recap of the current session (decisions, actions, questions, themes).
    Recap,
    /// Preview what the handoff artifact will look like when the meeting closes.
    Preview,
    /// Re-display the running list of decisions, open questions, and action items
    /// extracted from the live meeting transcript. Read-only — does not close.
    State,
    /// Operator marks a decision deterministically (e.g. `/decision Adopt TDD`).
    /// Optional `--rationale <text>` flag supplies structured rationale.
    /// Bypasses post-hoc heuristic extraction so the item cannot be missed.
    /// Unified to store `MeetingDecision` in issue #2086.
    Decision {
        text: String,
        rationale: Option<String>,
    },
    /// Operator records an action item inline (e.g.
    /// `/action Bob will write tests by friday`). The text is parsed for
    /// assignee/deadline using the same extractors as the heuristic path.
    Action(String),
    /// Operator marks an open question deterministically (e.g.
    /// `/question What is our SLO target?`).
    Question(String),
    /// Operator names the agent / persona / human expected to action this
    /// handoff (e.g. `/owner engineer`, `/owner ooda-curate`, `/owner alice`).
    /// Empty payload (a bare `/owner`) falls through to conversation so
    /// the operator's intent isn't silently coerced. Added in issue #1954.
    Owner(String),
    /// Operator sets the meeting's overarching objective (e.g.
    /// `/goal Agree on the release plan for v2`). Empty payload falls
    /// through to conversation. Added in issue #1987.
    Goal(String),
    /// Operator records an identified risk (e.g.
    /// `/risk Dependency on unstable API may delay launch`). Empty payload
    /// falls through to conversation. Required by spec line 637
    /// ("identified risks"). Added in issue #2084.
    Risk(String),
    /// Operator records a disagreement or dissenting view (e.g.
    /// `/disagree I think we should use Python instead`). Empty payload
    /// falls through to conversation. Required by spec line 645
    /// ("surface disagreement and uncertainty"). Added in issue #2084.
    Disagree(String),
    /// An argument-less slash token that resembles a command but matches no
    /// known command (e.g. a typo like `/colse`). `input` echoes the
    /// operator-typed token (original case preserved); `suggestion` is the
    /// closest known command within Levenshtein distance 2, or `None` when
    /// nothing is close. Rendered at the dispatch site as a "did you mean?"
    /// hint instead of being forwarded to the LLM. Added for the meeting-REPL
    /// UX prong (issue #2321).
    Unknown {
        input: String,
        suggestion: Option<String>,
    },
    /// Natural language — forwarded to the LLM.
    Conversation(String),
}

/// Canonical command tokens recognized by the meeting REPL. Used both as the
/// "is this a known command?" set for unknown-command detection and as the
/// candidate pool for did-you-mean suggestions. Includes the `/done` alias
/// for `/close`. Kept in sync with [`HELP_GROUPS`] by a unit test.
const KNOWN_COMMANDS: &[&str] = &[
    "/help",
    "/close",
    "/done",
    "/status",
    "/export",
    "/recap",
    "/preview",
    "/state",
    "/template",
    "/theme",
    "/decision",
    "/action",
    "/question",
    "/owner",
    "/goal",
    "/risk",
    "/disagree",
];

/// Maximum Levenshtein distance for a typo to earn a did-you-mean suggestion.
const SUGGESTION_MAX_DISTANCE: usize = 2;

/// Well-known absolute filesystem path roots (the Filesystem Hierarchy
/// Standard top-level directories). A bare single-component slash token that
/// matches one of these (e.g. `/home`, `/tmp`) is almost certainly a path the
/// operator typed, not a mistyped command — and some collide coincidentally
/// with a command within edit distance 2 (`/home` vs `/done`). Excluding them
/// keeps such paths as conversation instead of emitting a misleading
/// "did you mean?" hint. None of these are themselves commands or plausible
/// typos of commands, so the exclusion never suppresses a real suggestion.
/// Issue #2321.
const COMMON_PATH_ROOTS: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/media", "/mnt", "/opt", "/proc",
    "/root", "/run", "/sbin", "/srv", "/sys", "/tmp", "/usr", "/var",
];

/// Parse a single line of input into a `MeetingCommand`.
///
/// Only `/help`, `/close` (and `/done`), and `/status` are recognised as
/// commands. Everything else — including lines that happen to start with `/`
/// but aren't one of the above — is treated as conversation.
pub fn parse_command(input: &str) -> MeetingCommand {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return MeetingCommand::Conversation(String::new());
    }
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        "/help" => MeetingCommand::Help,
        "/close" | "/done" => MeetingCommand::Close,
        "/status" => MeetingCommand::Status,
        "/export" => MeetingCommand::Export,
        "/recap" => MeetingCommand::Recap,
        "/preview" => MeetingCommand::Preview,
        "/state" => MeetingCommand::State,
        "/template" => MeetingCommand::Template(String::new()),
        _ if lower.starts_with("/template ") => {
            let arg = trimmed["/template ".len()..].trim().to_string();
            MeetingCommand::Template(arg)
        }
        _ if lower.starts_with("/theme ") => {
            let arg = trimmed["/theme ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Theme(arg)
            }
        }
        _ if lower.starts_with("/decision ") => {
            let arg = trimmed["/decision ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                let (text, rationale) = parse_rationale_flag(&arg);
                if text.is_empty() {
                    MeetingCommand::Conversation(trimmed.to_string())
                } else {
                    MeetingCommand::Decision { text, rationale }
                }
            }
        }
        _ if lower.starts_with("/action ") => {
            let arg = trimmed["/action ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Action(arg)
            }
        }
        _ if lower.starts_with("/question ") => {
            let arg = trimmed["/question ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Question(arg)
            }
        }
        _ if lower.starts_with("/owner ") => {
            let arg = trimmed["/owner ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Owner(arg)
            }
        }
        _ if lower.starts_with("/goal ") => {
            let arg = trimmed["/goal ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Goal(arg)
            }
        }
        _ if lower.starts_with("/risk ") => {
            let arg = trimmed["/risk ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Risk(arg)
            }
        }
        _ if lower.starts_with("/disagree ") => {
            let arg = trimmed["/disagree ".len()..].trim().to_string();
            if arg.is_empty() {
                MeetingCommand::Conversation(trimmed.to_string())
            } else {
                MeetingCommand::Disagree(arg)
            }
        }
        _ => classify_unrecognized(trimmed, &lower),
    }
}

/// Classify a line that matched none of the explicit command arms.
///
/// A single command-like slash token (`/` followed only by ASCII letters,
/// e.g. `/colse`) that is not a known command is treated as a mistyped
/// command and routed to [`MeetingCommand::Unknown`] with a did-you-mean
/// suggestion. Everything else — multi-token slash lines (`/foo bar`), file
/// paths (`/home/user`, `/etc/hosts`, bare roots like `/tmp`), bare
/// empty-payload known commands (`/decision`), and plain prose — stays
/// [`MeetingCommand::Conversation`] so the operator's intent is never
/// silently coerced.
///
/// Scope boundary (deliberate, per issue #2321): only *argument-less* single
/// slash-tokens are treated as command attempts. A payload-bearing line such
/// as `/decison Adopt TDD` is left as conversation rather than suggested,
/// because distinguishing a typo+payload from an intentional `/word args`
/// conversational line is ambiguous and would risk hijacking real input.
/// Catching payload typos is tracked as a follow-up enhancement.
fn classify_unrecognized(trimmed: &str, lower: &str) -> MeetingCommand {
    if is_command_like(lower) && !KNOWN_COMMANDS.contains(&lower) {
        MeetingCommand::Unknown {
            input: trimmed.to_string(),
            suggestion: closest_command(lower),
        }
    } else {
        MeetingCommand::Conversation(trimmed.to_string())
    }
}

/// True when `lower` is a single slash-prefixed token of ASCII letters, e.g.
/// `/close` or `/colse`. Multi-token input (contains whitespace), file paths
/// (extra `/`), tokens with digits or punctuation, and well-known absolute
/// path roots (`/home`, `/tmp`, …) all return `false`, so operators can still
/// type paths and markdown lists that start with `/`.
///
/// `lower` is assumed already trimmed and lowercased (as produced by
/// [`parse_command`]).
fn is_command_like(lower: &str) -> bool {
    if COMMON_PATH_ROOTS.contains(&lower) {
        return false;
    }
    match lower.strip_prefix('/') {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphabetic()),
        None => false,
    }
}

/// Return the known command closest to `lower` within
/// [`SUGGESTION_MAX_DISTANCE`], or `None` if nothing is close enough. Ties
/// resolve to the earliest command in [`KNOWN_COMMANDS`] (so `/close` wins
/// over its `/done` alias).
fn closest_command(lower: &str) -> Option<String> {
    KNOWN_COMMANDS
        .iter()
        .map(|&cmd| (cmd, levenshtein(lower, cmd)))
        .filter(|&(_, dist)| dist <= SUGGESTION_MAX_DISTANCE)
        .min_by_key(|&(_, dist)| dist)
        .map(|(cmd, _)| cmd.to_string())
}

/// Classic Levenshtein edit distance (insertions, deletions, substitutions)
/// using a two-row rolling buffer. Operates on `char`s so multi-byte input
/// is counted correctly.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// One command shown in the grouped `/help` output.
pub struct HelpEntry {
    /// Canonical command token, e.g. `/decision`.
    pub token: &'static str,
    /// Full usage line shown to the operator, e.g.
    /// `/decision <text> [--rationale <why>]`.
    pub usage: &'static str,
    /// One-line description of what the command does.
    pub description: &'static str,
}

/// A titled group of related commands for the grouped `/help` output.
pub struct HelpGroup {
    pub title: &'static str,
    pub entries: &'static [HelpEntry],
}

/// Grouped, ordered command reference rendered by `/help`. Single source of
/// truth shared by the CLI REPL (colorized) and the dashboard chat (plain).
/// Every user-facing command appears in exactly one group (the `/done` alias
/// is intentionally omitted; it is documented under `/close`).
pub const HELP_GROUPS: &[HelpGroup] = &[
    HelpGroup {
        title: "Meeting control",
        entries: &[
            HelpEntry {
                token: "/help",
                usage: "/help",
                description: "Show this grouped command list",
            },
            HelpEntry {
                token: "/close",
                usage: "/close",
                description: "End the meeting and persist the handoff (alias: /done)",
            },
            HelpEntry {
                token: "/status",
                usage: "/status",
                description: "Show session info (topic, started_at, message count)",
            },
            HelpEntry {
                token: "/export",
                usage: "/export",
                description: "Export the meeting transcript as markdown",
            },
            HelpEntry {
                token: "/recap",
                usage: "/recap",
                description: "Show a color-coded session recap",
            },
            HelpEntry {
                token: "/preview",
                usage: "/preview",
                description: "Preview the handoff artifact before closing",
            },
            HelpEntry {
                token: "/state",
                usage: "/state",
                description: "Show current decisions, questions, actions, risks, disagreements",
            },
        ],
    },
    HelpGroup {
        title: "Capture",
        entries: &[
            HelpEntry {
                token: "/decision",
                usage: "/decision <text> [--rationale <why>]",
                description: "Record a decision (optional rationale)",
            },
            HelpEntry {
                token: "/action",
                usage: "/action <text>",
                description: "Record an action item (assignee/deadline parsed inline)",
            },
            HelpEntry {
                token: "/question",
                usage: "/question <text>",
                description: "Record an open question",
            },
            HelpEntry {
                token: "/risk",
                usage: "/risk <text>",
                description: "Record an identified risk",
            },
            HelpEntry {
                token: "/disagree",
                usage: "/disagree <text>",
                description: "Record a disagreement or dissenting view",
            },
            HelpEntry {
                token: "/theme",
                usage: "/theme <text>",
                description: "Record a theme for this meeting",
            },
            HelpEntry {
                token: "/owner",
                usage: "/owner <name>",
                description: "Name the next agent/persona/human to action this handoff",
            },
            HelpEntry {
                token: "/goal",
                usage: "/goal <text>",
                description: "Set the meeting's overarching objective",
            },
        ],
    },
    HelpGroup {
        title: "Templates",
        entries: &[HelpEntry {
            token: "/template",
            usage: "/template [name]",
            description: "List templates, or apply one (standup, 1on1, retro, planning)",
        }],
    },
];

/// Render the grouped `/help` reference as plain (uncolored) text. Used by the
/// dashboard chat transport and as a colorless fallback. The CLI REPL applies
/// ANSI color to the group titles separately.
pub fn render_help_plain() -> String {
    let mut out = String::new();
    for (i, group) in HELP_GROUPS.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(group.title);
        out.push_str(":\n");
        for entry in group.entries {
            out.push_str(&format!("  {} — {}\n", entry.usage, entry.description));
        }
    }
    out.push_str("\nEverything else is natural conversation with Simard.");
    out
}

/// Build the one-line notice shown when an operator types an unrecognized
/// command. With a `suggestion`, offers a did-you-mean; otherwise points the
/// operator at `/help`.
pub fn unknown_command_notice(input: &str, suggestion: Option<&str>) -> String {
    match suggestion {
        Some(s) => format!("Unknown command '{input}'. Did you mean '{s}'?"),
        None => format!("Unknown command '{input}'. Type /help for the full list."),
    }
}

/// Parse an optional `--rationale <text>` flag from a `/decision` argument.
///
/// Splits the input on `--rationale` (case-insensitive). Everything before
/// the flag is the decision text; everything after is the rationale.
/// Returns `(decision_text, Option<rationale>)`. If `--rationale` is
/// absent, the rationale is `None`. Added in issue #2086.
fn parse_rationale_flag(input: &str) -> (String, Option<String>) {
    let lower = input.to_lowercase();
    if let Some(idx) = lower.find("--rationale") {
        let text = input[..idx].trim().to_string();
        let rationale_start = idx + "--rationale".len();
        let rationale = input[rationale_start..].trim().to_string();
        let rationale = if rationale.is_empty() {
            None
        } else {
            Some(rationale)
        };
        (text, rationale)
    } else {
        (input.to_string(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_help() {
        assert_eq!(parse_command("/help"), MeetingCommand::Help);
        assert_eq!(parse_command("  /HELP  "), MeetingCommand::Help);
    }

    #[test]
    fn parse_close_variants() {
        assert_eq!(parse_command("/close"), MeetingCommand::Close);
        assert_eq!(parse_command("/done"), MeetingCommand::Close);
        assert_eq!(parse_command("  /Close "), MeetingCommand::Close);
    }

    #[test]
    fn parse_status() {
        assert_eq!(parse_command("/status"), MeetingCommand::Status);
    }

    #[test]
    fn parse_conversation_plain_text() {
        assert_eq!(
            parse_command("Let's discuss the roadmap"),
            MeetingCommand::Conversation("Let's discuss the roadmap".to_string()),
        );
    }

    #[test]
    fn parse_conversation_unknown_slash() {
        // Unrecognised slash commands fall through to Conversation so the
        // operator can still type things like file paths or markdown lists
        // that happen to start with `/`.
        assert_eq!(
            parse_command("/notarealcommand foo bar"),
            MeetingCommand::Conversation("/notarealcommand foo bar".to_string()),
        );
    }

    #[test]
    fn parse_template_no_arg() {
        assert_eq!(
            parse_command("/template"),
            MeetingCommand::Template(String::new())
        );
        assert_eq!(
            parse_command("  /TEMPLATE  "),
            MeetingCommand::Template(String::new())
        );
    }

    #[test]
    fn parse_template_with_arg() {
        assert_eq!(
            parse_command("/template standup"),
            MeetingCommand::Template("standup".to_string()),
        );
        assert_eq!(
            parse_command("  /Template  1on1  "),
            MeetingCommand::Template("1on1".to_string()),
        );
    }

    #[test]
    fn parse_export() {
        assert_eq!(parse_command("/export"), MeetingCommand::Export);
        assert_eq!(parse_command("  /EXPORT  "), MeetingCommand::Export);
    }

    #[test]
    fn parse_theme_with_arg() {
        assert_eq!(
            parse_command("/theme performance"),
            MeetingCommand::Theme("performance".to_string()),
        );
        assert_eq!(
            parse_command("  /Theme  scalability  "),
            MeetingCommand::Theme("scalability".to_string()),
        );
    }

    #[test]
    fn parse_theme_empty_arg_is_conversation() {
        // "/theme" with only whitespace after — not a valid theme, treated as conversation
        // parse_command trims input, so "/theme   " becomes "/theme" in the Conversation payload
        assert_eq!(
            parse_command("/theme   "),
            MeetingCommand::Conversation("/theme".to_string()),
        );
    }

    #[test]
    fn parse_recap() {
        assert_eq!(parse_command("/recap"), MeetingCommand::Recap);
        assert_eq!(parse_command("  /RECAP  "), MeetingCommand::Recap);
    }

    #[test]
    fn parse_preview() {
        assert_eq!(parse_command("/preview"), MeetingCommand::Preview);
        assert_eq!(parse_command("  /PREVIEW  "), MeetingCommand::Preview);
    }

    #[test]
    fn parse_empty_input() {
        assert_eq!(
            parse_command(""),
            MeetingCommand::Conversation(String::new()),
        );
        assert_eq!(
            parse_command("   "),
            MeetingCommand::Conversation(String::new()),
        );
    }

    // ── /state command (issue #1646 — TDD red phase) ─────────────────

    #[test]
    fn parse_state_exact_token() {
        // "/state" with no surplus tokens parses to State variant.
        assert_eq!(parse_command("/state"), MeetingCommand::State);
    }

    #[test]
    fn parse_state_case_and_whitespace_insensitive() {
        // Mirrors /help, /close, /status conventions.
        assert_eq!(parse_command("  /STATE  "), MeetingCommand::State);
        assert_eq!(parse_command("/State"), MeetingCommand::State);
    }

    #[test]
    fn parse_state_with_surplus_tokens_is_conversation() {
        // Security M4 / S5: /state takes no arguments. Surplus tokens must
        // NOT be silently coerced into a State command — they fall through
        // to Conversation so the operator's intent isn't misread.
        assert_eq!(
            parse_command("/state foo"),
            MeetingCommand::Conversation("/state foo".to_string()),
        );
        assert_eq!(
            parse_command("/state extra args"),
            MeetingCommand::Conversation("/state extra args".to_string()),
        );
    }

    // ── Inline /decision /action /question (issue #1730 seam (b)) ─────

    #[test]
    fn parse_decision_with_arg() {
        assert_eq!(
            parse_command("/decision Adopt TDD for new modules"),
            MeetingCommand::Decision {
                text: "Adopt TDD for new modules".to_string(),
                rationale: None,
            },
        );
        assert_eq!(
            parse_command("  /Decision   Ship phase 8  "),
            MeetingCommand::Decision {
                text: "Ship phase 8".to_string(),
                rationale: None,
            },
        );
    }

    #[test]
    fn parse_decision_empty_arg_is_conversation() {
        // Mirrors the /theme empty-arg behaviour: a bare `/decision` (or
        // `/decision ` with only whitespace) is not a valid recording —
        // surface as conversation so the operator's intent isn't lost.
        assert_eq!(
            parse_command("/decision"),
            MeetingCommand::Conversation("/decision".to_string()),
        );
        assert_eq!(
            parse_command("/decision   "),
            MeetingCommand::Conversation("/decision".to_string()),
        );
    }

    #[test]
    fn parse_action_with_arg() {
        assert_eq!(
            parse_command("/action Bob will write tests by friday"),
            MeetingCommand::Action("Bob will write tests by friday".to_string()),
        );
        assert_eq!(
            parse_command("  /ACTION  Update docs  "),
            MeetingCommand::Action("Update docs".to_string()),
        );
    }

    #[test]
    fn parse_action_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/action"),
            MeetingCommand::Conversation("/action".to_string()),
        );
        assert_eq!(
            parse_command("/action    "),
            MeetingCommand::Conversation("/action".to_string()),
        );
    }

    #[test]
    fn parse_question_with_arg() {
        assert_eq!(
            parse_command("/question What is our SLO target?"),
            MeetingCommand::Question("What is our SLO target?".to_string()),
        );
        assert_eq!(
            parse_command("  /Question   Who owns rollout?  "),
            MeetingCommand::Question("Who owns rollout?".to_string()),
        );
    }

    #[test]
    fn parse_question_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/question"),
            MeetingCommand::Conversation("/question".to_string()),
        );
        assert_eq!(
            parse_command("/question  "),
            MeetingCommand::Conversation("/question".to_string()),
        );
    }

    // ── Inline /owner (issue #1954) ──────────────────────────────────

    #[test]
    fn parse_owner_with_arg() {
        assert_eq!(
            parse_command("/owner engineer"),
            MeetingCommand::Owner("engineer".to_string()),
        );
        assert_eq!(
            parse_command("  /Owner  alice  "),
            MeetingCommand::Owner("alice".to_string()),
        );
    }

    #[test]
    fn parse_owner_preserves_case() {
        // GitHub handles are case-sensitive; the parser must preserve
        // operator-typed case rather than lowercasing.
        assert_eq!(
            parse_command("/owner RyanSweet"),
            MeetingCommand::Owner("RyanSweet".to_string()),
        );
    }

    #[test]
    fn parse_owner_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/owner"),
            MeetingCommand::Conversation("/owner".to_string()),
        );
        assert_eq!(
            parse_command("/owner    "),
            MeetingCommand::Conversation("/owner".to_string()),
        );
    }

    // ── Inline /goal (issue #1987) ───────────────────────────────────

    #[test]
    fn parse_goal_with_arg() {
        assert_eq!(
            parse_command("/goal Agree on the release plan for v2"),
            MeetingCommand::Goal("Agree on the release plan for v2".to_string()),
        );
        assert_eq!(
            parse_command("  /Goal  Ship the feature  "),
            MeetingCommand::Goal("Ship the feature".to_string()),
        );
    }

    #[test]
    fn parse_goal_preserves_case() {
        assert_eq!(
            parse_command("/goal Finalize OAuth Flow"),
            MeetingCommand::Goal("Finalize OAuth Flow".to_string()),
        );
    }

    #[test]
    fn parse_goal_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/goal"),
            MeetingCommand::Conversation("/goal".to_string()),
        );
        assert_eq!(
            parse_command("/goal    "),
            MeetingCommand::Conversation("/goal".to_string()),
        );
    }

    // ── /decision --rationale flag (issue #2086) ─────────────────────

    #[test]
    fn parse_decision_with_rationale_flag() {
        assert_eq!(
            parse_command("/decision Adopt TDD --rationale Memory safety and correctness"),
            MeetingCommand::Decision {
                text: "Adopt TDD".to_string(),
                rationale: Some("Memory safety and correctness".to_string()),
            },
        );
    }

    #[test]
    fn parse_decision_rationale_flag_case_insensitive() {
        assert_eq!(
            parse_command("/decision Use Rust --RATIONALE Performance matters"),
            MeetingCommand::Decision {
                text: "Use Rust".to_string(),
                rationale: Some("Performance matters".to_string()),
            },
        );
    }

    #[test]
    fn parse_decision_rationale_flag_empty_rationale() {
        // --rationale present but no text after it → rationale is None
        assert_eq!(
            parse_command("/decision Adopt TDD --rationale"),
            MeetingCommand::Decision {
                text: "Adopt TDD".to_string(),
                rationale: None,
            },
        );
    }

    #[test]
    fn parse_rationale_flag_helper() {
        let (text, rationale) = parse_rationale_flag("Adopt TDD --rationale Good for quality");
        assert_eq!(text, "Adopt TDD");
        assert_eq!(rationale, Some("Good for quality".to_string()));

        let (text, rationale) = parse_rationale_flag("No flag here");
        assert_eq!(text, "No flag here");
        assert_eq!(rationale, None);
    }

    // ── /risk (issue #2084) ──────────────────────────────────────────

    #[test]
    fn parse_risk_with_arg() {
        assert_eq!(
            parse_command("/risk Dependency on unstable API may delay launch"),
            MeetingCommand::Risk("Dependency on unstable API may delay launch".to_string()),
        );
        assert_eq!(
            parse_command("  /Risk  Single point of failure  "),
            MeetingCommand::Risk("Single point of failure".to_string()),
        );
    }

    #[test]
    fn parse_risk_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/risk"),
            MeetingCommand::Conversation("/risk".to_string()),
        );
        assert_eq!(
            parse_command("/risk    "),
            MeetingCommand::Conversation("/risk".to_string()),
        );
    }

    // ── /disagree (issue #2084) ──────────────────────────────────────

    #[test]
    fn parse_disagree_with_arg() {
        assert_eq!(
            parse_command("/disagree I think we should use Python instead"),
            MeetingCommand::Disagree("I think we should use Python instead".to_string()),
        );
        assert_eq!(
            parse_command("  /Disagree  This approach is too risky  "),
            MeetingCommand::Disagree("This approach is too risky".to_string()),
        );
    }

    #[test]
    fn parse_disagree_preserves_case() {
        assert_eq!(
            parse_command("/disagree Alice thinks we need more testing"),
            MeetingCommand::Disagree("Alice thinks we need more testing".to_string()),
        );
    }

    #[test]
    fn parse_disagree_empty_arg_is_conversation() {
        assert_eq!(
            parse_command("/disagree"),
            MeetingCommand::Conversation("/disagree".to_string()),
        );
        assert_eq!(
            parse_command("/disagree    "),
            MeetingCommand::Conversation("/disagree".to_string()),
        );
    }

    // ── Unknown-slash-command suggestions (issue #2321) ──────────────

    #[test]
    fn parse_exact_commands_are_not_unknown() {
        // Every known no-arg command must keep parsing to its own variant,
        // never to Unknown.
        assert_eq!(parse_command("/help"), MeetingCommand::Help);
        assert_eq!(parse_command("/close"), MeetingCommand::Close);
        assert_eq!(parse_command("/done"), MeetingCommand::Close);
        assert_eq!(parse_command("/status"), MeetingCommand::Status);
        assert_eq!(parse_command("/export"), MeetingCommand::Export);
        assert_eq!(parse_command("/recap"), MeetingCommand::Recap);
        assert_eq!(parse_command("/preview"), MeetingCommand::Preview);
        assert_eq!(parse_command("/state"), MeetingCommand::State);
        assert_eq!(
            parse_command("/template"),
            MeetingCommand::Template(String::new())
        );
    }

    #[test]
    fn parse_typo_within_distance_two_suggests_closest() {
        assert_eq!(
            parse_command("/colse"),
            MeetingCommand::Unknown {
                input: "/colse".to_string(),
                suggestion: Some("/close".to_string()),
            },
        );
        assert_eq!(
            parse_command("/clse"),
            MeetingCommand::Unknown {
                input: "/clse".to_string(),
                suggestion: Some("/close".to_string()),
            },
        );
        assert_eq!(
            parse_command("/statsu"),
            MeetingCommand::Unknown {
                input: "/statsu".to_string(),
                suggestion: Some("/status".to_string()),
            },
        );
        assert_eq!(
            parse_command("/recp"),
            MeetingCommand::Unknown {
                input: "/recp".to_string(),
                suggestion: Some("/recap".to_string()),
            },
        );
    }

    #[test]
    fn parse_unknown_preserves_typed_case_in_input() {
        // The echoed `input` keeps the operator's original case; the
        // suggestion is the canonical lowercase command.
        assert_eq!(
            parse_command("/COLSE"),
            MeetingCommand::Unknown {
                input: "/COLSE".to_string(),
                suggestion: Some("/close".to_string()),
            },
        );
    }

    #[test]
    fn parse_distant_garbage_is_unknown_with_no_suggestion() {
        assert_eq!(
            parse_command("/xyzzy"),
            MeetingCommand::Unknown {
                input: "/xyzzy".to_string(),
                suggestion: None,
            },
        );
        assert_eq!(
            parse_command("/zzzzzzzz"),
            MeetingCommand::Unknown {
                input: "/zzzzzzzz".to_string(),
                suggestion: None,
            },
        );
    }

    #[test]
    fn parse_conversational_slash_with_args_is_untouched() {
        // Multi-token slash lines remain conversation — never Unknown.
        assert_eq!(
            parse_command("/notarealcommand foo bar"),
            MeetingCommand::Conversation("/notarealcommand foo bar".to_string()),
        );
        assert_eq!(
            parse_command("/foo some text"),
            MeetingCommand::Conversation("/foo some text".to_string()),
        );
    }

    #[test]
    fn parse_file_path_is_conversation_not_unknown() {
        // Single tokens that are filesystem paths (extra `/`, dots, digits)
        // must stay conversation so operators can type paths.
        assert_eq!(
            parse_command("/home/user/file.txt"),
            MeetingCommand::Conversation("/home/user/file.txt".to_string()),
        );
        assert_eq!(
            parse_command("/etc/hosts"),
            MeetingCommand::Conversation("/etc/hosts".to_string()),
        );
        assert_eq!(
            parse_command("/v2"),
            MeetingCommand::Conversation("/v2".to_string()),
        );
    }

    #[test]
    fn parse_common_path_roots_stay_conversation() {
        // Bare single-component absolute paths (FHS roots) must not be
        // mistaken for commands — `/home` is within edit distance 2 of
        // `/done`, so without the guard it would wrongly suggest `/done`.
        for root in [
            "/home", "/tmp", "/var", "/usr", "/etc", "/bin", "/dev", "/opt", "/proc", "/sys",
            "/run", "/lib", "/srv", "/mnt", "/boot", "/root", "/sbin", "/media",
        ] {
            assert_eq!(
                parse_command(root),
                MeetingCommand::Conversation(root.to_string()),
                "FHS path root {root} must stay conversation",
            );
        }
        // Case-insensitive: the lowercased token still matches a known root.
        assert_eq!(
            parse_command("/HOME"),
            MeetingCommand::Conversation("/HOME".to_string()),
        );
    }

    #[test]
    fn parse_command_typo_with_payload_stays_conversation() {
        // Deliberate scope boundary (issue #2321): only argument-less single
        // slash-tokens are treated as command attempts. A typo that carries a
        // payload (`/decison Adopt TDD`) is multi-token, so it is left as
        // conversation rather than producing a suggestion — distinguishing it
        // from an intentional `/word args` line is ambiguous.
        assert_eq!(
            parse_command("/decison Adopt TDD"),
            MeetingCommand::Conversation("/decison Adopt TDD".to_string()),
        );
        assert_eq!(
            parse_command("/quesion Who owns this?"),
            MeetingCommand::Conversation("/quesion Who owns this?".to_string()),
        );
    }

    #[test]
    fn parse_bare_empty_payload_commands_stay_conversation() {
        // Known arg-requiring commands typed bare already fall through to
        // conversation; the Unknown detection must not change that.
        for cmd in [
            "/decision",
            "/action",
            "/question",
            "/owner",
            "/goal",
            "/risk",
            "/disagree",
            "/theme",
        ] {
            assert_eq!(
                parse_command(cmd),
                MeetingCommand::Conversation(cmd.to_string()),
                "bare {cmd} must stay conversation",
            );
        }
    }

    #[test]
    fn closest_command_respects_distance_threshold() {
        assert_eq!(closest_command("/colse"), Some("/close".to_string()));
        assert_eq!(closest_command("/templat"), Some("/template".to_string()));
        // Distance 3+ → no suggestion.
        assert_eq!(closest_command("/xyzzy"), None);
        assert_eq!(closest_command("/qqqqqq"), None);
    }

    #[test]
    fn levenshtein_basic_distances() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("close", "close"), 0);
        assert_eq!(levenshtein("clse", "close"), 1);
        assert_eq!(levenshtein("colse", "close"), 2);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    // ── Grouped /help (issue #2321) ──────────────────────────────────

    #[test]
    fn help_groups_have_expected_titles() {
        let titles: Vec<&str> = HELP_GROUPS.iter().map(|g| g.title).collect();
        assert_eq!(titles, vec!["Meeting control", "Capture", "Templates"]);
    }

    #[test]
    fn help_groups_render_all_user_commands_exactly_once() {
        // Every user-facing command (the canonical set minus the /done
        // alias) must appear in exactly one group, exactly once.
        let mut seen: Vec<&str> = Vec::new();
        for group in HELP_GROUPS {
            for entry in group.entries {
                assert!(
                    !seen.contains(&entry.token),
                    "{} appears more than once in /help",
                    entry.token,
                );
                seen.push(entry.token);
            }
        }
        let mut expected: Vec<&str> = KNOWN_COMMANDS
            .iter()
            .copied()
            .filter(|c| *c != "/done")
            .collect();
        seen.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            seen, expected,
            "grouped /help must cover every command once"
        );
    }

    #[test]
    fn render_help_plain_contains_groups_and_commands() {
        let help = render_help_plain();
        assert!(help.contains("Meeting control"));
        assert!(help.contains("Capture"));
        assert!(help.contains("Templates"));
        for group in HELP_GROUPS {
            for entry in group.entries {
                assert!(
                    help.contains(entry.token),
                    "plain help missing {}",
                    entry.token,
                );
            }
        }
        assert!(help.contains("natural conversation"));
    }

    #[test]
    fn known_commands_superset_of_help_tokens() {
        // Guard against drift: every displayed command is a recognized token.
        for group in HELP_GROUPS {
            for entry in group.entries {
                assert!(
                    KNOWN_COMMANDS.contains(&entry.token),
                    "{} shown in /help but not in KNOWN_COMMANDS",
                    entry.token,
                );
            }
        }
    }

    #[test]
    fn unknown_command_notice_formats() {
        assert_eq!(
            unknown_command_notice("/colse", Some("/close")),
            "Unknown command '/colse'. Did you mean '/close'?",
        );
        assert_eq!(
            unknown_command_notice("/xyzzy", None),
            "Unknown command '/xyzzy'. Type /help for the full list.",
        );
    }
}
