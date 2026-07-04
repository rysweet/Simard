//! Outside-in TDD integration test for Dashboard Chat persistence &
//! resumability (issue #2577, Step 7).
//!
//! This is an **integration** test target: it can only reach the crate's
//! *public* API. It therefore pins the two public contracts that make a chat
//! session durable and resumable, independent of the dashboard-internal
//! `chat_store` module (covered by in-crate unit tests):
//!
//!   1. `MeetingBackend::restore(history)` — the public rehydration hook. On
//!      reopening a session the persisted turns are replayed into a fresh
//!      backend so the **agent** (not just the UI) regains full prior context
//!      (FR-2). Per `docs/reference/dashboard-chat.md#meetingbackendrestore`,
//!      when the persisted history exceeds `MAX_HISTORY` (500) turns, `restore`
//!      seeds the working set with the most-recent 500 turns; the full
//!      transcript stays complete on disk.
//!
//!   2. The on-disk `<session_id>.json` history array is a list of
//!      `ConversationMessage` values and round-trips through serde **uncapped**
//!      (FR-1) — modeling the durable store's format using only the public
//!      `ConversationMessage` / `Role` types.
//!
//! `MeetingBackend::restore` does not exist yet, so this file fails to compile
//! until it is implemented — the intended TDD red state.

use serial_test::serial;
use tempfile::TempDir;

use simard::base_types::{
    BaseTypeDescriptor, BaseTypeId, BaseTypeOutcome, BaseTypeSession, BaseTypeTurnInput,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    standard_session_capabilities,
};
use simard::error::SimardResult;
use simard::meeting_backend::{ConversationMessage, MeetingBackend, Role};
use simard::metadata::{BackendDescriptor, Freshness};
use simard::runtime::RuntimeTopology;

/// In-memory `MAX_HISTORY` working-set cap enforced by `MeetingBackend`
/// (`src/meeting_backend/mod.rs`). Restore obeys the same ceiling for the live
/// inference window while the durable transcript stays uncapped on disk.
const MAX_HISTORY: usize = 500;

// ---------------------------------------------------------------------------
// Minimal non-blocking mock agent (mirrors the outside-in meeting test mock).
// ---------------------------------------------------------------------------

struct EchoSession {
    descriptor: BaseTypeDescriptor,
    is_open: bool,
    is_closed: bool,
}

impl EchoSession {
    fn new() -> Self {
        Self {
            descriptor: BaseTypeDescriptor {
                id: BaseTypeId::new("echo-mock-chat"),
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "mock",
                    "test:echo-mock-chat",
                    Freshness::now().unwrap(),
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            is_open: true,
            is_closed: false,
        }
    }
}

impl BaseTypeSession for EchoSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, _input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;
        Ok(BaseTypeOutcome {
            plan: String::new(),
            execution_summary: "ack".to_string(),
            evidence: Vec::new(),
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        self.is_closed = true;
        Ok(())
    }
}

fn backend() -> MeetingBackend {
    MeetingBackend::new_session(
        "Dashboard Chat",
        Box::new(EchoSession::new()),
        None,
        String::new(),
    )
}

fn turn(role: Role, content: &str, ts: &str) -> ConversationMessage {
    ConversationMessage {
        role,
        content: content.to_string(),
        timestamp: ts.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 1. MeetingBackend::restore rehydrates agent context (FR-2)
// ---------------------------------------------------------------------------

#[test]
#[serial(state_root)]
fn restore_rehydrates_full_history_in_order() {
    let history = vec![
        turn(
            Role::User,
            "How do I unblock a stuck OODA goal?",
            "2026-07-04T15:20:11Z",
        ),
        turn(
            Role::Assistant,
            "Inspect the goal board.",
            "2026-07-04T15:20:19Z",
        ),
        turn(
            Role::User,
            "And if it's still blocked?",
            "2026-07-04T15:21:00Z",
        ),
        turn(
            Role::Assistant,
            "Escalate to a decision.",
            "2026-07-04T15:21:08Z",
        ),
    ];

    let mut b = backend();
    b.restore(history.clone());

    assert_eq!(
        b.history(),
        history.as_slice(),
        "restore must seed the in-memory history exactly (role, content, timestamp, order)"
    );
    assert_eq!(
        b.status().message_count,
        history.len(),
        "status message_count reflects the rehydrated turns"
    );
}

#[test]
#[serial(state_root)]
fn restore_on_empty_history_is_noop() {
    let mut b = backend();
    b.restore(Vec::new());
    assert!(
        b.history().is_empty(),
        "restoring an empty history leaves the backend empty"
    );
}

#[test]
#[serial(state_root)]
fn restore_caps_working_set_to_most_recent_max_history() {
    // A very long persisted transcript (over the in-memory cap).
    let total = MAX_HISTORY + 100; // 600
    let full: Vec<ConversationMessage> = (0..total)
        .map(|i| {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            turn(
                role,
                &format!("turn {i}"),
                &format!("2026-07-04T15:00:{:02}Z", i % 60),
            )
        })
        .collect();

    let mut b = backend();
    b.restore(full.clone());

    let seeded = b.history();
    assert_eq!(
        seeded.len(),
        MAX_HISTORY,
        "restore seeds at most MAX_HISTORY turns into the live working set"
    );
    assert_eq!(
        seeded.last().unwrap(),
        full.last().unwrap(),
        "the MOST-RECENT turns are retained (newest turn present)"
    );
    assert_eq!(
        &seeded[0],
        &full[total - MAX_HISTORY],
        "the retained window is the most-recent MAX_HISTORY turns (oldest kept = index {})",
        total - MAX_HISTORY
    );
    // The caller's transcript vector is untouched (disk stays complete).
    assert_eq!(
        full.len(),
        total,
        "the persisted transcript itself remains uncapped"
    );
}

// ---------------------------------------------------------------------------
// 2. On-disk <id>.json history format (FR-1) — public ConversationMessage
// ---------------------------------------------------------------------------

#[test]
fn persisted_session_history_json_roundtrips_via_conversation_message() {
    // Models the documented <session_id>.json envelope. The `history` array is a
    // list of ConversationMessage; role serializes lowercase.
    let disk = serde_json::json!({
        "schema_version": 1,
        "meta": {
            "id": "018f3c9a-7b2e-7c41-9a10-2f6d0b1e4c88",
            "title": "How do I unblock a stuck OODA goal?",
            "created_at": "2026-07-04T15:20:11Z",
            "updated_at": "2026-07-04T15:20:19Z"
        },
        "history": [
            {"role": "user",      "content": "How do I unblock a stuck OODA goal?", "timestamp": "2026-07-04T15:20:11Z"},
            {"role": "assistant", "content": "Inspect the goal board.",              "timestamp": "2026-07-04T15:20:19Z"},
            {"role": "system",    "content": "Connected.",                           "timestamp": "2026-07-04T15:20:10Z"}
        ]
    });

    let history: Vec<ConversationMessage> =
        serde_json::from_value(disk["history"].clone()).expect("history deserializes");
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, Role::User);
    assert_eq!(history[1].role, Role::Assistant);
    assert_eq!(history[2].role, Role::System);
    assert_eq!(history[0].content, "How do I unblock a stuck OODA goal?");
    assert_eq!(history[1].timestamp, "2026-07-04T15:20:19Z");
}

#[test]
fn persisted_history_is_uncapped_on_disk() {
    // The durable format imposes no cap: a 600-turn transcript serializes and
    // deserializes intact (independent of the in-memory MAX_HISTORY window).
    let total = MAX_HISTORY + 100;
    let history: Vec<ConversationMessage> = (0..total)
        .map(|i| turn(Role::User, &format!("turn {i}"), "2026-07-04T15:20:11Z"))
        .collect();

    let json = serde_json::to_value(&history).unwrap();
    let back: Vec<ConversationMessage> = serde_json::from_value(json).unwrap();
    assert_eq!(
        back.len(),
        total,
        "on-disk history is uncapped ({total} turns)"
    );
    assert_eq!(back[0].content, "turn 0");
    assert_eq!(back[total - 1].content, format!("turn {}", total - 1));
}

// A `TempDir` import guard: keep the import used even as the integration surface
// evolves, so an unused-import warning never masks a real compile failure.
#[test]
fn tempdir_is_available_for_state_root_isolation() {
    let tmp = TempDir::new().unwrap();
    assert!(tmp.path().exists());
}
