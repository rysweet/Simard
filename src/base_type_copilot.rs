//! Real CopilotSdkAdapter — a base type that drives `amplihack copilot` via
//! the existing PTY terminal infrastructure and integrates with the memory
//! and knowledge bridges to enrich turn input with relevant context.
//!
//! The adapter launches a copilot subprocess through the PTY `script` wrapper
//! (reusing `terminal_session::execute_terminal_turn`), formats each turn's
//! objective with memory facts and domain knowledge, and parses the copilot's
//! structured output into [`BaseTypeOutcome`].

use crate::base_type_turn::{format_turn_input, parse_turn_output, prepare_turn_context};
use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, capability_set,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::knowledge_bridge::KnowledgeBridge;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use crate::sanitization::objective_metadata;
use crate::terminal_session::execute_terminal_turn;

/// Default command used to launch the copilot subprocess.
const DEFAULT_COPILOT_COMMAND: &str = "amplihack copilot";

/// Configuration for the copilot adapter.
#[derive(Clone, Debug)]
pub struct CopilotAdapterConfig {
    /// The shell command that launches the copilot.
    pub command: String,
    /// Working directory for the copilot session.
    pub working_directory: Option<String>,
}

impl Default for CopilotAdapterConfig {
    fn default() -> Self {
        Self {
            command: DEFAULT_COPILOT_COMMAND.to_string(),
            working_directory: None,
        }
    }
}

/// A base type factory that creates sessions driving `amplihack copilot`
/// through the PTY infrastructure with memory and knowledge enrichment.
#[derive(Debug)]
pub struct CopilotSdkAdapter {
    descriptor: BaseTypeDescriptor,
    config: CopilotAdapterConfig,
}

impl CopilotSdkAdapter {
    /// Create a new CopilotSdkAdapter with default configuration.
    pub fn registered(id: impl Into<String>) -> SimardResult<Self> {
        Self::with_config(id, CopilotAdapterConfig::default())
    }

    /// Create a new CopilotSdkAdapter with explicit configuration.
    ///
    /// The `config.command` is validated to reject shell metacharacters
    /// (`;`, `|`, `&`, `` ` ``, `$`) for defense-in-depth.
    pub fn with_config(id: impl Into<String>, config: CopilotAdapterConfig) -> SimardResult<Self> {
        validate_command(&config.command)?;
        let id = BaseTypeId::new(id);
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "copilot-sdk::pty-session",
                    "registered-base-type:copilot-sdk",
                    Freshness::now()?,
                ),
                capabilities: capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                    BaseTypeCapability::TerminalSession,
                ]),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            config,
        })
    }

    /// Access the adapter configuration.
    pub fn config(&self) -> &CopilotAdapterConfig {
        &self.config
    }
}

impl BaseTypeFactory for CopilotSdkAdapter {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open_session(
        &self,
        request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>> {
        if !self.descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: self.descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        Ok(Box::new(CopilotSdkSession {
            descriptor: self.descriptor.clone(),
            config: self.config.clone(),
            request,
            memory_bridge: None,
            knowledge_bridge: None,
            is_open: false,
            is_closed: false,
            turn_count: 0,
        }))
    }
}

/// A live copilot session that enriches objectives with memory and knowledge
/// before dispatching them through the terminal PTY.
struct CopilotSdkSession {
    descriptor: BaseTypeDescriptor,
    config: CopilotAdapterConfig,
    request: BaseTypeSessionRequest,
    memory_bridge: Option<Box<dyn CognitiveMemoryOps>>,
    knowledge_bridge: Option<KnowledgeBridge>,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
}

impl std::fmt::Debug for CopilotSdkSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopilotSdkSession")
            .field("descriptor", &self.descriptor)
            .field("is_open", &self.is_open)
            .field("is_closed", &self.is_closed)
            .field("turn_count", &self.turn_count)
            .finish()
    }
}

impl CopilotSdkSession {
    /// Build an enriched terminal objective from the turn input.
    ///
    /// The enriched objective includes a shell/command preamble that the
    /// terminal session infrastructure parses, followed by the formatted
    /// turn context as the command payload.
    ///
    /// When `identity_context` or `prompt_preamble` are provided (e.g. in
    /// meeting mode), they are prepended to the objective so the agent
    /// receives the full conversational context.
    fn build_enriched_objective(&self, input: &BaseTypeTurnInput) -> SimardResult<String> {
        let mut parts = Vec::new();
        if !input.prompt_preamble.is_empty() {
            parts.push(input.prompt_preamble.as_str());
        }
        if !input.identity_context.is_empty() {
            parts.push(input.identity_context.as_str());
        }
        parts.push(&input.objective);

        let combined_objective = parts.join("\n\n");
        let context = prepare_turn_context(
            &combined_objective,
            self.memory_bridge.as_deref(),
            self.knowledge_bridge.as_ref(),
        )?;
        let formatted = format_turn_input(&context);
        Ok(build_copilot_terminal_objective(&self.config, &formatted))
    }
}

impl BaseTypeSession for CopilotSdkSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;

        self.turn_count += 1;

        let enriched_objective = self.build_enriched_objective(&input)?;
        let enriched_input = BaseTypeTurnInput::objective_only(enriched_objective);

        tracing::info!(mode = %self.request.mode, turn = self.turn_count, "Copilot adapter: sending turn to Copilot SDK (this may take 30-90s)…");
        let terminal_outcome =
            execute_terminal_turn(&self.descriptor, &self.request, &enriched_input).map_err(
                |err| SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("copilot terminal turn failed: {err}"),
                },
            )?;

        // Extract the actual LLM response from the transcript.  The transcript
        // contains the command we sent, the copilot's response text, and then
        // our sentinel marker.  We extract everything between the command echo
        // and the sentinel as the meaningful response.
        let response_text = extract_copilot_response_from_evidence(&terminal_outcome.evidence);
        tracing::info!(
            response_len = response_text.len(),
            turn = self.turn_count,
            "Copilot adapter: received response"
        );
        // No fallback: an empty response after stripping noise/footer lines
        // means the Copilot CLI exited without producing a reply (auth
        // failure, rate limit, transcript truncated, etc.). Surface that as
        // an error rather than handing the caller an empty string or — worse
        // — leaking the billing-summary footer as if it were the assistant's
        // reply (issue #1062).
        if response_text.trim().is_empty() {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: "copilot returned no conversational content (auth failure, rate limit, or \
                     truncated transcript). Run `gh auth status` and inspect the most recent \
                     transcript in ~/.simard/agent_logs/."
                    .to_string(),
            });
        }

        // Record cost estimate based on prompt/response character sizes.
        let prompt_chars = enriched_input.objective.len();
        let completion_chars = response_text.len();
        if let Err(e) = crate::cost_tracking::record_cost(
            self.request.session_id.as_str(),
            "copilot",
            prompt_chars,
            completion_chars,
            &format!(
                "copilot turn {} on {}",
                self.turn_count, self.request.topology
            ),
        ) {
            eprintln!("[simard] cost tracking write failed: {e}");
        }

        let objective_summary = objective_metadata(&input.objective);
        let mut evidence = terminal_outcome.evidence;
        evidence.push(format!("copilot-adapter-command={}", self.config.command));
        evidence.push(format!("copilot-adapter-turn={}", self.turn_count));
        evidence.push(format!(
            "copilot-enriched-objective-length={}",
            enriched_input.objective.len()
        ));

        Ok(BaseTypeOutcome {
            plan: format!(
                "Copilot SDK adapter dispatched {} via '{}' on '{}' (turn {}).",
                objective_summary, self.config.command, self.request.topology, self.turn_count,
            ),
            execution_summary: response_text,
            evidence,
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        ensure_session_open(&self.descriptor, self.is_open, "close")?;
        self.is_closed = true;
        Ok(())
    }
}

/// Build a terminal-session-compatible objective string that launches the
/// copilot command with the enriched prompt.
///
/// Strategy: write the prompt to a temp file, run `<copilot-cmd> -p @file`,
/// then `exit` the shell.  The terminal infrastructure calls `finish()` which
/// waits for the process to exit naturally — no artificial timeouts.  The
/// copilot runs to completion however long it takes, and we read the full
/// transcript afterward.
pub(super) fn build_copilot_terminal_objective(
    config: &CopilotAdapterConfig,
    formatted_prompt: &str,
) -> String {
    let mut objective = String::new();

    if let Some(ref cwd) = config.working_directory {
        objective.push_str(&format!("working-directory: {cwd}\n"));
    }

    // Write the prompt to a temp file and pipe it via stdin to the copilot
    // command. Using stdin avoids `-p` flag incompatibility between the Python
    // and Rust versions of amplihack. The `--subprocess-safe` flag skips
    // interactive staging/env updates. Chain with `exit` so the shell exits
    // after the copilot finishes.
    let escaped = formatted_prompt
        .replace('\\', "\\\\")
        .replace('\'', "'\\''");
    objective.push_str(&format!(
        "command: SIMARD_PROMPT_FILE=$(mktemp /tmp/simard-copilot-prompt.XXXXXX) && \
         printf '%s' '{}' > \"$SIMARD_PROMPT_FILE\" && \
         cat \"$SIMARD_PROMPT_FILE\" | {} --subprocess-safe ; \
         rm -f \"$SIMARD_PROMPT_FILE\" ; exit\n",
        escaped, config.command,
    ));

    objective
}

/// Extract the actual copilot LLM response from the terminal transcript
/// embedded in the evidence vector.
///
/// Prefers the full transcript (`terminal-transcript-full`) over the
/// truncated preview.  The full transcript is newline-delimited; the
/// preview is pipe-delimited.
pub(super) fn extract_copilot_response_from_evidence(evidence: &[String]) -> String {
    // Prefer full transcript; use preview if transcript is empty.
    let transcript = evidence
        .iter()
        .find_map(|e| e.strip_prefix("terminal-transcript-full="))
        .or_else(|| {
            evidence
                .iter()
                .find_map(|e| e.strip_prefix("terminal-transcript-preview="))
        })
        .unwrap_or("");

    extract_response_from_transcript(transcript)
}

/// Parse the raw transcript text to isolate the copilot's response.
///
/// The transcript from `script` contains (in order):
///   Script started on ...
///   <bash prompt> <command echo>
///   <copilot bootstrap lines — hooks, XPIA defender, etc.>
///   <actual LLM response>
///   Total usage est: ...
///   API time spent: ...
///   bash-5.2$ exit
///   Script done on ...
///
/// We find the end of bootstrap noise and the start of usage stats to
/// isolate the actual LLM response in between.
pub(super) fn extract_response_from_transcript(transcript: &str) -> String {
    let lines: Vec<&str> = transcript.lines().collect();
    // Also try pipe-delimited (preview format) if no newlines found.
    let lines = if lines.len() <= 1 && transcript.contains(" | ") {
        transcript.split(" | ").collect::<Vec<_>>()
    } else {
        lines
    };

    let mut response_start = 0;
    let mut response_end = lines.len();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Bootstrap output: hook staging, XPIA defender, prompt file creation
        if trimmed.contains("Staged") && trimmed.contains("hook")
            || trimmed.contains("XPIA")
            || trimmed.contains("SIMARD_PROMPT_FILE")
            || trimmed.contains("amplihack copilot")
            || trimmed.starts_with("Script started on")
            || trimmed.starts_with("bash-") && trimmed.contains("$") && trimmed.contains("cat ")
            || is_transcript_noise_line(trimmed)
        {
            response_start = i + 1;
        }
        // Usage stats / session footer
        if is_copilot_footer_line(trimmed) && response_end == lines.len() {
            response_end = i;
        }
        // Shell exit / script done
        if (trimmed == "exit"
            || trimmed.ends_with("$ exit")
            || trimmed.starts_with("Script done on"))
            && response_end == lines.len()
        {
            response_end = i;
        }
    }

    if response_start >= response_end {
        // Delimiters not found — strip known noise lines from PTY output
        let stripped: String = lines
            .iter()
            .filter(|l| {
                let t = l.trim();
                !(t.is_empty()
                    || t.starts_with("Script ")
                    || t.contains("amplihack copilot")
                    || t.contains("SIMARD_PROMPT_FILE")
                    || t == "exit"
                    || t.ends_with("$ exit")
                    || is_copilot_footer_line(t)
                    || is_transcript_noise_line(t))
            })
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        return stripped;
    }

    let body: String = lines[response_start..response_end]
        .iter()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !is_copilot_footer_line(t) && !is_transcript_noise_line(t)
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    body
}

/// Recognize Copilot CLI session-footer / telemetry lines that must never
/// appear in the extracted assistant response.
///
/// Includes both the legacy `Total usage est:` / `API time spent:` /
/// `Total session time:` markers and the newer billing-summary footer
/// emitted by Copilot CLI ≥1.x:
///
/// ```text
/// Changes   +0 -0
/// Requests  7.5 Premium (10s)
/// ```
///
/// Without this guard the chat dashboard echoes the telemetry line back
/// to the user as if it were the assistant's reply (issue #1062).
pub(super) fn is_copilot_footer_line(trimmed: &str) -> bool {
    if trimmed.starts_with("Total usage est:")
        || trimmed.starts_with("API time spent:")
        || trimmed.starts_with("Total session time:")
    {
        return true;
    }
    // Newer Copilot CLI billing summary lines.
    if trimmed.starts_with("Changes") && (trimmed.contains(" +") || trimmed.contains(" -")) {
        return true;
    }
    if trimmed.starts_with("Requests")
        && (trimmed.contains("Premium") || trimmed.contains("Free") || trimmed.contains('('))
    {
        return true;
    }
    false
}

/// Detect transcript lines that are infrastructure artefacts rather than
/// conversational LLM output.
///
/// Filters:
///   * Shell `time` builtin output: `real 0m1.234s`, `user 0m0.123s`, `sys ...`
///   * Hook telemetry: `Staged ... hook`, `Loaded hook`, `Hook fired:`
///   * File-system artefacts emitted by tool plumbing: `Created file ...`,
///     `Modified file ...`, `Deleted file ...`, `Wrote file ...`
fn is_transcript_noise_line(trimmed: &str) -> bool {
    // Shell `time` builtin output (POSIX format: "real\t0m1.234s")
    for prefix in ["real\t", "real ", "user\t", "user ", "sys\t", "sys "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            // Looks like `0m1.234s` or `1.234s` — digit-led, ends with 's'
            let rest = rest.trim_start();
            if rest.ends_with('s')
                && rest
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
            {
                return true;
            }
        }
    }
    // Hook telemetry lines
    if (trimmed.contains("hook") || trimmed.contains("Hook"))
        && (trimmed.starts_with("Staged")
            || trimmed.starts_with("Loaded")
            || trimmed.starts_with("Unloaded")
            || trimmed.starts_with("Hook fired")
            || trimmed.starts_with("Hook:")
            || trimmed.starts_with("[hook]"))
    {
        return true;
    }
    // File-system artefacts from tool plumbing
    for prefix in [
        "Created file ",
        "Created file:",
        "Modified file ",
        "Modified file:",
        "Deleted file ",
        "Deleted file:",
        "Wrote file ",
        "Wrote file:",
    ] {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }
    // Amplihack CLI startup banners and version-update nags. These are
    // wrapped in ANSI color codes by the CLI, so test against the
    // ANSI-stripped form. The leading `ℹ` glyph amplihack prints is a
    // multi-byte UTF-8 character, so we match on the substring after it.
    let stripped = strip_ansi(trimmed);
    let stripped_trim = stripped.trim();
    let without_info_glyph = stripped_trim
        .trim_start_matches(|c: char| !c.is_ascii() || c.is_whitespace())
        .trim_start();
    if stripped_trim.contains("amplihack is available")
        || stripped_trim.starts_with("Run 'amplihack update'")
        || without_info_glyph.starts_with("NODE_OPTIONS=")
        || without_info_glyph.starts_with("amplihack ")
        || stripped_trim.starts_with("amplihack: ")
    {
        return true;
    }
    false
}

/// Remove ANSI escape sequences (CSI `\x1b[…m` color codes) from a line
/// so noise-detection can match on the visible text alone.
pub(super) fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // CSI: \x1b[ … final-byte-in-0x40..=0x7e
            let mut j = i + 2;
            while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                j += 1;
            }
            i = j.saturating_add(1);
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Parse copilot response text and extract a turn context summary for
/// evidence. This is a lightweight wrapper around the turn parser.
pub fn parse_copilot_response(raw: &str) -> SimardResult<crate::base_type_turn::TurnOutput> {
    parse_turn_output(raw)
}

/// Validate that a command string does not contain shell metacharacters.
///
/// This is a defense-in-depth check. The command is an operator-configured
/// value, not user input, but we reject obvious injection patterns to
/// prevent accidental misconfiguration.
fn validate_command(command: &str) -> SimardResult<()> {
    const FORBIDDEN: &[char] = &[';', '|', '&', '`', '$'];
    if let Some(ch) = command.chars().find(|c| FORBIDDEN.contains(c)) {
        return Err(SimardError::InvalidConfigValue {
            key: "command".to_string(),
            value: command.to_string(),
            help: format!(
                "command contains forbidden shell metacharacter '{ch}'; \
                 use a simple command without shell operators"
            ),
        });
    }
    if command.trim().is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: "command".to_string(),
            value: command.to_string(),
            help: "command must not be empty".to_string(),
        });
    }
    Ok(())
}
