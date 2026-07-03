//! A scripted [`ConversationChannel`] test double for driver + integration tests.
//!
//! Unlike the `apply_record`/`run_conversation` stubs, the mock is **fully
//! implemented** — it is test infrastructure, and a required deliverable of the
//! abstraction. `recv` replays a scripted list of inbound lines (all authorized)
//! and then ends the session; `send` captures every [`Outbound`]; `on_recorded`
//! counts its own invocations so a test can assert the hook fires once per
//! record command.

use std::collections::VecDeque;
use std::future::Future;

use crate::error::SimardResult;
use crate::meeting_backend::MeetingBackend;

use super::{ConversationChannel, Inbound, OperatorRef, Outbound};

/// A scripted `ConversationChannel` for driver + integration tests.
pub struct MockConversationChannel {
    inbox: VecDeque<String>,
    sent: Vec<Outbound>,
    recorded_hook_calls: usize,
}

impl MockConversationChannel {
    /// Build from a script of inbound lines; each `recv()` yields the next line
    /// (as an authorized [`Inbound`]), then `Ok(None)`.
    pub fn with_script(lines: Vec<&str>) -> Self {
        Self {
            inbox: lines.into_iter().map(|s| s.to_string()).collect(),
            sent: Vec::new(),
            recorded_hook_calls: 0,
        }
    }

    /// All [`Outbound`] messages captured by `send()`, in order.
    pub fn sent(&self) -> &[Outbound] {
        &self.sent
    }

    /// Count of `on_recorded` invocations (asserts the hook fires per record).
    pub fn recorded_hook_calls(&self) -> usize {
        self.recorded_hook_calls
    }
}

impl ConversationChannel for MockConversationChannel {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn recv(&mut self) -> impl Future<Output = SimardResult<Option<Inbound>>> + Send {
        // Advance the script synchronously so the returned future owns its data
        // and never borrows `self` across an `.await`.
        let next = self.inbox.pop_front();
        async move {
            Ok(next.map(|text| Inbound {
                from: OperatorRef {
                    id: "mock".to_string(),
                    authorized: true,
                },
                text,
            }))
        }
    }

    fn send(&mut self, out: Outbound) -> impl Future<Output = SimardResult<()>> + Send {
        self.sent.push(out);
        async move { Ok(()) }
    }

    fn on_recorded(
        &mut self,
        _backend: &MeetingBackend,
    ) -> impl Future<Output = SimardResult<()>> + Send {
        self.recorded_hook_calls += 1;
        async move { Ok(()) }
    }
}
