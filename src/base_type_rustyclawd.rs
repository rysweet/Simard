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

/// Maximum conversation messages to retain in history. Keep low because
/// each tool-use turn can be very large (tool inputs + outputs).
const MAX_HISTORY_MESSAGES: usize = 20;

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

    // Return the process stdout as the execution summary (the LLM's text response).
    let text_output = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let mut evidence = process_output_evidence("rustyclawd", &output);
    evidence.push(format!(
        "rustyclawd-process-session=node={} addr={} exit={}",
        request.runtime_node,
        request.mailbox_address,
        output.status.code().unwrap_or(-1),
    ));

    Ok((text_output, evidence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeFactory;
    use crate::runtime::RuntimeTopology;

    // ── RustyClawdAdapter construction ──

    #[test]
    fn registered_adapter_has_correct_backend_identity() {
        let adapter = RustyClawdAdapter::registered("rusty-clawd").unwrap();
        assert_eq!(
            adapter.descriptor().backend.identity,
            "rusty-clawd::session-backend"
        );
    }

    #[test]
    fn registered_adapter_has_expected_id() {
        let adapter = RustyClawdAdapter::registered("my-id").unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "my-id");
    }

    #[test]
    fn registered_adapter_supports_single_and_multi_process() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let desc = adapter.descriptor();
        assert!(desc.supports_topology(RuntimeTopology::SingleProcess));
        assert!(desc.supports_topology(RuntimeTopology::MultiProcess));
        assert!(!desc.supports_topology(RuntimeTopology::Distributed));
    }

    #[test]
    fn registered_adapter_has_standard_capabilities() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let caps = &adapter.descriptor().capabilities;
        assert!(
            !caps.is_empty(),
            "should have standard session capabilities"
        );
    }

    #[test]
    fn descriptor_returns_reference_to_stored_descriptor() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let d1 = adapter.descriptor();
        let d2 = adapter.descriptor();
        assert_eq!(d1.id, d2.id);
    }

    // ── open_session ──

    #[test]
    fn open_session_rejects_unsupported_topology() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000001")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::Distributed, // not supported
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_err());
        match result {
            Err(SimardError::UnsupportedTopology {
                base_type,
                topology,
            }) => {
                assert_eq!(base_type, "rc-test");
                assert_eq!(topology, RuntimeTopology::Distributed);
            }
            Err(other) => panic!("expected UnsupportedTopology, got {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn open_session_succeeds_for_supported_topology() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000002")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_ok());
    }

    // ── Session lifecycle guards ──
    // Note: BaseTypeSession is a trait object (Box<dyn BaseTypeSession>)
    // which does not implement Debug, so we use is_err() assertions.

    #[test]
    fn session_run_turn_before_open_fails() {
        use crate::base_types::{BaseTypeSessionRequest, BaseTypeTurnInput};
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000003")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let mut session = adapter.open_session(request).unwrap();

        let input = BaseTypeTurnInput {
            objective: "test".to_string(),
            identity_context: "".to_string(),
            prompt_preamble: "".to_string(),
        };
        let result = session.run_turn(input);
        assert!(result.is_err(), "run_turn before open should fail");
    }

    #[test]
    fn session_close_before_open_fails() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000004")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let mut session = adapter.open_session(request).unwrap();
        let result = session.close();
        assert!(result.is_err(), "close before open should fail");
    }

    // ── RustyClawdSession debug format (via direct construction) ──

    #[test]
    fn session_struct_debug_format_is_readable() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let descriptor = RustyClawdAdapter::registered("rc-dbg")
            .unwrap()
            .descriptor
            .clone();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000005")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let session = RustyClawdSession {
            descriptor,
            request,
            is_open: false,
            is_closed: false,
            client: None,
            rt: None,
            conversation_history: Vec::new(),
        };
        let debug_str = format!("{session:?}");
        assert!(debug_str.contains("RustyClawdSession"));
        assert!(debug_str.contains("is_open"));
        assert!(debug_str.contains("is_closed"));
    }

    // ── Tool definitions ──

    #[test]
    fn tool_definitions_contains_expected_tools() {
        let tools = rustyclawd_tool_definitions();
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"Bash"));
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"Write"));
        assert!(names.contains(&"Edit"));
    }

    #[test]
    fn tool_definitions_all_have_descriptions() {
        let tools = rustyclawd_tool_definitions();
        for tool in &tools {
            assert!(
                !tool.description.is_empty(),
                "tool {} has empty description",
                tool.name
            );
        }
    }

    // ── MAX_HISTORY_MESSAGES constant ──

    #[test]
    fn max_history_messages_is_reasonable() {
        let m = MAX_HISTORY_MESSAGES;
        assert!(m > 0, "must be positive, got {m}");
        assert!(m <= 100, "must be <= 100, got {m}");
    }

    // ── execute_tool_locally tests ──

    #[tokio::test]
    async fn execute_tool_locally_unknown_tool_returns_error_json() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("UnknownTool", &input).await.unwrap();
        let error = result.get("error").and_then(|v| v.as_str()).unwrap();
        assert!(error.contains("unknown tool"));
        assert!(error.contains("UnknownTool"));
    }

    #[tokio::test]
    async fn execute_tool_locally_read_nonexistent_file_returns_error() {
        let input = serde_json::json!({ "file_path": "/nonexistent/path/to/file.txt" });
        let result = execute_tool_locally("Read", &input).await.unwrap();
        assert!(
            result.get("error").is_some(),
            "should return error for missing file"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_write_to_invalid_path_returns_error() {
        let input =
            serde_json::json!({ "file_path": "/nonexistent/dir/file.txt", "content": "hello" });
        let result = execute_tool_locally("Write", &input).await.unwrap();
        assert!(
            result.get("error").is_some(),
            "should return error for invalid path"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_nonexistent_file_returns_error() {
        let input = serde_json::json!({
            "file_path": "/nonexistent/dir/file.txt",
            "old_string": "old",
            "new_string": "new"
        });
        let result = execute_tool_locally("Edit", &input).await.unwrap();
        assert!(
            result.get("error").is_some(),
            "should return error for missing file"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_read_with_empty_path_returns_error() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("Read", &input).await.unwrap();
        assert!(
            result.get("error").is_some(),
            "empty path should yield error"
        );
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_missing_command_runs_empty_string() {
        let input = serde_json::json!({});
        let result = execute_tool_locally("Bash", &input).await.unwrap();
        // Running empty command succeeds (sh -c "")
        assert!(result.get("exit_code").is_some());
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_echo_captures_stdout() {
        let input = serde_json::json!({ "command": "echo hello_test_42" });
        let result = execute_tool_locally("Bash", &input).await.unwrap();
        let stdout = result.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
        assert!(stdout.contains("hello_test_42"));
        let exit_code = result.get("exit_code").and_then(|v| v.as_i64()).unwrap();
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn execute_tool_locally_bash_failing_command_has_nonzero_exit() {
        let input = serde_json::json!({ "command": "false" });
        let result = execute_tool_locally("Bash", &input).await.unwrap();
        let exit_code = result.get("exit_code").and_then(|v| v.as_i64()).unwrap();
        assert_ne!(exit_code, 0);
    }

    #[tokio::test]
    async fn execute_tool_locally_write_and_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("simard-test-rw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test_roundtrip.txt");
        let path_str = file_path.to_str().unwrap();

        let write_input =
            serde_json::json!({ "file_path": path_str, "content": "roundtrip_content" });
        let write_result = execute_tool_locally("Write", &write_input).await.unwrap();
        assert_eq!(
            write_result.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );

        let read_input = serde_json::json!({ "file_path": path_str });
        let read_result = execute_tool_locally("Read", &read_input).await.unwrap();
        let content = read_result.get("content").and_then(|v| v.as_str()).unwrap();
        assert_eq!(content, "roundtrip_content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn execute_tool_locally_edit_replaces_content() {
        let dir = std::env::temp_dir().join(format!("simard-test-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("test_edit.txt");
        let path_str = file_path.to_str().unwrap();

        std::fs::write(&file_path, "hello world").unwrap();

        let edit_input = serde_json::json!({
            "file_path": path_str,
            "old_string": "hello",
            "new_string": "goodbye"
        });
        let edit_result = execute_tool_locally("Edit", &edit_input).await.unwrap();
        assert_eq!(
            edit_result.get("status").and_then(|v| v.as_str()),
            Some("ok")
        );

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "goodbye world");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── System prompt construction (in execute_rustyclawd_client) ──
    // Tested indirectly through adapter — the system prompt logic is:
    // empty identity + preamble → default prompt, otherwise concatenated.

    #[test]
    fn adapter_debug_format_contains_type_name() {
        let adapter = RustyClawdAdapter::registered("debug-test").unwrap();
        let debug = format!("{adapter:?}");
        assert!(debug.contains("RustyClawdAdapter"));
    }
}
