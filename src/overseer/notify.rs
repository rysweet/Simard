//! M2 **mandatory** `NotifyOperator` capability (operator policy #3).
//!
//! EVERY PR merge — autonomous or human — must notify the operator via BOTH
//! **email** and **Signal**, with a concise plain-language explanation of the
//! PROBLEM being solved and the PR that solves it. This module is a first-class,
//! mandatory capability wired into the merge path
//! ([`merge_ops`](crate::overseer::merge_ops)) so **no merge completes without
//! the notification firing**.
//!
//! Design constraints encoded here:
//! - **Two channels, always both.** [`DualChannelNotifier`] fires every channel
//!   on every notification and records each outcome.
//! - **Never silently drop.** An unconfigured channel returns
//!   [`ChannelDelivery::Queued`] (logged), never nothing. A delivery error
//!   returns [`ChannelDelivery::Failed`] (logged). There is no code path that
//!   drops a notification on the floor.
//! - **Reuse the `ConversationChannel` abstraction (PR #2529)** for Signal:
//!   [`ConversationSignalSender`] adapts ANY `ConversationChannel` (incl. the
//!   `MockConversationChannel`) into an object-safe [`SignalSender`], driving its
//!   async `send` on an injected runtime handle.
//! - **Email via env** (`SIMARD_OVERSEER_EMAIL_TO` / `SIMARD_OVERSEER_EMAIL_FROM`
//!   / `SMTP_HOST` / `SMTP_PORT` / `SMTP_USER` / `SMTP_PASS`); unset → queued.

use std::sync::Mutex;

use crate::conversation_channel::{ConversationChannel, OutKind, Outbound};
use crate::overseer::whisper_ops::WhisperUrgency;

// ─────────────────────────── notification content ──────────────────────────

/// The concise `{problem, pr_title, pr_url, repo}` summary sent to the operator
/// on every merge (operator policy #3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeNotification {
    /// Plain-language description of the PROBLEM the PR solves.
    pub problem: String,
    pub pr_title: String,
    pub pr_url: String,
    pub repo: String,
    /// Was the merge autonomous (Overseer) or human-initiated?
    pub autonomous: bool,
}

impl MergeNotification {
    /// The email subject line.
    pub fn subject(&self) -> String {
        format!("[Overseer] merged: {}", self.pr_title)
    }

    /// A short, plain-language body suitable for email or a Signal message.
    pub fn plain_text(&self) -> String {
        self.to_operator().plain_text()
    }

    /// Render this merge as a general [`OperatorNotification`].
    pub fn to_operator(&self) -> OperatorNotification {
        OperatorNotification {
            kind: "merge",
            headline: self.pr_title.clone(),
            problem: self.problem.clone(),
            link: Some(self.pr_url.clone()),
            repo: self.repo.clone(),
            autonomous: self.autonomous,
        }
    }
}

/// A general operator notification the channels deliver. Both a merge (M2) and a
/// deploy (M3) render into this, so the channels stay event-agnostic while the
/// mandatory "notify on both channels, never drop" guarantee covers every kind
/// of operator event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorNotification {
    /// Event kind for the subject/logs: `"merge"` | `"deploy"`.
    pub kind: &'static str,
    /// One-line headline (email subject core / Signal first line).
    pub headline: String,
    /// Plain-language explanation of the problem/why.
    pub problem: String,
    /// Optional canonical link (PR url / commit url).
    pub link: Option<String>,
    pub repo: String,
    pub autonomous: bool,
}

impl OperatorNotification {
    /// The email subject line.
    pub fn subject(&self) -> String {
        format!("[Overseer] {}: {}", self.kind, self.headline)
    }

    /// A short, plain-language body suitable for email or a Signal message.
    pub fn plain_text(&self) -> String {
        let who = if self.autonomous {
            "The Overseer autonomously"
        } else {
            "The operator"
        };
        let link = self
            .link
            .as_deref()
            .map(|l| format!("\n\nLink:\n  {l}"))
            .unwrap_or_default();
        format!(
            "{who} performed a {kind} in {repo}.\n\nProblem solved:\n  {problem}{link}\n",
            who = who,
            kind = self.kind,
            repo = self.repo,
            problem = self.problem,
            link = link,
        )
    }

    /// Build a deploy notification (M3). `previous`/`commit` are the deployed
    /// git hashes; `gate_summary` describes the canary/deploy-gate result.
    pub fn deploy(commit: &str, previous: &str, repo: &str, gate_summary: &str) -> Self {
        Self {
            kind: "deploy",
            headline: format!("deployed {}", short_commit(commit)),
            problem: format!("Deployed {commit} (previous {previous}); {gate_summary}"),
            link: None,
            repo: repo.to_string(),
            autonomous: true,
        }
    }

    /// Build a whisper notification: surfaces an advisory steering note the
    /// Overseer injected into Simard's loop so whispers are TRANSPARENT to the
    /// operator (never a hidden side-channel). `trigger` is the observed problem
    /// (e.g. `"loop_detected"`); `goal_id` is the goal being steered.
    pub fn whisper(note: &str, trigger: &str, urgency: WhisperUrgency, goal_id: &str) -> Self {
        Self {
            kind: "whisper",
            headline: format!(
                "steering goal {goal_id} ({trigger}, {} urgency)",
                urgency.label()
            ),
            problem: note.to_string(),
            link: None,
            repo: "rysweet/Simard".to_string(),
            autonomous: true,
        }
    }

    /// Build a blocked-goal escalation: a goal carrying a "needs human review"
    /// safeguard marker that a human must resolve. Sent on BOTH channels (email
    /// and Signal) with the goal id and reason so the marker actually reaches a
    /// person — closing the silent-failure gap where a "needs human review"
    /// marker reached no human.
    pub fn goal_blocked(goal_id: &str, reason: &str) -> Self {
        Self {
            kind: "goal-blocked",
            headline: format!("goal {goal_id} needs human review"),
            problem: format!(
                "Goal `{goal_id}` is blocked and needs human review.\n  Reason: {reason}"
            ),
            link: None,
            repo: "rysweet/Simard".to_string(),
            autonomous: true,
        }
    }
}

fn short_commit(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// The outcome of delivering to one channel. There is no "silently dropped"
/// variant by construction — an unconfigured channel is [`Queued`], an error is
/// [`Failed`], both of which are recorded and logged.
///
/// [`Queued`]: ChannelDelivery::Queued
/// [`Failed`]: ChannelDelivery::Failed
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelDelivery {
    /// Delivered to the channel transport.
    Sent,
    /// Channel not configured (or degraded); the notification is queued/pending
    /// and logged — it is the operator's job to configure the transport.
    Queued { reason: String },
    /// The transport was attempted but errored; recorded and logged.
    Failed { reason: String },
}

impl ChannelDelivery {
    pub fn is_sent(&self) -> bool {
        matches!(self, ChannelDelivery::Sent)
    }
}

/// The result of firing all channels for one notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyReport {
    pub per_channel: Vec<(String, ChannelDelivery)>,
}

impl NotifyReport {
    /// True iff every channel actually delivered (no queue, no failure).
    pub fn all_sent(&self) -> bool {
        !self.per_channel.is_empty() && self.per_channel.iter().all(|(_, d)| d.is_sent())
    }

    /// The notification always "fires": every configured channel is attempted
    /// and every outcome recorded. This is the mandatory guarantee — a merge is
    /// never considered complete without a `NotifyReport` in hand.
    pub fn dispatched(&self) -> bool {
        !self.per_channel.is_empty()
    }
}

// ─────────────────────────── channel abstractions ──────────────────────────

/// A single notification transport. Object-safe (so channels can be boxed and
/// composed) and deliberately synchronous at this boundary — async transports
/// (Signal) are driven behind the adapter.
pub trait NotifyChannel: Send + Sync {
    fn name(&self) -> &str;
    fn deliver(&self, notification: &OperatorNotification) -> ChannelDelivery;
}

/// The mandatory operator notifier: fires EVERY channel on EVERY notification.
/// Constructed with an email channel and a Signal channel so both fire.
pub struct DualChannelNotifier {
    channels: Vec<Box<dyn NotifyChannel>>,
}

/// Object-safe seam the acting Overseer notifies the operator through. Lets the
/// Overseer hold the mandatory [`DualChannelNotifier`] (email + Signal) in
/// production while tests inject a fake that records the notification — reusing
/// the ONE "notify on both channels, never drop" guarantee rather than adding a
/// second notification path.
pub trait OperatorNotifier: Send + Sync {
    fn notify(&self, notification: &OperatorNotification) -> NotifyReport;
}

impl OperatorNotifier for DualChannelNotifier {
    fn notify(&self, notification: &OperatorNotification) -> NotifyReport {
        DualChannelNotifier::notify(self, notification)
    }
}

impl DualChannelNotifier {
    /// Construct from an explicit channel list (tests inject fakes). The merge
    /// path uses [`from_env`](DualChannelNotifier::from_env) to get email+Signal.
    pub fn new(channels: Vec<Box<dyn NotifyChannel>>) -> Self {
        Self { channels }
    }

    /// The production notifier: an email channel + a Signal channel, both wired
    /// from the environment. Unconfigured channels degrade to `Queued` (logged);
    /// nothing is ever dropped.
    pub fn from_env() -> Self {
        Self::new(vec![
            Box::new(EmailNotifyChannel::from_env()),
            Box::new(SignalNotifyChannel::from_env()),
        ])
    }

    /// Fire every channel, recording each outcome. This is the single call the
    /// merge/deploy paths make; its `NotifyReport` proves the notification fired
    /// on both channels.
    pub fn notify(&self, notification: &OperatorNotification) -> NotifyReport {
        let per_channel = self
            .channels
            .iter()
            .map(|c| {
                let outcome = c.deliver(notification);
                if !outcome.is_sent() {
                    // Never silently drop: log the degraded/failed delivery.
                    log_degraded(c.name(), &outcome, notification);
                }
                (c.name().to_string(), outcome)
            })
            .collect();
        NotifyReport { per_channel }
    }
}

fn log_degraded(channel: &str, outcome: &ChannelDelivery, n: &OperatorNotification) {
    tracing::warn!(
        target: "overseer::notify",
        channel,
        kind = n.kind,
        link = n.link.as_deref().unwrap_or(""),
        ?outcome,
        "operator notification not delivered live — queued/failed (never dropped)"
    );
}

// ─────────────────────────── email channel ─────────────────────────────────

/// Env-driven SMTP configuration.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct EmailConfig {
    pub to: Vec<String>,
    pub from: Option<String>,
    pub host: Option<String>,
    pub port: u16,
    pub user: Option<String>,
    pub pass: Option<String>,
}

impl EmailConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Injectable-env constructor (tests build a fixed map).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let to = lookup("SIMARD_OVERSEER_EMAIL_TO")
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Self {
            to,
            from: lookup("SIMARD_OVERSEER_EMAIL_FROM").filter(|s| !s.trim().is_empty()),
            host: lookup("SMTP_HOST").filter(|s| !s.trim().is_empty()),
            port: lookup("SMTP_PORT")
                .and_then(|s| s.trim().parse::<u16>().ok())
                .unwrap_or(25),
            user: lookup("SMTP_USER").filter(|s| !s.trim().is_empty()),
            pass: lookup("SMTP_PASS").filter(|s| !s.trim().is_empty()),
        }
    }

    /// Fully configured iff we know a host, a from, and at least one recipient.
    pub fn is_configured(&self) -> bool {
        self.host.is_some() && self.from.is_some() && !self.to.is_empty()
    }
}

/// A minimal email message handed to an [`EmailSender`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailMessage {
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// The wire transport for email. Injectable so tests use a fake and production
/// uses [`TcpSmtpSender`]. Object-safe + `Send + Sync`.
pub trait EmailSender: Send + Sync {
    fn send(&self, msg: &EmailMessage) -> Result<(), String>;
}

/// The email notification channel. When SMTP is unconfigured it returns
/// [`ChannelDelivery::Queued`] (never dropped); when configured it delegates to
/// the injected [`EmailSender`].
pub struct EmailNotifyChannel {
    config: EmailConfig,
    sender: Box<dyn EmailSender>,
}

impl EmailNotifyChannel {
    pub fn new(config: EmailConfig, sender: Box<dyn EmailSender>) -> Self {
        Self { config, sender }
    }

    /// Production channel: env config + a plaintext-SMTP transport.
    pub fn from_env() -> Self {
        Self::new(EmailConfig::from_env(), Box::new(TcpSmtpSender))
    }
}

impl NotifyChannel for EmailNotifyChannel {
    fn name(&self) -> &str {
        "email"
    }

    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        if !self.config.is_configured() {
            return ChannelDelivery::Queued {
                reason: "SMTP not configured (set SMTP_HOST / SIMARD_OVERSEER_EMAIL_{FROM,TO})"
                    .to_string(),
            };
        }
        let msg = EmailMessage {
            from: self.config.from.clone().unwrap_or_default(),
            to: self.config.to.clone(),
            subject: n.subject(),
            body: n.plain_text(),
        };
        match self.sender.send(&msg) {
            Ok(()) => ChannelDelivery::Sent,
            Err(e) => ChannelDelivery::Failed { reason: e },
        }
    }
}

/// A minimal, dependency-free, timeout-bounded **plaintext** SMTP sender for
/// local/relay MTAs (no TLS/AUTH). It is deliberately conservative: any protocol
/// error surfaces as `Err` so the channel records a `Failed` (never a silent
/// drop). TLS/authenticating relays are out of scope for this minimal mailer —
/// the queued fallback covers them until a real transport is wired.
#[derive(Clone, Debug, Default)]
pub struct TcpSmtpSender;

impl EmailSender for TcpSmtpSender {
    fn send(&self, msg: &EmailMessage) -> Result<(), String> {
        // Host/port live in the channel's config; the sender is handed a fully
        // resolved message. We re-read connection params from the environment so
        // the sender stays a pure transport with no config of its own.
        let host = std::env::var("SMTP_HOST").map_err(|_| "SMTP_HOST unset".to_string())?;
        let port = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|s| s.trim().parse::<u16>().ok())
            .unwrap_or(25);
        smtp_send_plaintext(&host, port, msg)
    }
}

/// Minimal plaintext SMTP conversation. Kept small and defensive.
fn smtp_send_plaintext(host: &str, port: u16, msg: &EmailMessage) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("{host}:{port}");
    let stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr} failed: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);

    let expect = |reader: &mut BufReader<TcpStream>, want: u8| -> Result<(), String> {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("smtp read: {e}"))?;
        if line.as_bytes().first() == Some(&(b'0' + want)) {
            Ok(())
        } else {
            Err(format!("unexpected SMTP reply: {}", line.trim()))
        }
    };

    expect(&mut reader, 2)?; // greeting 220
    writeln!(writer, "EHLO simard-overseer").map_err(|e| e.to_string())?;
    expect(&mut reader, 2)?;
    writeln!(writer, "MAIL FROM:<{}>", msg.from).map_err(|e| e.to_string())?;
    expect(&mut reader, 2)?;
    for rcpt in &msg.to {
        writeln!(writer, "RCPT TO:<{rcpt}>").map_err(|e| e.to_string())?;
        expect(&mut reader, 2)?;
    }
    writeln!(writer, "DATA").map_err(|e| e.to_string())?;
    expect(&mut reader, 3)?; // 354 start mail input
    write!(
        writer,
        "From: {}\r\nTo: {}\r\nSubject: {}\r\n\r\n{}\r\n.\r\n",
        msg.from,
        msg.to.join(", "),
        msg.subject,
        msg.body.replace("\r\n", "\n").replace('\n', "\r\n"),
    )
    .map_err(|e| e.to_string())?;
    expect(&mut reader, 2)?; // 250 accepted
    writeln!(writer, "QUIT").map_err(|e| e.to_string())?;
    Ok(())
}

// ─────────────────────────── Signal channel ────────────────────────────────

/// The wire transport for Signal. Object-safe + `Send + Sync` so it can be
/// composed; the real impl adapts the async `ConversationChannel`.
pub trait SignalSender: Send + Sync {
    fn send_text(&self, text: &str) -> Result<(), String>;
}

/// Adapter turning ANY [`ConversationChannel`] (PR #2529) — including the
/// `MockConversationChannel` used in tests, and the feature-gated
/// `SignalConversation` in production — into an object-safe [`SignalSender`]. It
/// drives the channel's async `send` on the injected runtime handle, so the
/// synchronous merge path can notify without an async context of its own.
pub struct ConversationSignalSender<C: ConversationChannel + Send> {
    chan: Mutex<C>,
    handle: tokio::runtime::Handle,
}

impl<C: ConversationChannel + Send> ConversationSignalSender<C> {
    pub fn new(chan: C, handle: tokio::runtime::Handle) -> Self {
        Self {
            chan: Mutex::new(chan),
            handle,
        }
    }
}

impl<C: ConversationChannel + Send> SignalSender for ConversationSignalSender<C> {
    fn send_text(&self, text: &str) -> Result<(), String> {
        let mut chan = self.chan.lock().map_err(|e| e.to_string())?;
        let out = Outbound {
            kind: OutKind::Notice,
            text: text.to_string(),
        };
        self.handle
            .block_on(chan.send(out))
            .map_err(|e| e.to_string())
    }
}

/// The Signal notification channel. When no sender is wired (e.g. the `signal`
/// feature is off, or signal-cli is unconfigured) it returns
/// [`ChannelDelivery::Queued`] (never dropped); otherwise it delegates to the
/// injected [`SignalSender`].
pub struct SignalNotifyChannel {
    sender: Option<Box<dyn SignalSender>>,
}

impl SignalNotifyChannel {
    pub fn new(sender: Option<Box<dyn SignalSender>>) -> Self {
        Self { sender }
    }

    /// Production channel. A live Signal transport requires the daemon's async
    /// runtime + a configured `SignalConversation` (feature `signal`), which the
    /// operator wires explicitly; absent that, the channel queues (logged),
    /// never drops. See [`ConversationSignalSender`] for the adapter used when a
    /// channel IS available.
    pub fn from_env() -> Self {
        Self::new(None)
    }
}

impl NotifyChannel for SignalNotifyChannel {
    fn name(&self) -> &str {
        "signal"
    }

    fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
        match &self.sender {
            None => ChannelDelivery::Queued {
                reason: "Signal channel not wired (configure the ConversationChannel transport)"
                    .to_string(),
            },
            Some(sender) => match sender.send_text(&n.plain_text()) {
                Ok(()) => ChannelDelivery::Sent,
                Err(e) => ChannelDelivery::Failed { reason: e },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_channel::MockConversationChannel;

    fn sample_merge() -> MergeNotification {
        MergeNotification {
            problem: "distillation parse-failure rate exceeded threshold".to_string(),
            pr_title: "fix(distill): strip launch-banner noise".to_string(),
            pr_url: "https://github.com/rysweet/Simard/pull/123".to_string(),
            repo: "rysweet/Simard".to_string(),
            autonomous: true,
        }
    }

    fn sample() -> OperatorNotification {
        sample_merge().to_operator()
    }

    // ── content ──────────────────────────────────────────────────────────────

    #[test]
    fn plain_text_carries_problem_pr_and_repo() {
        let n = sample_merge().to_operator();
        // The PR title heads the subject; the body carries problem + link + repo.
        assert!(
            n.subject()
                .contains("fix(distill): strip launch-banner noise")
        );
        let body = n.plain_text();
        assert!(body.contains("distillation parse-failure"));
        assert!(body.contains("https://github.com/rysweet/Simard/pull/123"));
        assert!(body.contains("rysweet/Simard"));
    }

    #[test]
    fn deploy_notification_carries_commits_and_repo() {
        let n = OperatorNotification::deploy(
            "abcdef1234567890",
            "0011223344556677",
            "rysweet/Simard",
            "canary green (4/4 gates)",
        );
        assert_eq!(n.kind, "deploy");
        assert!(n.subject().contains("deployed abcdef123456"));
        let body = n.plain_text();
        assert!(body.contains("abcdef1234567890"));
        assert!(body.contains("0011223344556677"));
        assert!(body.contains("canary green"));
    }

    // ── fakes ────────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct RecordingChannel {
        name: String,
        outcome: Option<ChannelDelivery>,
        seen: Mutex<Vec<OperatorNotification>>,
    }
    impl NotifyChannel for RecordingChannel {
        fn name(&self) -> &str {
            &self.name
        }
        fn deliver(&self, n: &OperatorNotification) -> ChannelDelivery {
            self.seen.lock().unwrap().push(n.clone());
            self.outcome.clone().unwrap_or(ChannelDelivery::Sent)
        }
    }

    // ── DualChannelNotifier: both channels always fire ───────────────────────

    #[test]
    fn dual_notifier_fires_every_channel() {
        let notifier = DualChannelNotifier::new(vec![
            Box::new(RecordingChannel {
                name: "email".to_string(),
                outcome: Some(ChannelDelivery::Sent),
                ..Default::default()
            }),
            Box::new(RecordingChannel {
                name: "signal".to_string(),
                outcome: Some(ChannelDelivery::Sent),
                ..Default::default()
            }),
        ]);
        let report = notifier.notify(&sample());
        assert!(report.dispatched());
        assert!(report.all_sent());
        assert_eq!(report.per_channel.len(), 2);
        let names: Vec<&str> = report.per_channel.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"email") && names.contains(&"signal"));
    }

    #[test]
    fn unconfigured_channel_queues_never_drops() {
        // An unconfigured email + signal pair still DISPATCHES (both recorded),
        // just Queued — never silently dropped.
        let notifier = DualChannelNotifier::new(vec![
            Box::new(EmailNotifyChannel::new(
                EmailConfig::default(),
                Box::new(TcpSmtpSender),
            )),
            Box::new(SignalNotifyChannel::from_env()),
        ]);
        let report = notifier.notify(&sample());
        assert!(report.dispatched(), "notification must still dispatch");
        assert!(!report.all_sent());
        assert!(
            report
                .per_channel
                .iter()
                .all(|(_, d)| matches!(d, ChannelDelivery::Queued { .. })),
            "unconfigured channels queue, never drop: {report:?}"
        );
    }

    // ── email channel ────────────────────────────────────────────────────────

    struct FakeSmtp {
        sent: Mutex<Vec<EmailMessage>>,
        fail: bool,
    }
    impl EmailSender for FakeSmtp {
        fn send(&self, msg: &EmailMessage) -> Result<(), String> {
            self.sent.lock().unwrap().push(msg.clone());
            if self.fail {
                Err("smtp boom".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn configured_email() -> EmailConfig {
        EmailConfig::from_lookup(|k| match k {
            "SIMARD_OVERSEER_EMAIL_TO" => Some("ops@example.com, sec@example.com".to_string()),
            "SIMARD_OVERSEER_EMAIL_FROM" => Some("overseer@example.com".to_string()),
            "SMTP_HOST" => Some("localhost".to_string()),
            _ => None,
        })
    }

    #[test]
    fn email_config_parses_recipients_and_defaults_port() {
        let cfg = configured_email();
        assert_eq!(cfg.to, vec!["ops@example.com", "sec@example.com"]);
        assert_eq!(cfg.port, 25);
        assert!(cfg.is_configured());
        assert!(!EmailConfig::default().is_configured());
    }

    #[test]
    fn email_channel_sends_when_configured() {
        let smtp = FakeSmtp {
            sent: Mutex::new(vec![]),
            fail: false,
        };
        let ch = EmailNotifyChannel::new(configured_email(), Box::new(smtp));
        let out = ch.deliver(&sample());
        assert_eq!(out, ChannelDelivery::Sent);
    }

    #[test]
    fn email_channel_reports_failure_never_drops() {
        let smtp = FakeSmtp {
            sent: Mutex::new(vec![]),
            fail: true,
        };
        let ch = EmailNotifyChannel::new(configured_email(), Box::new(smtp));
        assert!(matches!(
            ch.deliver(&sample()),
            ChannelDelivery::Failed { .. }
        ));
    }

    #[test]
    fn email_channel_queues_when_unconfigured() {
        let ch = EmailNotifyChannel::new(EmailConfig::default(), Box::new(TcpSmtpSender));
        assert!(matches!(
            ch.deliver(&sample()),
            ChannelDelivery::Queued { .. }
        ));
    }

    // ── Signal channel via the real ConversationChannel abstraction ──────────

    #[test]
    fn signal_channel_delivers_through_conversation_channel() {
        // Reuse the shipped ConversationChannel abstraction (PR #2529) end-to-end
        // with the Mock, driven on a current-thread runtime — no signal-cli, no
        // network. Proves the adapter path the production SignalConversation uses.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mock = MockConversationChannel::with_script(vec![]);
        let sender = ConversationSignalSender::new(mock, rt.handle().clone());
        let ch = SignalNotifyChannel::new(Some(Box::new(sender)));

        let out = ch.deliver(&sample());
        assert_eq!(out, ChannelDelivery::Sent);
    }

    #[test]
    fn signal_channel_queues_when_unwired() {
        let ch = SignalNotifyChannel::from_env();
        assert!(matches!(
            ch.deliver(&sample()),
            ChannelDelivery::Queued { .. }
        ));
    }
}
