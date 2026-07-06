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
        let per_channel: Vec<(String, ChannelDelivery)> = self
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
        let report = NotifyReport { per_channel };

        // One structured, secret-safe summary per notification so the
        // dispatched-but-not-all_sent state (e.g. Signal Sent + email Queued) is
        // plainly observable. Each channel field carries only the bare
        // ChannelDelivery variant name — never the `reason` string — so no
        // credential-adjacent text can land in the summary. The paired
        // `log_degraded` warning above still carries the full `?outcome` for
        // diagnosis.
        let channels = report
            .per_channel
            .iter()
            .map(|(name, d)| format!("{name}={}", delivery_variant(d)))
            .collect::<Vec<_>>()
            .join(" ");
        tracing::info!(
            target: "overseer::notify",
            dispatched = report.dispatched(),
            all_sent = report.all_sent(),
            kind = notification.kind,
            channels = %channels,
            "operator notification dispatched"
        );

        report
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

    /// Whether an AUTHENTICATED STARTTLS relay should be used: true iff BOTH
    /// `SMTP_USER` and `SMTP_PASS` are set. This is the sole selector
    /// [`EmailNotifyChannel::from_env`] uses to pick [`StartTlsSmtpSender`] over
    /// the plaintext [`TcpSmtpSender`]; it does not depend on the port.
    pub fn use_authenticated(&self) -> bool {
        self.user.is_some() && self.pass.is_some()
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

    /// Production channel: env config selects the SMTP transport. When `SMTP_USER`
    /// and `SMTP_PASS` are both set ([`EmailConfig::use_authenticated`]) it uses
    /// the STARTTLS + AUTH LOGIN [`StartTlsSmtpSender`] (e.g. office365); otherwise
    /// it falls back to the minimal plaintext [`TcpSmtpSender`] for a local relay.
    pub fn from_env() -> Self {
        let config = EmailConfig::from_env();
        let sender: Box<dyn EmailSender> = if config.use_authenticated() {
            Box::new(StartTlsSmtpSender)
        } else {
            Box::new(TcpSmtpSender)
        };
        Self::new(config, sender)
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

/// Neutralize email header / SMTP command injection (CWE-93) for any value that
/// is written into a DATA header line or the `MAIL FROM` / `RCPT TO` envelope.
/// CR, LF and every other control character are replaced with a space (so an
/// injected `\r\n` can no longer terminate the current line and smuggle a header
/// or SMTP verb), and the result is length-bounded (by **bytes**, on a UTF-8
/// char boundary) well under the RFC 5322 998-octet line limit. Some fields are
/// board-derived (e.g. a `goal_id` carried into the Subject), so sanitizing at
/// this single transport choke point covers every caller regardless of how the
/// field was constructed.
fn sanitize_header_value(value: &str) -> String {
    // Bound the *byte* length: a char cap would let multi-byte UTF-8 blow past
    // the 998-octet line limit this claims to stay under. Accumulate whole
    // chars so we never split a codepoint, stopping before the next char would
    // exceed the budget.
    const MAX_HEADER_BYTES: usize = 512;
    let mut out = String::with_capacity(value.len().min(MAX_HEADER_BYTES));
    for c in value.chars() {
        let c = if c.is_control() { ' ' } else { c };
        if out.len() + c.len_utf8() > MAX_HEADER_BYTES {
            break;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// SMTP transparency / "dot-stuffing" (RFC 5321 §4.5.2). The DATA section is
/// terminated by a line containing a single `.`; any body line that itself
/// begins with `.` must be escaped with an extra leading `.` so it cannot be
/// mistaken for that terminator. Without this, board-derived content carried
/// into the body (e.g. a goal id or a free-form block `reason`) that contains a
/// lone `.` line would prematurely close DATA and let the server interpret the
/// following bytes as SMTP commands — the same CWE-93 injection the header
/// sanitizer defends against, but via the body vector. Line endings are
/// normalized to CRLF (multi-line bodies are legitimate); the receiving MTA
/// strips the added dot, so benign content — including lines that really start
/// with `.` — round-trips unchanged.
fn dot_stuff_body(body: &str) -> String {
    let normalized = body.replace("\r\n", "\n");
    let mut out = String::with_capacity(normalized.len() + 8);
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        if line.starts_with('.') {
            out.push('.');
        }
        out.push_str(line);
    }
    out
}

/// Minimal plaintext SMTP conversation. Kept small and defensive.
fn smtp_send_plaintext(host: &str, port: u16, msg: &EmailMessage) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    // Defense-in-depth: strip CRLF from every header- and envelope-bound field
    // before it touches the wire so untrusted content (e.g. a goal id in the
    // Subject) cannot inject SMTP headers or commands. The body is CRLF
    // normalized *and* dot-stuffed separately below (multi-line bodies are
    // legitimate, but a lone "." line must not be able to close DATA early).
    let from = sanitize_header_value(&msg.from);
    let recipients: Vec<String> = msg.to.iter().map(|r| sanitize_header_value(r)).collect();
    let subject = sanitize_header_value(&msg.subject);

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
    writeln!(writer, "MAIL FROM:<{from}>").map_err(|e| e.to_string())?;
    expect(&mut reader, 2)?;
    for rcpt in &recipients {
        writeln!(writer, "RCPT TO:<{rcpt}>").map_err(|e| e.to_string())?;
        expect(&mut reader, 2)?;
    }
    writeln!(writer, "DATA").map_err(|e| e.to_string())?;
    expect(&mut reader, 3)?; // 354 start mail input
    write!(
        writer,
        "From: {}\r\nTo: {}\r\nSubject: {}\r\n\r\n{}\r\n.\r\n",
        from,
        recipients.join(", "),
        subject,
        dot_stuff_body(&msg.body),
    )
    .map_err(|e| e.to_string())?;
    expect(&mut reader, 2)?; // 250 accepted
    writeln!(writer, "QUIT").map_err(|e| e.to_string())?;
    Ok(())
}

// ─────────────── authenticated STARTTLS SMTP relay (issue #2631) ────────────
//
// Delivering to a microsoft.com recipient needs an AUTHENTICATED relay
// (office365 / internal). When `SMTP_USER` + `SMTP_PASS` are set,
// [`EmailNotifyChannel::from_env`] selects [`StartTlsSmtpSender`], which performs
// a real STARTTLS + AUTH LOGIN submission. The wire protocol is split from the
// TLS seam as [`smtp_converse`] so it is unit-testable without a network. See
// docs/reference/overseer-operator-notifications.md (Part B).

/// SMTP AUTH credentials, sourced from `SMTP_USER` / `SMTP_PASS` (never
/// hardcoded). Intentionally derives NO `Debug`, so the password cannot leak via
/// a `?`-formatted log line.
pub struct SmtpAuth {
    pub user: String,
    pub pass: String,
}

/// Send one SMTP command line, CRLF-terminated (RFC 5321 requires CRLF — bare LF
/// is rejected by strict relays such as office365). Kept as a `write_all` of an
/// explicit `\r\n` so the fixed line endings are unambiguous.
fn smtp_send_line<S: std::io::Write>(stream: &mut S, line: &str) -> Result<(), String> {
    stream
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(b"\r\n").map_err(|e| e.to_string())
}

/// Read one CRLF-terminated line, byte-at-a-time so we never over-read past the
/// terminator. This matters during the plaintext STARTTLS prelude: any byte
/// consumed after the `220` would be stolen from the subsequent TLS handshake.
/// The trailing CR/LF is stripped. An empty result signals end-of-stream.
fn smtp_read_line<S: std::io::Read>(stream: &mut S) -> Result<String, String> {
    let mut buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .map_err(|e| format!("smtp read: {e}"))?;
        if n == 0 || byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            buf.push(byte[0]);
        }
        if buf.len() > 8192 {
            return Err("smtp: reply line too long".to_string());
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Read a (possibly multi-line) SMTP reply, returning its 3-digit status code and
/// the aggregated text of every continuation line. Continuation lines are
/// `NNN-…`; the final line is `NNN …` (a space in the 4th column).
fn smtp_read_reply<S: std::io::Read>(stream: &mut S) -> Result<(u16, String), String> {
    let mut code = 0u16;
    let mut text = String::new();
    loop {
        let line = smtp_read_line(stream)?;
        if line.is_empty() {
            return Err("smtp: unexpected end of stream while reading reply".to_string());
        }
        code = line
            .get(..3)
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(code);
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&line);
        // A '-' in the 4th column means more lines follow; anything else ends it.
        if line.as_bytes().get(3) != Some(&b'-') {
            break;
        }
    }
    Ok((code, text))
}

/// Read a reply and require an exact status code. `step` names the phase for the
/// error; the error carries only server-sent reply text — never a credential.
fn smtp_expect<S: std::io::Read>(stream: &mut S, want: u16, step: &str) -> Result<String, String> {
    let (code, text) = smtp_read_reply(stream)?;
    if code == want {
        Ok(text)
    } else {
        Err(format!(
            "smtp {step}: expected {want}, got {code}: {}",
            text.replace('\n', " ")
        ))
    }
}

/// The plaintext STARTTLS prelude, shared by the hermetic [`smtp_converse`] and
/// the live [`StartTlsSmtpSender`]. Reads the greeting, sends EHLO, REQUIRES the
/// server to advertise STARTTLS (fail-closed — we never authenticate in the
/// clear), issues STARTTLS and consumes its `220`. On return the caller performs
/// the real TLS handshake (production) or simply continues on the same in-memory
/// stream (tests).
fn smtp_starttls_prelude<S: std::io::Read + std::io::Write>(stream: &mut S) -> Result<(), String> {
    smtp_expect(stream, 220, "greeting")?;
    smtp_send_line(stream, "EHLO simard-overseer")?;
    let caps = smtp_expect(stream, 250, "EHLO")?;
    if !caps.to_ascii_uppercase().contains("STARTTLS") {
        return Err(
            "smtp: server does not advertise STARTTLS; refusing to authenticate in the clear"
                .to_string(),
        );
    }
    smtp_send_line(stream, "STARTTLS")?;
    smtp_expect(stream, 220, "STARTTLS")?;
    Ok(())
}

/// The authenticated submission that runs AFTER STARTTLS — over the TLS stream in
/// production, over the same in-memory duplex in tests. Re-issues EHLO inside the
/// secured session, performs AUTH LOGIN (positional base64 of user then pass),
/// then MAIL / RCPT / DATA / QUIT. Header/envelope fields are sanitized and the
/// body dot-stuffed to block CWE-93 injection.
fn smtp_submit_authenticated<S: std::io::Read + std::io::Write>(
    stream: &mut S,
    msg: &EmailMessage,
    auth: &SmtpAuth,
) -> Result<(), String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;

    let from = sanitize_header_value(&msg.from);
    let recipients: Vec<String> = msg.to.iter().map(|r| sanitize_header_value(r)).collect();
    let subject = sanitize_header_value(&msg.subject);

    // EHLO again inside the secured channel.
    smtp_send_line(stream, "EHLO simard-overseer")?;
    smtp_expect(stream, 250, "EHLO(tls)")?;

    // AUTH LOGIN: the server prompts (base64 "Username:"/"Password:"); we answer
    // positionally with base64(user) then base64(pass). base64 is an ENCODING,
    // not encryption — safe only because we are now inside TLS.
    smtp_send_line(stream, "AUTH LOGIN")?;
    smtp_expect(stream, 334, "AUTH LOGIN")?;
    smtp_send_line(stream, &b64.encode(auth.user.as_bytes()))?;
    smtp_expect(stream, 334, "AUTH user")?;
    smtp_send_line(stream, &b64.encode(auth.pass.as_bytes()))?;
    smtp_expect(stream, 235, "AUTH pass")?;

    smtp_send_line(stream, &format!("MAIL FROM:<{from}>"))?;
    smtp_expect(stream, 250, "MAIL FROM")?;
    for rcpt in &recipients {
        smtp_send_line(stream, &format!("RCPT TO:<{rcpt}>"))?;
        smtp_expect(stream, 250, "RCPT TO")?;
    }
    smtp_send_line(stream, "DATA")?;
    smtp_expect(stream, 354, "DATA")?;
    let data = format!(
        "From: {}\r\nTo: {}\r\nSubject: {}\r\n\r\n{}\r\n.\r\n",
        from,
        recipients.join(", "),
        subject,
        dot_stuff_body(&msg.body),
    );
    stream
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())?;
    smtp_expect(stream, 250, "end-of-DATA")?;
    smtp_send_line(stream, "QUIT")?;
    // A courteous QUIT; the server's 221 is best-effort (it may close first).
    let _ = smtp_read_reply(stream);
    Ok(())
}

/// The pure SMTP client state machine for an authenticated submission, driven
/// over any duplex stream so it is hermetically testable. It requests STARTTLS
/// and — only once TLS is in effect — performs AUTH LOGIN (positional base64 of
/// user then pass) followed by MAIL / RCPT / DATA. It fails closed, NEVER
/// emitting `AUTH` in the clear, if the server does not offer STARTTLS.
///
/// In tests the STARTTLS step is a plain protocol step over an in-memory duplex
/// (no real handshake); production reuses the same [`smtp_starttls_prelude`] +
/// [`smtp_submit_authenticated`] helpers with a genuine TLS upgrade spliced
/// between them (see [`StartTlsSmtpSender::send`]).
pub fn smtp_converse<S: std::io::Read + std::io::Write>(
    stream: &mut S,
    msg: &EmailMessage,
    auth: &SmtpAuth,
) -> Result<(), String> {
    smtp_starttls_prelude(stream)?;
    smtp_submit_authenticated(stream, msg, auth)
}

/// Build the rustls client config for the relay TLS upgrade: the stock verifier
/// with the OS trust store (`rustls-native-certs`), falling back to the compiled
/// `webpki-roots` bundle if the OS store yields nothing. Uses the `ring` crypto
/// provider explicitly so we never depend on an ambiguous process-default
/// provider. No `dangerous_configuration`; the peer name is validated by the
/// caller against `SMTP_HOST`.
fn tls_client_config() -> Result<rustls::ClientConfig, String> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        // A partial store is fine; skip any individually-unparsable cert.
        let _ = roots.add(cert);
    }
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls versions: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(config)
}

/// A real STARTTLS + AUTH LOGIN relay sender (e.g. `smtp.office365.com:587`).
/// Selected by [`EmailNotifyChannel::from_env`] when `SMTP_USER` + `SMTP_PASS`
/// are set. Connection params and credentials are read from the environment
/// (`SMTP_HOST` / `SMTP_PORT` / `SMTP_USER` / `SMTP_PASS`) so the sender carries
/// no config of its own and no secret is ever hardcoded. The TLS handshake is the
/// only seam not exercised by the pure [`smtp_converse`] tests.
#[derive(Clone, Debug, Default)]
pub struct StartTlsSmtpSender;

impl EmailSender for StartTlsSmtpSender {
    fn send(&self, msg: &EmailMessage) -> Result<(), String> {
        use std::net::TcpStream;
        use std::time::Duration;

        let host = std::env::var("SMTP_HOST").map_err(|_| "SMTP_HOST unset".to_string())?;
        let port = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|s| s.trim().parse::<u16>().ok())
            .unwrap_or(587);
        let auth = SmtpAuth {
            user: std::env::var("SMTP_USER").map_err(|_| "SMTP_USER unset".to_string())?,
            pass: std::env::var("SMTP_PASS").map_err(|_| "SMTP_PASS unset".to_string())?,
        };

        let addr = format!("{host}:{port}");
        let mut tcp =
            TcpStream::connect(&addr).map_err(|e| format!("connect {addr} failed: {e}"))?;
        tcp.set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;

        // Plaintext prelude: greeting → EHLO → require STARTTLS → STARTTLS → 220.
        smtp_starttls_prelude(&mut tcp)?;

        // Upgrade the SAME socket to TLS, verifying the relay certificate against
        // SMTP_HOST with the standard verifier.
        let config = tls_client_config()?;
        let server_name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|e| format!("invalid SMTP_HOST '{host}': {e}"))?;
        let conn = rustls::ClientConnection::new(std::sync::Arc::new(config), server_name)
            .map_err(|e| format!("tls setup: {e}"))?;
        let mut tls = rustls::StreamOwned::new(conn, tcp);

        // Authenticated submission INSIDE TLS.
        smtp_submit_authenticated(&mut tls, msg, &auth)
    }
}

/// The [`ChannelDelivery`] variant name (`"Sent"` | `"Queued"` | `"Failed"`) with
/// NO `reason` string — safe for a one-line structured summary that must never
/// carry a secret-adjacent reason.
pub fn delivery_variant(d: &ChannelDelivery) -> &'static str {
    match d {
        ChannelDelivery::Sent => "Sent",
        ChannelDelivery::Queued { .. } => "Queued",
        ChannelDelivery::Failed { .. } => "Failed",
    }
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
            Some(sender) => match sender.send_text(&signal_wire_body(n)) {
                Ok(()) => ChannelDelivery::Sent,
                Err(e) => ChannelDelivery::Failed { reason: e },
            },
        }
    }
}

/// The exact text a Signal operator-notification puts on the wire. It wraps the
/// plain body in the reserved anti-self-ingest marker so the INBOUND Signal
/// processor deterministically skips Simard's own notification when it is synced
/// back to a linked device (independent of the fragile echo window).
///
/// The marker constant lives in `signal_conversation::gating`, which is compiled
/// only under the `signal` feature, so this wrap is feature-gated. With `signal`
/// off there is no inbound Signal processor to self-ingest and
/// `SignalNotifyChannel::from_env` wires no sender (always `Queued`), so the
/// unwrapped branch is never reached in production.
#[cfg(feature = "signal")]
fn signal_wire_body(n: &OperatorNotification) -> String {
    crate::signal_conversation::gating::wrap_operator_notification(&n.plain_text())
}

#[cfg(not(feature = "signal"))]
fn signal_wire_body(n: &OperatorNotification) -> String {
    n.plain_text()
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

    // ── security: email header / SMTP command injection (CWE-93) ─────────────

    #[test]
    fn sanitize_header_value_strips_crlf_injection() {
        // A board-derived goal id carrying CRLF must not be able to terminate the
        // Subject line and smuggle extra headers or an injected message body.
        let malicious = "goal-42\r\nBcc: attacker@evil.test\r\n\r\nInjected body";
        let cleaned = sanitize_header_value(malicious);
        assert!(!cleaned.contains('\r'), "CR must be stripped: {cleaned:?}");
        assert!(!cleaned.contains('\n'), "LF must be stripped: {cleaned:?}");
        // The legitimate leading token survives (just space-normalized).
        assert!(cleaned.starts_with("goal-42"));
    }

    #[test]
    fn sanitize_header_value_neutralizes_smtp_command_injection() {
        // A from/recipient value carrying CRLF must not inject an SMTP verb into
        // the MAIL FROM / RCPT TO envelope conversation.
        let malicious = "ops@simard.test\r\nDATA\r\nSpoofed";
        let cleaned = sanitize_header_value(malicious);
        assert!(!cleaned.contains('\r') && !cleaned.contains('\n'));
    }

    #[test]
    fn sanitize_header_value_is_length_bounded() {
        let cleaned = sanitize_header_value(&"a".repeat(10_000));
        assert!(cleaned.len() <= 512, "header value must be bounded");
    }

    #[test]
    fn sanitize_header_value_preserves_benign_input() {
        let s = "[Overseer] goal-blocked: goal abc123 needs human review";
        assert_eq!(sanitize_header_value(s), s);
    }

    #[test]
    fn sanitize_header_value_bounds_bytes_not_chars() {
        // Multi-byte UTF-8 must be bounded by *bytes* (to stay under the RFC 5322
        // 998-octet line limit) and never split a codepoint mid-char.
        let cleaned = sanitize_header_value(&"é".repeat(10_000));
        assert!(
            cleaned.len() <= 512,
            "byte length must be bounded, got {}",
            cleaned.len()
        );
        // Still valid UTF-8 with no partial char: every retained char is intact.
        assert!(cleaned.chars().all(|c| c == 'é'));
    }

    #[test]
    fn dot_stuff_body_escapes_lone_dot_line_that_would_close_data() {
        // A board-derived block reason embedding a lone "." line must not be able
        // to terminate the DATA section early and inject SMTP commands (CWE-93).
        let body = "Reason: parked\n.\nMAIL FROM:<spoof@evil.test>\nspoofed";
        let stuffed = dot_stuff_body(body);
        assert!(
            !stuffed.split("\r\n").any(|line| line == "."),
            "no bare dot line may survive: {stuffed:?}"
        );
        assert!(
            stuffed.contains("\r\n..\r\n"),
            "the lone dot line must be dot-stuffed to \"..\": {stuffed:?}"
        );
    }

    #[test]
    fn dot_stuff_body_escapes_every_leading_dot_line() {
        let stuffed = dot_stuff_body(".hidden\nnormal\n..double");
        assert!(stuffed.starts_with("..hidden"), "{stuffed:?}");
        assert!(stuffed.contains("\r\n...double"), "{stuffed:?}");
        assert!(stuffed.contains("\r\nnormal\r\n"), "{stuffed:?}");
    }

    #[test]
    fn dot_stuff_body_preserves_benign_multiline_and_normalizes_crlf() {
        // Legitimate multi-line content round-trips (CRLF-normalized) untouched.
        assert_eq!(
            dot_stuff_body("line one\nline two\nline three"),
            "line one\r\nline two\r\nline three"
        );
        // Mixed CRLF input is normalized, and only leading-dot lines are escaped.
        assert_eq!(dot_stuff_body("a\r\n.b\r\nc"), "a\r\n..b\r\nc");
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

    // ── Part A: Signal notifications carry the anti-self-ingest marker (#2631) ─

    #[cfg(feature = "signal")]
    #[test]
    fn signal_channel_wraps_body_with_operator_marker() {
        use crate::signal_conversation::gating::OPERATOR_NOTIFY_MARKER;
        use std::sync::Arc;

        // A recording SignalSender captures exactly what the channel would put on
        // the wire, so we can assert the outbound notification is wrapped.
        #[derive(Default)]
        struct RecordingSignal {
            sent: Mutex<Vec<String>>,
        }
        impl SignalSender for Arc<RecordingSignal> {
            fn send_text(&self, text: &str) -> Result<(), String> {
                self.sent.lock().unwrap().push(text.to_string());
                Ok(())
            }
        }

        let rec = Arc::new(RecordingSignal::default());
        let ch = SignalNotifyChannel::new(Some(Box::new(Arc::clone(&rec))));

        let out = ch.deliver(&sample());
        assert_eq!(out, ChannelDelivery::Sent);

        let sent = rec.sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "exactly one Signal message");
        assert!(
            sent[0].contains(OPERATOR_NOTIFY_MARKER),
            "a Signal notification MUST carry the anti-self-ingest marker so the \
             inbound processor skips its own echo: {:?}",
            sent[0]
        );
        // The operator-readable problem text is still present.
        assert!(
            sent[0].contains("distillation parse-failure"),
            "the human-readable body must be preserved: {:?}",
            sent[0]
        );
    }

    // ── Part B: authenticated STARTTLS + AUTH LOGIN SMTP (#2631) ──────────────

    /// A scripted in-memory duplex: serves pre-canned server reply bytes on `read`
    /// (SMTP is lock-step, so ordered replies suffice) and captures every client
    /// byte on `write`. Lets [`smtp_converse`] be exercised with no network.
    struct ScriptedDuplex {
        to_client: std::io::Cursor<Vec<u8>>,
        from_client: Vec<u8>,
    }
    impl ScriptedDuplex {
        fn new(server_replies: &[&str]) -> Self {
            Self {
                to_client: std::io::Cursor::new(server_replies.concat().into_bytes()),
                from_client: Vec::new(),
            }
        }
        fn written(&self) -> String {
            String::from_utf8_lossy(&self.from_client).into_owned()
        }
    }
    impl std::io::Read for ScriptedDuplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.to_client.read(buf)
        }
    }
    impl std::io::Write for ScriptedDuplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.from_client.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn office365_message() -> EmailMessage {
        EmailMessage {
            from: "overseer@contoso.onmicrosoft.com".to_string(),
            to: vec!["rysweet@microsoft.com".to_string()],
            subject: "[Overseer] goal-blocked: g-1 needs human review".to_string(),
            body: "Goal `g-1` is blocked and needs human review.".to_string(),
        }
    }

    #[test]
    fn smtp_converse_negotiates_starttls_then_auth_login_and_sends() {
        // Happy path: greeting → EHLO(STARTTLS) → STARTTLS → EHLO(AUTH) →
        // AUTH LOGIN → base64(user) → base64(pass) → MAIL/RCPT/DATA → QUIT.
        let server = [
            "220 smtp.example.test ESMTP\r\n",
            "250-smtp.example.test\r\n250-STARTTLS\r\n250 AUTH LOGIN\r\n",
            "220 ready to start TLS\r\n",
            "250-smtp.example.test\r\n250 AUTH LOGIN\r\n",
            "334 VXNlcm5hbWU6\r\n", // base64("Username:")
            "334 UGFzc3dvcmQ6\r\n", // base64("Password:")
            "235 2.7.0 Authentication successful\r\n",
            "250 2.1.0 Sender OK\r\n",
            "250 2.1.5 Recipient OK\r\n",
            "354 End data with <CR><LF>.<CR><LF>\r\n",
            "250 2.0.0 OK queued\r\n",
            "221 2.0.0 Service closing\r\n",
        ];
        let mut duplex = ScriptedDuplex::new(&server);
        let msg = office365_message();
        // Single-character credentials so the expected base64 is trivial and no
        // real secret appears: base64("u") = "dQ==", base64("p") = "cA==".
        let auth = SmtpAuth {
            user: "u".to_string(),
            pass: "p".to_string(),
        };

        let res = smtp_converse(&mut duplex, &msg, &auth);
        assert!(
            res.is_ok(),
            "authenticated submission should succeed: {res:?}"
        );

        let wire = duplex.written();
        assert!(
            wire.contains("STARTTLS\r\n"),
            "must request STARTTLS: {wire:?}"
        );
        assert!(wire.contains("AUTH LOGIN\r\n"), "must AUTH LOGIN: {wire:?}");
        assert!(
            wire.contains("dQ=="),
            "must send base64(user) positionally after the first 334: {wire:?}"
        );
        assert!(
            wire.contains("cA=="),
            "must send base64(pass) positionally after the second 334: {wire:?}"
        );
        assert!(
            wire.contains("MAIL FROM:<overseer@contoso.onmicrosoft.com>"),
            "envelope sender: {wire:?}"
        );
        assert!(
            wire.contains("RCPT TO:<rysweet@microsoft.com>"),
            "envelope recipient: {wire:?}"
        );
        assert!(wire.contains("QUIT"), "must close with QUIT: {wire:?}");
    }

    #[test]
    fn smtp_converse_fails_closed_and_leaks_no_auth_without_starttls() {
        // The server does NOT advertise STARTTLS. With credentials configured the
        // sender MUST fail rather than authenticate in the clear, and MUST NOT
        // write any AUTH bytes (base64 is encoding, not encryption).
        let server = [
            "220 smtp.example.test ESMTP\r\n",
            "250-smtp.example.test\r\n250 AUTH LOGIN\r\n", // EHLO reply, NO STARTTLS
        ];
        let mut duplex = ScriptedDuplex::new(&server);
        let msg = office365_message();
        let auth = SmtpAuth {
            user: "u".to_string(),
            pass: "p".to_string(),
        };

        let res = smtp_converse(&mut duplex, &msg, &auth);
        assert!(
            res.is_err(),
            "must fail closed when STARTTLS is unavailable, not send credentials in clear"
        );

        let wire = duplex.written();
        assert!(
            !wire.contains("AUTH"),
            "must NOT emit AUTH without TLS: {wire:?}"
        );
        assert!(
            !wire.contains("dQ=="),
            "must NOT emit the base64 username in clear: {wire:?}"
        );
        assert!(
            !wire.contains("cA=="),
            "must NOT emit the base64 password in clear: {wire:?}"
        );
    }

    #[test]
    fn email_config_use_authenticated_requires_both_user_and_pass() {
        // office365 worked example: full auth config selects the STARTTLS sender.
        let both = EmailConfig::from_lookup(|k| match k {
            "SIMARD_OVERSEER_EMAIL_TO" => Some("rysweet@microsoft.com".to_string()),
            "SIMARD_OVERSEER_EMAIL_FROM" => Some("overseer@contoso.onmicrosoft.com".to_string()),
            "SMTP_HOST" => Some("smtp.office365.com".to_string()),
            "SMTP_PORT" => Some("587".to_string()),
            "SMTP_USER" => Some("overseer@contoso.onmicrosoft.com".to_string()),
            "SMTP_PASS" => Some("<app-password-placeholder>".to_string()),
            _ => None,
        });
        assert!(both.is_configured());
        assert_eq!(both.port, 587);
        assert!(
            both.use_authenticated(),
            "SMTP_USER + SMTP_PASS ⇒ authenticated STARTTLS sender"
        );

        // Missing the password ⇒ plaintext sender selection.
        let no_pass = EmailConfig::from_lookup(|k| match k {
            "SIMARD_OVERSEER_EMAIL_TO" => Some("ops@example.test".to_string()),
            "SIMARD_OVERSEER_EMAIL_FROM" => Some("o@example.test".to_string()),
            "SMTP_HOST" => Some("localhost".to_string()),
            "SMTP_USER" => Some("u".to_string()),
            _ => None,
        });
        assert!(
            !no_pass.use_authenticated(),
            "without SMTP_PASS the authenticated relay must not be selected"
        );
    }

    #[test]
    fn start_tls_sender_is_an_email_sender() {
        // Compile-time contract: the authenticated relay implements the injectable
        // EmailSender seam (the live TLS send itself is the untested seam).
        let _sender: Box<dyn EmailSender> = Box::new(StartTlsSmtpSender);
    }

    #[test]
    fn smtp_converse_sanitizes_header_and_body_injection_in_auth_path() {
        // A CRLF-injected subject and a lone-dot body line must be neutralized in
        // the authenticated submission too: the STARTTLS path reuses the shared
        // sanitize_header_value + dot_stuff_body hardening (CWE-93).
        let server = [
            "220 smtp.example.test ESMTP\r\n",
            "250-smtp.example.test\r\n250-STARTTLS\r\n250 AUTH LOGIN\r\n",
            "220 ready to start TLS\r\n",
            "250-smtp.example.test\r\n250 AUTH LOGIN\r\n",
            "334 VXNlcm5hbWU6\r\n",
            "334 UGFzc3dvcmQ6\r\n",
            "235 2.7.0 Authentication successful\r\n",
            "250 2.1.0 Sender OK\r\n",
            "250 2.1.5 Recipient OK\r\n",
            "354 End data\r\n",
            "250 2.0.0 OK queued\r\n",
            "221 2.0.0 bye\r\n",
        ];
        let mut duplex = ScriptedDuplex::new(&server);
        let msg = EmailMessage {
            from: "overseer@contoso.onmicrosoft.com".to_string(),
            to: vec!["rysweet@microsoft.com".to_string()],
            subject: "goal-blocked\r\nBcc: attacker@evil.test".to_string(),
            body: "Goal blocked.\n.\nMAIL FROM:<spoof@evil.test>".to_string(),
        };
        let auth = SmtpAuth {
            user: "u".to_string(),
            pass: "p".to_string(),
        };
        assert!(smtp_converse(&mut duplex, &msg, &auth).is_ok());
        let wire = duplex.written();
        // The injected header cannot begin its own line in the DATA section.
        assert!(
            !wire.contains("\r\nBcc: attacker@evil.test"),
            "header injection neutralized: {wire:?}"
        );
        // The lone-dot body line is dot-stuffed so it cannot close DATA early and
        // smuggle the spoofed MAIL FROM as an SMTP command.
        assert!(
            !wire.contains("\r\n.\r\nMAIL FROM:<spoof@evil.test>"),
            "body dot-stuffed: {wire:?}"
        );
    }

    // ── Part C: all_sent vs dispatched, observably distinguishable (#2631) ────

    #[test]
    fn delivery_variant_names_omit_the_reason_string() {
        assert_eq!(delivery_variant(&ChannelDelivery::Sent), "Sent");
        assert_eq!(
            delivery_variant(&ChannelDelivery::Queued {
                reason: "SMTP not configured".to_string()
            }),
            "Queued"
        );
        assert_eq!(
            delivery_variant(&ChannelDelivery::Failed {
                reason: "smtp boom".to_string()
            }),
            "Failed"
        );
        // The bare variant name must never carry the (secret-adjacent) reason.
        assert!(
            !delivery_variant(&ChannelDelivery::Failed {
                reason: "topsecret".to_string()
            })
            .contains("topsecret")
        );
    }

    #[test]
    fn dispatched_but_not_all_sent_when_signal_delivers_and_email_queues() {
        // The crux of Part C: Signal (the wired primary path) reaches the operator,
        // so the escalation is DISPATCHED even though email is not yet configured —
        // it is never considered "lost". `all_sent()` honestly reports the gap.
        let notifier = DualChannelNotifier::new(vec![
            Box::new(RecordingChannel {
                name: "signal".to_string(),
                outcome: Some(ChannelDelivery::Sent),
                ..Default::default()
            }),
            Box::new(RecordingChannel {
                name: "email".to_string(),
                outcome: Some(ChannelDelivery::Queued {
                    reason: "SMTP not configured".to_string(),
                }),
                ..Default::default()
            }),
        ]);
        let report = notifier.notify(&sample());
        assert!(report.dispatched(), "reached the operator via Signal");
        assert!(!report.all_sent(), "email did not deliver");

        let by = |name: &str| {
            report
                .per_channel
                .iter()
                .find(|(c, _)| c == name)
                .map(|(_, d)| d.clone())
                .unwrap()
        };
        assert_eq!(by("signal"), ChannelDelivery::Sent);
        assert!(
            matches!(by("email"), ChannelDelivery::Queued { reason } if reason.contains("SMTP")),
            "the queued email must identify the channel AND the reason"
        );
    }

    #[test]
    fn all_sent_true_only_when_both_channels_deliver() {
        let notifier = DualChannelNotifier::new(vec![
            Box::new(RecordingChannel {
                name: "signal".to_string(),
                outcome: Some(ChannelDelivery::Sent),
                ..Default::default()
            }),
            Box::new(RecordingChannel {
                name: "email".to_string(),
                outcome: Some(ChannelDelivery::Sent),
                ..Default::default()
            }),
        ]);
        let report = notifier.notify(&sample());
        assert!(report.dispatched());
        assert!(
            report.all_sent(),
            "all_sent is true only when every channel delivered"
        );
    }
}
