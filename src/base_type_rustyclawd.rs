//! RustyClawd agent base type — wraps the rustyclawd-core SDK to delegate
//! objectives through its LLM-calling, tool-executing agent pipeline.
//!
//! This is the primary production base type. It uses `rustyclawd_core::Client`
//! with `execute_with_tools` to run a full agent loop: Simard sends an objective,
//! RustyClawd calls Claude, Claude requests tool use, tools execute locally, and
//! the loop continues until the objective is resolved.
//!
//! When no API key is available, falls back to spawning the `rustyclawd` binary
//! as a subprocess.

use std::fmt::{self, Formatter};
use std::process::{Command, Stdio};

use rustyclawd_core::client::{
    Client as RcClient, ClientError, Config as RcConfig, ContentBlock as RcContentBlock,
    CreateMessageRequest, Message as RcMessage, ToolDefinition,
};

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome, BaseTypeSession,
    BaseTypeSessionRequest, BaseTypeTurnInput, ensure_session_not_already_open,
    ensure_session_not_closed, ensure_session_open, joined_prompt_ids, process_output_evidence,
    standard_session_capabilities,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use crate::sanitization::objective_metadata;

#[derive(Debug)]
pub struct RustyClawdAdapter {
    descriptor: BaseTypeDescriptor,
}

impl RustyClawdAdapter {
    pub fn registered(id: impl Into<String>) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "rusty-clawd::session-backend",
                    "registered-base-type:rusty-clawd",
                    Freshness::now()?,
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [
                    RuntimeTopology::SingleProcess,
                    RuntimeTopology::MultiProcess,
                ]
                .into_iter()
                .collect(),
            },
        })
    }
}

impl BaseTypeFactory for RustyClawdAdapter {
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

        Ok(Box::new(RustyClawdSession {
            descriptor: self.descriptor.clone(),
            request,
            is_open: false,
            is_closed: false,
            client: None,
            rt: None,
            conversation_history: Vec::new(),
        }))
    }
}

/// Maximum conversation turns to retain in history (prevents unbounded growth).
const MAX_HISTORY_MESSAGES: usize = 100;

struct RustyClawdSession {
    descriptor: BaseTypeDescriptor,
    request: BaseTypeSessionRequest,
    is_open: bool,
    is_closed: bool,
    /// RustyClawd API client, initialized on open() from environment config.
    client: Option<RcClient>,
    /// Tokio runtime for bridging async rustyclawd client calls into sync
    /// BaseTypeSession methods.
    rt: Option<tokio::runtime::Runtime>,
    /// Accumulated conversation history for multi-turn sessions (meetings, etc.).
    conversation_history: Vec<RcMessage>,
}

impl fmt::Debug for RustyClawdSession {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustyClawdSession")
            .field("descriptor", &self.descriptor)
            .field("is_open", &self.is_open)
            .field("is_closed", &self.is_closed)
            .field("client", &self.client.is_some())
            .finish()
    }
}

impl BaseTypeSession for RustyClawdSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: format!("failed to create tokio runtime: {e}"),
            })?;

        let client_result = rt.block_on(async {
            let config = RcConfig::from_default_location().await?;
            RcClient::new(config)
        });

        match client_result {
            Ok(client) => {
                self.client = Some(client);
            }
            Err(ClientError::ApiKeyNotFound) => {
                // No API key available — session will use process fallback on run_turn.
                self.client = None;
            }
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("failed to initialize RustyClawd client: {e}"),
                });
            }
        }

        self.rt = Some(rt);
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;

        let prompt_ids = joined_prompt_ids(&self.request.prompt_assets);
        let objective_summary = objective_metadata(&input.objective);

        let plan = format!(
            "Launch RustyClawd backend '{}' for '{}' on '{}' with prompt assets [{}].",
            self.descriptor.backend.identity, self.request.mode, self.request.topology, prompt_ids,
        );

        let (execution_summary, process_evidence) =
            if let (Some(client), Some(rt)) = (self.client.as_ref(), self.rt.as_ref()) {
                execute_rustyclawd_client(
                    client,
                    rt,
                    &input,
                    &self.descriptor,
                    &self.request,
                    &mut self.conversation_history,
                )?
            } else {
                // Fallback: no API key available, run via process spawn.
                execute_rustyclawd_process_fallback(&input, &self.descriptor, &self.request)?
            };

        let mut evidence = vec![
            format!("selected-base-type={}", self.descriptor.id),
            format!(
                "backend-implementation={}",
                self.descriptor.backend.identity
            ),
            format!("prompt-assets=[{}]", prompt_ids),
            format!("runtime-node={}", self.request.runtime_node),
            format!("mailbox-address={}", self.request.mailbox_address),
            format!("objective-summary={}", objective_summary),
        ];
        evidence.extend(process_evidence);

        Ok(BaseTypeOutcome {
            plan,
            execution_summary,
            evidence,
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        ensure_session_open(&self.descriptor, self.is_open, "close")?;
        self.client = None;
        self.rt = None;
        self.is_closed = true;
        Ok(())
    }
}

/// Build tool definitions for the standard tool set provided to the RustyClawd
/// client. These mirror the tools available in rustyclawd-tools.
fn rustyclawd_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "Bash",
            "Execute shell commands. Returns stdout, stderr and exit code.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout": { "type": "integer", "description": "Timeout in milliseconds" }
                },
                "required": ["command"]
            }),
        ),
        ToolDefinition::new(
            "Read",
            "Read file contents from the filesystem.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to read" },
                    "offset": { "type": "integer", "description": "Line offset" },
                    "limit": { "type": "integer", "description": "Max lines to read" }
                },
                "required": ["file_path"]
            }),
        ),
        ToolDefinition::new(
            "Write",
            "Write content to a file.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to write" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["file_path", "content"]
            }),
        ),
        ToolDefinition::new(
            "Edit",
            "Edit a file by replacing text.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path" },
                    "old_string": { "type": "string" },
                    "new_string": { "type": "string" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        ),
    ]
}

/// Execute a turn using the RustyClawd crate Client API and tool loop.
///
/// This is the primary execution path when an API key is available. It builds
/// a `CreateMessageRequest` from the turn input and delegates tool execution
/// to a local process dispatcher backed by `rustyclawd_tools::BashTool`.
fn execute_rustyclawd_client(
    client: &RcClient,
    rt: &tokio::runtime::Runtime,
    input: &BaseTypeTurnInput,
    descriptor: &BaseTypeDescriptor,
    request: &BaseTypeSessionRequest,
    conversation_history: &mut Vec<RcMessage>,
) -> SimardResult<(String, Vec<String>)> {
    let system_prompt = if input.identity_context.is_empty() && input.prompt_preamble.is_empty() {
        "You are Simard, an autonomous engineer. Execute the given objective using your available tools.".to_string()
    } else {
        format!("{}\n---\n{}", input.prompt_preamble, input.identity_context)
    };

    // Append user message to conversation history
    conversation_history.push(RcMessage::user(&input.objective));

    // Truncate if history exceeds max
    if conversation_history.len() > MAX_HISTORY_MESSAGES {
        let drain_count = conversation_history.len() - MAX_HISTORY_MESSAGES;
        conversation_history.drain(..drain_count);
    }

    let tools = rustyclawd_tool_definitions();
    let api_request =
        CreateMessageRequest::new("claude-sonnet-4-6", conversation_history.clone(), 8192)
            .with_system(system_prompt)
            .with_tools(tools);

    let response = rt.block_on(async {
        client
            .execute_with_tools(api_request, |tool_name, tool_input| async move {
                execute_tool_locally(&tool_name, &tool_input).await
            })
            .await
    });

    match response {
        Ok(resp) => {
            let text_output: String = resp
                .content
                .iter()
                .filter_map(|block| match block {
                    RcContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");

            let evidence = vec![
                format!("rustyclawd-model={}", resp.model),
                format!("rustyclawd-input-tokens={}", resp.usage.input_tokens),
                format!("rustyclawd-output-tokens={}", resp.usage.output_tokens),
                format!(
                    "rustyclawd-stop-reason={}",
                    resp.stop_reason.as_deref().unwrap_or("none")
                ),
                format!(
                    "rustyclawd-session=node={} addr={} model={}",
                    request.runtime_node, request.mailbox_address, resp.model,
                ),
            ];

            // Append assistant response to conversation history for multi-turn.
            if !text_output.is_empty() {
                conversation_history.push(RcMessage::assistant(&text_output));
            }

            // Return the LLM's actual text response as the execution summary.
            Ok((text_output, evidence))
        }
        Err(e) => Err(SimardError::AdapterInvocationFailed {
            base_type: descriptor.id.to_string(),
            reason: format!("RustyClawd client execution failed: {e}"),
        }),
    }
}

/// Execute a tool call locally using process spawning.
async fn execute_tool_locally(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> Result<serde_json::Value, ClientError> {
    match tool_name {
        "Bash" => {
            let command = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let timeout_ms = tool_input
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(120_000);

            let mut cmd = tokio::process::Command::new("sh");
            cmd.args(["-c", command]);
            // Pipe stdout/stderr so tool output doesn't leak to the terminal.
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            let config = rustyclawd_tools::ProcessSpawnConfig::default();
            let child = rustyclawd_tools::spawn_with_isolation(cmd, &config)
                .await
                .map_err(|e| ClientError::Unknown(format!("spawn failed: {e}")))?;

            let output = tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                child.wait_with_output(),
            )
            .await
            .map_err(|_| ClientError::Timeout("tool execution timed out".to_string()))?
            .map_err(|e| ClientError::Unknown(format!("process error: {e}")))?;

            Ok(serde_json::json!({
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
                "exit_code": output.status.code().unwrap_or(-1),
            }))
        }
        "Read" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => Ok(serde_json::json!({ "content": contents })),
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        "Write" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = tool_input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::write(path, content).await {
                Ok(()) => Ok(serde_json::json!({ "status": "ok" })),
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        "Edit" => {
            let path = tool_input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let old = tool_input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new = tool_input
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match tokio::fs::read_to_string(path).await {
                Ok(contents) => {
                    let replaced = contents.replacen(old, new, 1);
                    match tokio::fs::write(path, &replaced).await {
                        Ok(()) => Ok(serde_json::json!({ "status": "ok" })),
                        Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
                    }
                }
                Err(e) => Ok(serde_json::json!({ "error": format!("{e}") })),
            }
        }
        _ => Ok(serde_json::json!({ "error": format!("unknown tool: {tool_name}") })),
    }
}

/// Fallback execution via external process when no API key is available.
fn execute_rustyclawd_process_fallback(
    input: &BaseTypeTurnInput,
    descriptor: &BaseTypeDescriptor,
    request: &BaseTypeSessionRequest,
) -> SimardResult<(String, Vec<String>)> {
    let rustyclawd_bin =
        std::env::var("RUSTYCLAWD_BIN").unwrap_or_else(|_| "rustyclawd".to_string());
    let prompt_input = format!(
        "{}\n---\n{}\n---\n{}",
        input.prompt_preamble, input.identity_context, input.objective,
    );

    let child_result = Command::new(&rustyclawd_bin)
        .arg("--non-interactive")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child_result {
        Ok(child) => child,
        Err(error) => {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: descriptor.id.to_string(),
                reason: format!(
                    "failed to spawn RustyClawd process '{}': {error}",
                    rustyclawd_bin,
                ),
            });
        }
    };

    if let Some(ref mut stdin) = child.stdin {
        use std::io::Write;
        let _ = stdin.write_all(prompt_input.as_bytes());
    }
    drop(child.stdin.take());

    let output =
        child
            .wait_with_output()
            .map_err(|error| SimardError::AdapterInvocationFailed {
                base_type: descriptor.id.to_string(),
                reason: format!("failed to collect RustyClawd output: {error}"),
            })?;

    let execution_summary = format!(
        "RustyClawd session executed via process '{}' on node '{}' at '{}' with exit code {}.",
        descriptor.backend.identity,
        request.runtime_node,
        request.mailbox_address,
        output.status.code().unwrap_or(-1),
    );

    Ok((
        execution_summary,
        process_output_evidence("rustyclawd", &output),
    ))
}
