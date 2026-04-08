use std::process::{Command, Stdio};

use rustyclawd_core::client::{
    Client as RcClient, ContentBlock as RcContentBlock, CreateMessageRequest, Message as RcMessage,
};

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeSessionRequest, BaseTypeTurnInput, process_output_evidence,
};
use crate::error::{SimardError, SimardResult};

use super::MAX_HISTORY_MESSAGES;
use super::tool_executor::execute_tool_locally;
use super::tools::rustyclawd_tool_definitions;

/// Execute a turn using the RustyClawd crate Client API and tool loop.
///
/// This is the primary execution path when an API key is available. It builds
/// a `CreateMessageRequest` from the turn input and delegates tool execution
/// to a local process dispatcher backed by `rustyclawd_tools::BashTool`.
pub(super) fn execute_rustyclawd_client(
    client: &RcClient,
    rt: &tokio::runtime::Runtime,
    input: &BaseTypeTurnInput,
    descriptor: &BaseTypeDescriptor,
    request: &BaseTypeSessionRequest,
    conversation_history: &mut Vec<RcMessage>,
) -> SimardResult<(String, Vec<String>)> {
    let system_prompt = if input.identity_context.is_empty() && input.prompt_preamble.is_empty() {
        include_str!("../../prompt_assets/simard/rustyclawd_default_system.md")
            .trim()
            .to_string()
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

/// Fallback execution via external process when no API key is available.
pub(super) fn execute_rustyclawd_process_fallback(
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
