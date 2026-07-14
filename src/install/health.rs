use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HealthCheckErrorKind {
    Spawn,
    NonzeroExit,
    Timeout,
    Transport,
    MalformedResponse,
    Unhealthy,
}

#[derive(Debug)]
pub(crate) struct HealthCheckError {
    kind: HealthCheckErrorKind,
    message: String,
}

impl HealthCheckError {
    fn new(kind: HealthCheckErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> HealthCheckErrorKind {
        self.kind
    }
}

impl Display for HealthCheckError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for HealthCheckError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct HealthResponse {
    pub healthy: bool,
}

#[cfg(test)]
pub(crate) fn run(
    executable: &Path,
    timeout: Duration,
) -> Result<HealthResponse, HealthCheckError> {
    run_with_args(executable, &[], timeout)
}

pub(crate) fn run_with_args(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<HealthResponse, HealthCheckError> {
    let mut child = Command::new(executable)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            HealthCheckError::new(
                HealthCheckErrorKind::Spawn,
                format!("health check failed to start: {error}"),
            )
        })?;
    wait_for_exit(&mut child, timeout)?;
    let output = child.wait_with_output().map_err(|error| {
        HealthCheckError::new(
            HealthCheckErrorKind::Transport,
            format!("health check output transport failed: {error}"),
        )
    })?;
    validate_output(output)
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Result<(), HealthCheckError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5));
            }
            Ok(None) => {
                terminate(child);
                return Err(HealthCheckError::new(
                    HealthCheckErrorKind::Timeout,
                    "health check timed out",
                ));
            }
            Err(error) => {
                terminate(child);
                return Err(HealthCheckError::new(
                    HealthCheckErrorKind::Transport,
                    format!("health check status transport failed: {error}"),
                ));
            }
        }
    }
}

fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn validate_output(output: Output) -> Result<HealthResponse, HealthCheckError> {
    if !output.status.success() {
        return Err(HealthCheckError::new(
            HealthCheckErrorKind::NonzeroExit,
            format!(
                "health check exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    if output.stdout.is_empty() {
        return Err(HealthCheckError::new(
            HealthCheckErrorKind::Transport,
            "health check returned an empty response",
        ));
    }
    let response: HealthResponse = serde_json::from_slice(&output.stdout).map_err(|error| {
        HealthCheckError::new(
            HealthCheckErrorKind::MalformedResponse,
            format!("health check response is malformed: {error}"),
        )
    })?;
    if !response.healthy {
        return Err(HealthCheckError::new(
            HealthCheckErrorKind::Unhealthy,
            "health check reported unhealthy",
        ));
    }
    Ok(response)
}
