use std::fmt::{self, Formatter};

use rustyclawd_core::client::{
    Client as RcClient, ClientError, Config as RcConfig, Message as RcMessage,
};

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSession, BaseTypeSessionRequest,
    BaseTypeTurnInput, ensure_session_not_already_open, ensure_session_not_closed,
    ensure_session_open, joined_prompt_ids,
};
use crate::error::{SimardError, SimardResult};
use crate::runtime_config::RuntimeConfig;
use crate::sanitization::objective_metadata;
use crate::session_builder::LlmProvider;

use super::execution::execute_rustyclawd_client;

/// Build the `rustyclawd_core` `Config` for the configured provider.
///
/// Splits the provider branch out so it is independently testable without
/// going through the full session lifecycle.
///
/// * `Copilot` — uses `RcConfig::new_copilot()` (sync, infallible). Token
///   is resolved at HTTP-call time via `gh auth token`.
/// * `RustyClawd` — uses `RcConfig::from_default_location().await` which
///   reads `ANTHROPIC_API_KEY` env / `.env` / legacy on-disk key file.
pub(super) fn build_rc_config(
    provider: LlmProvider,
    rt: &tokio::runtime::Runtime,
) -> std::result::Result<RcConfig, ClientError> {
    match provider {
        LlmProvider::Copilot => Ok(RcConfig::new_copilot()),
        LlmProvider::RustyClawd => rt.block_on(RcConfig::from_default_location()),
    }
}

pub(super) struct RustyClawdSession {
    pub(super) descriptor: BaseTypeDescriptor,
    pub(super) request: BaseTypeSessionRequest,
    pub(super) is_open: bool,
    pub(super) is_closed: bool,
    /// RustyClawd API client, initialized on open() from environment config.
    pub(super) client: Option<RcClient>,
    /// Tokio runtime for bridging async rustyclawd client calls into sync
    /// BaseTypeSession methods.
    pub(super) rt: Option<tokio::runtime::Runtime>,
    /// Accumulated conversation history for multi-turn sessions (meetings, etc.).
    pub(super) conversation_history: Vec<RcMessage>,
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

        // Resolve the configured provider before opening — RustyClawd is
        // multi-provider, and we honor the operator's `SIMARD_LLM_PROVIDER`
        // env var / `~/.simard/config.toml` choice. There is no silent
        // default: a missing config is surfaced verbatim so the operator
        // sees what to fix.
        let provider = RuntimeConfig::load()
            .map(|cfg| cfg.llm_provider)
            .map_err(|e| match e {
            SimardError::MissingRequiredConfig { .. } => SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason:
                    "Simard llm_provider not configured. Set SIMARD_LLM_PROVIDER=copilot|rustyclawd \
                     or add `llm_provider = \"...\"` to ~/.simard/config.toml."
                        .to_string(),
            },
            other => SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: format!("failed to load runtime config: {other}"),
            },
        })?;

        tracing::info!(
            provider = ?provider,
            "RustyClawd: attempting client initialization for resolved provider…"
        );
        let client_result = build_rc_config(provider, &rt).and_then(RcClient::new);

        match client_result {
            Ok(client) => {
                tracing::info!(
                    provider = ?provider,
                    "RustyClawd: API client initialized successfully"
                );
                self.client = Some(client);
            }
            Err(ClientError::ApiKeyNotFound) => {
                let reason = match provider {
                    LlmProvider::Copilot => {
                        "RustyClawd configured for Copilot provider but token resolution failed. \
                         Run `gh auth login` (and `gh auth refresh --hostname github.com \
                         --scopes copilot` if the Copilot scope is rejected)."
                            .to_string()
                    }
                    LlmProvider::RustyClawd => {
                        "RustyClawd configured for Anthropic provider but no API key found. \
                         Set ANTHROPIC_API_KEY env var or place key in ~/.rustyclawd/config."
                            .to_string()
                    }
                };
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason,
                });
            }
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!(
                        "failed to initialize RustyClawd client (provider={provider:?}): {e}"
                    ),
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

        let (execution_summary, process_evidence) = if let (Some(client), Some(rt)) =
            (self.client.as_ref(), self.rt.as_ref())
        {
            tracing::info!(backend = %self.descriptor.backend.identity, "RustyClawd: executing via direct API client");
            execute_rustyclawd_client(
                client,
                rt,
                &input,
                &self.descriptor,
                &self.request,
                &mut self.conversation_history,
            )?
        } else {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: "RustyClawd API client not initialized — open() should have caught this"
                    .to_string(),
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeSessionRequest;
    use crate::identity::OperatingMode;
    use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
    use crate::session::SessionId;

    use super::super::MAX_HISTORY_MESSAGES;
    use super::super::adapter::RustyClawdAdapter;

    // ── RustyClawdSession debug format ──

    #[test]
    fn session_struct_debug_format_is_readable() {
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

    #[test]
    fn session_debug_format_shows_client_none() {
        let descriptor = RustyClawdAdapter::registered("rc-dbg2")
            .unwrap()
            .descriptor
            .clone();
        let session = RustyClawdSession {
            descriptor,
            request: BaseTypeSessionRequest {
                session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000020")
                    .unwrap(),
                mode: OperatingMode::Engineer,
                topology: RuntimeTopology::SingleProcess,
                prompt_assets: vec![],
                runtime_node: RuntimeNodeId::local(),
                mailbox_address: RuntimeAddress::new("test-addr"),
            },
            is_open: false,
            is_closed: false,
            client: None,
            rt: None,
            conversation_history: Vec::new(),
        };
        let debug_str = format!("{session:?}");
        assert!(debug_str.contains("false")); // is_open and is_closed
        assert!(debug_str.contains("RustyClawdSession"));
    }

    // ── MAX_HISTORY_MESSAGES constant ──

    #[test]
    fn max_history_messages_is_reasonable() {
        let m = MAX_HISTORY_MESSAGES;
        assert!(m > 0, "must be positive, got {m}");
        assert!(m <= 100, "must be <= 100, got {m}");
    }

    #[test]
    fn max_history_messages_is_at_least_10() {
        let m = MAX_HISTORY_MESSAGES;
        assert!(m >= 10, "too low: {m}");
    }

    // ── Provider-aware config selection (issue #1193) ──

    #[test]
    fn build_rc_config_returns_copilot_endpoint_for_copilot_provider() {
        // RcConfig::new_copilot() is sync + infallible — no network needed.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let cfg =
            build_rc_config(LlmProvider::Copilot, &rt).expect("Copilot path must be infallible");
        assert_eq!(cfg.api_url, "https://api.githubcopilot.com");
    }
}
