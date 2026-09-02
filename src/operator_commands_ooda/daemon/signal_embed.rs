//! In-process hosting of the Signal operator channel inside the OODA daemon
//! (converge-to-single-daemon).
//!
//! Historically the Signal channel ran as its OWN long-lived process
//! (`simard signal run`, deployed as a separate `simard-signal.service`), even
//! though every other long-running concern the daemon owns — the embedded
//! dashboard, the memory IPC server, the cognitive-thread scheduler (Creative
//! Ideas / Maintenance / Engineer-Log), the acting Overseer, and the daily
//! Journal — already runs IN this process on a panic-isolated background
//! thread/task. Signal was the lone outlier: out-of-process it had to reach
//! cognitive memory over the IPC socket instead of the in-process writer, and
//! it doubled the deploy/supervision surface (a second unit, a second journal,
//! a second restart cadence).
//!
//! This module folds Signal into the ONE daemon. It runs the channel on a
//! dedicated background OS thread that owns a current-thread Tokio runtime —
//! the same "background thread, never inline, panic-isolated" convention the
//! cognitive threads use. The thread runs a reconnect-with-backoff loop so a
//! flapping `signal-cli` daemon (the reason the standalone service used
//! `Restart=always`) recovers in-process without taking the OODA loop down,
//! and each attempt is wrapped in [`std::panic::catch_unwind`] so a bug in the
//! channel can never crash or stall the authoritative cycle.
//!
//! Gating:
//!   * `SIMARD_SIGNAL_ENABLED` — DEFAULT-ON, opt-out (consistent with the
//!     default-ON dashboard / Creative-Ideas / Overseer / Journal). Only an
//!     explicit falsey value disables it.
//!   * The channel stays DORMANT (logged, skipped, never a daemon-startup
//!     failure) until a usable `[signal]` config table is present — matching
//!     the standalone `simard signal run` contract.
//!
//! The standalone `simard signal run` subcommand is retained for
//! standalone/debug use; the DEPLOYED path is this embedded thread.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Parse the `SIMARD_SIGNAL_ENABLED` gate. DEFAULT-ON: unset, empty, or any
/// value that is not an explicit falsey token enables the embedded channel.
/// Only `0`, `false`, `no`, or `off` (case-insensitive, trimmed) disable it —
/// symmetric with how the Creative-Ideas / Journal / Overseer gates treat their
/// opt-out env vars.
pub fn signal_embed_enabled(raw: Option<&str>) -> bool {
    match raw {
        None => true,
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
    }
}

/// Read `SIMARD_SIGNAL_ENABLED` from the process environment through
/// [`signal_embed_enabled`].
pub fn signal_embed_enabled_from_env() -> bool {
    signal_embed_enabled(std::env::var("SIMARD_SIGNAL_ENABLED").ok().as_deref())
}

/// Handle to the embedded Signal channel's background thread. Held for the
/// daemon's lifetime; the thread observes the shared shutdown flag and exits on
/// its own, so dropping this guard does NOT block (the daemon's own shutdown
/// path sets the flag). When the channel is disabled, dormant, or compiled out
/// this holds nothing.
#[must_use = "hold this guard for the daemon's lifetime; the embedded Signal thread reads the shared shutdown flag"]
pub struct EmbeddedSignal {
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl EmbeddedSignal {
    /// The inert guard: no thread. Used when the channel is disabled, dormant,
    /// or compiled out.
    fn inert() -> Self {
        Self { _thread: None }
    }
}

/// Spawn the Signal operator channel on an in-process background thread,
/// mirroring the embedded dashboard's "share one process" model. Returns a
/// guard to keep for the daemon's lifetime.
///
/// This never returns `Err`: a disabled gate, an absent/unparseable `[signal]`
/// config, or a thread-spawn failure all degrade to an inert guard with a
/// logged reason, because the Signal channel must never gate OODA daemon
/// startup.
#[cfg(feature = "signal")]
pub fn spawn_embedded_signal_channel(
    state_root: &std::path::Path,
    shutdown: Arc<AtomicBool>,
) -> EmbeddedSignal {
    use crate::signal_conversation::SignalConfig;

    use super::helpers::daemon_log;

    if !signal_embed_enabled_from_env() {
        daemon_log(
            state_root,
            "[simard] OODA daemon: Signal channel DISABLED (SIMARD_SIGNAL_ENABLED opt-out)",
        );
        return EmbeddedSignal::inert();
    }

    // Dormant until configured — never a startup failure. Any load error
    // (missing endpoint/account, unparseable table) is treated as "not
    // configured yet"; the operator can add the `[signal]` table and restart.
    match SignalConfig::load_from(state_root) {
        Ok(config) => {
            if config.allowlist.is_empty() {
                daemon_log(
                    state_root,
                    "[simard] OODA daemon: Signal channel — WARNING: operator allowlist is empty; \
                     the channel is fail-closed and will accept no commands until \
                     [signal].allowlist (or SIMARD_SIGNAL_ALLOWLIST) is set",
                );
            }
            daemon_log(
                state_root,
                &format!(
                    "[simard] OODA daemon: Signal channel ENABLED (embedded) — endpoint {}, \
                     account {}, {} allowlisted operator(s)",
                    config.endpoint,
                    config.account,
                    config.allowlist.len()
                ),
            );
        }
        Err(e) => {
            daemon_log(
                state_root,
                &format!(
                    "[simard] OODA daemon: Signal channel DORMANT (no usable [signal] config: {e}); \
                     configure [signal] and restart, or run `simard signal run` standalone"
                ),
            );
            return EmbeddedSignal::inert();
        }
    }

    let task_state_root = state_root.to_path_buf();
    let thread = std::thread::Builder::new()
        .name("simard-signal".to_string())
        .spawn(move || signal_supervisor_loop(task_state_root, shutdown));

    match thread {
        Ok(handle) => EmbeddedSignal {
            _thread: Some(handle),
        },
        Err(e) => {
            daemon_log(
                state_root,
                &format!(
                    "[simard] OODA daemon: Signal channel thread spawn failed: {e}; \
                     channel not started"
                ),
            );
            EmbeddedSignal::inert()
        }
    }
}

/// The supervised reconnect-with-backoff loop, run on the dedicated Signal
/// thread. Owns a current-thread Tokio runtime and drives
/// [`crate::signal_conversation::run`] to completion, reconnecting until the
/// daemon shuts down. Each attempt is panic-isolated.
#[cfg(feature = "signal")]
fn signal_supervisor_loop(state_root: std::path::PathBuf, shutdown: Arc<AtomicBool>) {
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use crate::signal_conversation::SignalConfig;

    use super::helpers::daemon_log;

    const MIN_BACKOFF: Duration = Duration::from_secs(5);
    const MAX_BACKOFF: Duration = Duration::from_secs(300);

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            daemon_log(
                &state_root,
                &format!(
                    "[simard] OODA daemon: Signal channel runtime build failed: {e}; \
                     channel not started"
                ),
            );
            return;
        }
    };

    let mut backoff = MIN_BACKOFF;

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        // Reload config each attempt so an operator's `[signal]` edits take
        // effect on the next reconnect without a full daemon restart.
        let config = match SignalConfig::load_from(&state_root) {
            Ok(c) => c,
            Err(e) => {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] OODA daemon: Signal channel — config no longer usable ({e}); \
                         retrying in {}s",
                        backoff.as_secs()
                    ),
                );
                if shutdown_aware_sleep(backoff, &shutdown) {
                    break;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };

        // `run` returns Ok when the signal-cli socket closes and Err on
        // connect / provider failure; either way we back off and reconnect
        // unless shutting down. catch_unwind isolates a panic in the channel so
        // it can never propagate out of this thread and abort the daemon.
        let result =
            std::panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(signal_run(config))));

        match result {
            Ok(Ok(())) => {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] OODA daemon: Signal channel closed (signal-cli socket ended); \
                         reconnecting in {}s",
                        MIN_BACKOFF.as_secs()
                    ),
                );
                backoff = MIN_BACKOFF;
            }
            Ok(Err(e)) => {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] OODA daemon: Signal channel error: {e}; reconnecting in {}s",
                        backoff.as_secs()
                    ),
                );
            }
            Err(_) => {
                daemon_log(
                    &state_root,
                    &format!(
                        "[simard] OODA daemon: Signal channel PANICKED (isolated — daemon \
                         unaffected); reconnecting in {}s",
                        backoff.as_secs()
                    ),
                );
            }
        }

        if shutdown_aware_sleep(backoff, &shutdown) {
            break;
        }
        // A clean close reset backoff to MIN above; grow it for error/panic.
        if backoff < MAX_BACKOFF {
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    daemon_log(
        &state_root,
        "[simard] OODA daemon: Signal channel thread exiting (daemon shutdown)",
    );
}

/// Thin async wrapper so [`signal_supervisor_loop`] can `block_on` the channel
/// entrypoint by value.
#[cfg(feature = "signal")]
async fn signal_run(
    config: crate::signal_conversation::SignalConfig,
) -> crate::error::SimardResult<()> {
    crate::signal_conversation::run(config).await
}

/// Feature-off build: the Signal code is compiled out, so there is nothing to
/// host. Return the inert guard after logging, so the daemon behaves identically
/// minus the channel.
#[cfg(not(feature = "signal"))]
pub fn spawn_embedded_signal_channel(
    state_root: &std::path::Path,
    _shutdown: Arc<AtomicBool>,
) -> EmbeddedSignal {
    super::helpers::daemon_log(
        state_root,
        "[simard] OODA daemon: Signal channel not compiled into this build \
         (built without the `signal` feature)",
    );
    EmbeddedSignal::inert()
}

/// Sleep for `total`, waking early (returning `true`) if `shutdown` is set. Polls
/// in ≤1s slices so a SIGTERM during a long backoff is honoured promptly. Sync
/// (runs on the dedicated Signal thread, never inside an async context).
#[cfg(feature = "signal")]
fn shutdown_aware_sleep(total: std::time::Duration, shutdown: &AtomicBool) -> bool {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    let slice = Duration::from_secs(1);
    let mut remaining = total;
    while !remaining.is_zero() {
        if shutdown.load(Ordering::SeqCst) {
            return true;
        }
        let step = remaining.min(slice);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
    shutdown.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_by_default_when_unset() {
        assert!(signal_embed_enabled(None));
    }

    #[test]
    fn enabled_for_truthy_and_arbitrary_values() {
        for v in ["1", "true", "yes", "on", "TRUE", " On ", "anything"] {
            assert!(signal_embed_enabled(Some(v)), "{v:?} should enable");
        }
    }

    #[test]
    fn disabled_only_for_explicit_falsey() {
        for v in ["0", "false", "no", "off", "FALSE", " Off ", "NO"] {
            assert!(!signal_embed_enabled(Some(v)), "{v:?} should disable");
        }
    }

    #[test]
    fn empty_string_enables() {
        // Empty/whitespace is not an explicit opt-out, so default-ON wins.
        assert!(signal_embed_enabled(Some("")));
        assert!(signal_embed_enabled(Some("   ")));
    }
}
