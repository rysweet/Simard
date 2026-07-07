//! Real CopilotSdkAdapter — a base type that drives `amplihack copilot` via
//! the existing PTY terminal infrastructure and integrates with the memory
//! and knowledge bridges to enrich turn input with relevant context.
//!
//! The adapter launches a copilot subprocess through the PTY `script` wrapper
//! (reusing `terminal_session::execute_terminal_turn`), formats each turn's
//! objective with memory facts and domain knowledge, and parses the copilot's
//! structured output into [`BaseTypeOutcome`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::NamedTempFile;

use crate::base_type_turn::{
    EnrichmentClients, EnrichmentSource, TurnContext, format_turn_input, parse_turn_output,
};
use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, capability_set,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
};
#[cfg(test)]
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
#[cfg(test)]
use crate::knowledge_client::KnowledgeClient;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use crate::sanitization::objective_metadata;
use crate::spawn_payload::attach_prompt_std;
use crate::terminal_session::execute_terminal_turn;

/// Default command used to launch the copilot subprocess (non-meeting mode).
const DEFAULT_COPILOT_COMMAND: &str = "amplihack copilot";

/// Direct copilot binary used for meeting mode.
/// Meeting sessions invoke `copilot` directly to avoid `amplihack copilot`
/// injecting custom instructions (dev-orchestrator, auto-intent-router) that
/// cause the copilot to treat conversational prompts as engineering tasks.
const MEETING_COPILOT_BINARY: &str = "copilot";

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
///
/// Enrichment is sourced via the shared [`EnrichmentSource`] policy
/// ([`crate::base_type_turn`]): [`EnrichmentSource::Disabled`] by default so
/// lightweight callers and unit tests incur no filesystem side effects, and
/// [`EnrichmentSource::Native`] (opted into via [`CopilotSdkAdapter::with_enrichment`])
/// for the live production path, wiring the same cognitive-memory store and
/// native knowledge bridge the rest of the runtime uses (issue #1664).
#[derive(Debug)]
pub struct CopilotSdkAdapter {
    descriptor: BaseTypeDescriptor,
    config: CopilotAdapterConfig,
    enrichment: EnrichmentSource,
    /// Test-only seam: override the meeting-mode copilot binary
    /// ([`MEETING_COPILOT_BINARY`]) with an explicit program — typically an
    /// absolute path to a fake `copilot` used by hermetic meeting-turn tests
    /// (issue #2732). `None` in production, so live meeting turns always exec
    /// the real `copilot` binary; there is no production behavior change.
    meeting_binary_override: Option<String>,
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
            enrichment: EnrichmentSource::default(),
            meeting_binary_override: None,
        })
    }

    /// Test-only seam: override the meeting-mode copilot binary with `binary`
    /// (typically an absolute path to a fake `copilot`) so meeting-turn tests
    /// exercise the real `run_meeting_turn` path hermetically, without spawning
    /// the real `copilot` subprocess or depending on its auth/network state
    /// (issue #2732). Not used on any production path.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_meeting_binary_override(mut self, binary: impl Into<String>) -> Self {
        self.meeting_binary_override = Some(binary.into());
        self
    }

    /// Access the adapter configuration.
    pub fn config(&self) -> &CopilotAdapterConfig {
        &self.config
    }

    /// Enable per-turn memory + knowledge enrichment for sessions opened by
    /// this adapter, reading cognitive memory from `state_root`.
    ///
    /// Without this, sessions are created with both bridges set to `None` and
    /// every turn runs `prepare_turn_context(objective, None, None)` — the
    /// hardcoded-`None` regression of issue #1664 that silently dropped all
    /// memory and knowledge enrichment in the primary production adapter.
    ///
    /// Clients are launched lazily in [`BaseTypeFactory::open_session`]; a
    /// launch failure logs and degrades to `None` so a missing knowledge pack
    /// or an unavailable memory store never breaks turn dispatch (see
    /// [`crate::base_type_turn::launch_enrichment_bridges`]).
    #[must_use]
    pub fn with_enrichment(mut self, state_root: PathBuf) -> Self {
        self.enrichment = EnrichmentSource::Native { state_root };
        self
    }

    /// Build a concrete [`CopilotSdkSession`], resolving enrichment bridges.
    ///
    /// Shared by [`BaseTypeFactory::open_session`] (which boxes the result)
    /// and unit tests that need to inspect the wired bridges directly.
    fn build_session(&self, request: BaseTypeSessionRequest) -> SimardResult<CopilotSdkSession> {
        if !self.descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: self.descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        // Issue #1664: wire real memory + knowledge bridges instead of the
        // previously-hardcoded `None`/`None`. When enrichment is configured
        // the bridges are launched here (with graceful degradation) via the
        // shared [`EnrichmentSource::resolve`] and stored in the normalized
        // `EnrichmentClients` bundle (issue #1665) so that the shared
        // `enrich_input` entry point actually injects memory facts, procedures,
        // and domain knowledge into every turn.
        let enrichment = self.enrichment.resolve();

        Ok(CopilotSdkSession {
            descriptor: self.descriptor.clone(),
            config: self.config.clone(),
            request,
            enrichment,
            is_open: false,
            is_closed: false,
            turn_count: 0,
            session_uuid: None,
            meeting_binary_override: self.meeting_binary_override.clone(),
        })
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
        Ok(Box::new(self.build_session(request)?))
    }
}

/// A live copilot session that enriches objectives with memory and knowledge
/// before dispatching them through the terminal PTY (non-meeting mode) or
/// a direct `copilot` subprocess (meeting mode).
struct CopilotSdkSession {
    descriptor: BaseTypeDescriptor,
    config: CopilotAdapterConfig,
    request: BaseTypeSessionRequest,
    enrichment: EnrichmentClients,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
    /// Persistent session UUID for meeting mode.
    /// Generated on `open()` and passed as `--session-id` to every per-turn
    /// `copilot` invocation so that copilot maintains conversation context
    /// across turns without needing a persistent interactive process.
    session_uuid: Option<String>,
    /// Test-only seam inherited from the adapter (issue #2732). `None` in
    /// production → meeting turns exec the real [`MEETING_COPILOT_BINARY`].
    meeting_binary_override: Option<String>,
}

impl std::fmt::Debug for CopilotSdkSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopilotSdkSession")
            .field("descriptor", &self.descriptor)
            .field("is_open", &self.is_open)
            .field("is_closed", &self.is_closed)
            .field("turn_count", &self.turn_count)
            .field("session_uuid", &self.session_uuid)
            .finish()
    }
}

impl CopilotSdkSession {
    /// Whether this session is in meeting mode.
    fn is_meeting_mode(&self) -> bool {
        self.request.mode == OperatingMode::Meeting
    }

    /// Test-only constructor for direct field inspection.
    #[cfg(test)]
    fn new_for_test(request: BaseTypeSessionRequest) -> Self {
        let id = BaseTypeId::new("copilot-test");
        Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<CopilotSdkAdapter>(
                    "copilot-sdk::pty-session",
                    "registered-base-type:copilot-sdk",
                    Freshness::now().expect("Freshness::now"),
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
            config: CopilotAdapterConfig::default(),
            request,
            enrichment: EnrichmentClients::new(),
            is_open: false,
            is_closed: false,
            turn_count: 0,
            session_uuid: None,
            meeting_binary_override: None,
        }
    }

    /// Test-only builder that injects pre-constructed enrichment bridges so a
    /// test can assert that `enrich_input` consumes them (issues #1664/#1665).
    #[cfg(test)]
    fn with_test_bridges(
        mut self,
        memory_client: Option<Box<dyn CognitiveMemoryOps>>,
        knowledge_client: Option<KnowledgeClient>,
    ) -> Self {
        self.enrichment = EnrichmentClients {
            memory: memory_client,
            knowledge: knowledge_client,
        };
        self
    }

    /// Build an enriched terminal objective from the turn input.
    ///
    /// The enriched objective includes a shell/command preamble that the
    /// terminal session infrastructure parses, followed by the formatted
    /// turn context as the command payload.
    ///
    /// When `identity_context` or `prompt_preamble` are provided (e.g. in
    /// meeting mode), they are prepended to the objective so the agent
    /// receives the full conversational context.
    ///
    /// The returned [`NamedTempFile`] holds the on-disk prompt file referenced
    /// by the shell command. The caller must keep it alive for the duration
    /// of the terminal turn so the copilot subprocess can `cat` it; the file
    /// is auto-deleted when dropped (see issue #1871: passing the prompt by
    /// file path rather than inlining it into the shell command avoids the
    /// single-quote / `printf %s` escaping bugs that broke multi-KB prompts).
    fn build_enriched_objective(
        &self,
        input: &BaseTypeTurnInput,
    ) -> SimardResult<(String, NamedTempFile)> {
        let formatted = self.render_enriched_prompt(input)?;
        let prompt_file = write_prompt_to_tempfile(&formatted)?;
        let objective = build_copilot_terminal_objective(&self.config, prompt_file.path());
        Ok((objective, prompt_file))
    }

    /// Render the enriched, formatted prompt string submitted to the copilot
    /// subprocess.
    ///
    /// Routes the turn through the shared [`BaseTypeSession::enrich_input`]
    /// entry point (issue #1665) — which recalls memory/knowledge into
    /// `prompt_preamble` — then folds the preamble, identity context, and
    /// objective into a single prompt body and wraps it with the standard
    /// objective/instructions scaffold via [`format_turn_input`].
    ///
    /// With no bridges configured (the production default until #1664 wires
    /// them through), `enrich_input` returns the input unchanged, so the output
    /// is byte-identical to the pre-#1665 Copilot prompt.
    ///
    /// Decision (issue #2383, MEDIUM observation): the current ordering folds
    /// the recalled enrichment (carried in `prompt_preamble`) and identity
    /// context ahead of the bare objective under a single `## Objective`
    /// heading. This is intentionally kept as-is. It is well-formed, matches the
    /// preamble→identity→objective layering every other adapter uses, and
    /// reordering would alter the production prompt for every Copilot turn —
    /// out of scope for the RustyClawd wiring fix and a model-behavior risk not
    /// worth taking without evidence. KEEP.
    fn render_enriched_prompt(&self, input: &BaseTypeTurnInput) -> SimardResult<String> {
        let enriched = self.enrich_input(input)?;

        let mut parts = Vec::new();
        if !enriched.prompt_preamble.is_empty() {
            parts.push(enriched.prompt_preamble.as_str());
        }
        if !enriched.identity_context.is_empty() {
            parts.push(enriched.identity_context.as_str());
        }
        parts.push(enriched.objective.as_str());
        let combined_objective = parts.join("\n\n");

        // The enrichment is already folded into `combined_objective`; render the
        // scaffold around it with empty context sections to avoid re-querying.
        let context = TurnContext {
            objective: combined_objective,
            memory_facts: Vec::new(),
            knowledge: Vec::new(),
            procedures: Vec::new(),
        };
        Ok(format_turn_input(&context))
    }

    /// Build an enriched prompt for meeting mode (no PTY command wrapping).
    ///
    /// Similar to `build_enriched_objective` but writes only the formatted
    /// prompt content to a temp file — without the shell `command:` / PTY
    /// preamble. Test-only since #2640: production meeting turns stream the
    /// rendered prompt to copilot on stdin (see [`build_meeting_command`] +
    /// [`run_meeting_turn`]) rather than via a temp file, but the enrichment
    /// rendering it exercises is still worth pinning.
    #[cfg(test)]
    fn build_meeting_prompt(&self, input: &BaseTypeTurnInput) -> SimardResult<NamedTempFile> {
        let formatted = self.render_enriched_prompt(input)?;
        write_prompt_to_tempfile(&formatted)
    }

    /// Run a single meeting-mode turn by invoking `copilot` directly as a
    /// subprocess with `--no-custom-instructions --silent --session-id`.
    ///
    /// This avoids the PTY/`script` wrapper and `amplihack copilot` custom
    /// instruction injection that caused meeting prompts to be misinterpreted
    /// as engineering tasks (issue #2170).
    ///
    /// The (possibly large) prompt is delivered to copilot on STDIN via the
    /// single spawn facade ([`crate::spawn_payload::attach_prompt_std`]), never
    /// as an argv token: passing it as `-p "$(cat …)"` inlined the whole prompt
    /// into `argv`, which for large-context turns exceeded `ARG_MAX` and made
    /// `exec` fail with `E2BIG` ("Argument list too long", exit 126) — the live
    /// defect fixed by #2640.
    fn run_meeting_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        let session_id = self
            .session_uuid
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();

        // Render the enriched prompt once; it is streamed on stdin below.
        let formatted = self.render_enriched_prompt(&input)?;

        // Resolve the meeting binary. Production always uses the const
        // `MEETING_COPILOT_BINARY` ("copilot"); hermetic tests inject a fake
        // via `with_meeting_binary_override` so no real `copilot` subprocess
        // (and no auth/network dependency) is spawned in the default test run
        // (#2732).
        let meeting_binary = self
            .meeting_binary_override
            .as_deref()
            .unwrap_or(MEETING_COPILOT_BINARY);

        // Direct exec of the copilot binary with only bounded flags — the prompt
        // is NOT on the command line (issue #2640).
        let mut command = build_meeting_command(
            meeting_binary,
            &session_id,
            self.config.working_directory.as_deref(),
        );
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Force stdin delivery through the single spawn facade: copilot reads
        // the prompt from stdin when no `-p` is given, so the prompt never
        // contributes to `argv` regardless of size (issue #2640).
        let applied = attach_prompt_std(&mut command, formatted.as_bytes()).map_err(|err| {
            SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: format!("failed to prepare copilot meeting prompt delivery: {err}"),
            }
        })?;

        let mut child = command
            .spawn()
            .map_err(|err| SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: format!("failed to spawn copilot meeting subprocess: {err}"),
            })?;

        // Feed the prompt on a separate thread so a large prompt cannot deadlock
        // against the child filling its stdout pipe while we are still writing
        // stdin. The feeder closes stdin on completion so copilot reads EOF.
        let stdin = child.stdin.take();
        let feeder = std::thread::spawn(move || applied.feed(stdin));

        let output =
            child
                .wait_with_output()
                .map_err(|err| SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("failed to wait on copilot meeting subprocess: {err}"),
                })?;

        // Surface a stdin-feed failure loudly — no silent fallback (issue #2640).
        match feeder.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("failed to stream copilot meeting prompt on stdin: {err}"),
                });
            }
            Err(_) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: "copilot meeting prompt feeder thread panicked".to_string(),
                });
            }
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: format!(
                    "copilot meeting subprocess exited with {}: {}",
                    output.status,
                    stderr.trim()
                ),
            });
        }

        let response_text = String::from_utf8_lossy(&output.stdout).to_string();
        tracing::info!(
            response_len = response_text.len(),
            turn = self.turn_count,
            session_uuid = %session_id,
            "Copilot adapter (meeting mode): received response"
        );

        if response_text.trim().is_empty() {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: "copilot meeting subprocess returned no output (auth failure, rate limit, \
                     or empty response). Run `gh auth status` to verify credentials."
                    .to_string(),
            });
        }

        // Record cost estimate.
        let prompt_chars = input.objective.len();
        let completion_chars = response_text.len();
        if let Err(e) = crate::cost_tracking::record_cost(
            self.request.session_id.as_str(),
            "copilot-meeting",
            prompt_chars,
            completion_chars,
            &format!(
                "copilot meeting turn {} on {}",
                self.turn_count, self.request.topology
            ),
        ) {
            eprintln!("[simard] cost tracking write failed: {e}");
        }

        let objective_summary = objective_metadata(&input.objective);
        let evidence = vec![
            format!("copilot-adapter-mode=meeting"),
            format!("copilot-meeting-session-id={session_id}"),
            format!("copilot-adapter-command={MEETING_COPILOT_BINARY}"),
            format!("copilot-adapter-turn={}", self.turn_count),
        ];

        Ok(BaseTypeOutcome {
            plan: format!(
                "Copilot meeting adapter dispatched {} via '{}' on '{}' (turn {}, session {}).",
                objective_summary,
                MEETING_COPILOT_BINARY,
                self.request.topology,
                self.turn_count,
                session_id,
            ),
            execution_summary: response_text,
            evidence,
        })
    }

    /// Run a single PTY-based turn (non-meeting mode).
    ///
    /// This is the original `run_turn` path: builds an enriched objective
    /// with shell/command preamble and dispatches through
    /// `execute_terminal_turn`.
    fn run_pty_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        let (enriched_objective, _prompt_file) = self.build_enriched_objective(&input)?;
        let enriched_input = BaseTypeTurnInput::objective_only(enriched_objective);

        tracing::info!(mode = %self.request.mode, turn = self.turn_count, "Copilot adapter: sending turn to Copilot SDK (this may take 30-90s)…");
        let terminal_outcome =
            execute_terminal_turn(&self.descriptor, &self.request, &enriched_input).map_err(
                |err| SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("copilot terminal turn failed: {err}"),
                },
            )?;

        let response_text = extract_copilot_response_from_evidence(&terminal_outcome.evidence);
        tracing::info!(
            response_len = response_text.len(),
            turn = self.turn_count,
            "Copilot adapter: received response"
        );

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
}

impl BaseTypeSession for CopilotSdkSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn enrichment(&self) -> Option<&EnrichmentClients> {
        Some(&self.enrichment)
    }

    fn enrichment_mut(&mut self) -> Option<&mut EnrichmentClients> {
        Some(&mut self.enrichment)
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        if self.is_meeting_mode() {
            let uuid = uuid::Uuid::new_v4().to_string();
            tracing::info!(session_uuid = %uuid, "Copilot adapter: meeting mode session opened");
            self.session_uuid = Some(uuid);
        }
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;

        self.turn_count += 1;

        if self.is_meeting_mode() {
            self.run_meeting_turn(input)
        } else {
            self.run_pty_turn(input)
        }
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        ensure_session_open(&self.descriptor, self.is_open, "close")?;
        self.session_uuid = None;
        self.is_closed = true;
        Ok(())
    }
}

/// Build the meeting-mode copilot invocation as a direct [`Command`] — the
/// binary is exec'd itself (no `sh -c` wrapper) with only bounded flags, and the
/// prompt is delivered separately on STDIN by the caller. Keeping the prompt out
/// of `argv` is what makes the invocation immune to the E2BIG / "Argument list
/// too long" (exit 126) failure that broke Simard's meeting + OODA loop when the
/// prompt was passed via `-p "$(cat …)"` (issue #2640).
///
/// `program` is the copilot binary (e.g. `copilot`); `session_id` threads
/// conversation context across turns; `working_dir` sets the child's working
/// directory when provided.
pub fn build_meeting_command(
    program: &str,
    session_id: &str,
    working_dir: Option<&str>,
) -> Command {
    let mut command = Command::new(program);
    command
        .arg("--no-custom-instructions")
        .arg("--silent")
        .arg("--allow-all-tools")
        .arg("--session-id")
        .arg(session_id);
    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }
    command
}

/// Build a terminal-session-compatible objective string that launches the
/// copilot command and reads the prompt from a pre-written file on disk.
///
/// **Why a file, not a `printf '%s' '<escaped>'` inline literal**: prior to
/// issue #1871 this function embedded the entire enriched prompt as a
/// single-quoted argument to `printf '%s'`. That string was then sent as
/// PTY input via `writeln!(stdin, …)`. Two failure modes followed:
///
/// 1. **PTY line-buffer overflow.** Line discipline in canonical mode
///    has a small input buffer (~4 KB on Linux); larger prompts truncated
///    silently, leaving bash in continuation mode.
/// 2. **Apostrophe escaping interactions.** Even within the buffer limit,
///    the `\\` → `\\\\` + `'\''` escape sequence broke for some payloads,
///    producing a malformed command that bash interpreted as an unterminated
///    string. The transcript parser then returned the bootstrap noise as
///    the "response", which downstream callers (e.g. the merge-readiness
///    judge in PR #1870) parsed as their own prompt.
///
/// The fix: write the prompt to a temp file from Rust code (out-of-band,
/// via `write_prompt_to_tempfile`), and pass only the path through the
/// shell. Paths returned by [`NamedTempFile`] are guaranteed-safe ASCII
/// (`/tmp/.tmpXXXXXX`), so single-quoting them is unconditional and the
/// command line stays well under a hundred bytes regardless of prompt size.
///
/// **Why the prompt is piped on STDIN, never passed via `-p` (issue #2640)**:
/// the interim form `-p "$(cat 'PATH')"` still inlined the prompt into `argv`
/// — the `$(cat …)` command substitution expands the whole file into a single
/// argument. For decision-cycle / engineer goals with large accumulated context
/// (>256 KB), that argument blew past the kernel's `ARG_MAX` and `exec` failed
/// with `E2BIG` ("Argument list too long", surfaced as exit 126), repeatedly
/// breaking Simard's live OODA loop. The prompt is now delivered by piping the
/// file into copilot's STDIN (`cat 'PATH' | <cmd> …`): copilot reads the prompt
/// from stdin when no `-p` is given, so the prompt no longer contributes to
/// `argv` and the invocation is immune to prompt size.
///
/// Cleanup is owned by Rust: the [`NamedTempFile`] guard in `run_turn`
/// unlinks the file on Drop. We deliberately do NOT chain `rm -f` in the
/// shell command — that would race with the Rust guard and could mask
/// transcript-debugging artefacts.
pub(super) fn build_copilot_terminal_objective(
    config: &CopilotAdapterConfig,
    prompt_file_path: &Path,
) -> String {
    let mut objective = String::new();

    if let Some(ref cwd) = config.working_directory {
        objective.push_str(&format!("working-directory: {cwd}\n"));
    }

    // Single-quote the path. NamedTempFile only produces paths from a
    // restricted ASCII alphabet (`/`, `.`, alnum, `_`), so single-quoting is
    // safe — but we still assert-check for defense-in-depth at construction.
    let path_str = prompt_file_path.to_string_lossy();
    debug_assert!(
        !path_str.contains('\''),
        "tempfile path unexpectedly contains a single quote: {path_str}"
    );

    // Pipe the prompt file into the copilot on STDIN (`cat 'PATH' | <cmd>`) so
    // the prompt never touches `argv` — the E2BIG-safe transport (issue #2640).
    // `--subprocess-safe` skips interactive staging; `--allow-all-tools` is
    // required by copilot for non-interactive runs; copilot reads the prompt
    // from stdin because no `-p` is passed. Chain `; exit` so the PTY shell
    // returns after the copilot finishes.
    objective.push_str(&format!(
        "command: cat '{}' | {} --subprocess-safe --allow-all-tools ; exit\n",
        path_str, config.command,
    ));

    objective
}

/// Write an enriched prompt to a freshly-created `NamedTempFile` and return
/// the handle. The file is auto-deleted on Drop, so the caller must keep
/// the handle alive for as long as the copilot subprocess may read from it.
///
/// Returns [`SimardError::AdapterInvocationFailed`] if the temp file cannot
/// be created or written — both are extremely unlikely on a healthy host
/// but worth surfacing rather than swallowing.
pub(super) fn write_prompt_to_tempfile(prompt: &str) -> SimardResult<NamedTempFile> {
    let mut file = NamedTempFile::with_prefix("simard-copilot-prompt-").map_err(|error| {
        SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: format!("failed to create copilot prompt temp file: {error}"),
        }
    })?;
    file.write_all(prompt.as_bytes())
        .map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: format!("failed to write copilot prompt temp file: {error}"),
        })?;
    file.flush()
        .map_err(|error| SimardError::AdapterInvocationFailed {
            base_type: "copilot-sdk".to_string(),
            reason: format!("failed to flush copilot prompt temp file: {error}"),
        })?;
    Ok(file)
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

mod transcript;
pub use transcript::*;

#[cfg(test)]
mod tests;

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
