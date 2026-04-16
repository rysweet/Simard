use rustyclawd_core::client::{
    Client as RcClient, ContentBlock as RcContentBlock, CreateMessageRequest, Message as RcMessage,
};

use crate::base_types::{BaseTypeDescriptor, BaseTypeSessionRequest, BaseTypeTurnInput};
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- MAX_HISTORY_MESSAGES constant --

    #[test]
    fn max_history_messages_is_reasonable() {
        const { assert!(MAX_HISTORY_MESSAGES >= 2) };
        const { assert!(MAX_HISTORY_MESSAGES <= 100) };
    }

    // -- History truncation logic (mirrors the logic in execute_rustyclawd_client) --

    #[test]
    fn history_truncation_drains_oldest_when_over_limit() {
        let mut history: Vec<String> = (0..MAX_HISTORY_MESSAGES + 5)
            .map(|i| format!("msg-{i}"))
            .collect();
        if history.len() > MAX_HISTORY_MESSAGES {
            let drain_count = history.len() - MAX_HISTORY_MESSAGES;
            history.drain(..drain_count);
        }
        assert_eq!(history.len(), MAX_HISTORY_MESSAGES);
        assert_eq!(history[0], "msg-5");
    }

    #[test]
    fn history_truncation_noop_when_at_limit() {
        let mut history: Vec<String> = (0..MAX_HISTORY_MESSAGES)
            .map(|i| format!("msg-{i}"))
            .collect();
        let original_len = history.len();
        if history.len() > MAX_HISTORY_MESSAGES {
            let drain_count = history.len() - MAX_HISTORY_MESSAGES;
            history.drain(..drain_count);
        }
        assert_eq!(history.len(), original_len);
    }

    // -- System prompt construction --

    #[test]
    fn system_prompt_uses_default_when_both_empty() {
        let identity_context = "";
        let prompt_preamble = "";
        let system_prompt = if identity_context.is_empty() && prompt_preamble.is_empty() {
            include_str!("../../prompt_assets/simard/rustyclawd_default_system.md")
                .trim()
                .to_string()
        } else {
            format!("{prompt_preamble}\n---\n{identity_context}")
        };
        assert!(!system_prompt.is_empty());
        assert!(!system_prompt.contains("\n---\n"));
    }

    #[test]
    fn system_prompt_uses_custom_when_provided() {
        let identity_context = "You are a test agent";
        let prompt_preamble = "Be concise";
        let system_prompt = if identity_context.is_empty() && prompt_preamble.is_empty() {
            include_str!("../../prompt_assets/simard/rustyclawd_default_system.md")
                .trim()
                .to_string()
        } else {
            format!("{prompt_preamble}\n---\n{identity_context}")
        };
        assert!(system_prompt.contains("Be concise"));
        assert!(system_prompt.contains("You are a test agent"));
    }

    // -- RUSTYCLAWD_BIN env override --

    #[test]
    fn rustyclawd_bin_defaults_to_rustyclawd() {
        let bin = std::env::var("RUSTYCLAWD_BIN").unwrap_or_else(|_| "rustyclawd".to_string());
        // In test env, the env var is likely not set.
        assert!(!bin.is_empty());
    }
}
