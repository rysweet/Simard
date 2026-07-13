use rustyclawd_core::client::{
    Client, ClientError, Config, CreateMessageRequest, Message, ToolDefinition,
};
use serde::Deserialize;

use crate::runtime_config::RuntimeConfig;
use crate::session_builder::LlmProvider;

use super::{
    Action, BlockerRef, CompletionRef, EvidenceRef, GoalSessionInvocation, GoalSessionTools,
    OpaqueBytes, RecipeProcessError, RetryPolicy,
};

const ACTOR_SYSTEM_PROMPT: &str = "\
You are Simard's final goal-session Act actor. Semantic judgment is yours.
Rust does not interpret your prose. Read the opaque context with
read_semantic_context, then invoke exactly one terminal capability:
record_action, record_no_action, record_blocked, or record_completed.
Use a stable request_id. Preserve task, reason, and raw semantic byte envelopes
exactly when forwarding them. A tool failure is terminal: do not repair it,
substitute another outcome, or invoke another terminal. Your final text is
diagnostic only; the durable capability record is the sole business outcome.";

pub struct RustyClawdGoalSessionActor {
    model: String,
}

impl Default for RustyClawdGoalSessionActor {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-6".to_string(),
        }
    }
}

impl RustyClawdGoalSessionActor {
    pub fn run(
        &self,
        invocation: &GoalSessionInvocation,
        tools: &GoalSessionTools<'_>,
    ) -> Result<(), RecipeProcessError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| RecipeProcessError::failed(format!("actor runtime: {error}")))?;
        let provider = RuntimeConfig::load()
            .map(|config| config.llm_provider)
            .map_err(|error| {
                RecipeProcessError::failed(format!("actor provider configuration: {error}"))
            })?;
        let client = match provider {
            LlmProvider::Copilot => runtime.block_on(Client::new_copilot()).map_err(|error| {
                RecipeProcessError::failed(format!("actor Copilot authentication: {error}"))
            })?,
            LlmProvider::RustyClawd => {
                let config =
                    runtime
                        .block_on(Config::from_default_location())
                        .map_err(|error| {
                            RecipeProcessError::failed(format!(
                                "actor provider credentials: {error}"
                            ))
                        })?;
                Client::new(config)
                    .map_err(|error| RecipeProcessError::failed(format!("actor client: {error}")))?
            }
        };
        let objective = format!(
            "Act for authenticated session {}, cycle {}, goal {}. Read all five semantic context fields before choosing one terminal.",
            invocation.session_id, invocation.cycle_id, invocation.goal_id
        );
        let request = CreateMessageRequest::new(&self.model, vec![Message::user(objective)], 4096)
            .with_system(ACTOR_SYSTEM_PROMPT.to_string())
            .with_tools(goal_session_tool_definitions());

        runtime
            .block_on(async {
                client
                    .execute_with_tools(request, |name, input| async move {
                        dispatch_tool(invocation, tools, &name, input)
                    })
                    .await
            })
            .map(|_| ())
            .map_err(|error| RecipeProcessError::failed(format!("actor recipe step: {error}")))
    }
}

fn goal_session_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "read_semantic_context",
            "Read one opaque semantic input without normalization.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "field": {
                        "type": "string",
                        "enum": ["task", "reason", "observe_output", "orient_output", "decide_output"]
                    }
                },
                "required": ["field"],
                "additionalProperties": false
            }),
        ),
        ToolDefinition::new(
            "record_action",
            "Record one typed machine-action request as this cycle's terminal.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "request_id": {"type": "string"},
                    "action": {"type": "object"},
                    "raw_semantic": {"type": "object"},
                    "evidence": {"type": "array"}
                },
                "required": ["request_id", "action", "raw_semantic", "evidence"],
                "additionalProperties": false
            }),
        ),
        ToolDefinition::new(
            "record_no_action",
            "Record an explicit semantic no-action terminal.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "request_id": {"type": "string"},
                    "reason": {"type": "object"},
                    "raw_semantic": {"type": "object"},
                    "evidence": {"type": "array"}
                },
                "required": ["request_id", "reason", "raw_semantic", "evidence"],
                "additionalProperties": false
            }),
        ),
        ToolDefinition::new(
            "record_blocked",
            "Record a typed blocked terminal and retry policy.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "request_id": {"type": "string"},
                    "reason": {"type": "object"},
                    "blocker": {"type": "object"},
                    "retry": {"type": "object"},
                    "raw_semantic": {"type": "object"},
                    "evidence": {"type": "array"}
                },
                "required": ["request_id", "reason", "blocker", "retry", "raw_semantic", "evidence"],
                "additionalProperties": false
            }),
        ),
        ToolDefinition::new(
            "record_completed",
            "Record a completed terminal backed by typed verification evidence.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "request_id": {"type": "string"},
                    "summary": {"type": "object"},
                    "completion": {"type": "object"},
                    "raw_semantic": {"type": "object"},
                    "evidence": {"type": "array"}
                },
                "required": ["request_id", "summary", "completion", "raw_semantic", "evidence"],
                "additionalProperties": false
            }),
        ),
    ]
}

fn dispatch_tool(
    invocation: &GoalSessionInvocation,
    tools: &GoalSessionTools<'_>,
    name: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, ClientError> {
    let result = match name {
        "read_semantic_context" => read_context(invocation, input),
        "record_action" => decode::<ActionCall>(input).and_then(|call| {
            tools
                .record_action(
                    &call.request_id,
                    call.action,
                    call.raw_semantic,
                    call.evidence,
                )
                .and_then(to_json)
                .map_err(|error| error.to_string())
        }),
        "record_no_action" => decode::<NoActionCall>(input).and_then(|call| {
            tools
                .record_no_action(
                    &call.request_id,
                    call.reason,
                    call.raw_semantic,
                    call.evidence,
                )
                .and_then(to_json)
                .map_err(|error| error.to_string())
        }),
        "record_blocked" => decode::<BlockedCall>(input).and_then(|call| {
            tools
                .record_blocked(
                    &call.request_id,
                    call.reason,
                    call.blocker,
                    call.retry,
                    call.raw_semantic,
                    call.evidence,
                )
                .and_then(to_json)
                .map_err(|error| error.to_string())
        }),
        "record_completed" => decode::<CompletedCall>(input).and_then(|call| {
            tools
                .record_completed(
                    &call.request_id,
                    call.summary,
                    call.completion,
                    call.raw_semantic,
                    call.evidence,
                )
                .and_then(to_json)
                .map_err(|error| error.to_string())
        }),
        _ => Err(format!("capability {name:?} is not granted to this actor")),
    };
    if let Err(error) = &result {
        tools.note_tool_failure(error);
    }
    result.map_err(ClientError::Unknown)
}

fn read_context(
    invocation: &GoalSessionInvocation,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReadContext {
        field: ContextField,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ContextField {
        Task,
        Reason,
        ObserveOutput,
        OrientOutput,
        DecideOutput,
    }

    let call: ReadContext = decode(input)?;
    let value = match call.field {
        ContextField::Task => &invocation.task,
        ContextField::Reason => &invocation.reason,
        ContextField::ObserveOutput => &invocation.observe_output,
        ContextField::OrientOutput => &invocation.orient_output,
        ContextField::DecideOutput => &invocation.decide_output,
    };
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn decode<T: for<'de> Deserialize<'de>>(input: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(input).map_err(|error| format!("invalid typed tool arguments: {error}"))
}

fn to_json<T: serde::Serialize>(value: T) -> Result<serde_json::Value, super::CapabilityError> {
    serde_json::to_value(value).map_err(|error| {
        super::CapabilityError::new(
            super::CapabilityErrorCode::PersistenceFailed,
            format!("capability result serialization failed: {error}"),
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionCall {
    request_id: String,
    action: Action,
    raw_semantic: OpaqueBytes,
    evidence: Vec<EvidenceRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoActionCall {
    request_id: String,
    reason: OpaqueBytes,
    raw_semantic: OpaqueBytes,
    evidence: Vec<EvidenceRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockedCall {
    request_id: String,
    reason: OpaqueBytes,
    blocker: BlockerRef,
    retry: RetryPolicy,
    raw_semantic: OpaqueBytes,
    evidence: Vec<EvidenceRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletedCall {
    request_id: String,
    summary: OpaqueBytes,
    completion: CompletionRef,
    raw_semantic: OpaqueBytes,
    evidence: Vec<EvidenceRef>,
}
