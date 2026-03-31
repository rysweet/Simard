//! Remote session management for distributed agent orchestration.
//!
//! A `RemoteSession` represents a coding agent running on a remote VM
//! provisioned via azlin. Lifecycle: Creating -> Running -> Transferring
//! -> Completed (or Failed). Each session is isolated by agent_name.

use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{SimardError, SimardResult};
use crate::remote_azlin::{
    AzlinConfig, AzlinExecutor, AzlinVm, azlin_create, azlin_destroy, azlin_ssh,
};

/// Status of a remote session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteStatus {
    /// VM is being provisioned.
    Creating,
    /// Agent is deployed and operating.
    Running,
    /// Memory is being transferred to or from the VM.
    Transferring,
    /// Session completed successfully.
    Completed,
    /// Session failed with the given reason.
    Failed(String),
}

impl Display for RemoteStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Creating => f.write_str("creating"),
            Self::Running => f.write_str("running"),
            Self::Transferring => f.write_str("transferring"),
            Self::Completed => f.write_str("completed"),
            Self::Failed(reason) => write!(f, "failed: {reason}"),
        }
    }
}

/// Configuration for creating a remote session.
#[derive(Clone, Debug)]
pub struct RemoteConfig {
    /// Name for the remote VM (must be unique across active sessions).
    pub vm_name: String,
    /// Agent identity name for this remote session.
    pub agent_name: String,
    /// azlin configuration for VM creation.
    pub azlin_config: AzlinConfig,
}

/// A remote coding session running on a provisioned VM.
#[derive(Clone, Debug)]
pub struct RemoteSession {
    /// The VM name as registered with azlin.
    pub vm_name: String,
    /// The public IP address of the VM.
    pub ip_address: String,
    /// The agent identity running on this VM.
    pub agent_name: String,
    /// Current lifecycle status.
    pub status: RemoteStatus,
    /// When the session was created (unix epoch seconds).
    pub created_at: u64,
    /// The underlying azlin VM handle.
    vm: AzlinVm,
}

impl Display for RemoteSession {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RemoteSession(vm={}, agent={}, ip={}, status={})",
            self.vm_name, self.agent_name, self.ip_address, self.status
        )
    }
}

impl RemoteSession {
    /// Whether the session is in a terminal state (completed or failed).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            RemoteStatus::Completed | RemoteStatus::Failed(_)
        )
    }

    /// Whether the session's VM is reachable (creating or running or transferring).
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            RemoteStatus::Creating | RemoteStatus::Running | RemoteStatus::Transferring
        )
    }

    /// Get a reference to the underlying azlin VM.
    pub fn vm(&self) -> &AzlinVm {
        &self.vm
    }
}

/// Create a new remote session by provisioning a VM via azlin.
///
/// On success, the session starts in `Running` status with the VM's IP
/// address populated. On failure, returns a descriptive error.
pub fn create_remote_session(
    config: &RemoteConfig,
    executor: &dyn AzlinExecutor,
) -> SimardResult<RemoteSession> {
    if config.vm_name.is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: "vm_name".to_string(),
            value: String::new(),
            help: "remote session VM name cannot be empty".to_string(),
        });
    }
    if config.agent_name.is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: "agent_name".to_string(),
            value: String::new(),
            help: "remote session agent name cannot be empty".to_string(),
        });
    }

    let vm = azlin_create(&config.vm_name, &config.azlin_config, executor)?;
    let now = current_epoch_seconds()?;

    Ok(RemoteSession {
        vm_name: vm.name.clone(),
        ip_address: vm.ip.clone(),
        agent_name: config.agent_name.clone(),
        status: RemoteStatus::Running,
        created_at: now,
        vm,
    })
}

/// Deploy an agent binary to a remote session via SCP.
///
/// Uses the azlin SSH connection to copy the binary and make it executable.
/// The session must be in `Running` status.
pub fn deploy_agent(
    session: &RemoteSession,
    binary_path: &Path,
    executor: &dyn AzlinExecutor,
) -> SimardResult<()> {
    if session.status != RemoteStatus::Running {
        return Err(SimardError::BridgeTransportError {
            bridge: "remote-session".to_string(),
            reason: format!(
                "cannot deploy to session '{}' in status '{}'",
                session.vm_name, session.status
            ),
        });
    }

    if !binary_path
        .as_os_str()
        .to_string_lossy()
        .ends_with("simard")
    {
        return Err(SimardError::InvalidConfigValue {
            key: "binary_path".to_string(),
            value: binary_path.display().to_string(),
            help: "binary path must point to a simard binary".to_string(),
        });
    }

    // Use scp via the VM's IP to deploy the binary.
    let remote_dest = format!("azureuser@{}:/usr/local/bin/simard", session.ip_address);
    executor.run(&["scp", &binary_path.display().to_string(), &remote_dest])?;

    // Make the binary executable.
    let ssh_target = format!("azureuser@{}", session.ip_address);
    executor.run(&["ssh", &ssh_target, "chmod", "+x", "/usr/local/bin/simard"])?;

    Ok(())
}

/// Establish a PTY connection to a remote session.
///
/// Returns the SSH command string that can be used to connect to the VM.
/// The session must be in `Running` status.
pub fn establish_pty(
    session: &RemoteSession,
    executor: &dyn AzlinExecutor,
) -> SimardResult<String> {
    if session.status != RemoteStatus::Running {
        return Err(SimardError::BridgeTransportError {
            bridge: "remote-session".to_string(),
            reason: format!(
                "cannot establish PTY to session '{}' in status '{}'",
                session.vm_name, session.status
            ),
        });
    }

    azlin_ssh(&session.vm, executor)
}

/// Destroy a remote session by tearing down its VM.
///
/// Transitions the session to `Completed` on success or `Failed` on error.
/// The session's VM is destroyed regardless of current status (cleanup).
pub fn destroy_session(
    session: &mut RemoteSession,
    executor: &dyn AzlinExecutor,
) -> SimardResult<()> {
    if session.is_terminal() {
        return Err(SimardError::BridgeTransportError {
            bridge: "remote-session".to_string(),
            reason: format!(
                "session '{}' is already in terminal status '{}'",
                session.vm_name, session.status
            ),
        });
    }

    match azlin_destroy(&session.vm, executor) {
        Ok(()) => {
            session.status = RemoteStatus::Completed;
            Ok(())
        }
        Err(e) => {
            session.status = RemoteStatus::Failed(e.to_string());
            Err(e)
        }
    }
}

/// Transition a session to the Transferring state.
///
/// Used before memory export/import operations to signal that the session
/// is temporarily busy with data transfer.
pub fn begin_transfer(session: &mut RemoteSession) -> SimardResult<()> {
    if session.status != RemoteStatus::Running {
        return Err(SimardError::BridgeTransportError {
            bridge: "remote-session".to_string(),
            reason: format!(
                "cannot begin transfer for session '{}' in status '{}'",
                session.vm_name, session.status
            ),
        });
    }
    session.status = RemoteStatus::Transferring;
    Ok(())
}

/// Transition a session back to Running after a transfer completes.
pub fn end_transfer(session: &mut RemoteSession) -> SimardResult<()> {
    if session.status != RemoteStatus::Transferring {
        return Err(SimardError::BridgeTransportError {
            bridge: "remote-session".to_string(),
            reason: format!(
                "cannot end transfer for session '{}' in status '{}'",
                session.vm_name, session.status
            ),
        });
    }
    session.status = RemoteStatus::Running;
    Ok(())
}

fn current_epoch_seconds() -> SimardResult<u64> {
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
        SimardError::ClockBeforeUnixEpoch {
            reason: e.to_string(),
        }
    })?;
    Ok(duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_azlin::MockAzlinExecutor;

    fn mock_executor() -> MockAzlinExecutor {
        MockAzlinExecutor::new(|args| match args.first().copied() {
            Some("create") => Ok("name=test-vm\nip=10.0.0.1\nstatus=running\n".to_string()),
            Some("ssh") => Ok("ssh azureuser@10.0.0.1".to_string()),
            Some("destroy") => Ok(String::new()),
            Some("scp") => Ok(String::new()),
            _ => Ok(String::new()),
        })
    }

    fn test_config() -> RemoteConfig {
        RemoteConfig {
            vm_name: "test-vm".to_string(),
            agent_name: "remote-engineer-1".to_string(),
            azlin_config: AzlinConfig::default(),
        }
    }

    #[test]
    fn create_session_succeeds() {
        let executor = mock_executor();
        let session = create_remote_session(&test_config(), &executor).unwrap();
        assert_eq!(session.vm_name, "test-vm");
        assert_eq!(session.ip_address, "10.0.0.1");
        assert_eq!(session.agent_name, "remote-engineer-1");
        assert_eq!(session.status, RemoteStatus::Running);
        assert!(!session.is_terminal());
        assert!(session.is_active());
    }

    #[test]
    fn create_rejects_empty_vm_name() {
        let executor = mock_executor();
        let mut config = test_config();
        config.vm_name = String::new();
        let err = create_remote_session(&config, &executor).unwrap_err();
        assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
    }

    #[test]
    fn create_rejects_empty_agent_name() {
        let executor = mock_executor();
        let mut config = test_config();
        config.agent_name = String::new();
        let err = create_remote_session(&config, &executor).unwrap_err();
        assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
    }

    #[test]
    fn establish_pty_returns_ssh_command() {
        let executor = mock_executor();
        let session = create_remote_session(&test_config(), &executor).unwrap();
        let cmd = establish_pty(&session, &executor).unwrap();
        assert!(cmd.contains("azureuser@10.0.0.1"));
    }

    #[test]
    fn destroy_session_transitions_to_completed() {
        let executor = mock_executor();
        let mut session = create_remote_session(&test_config(), &executor).unwrap();
        destroy_session(&mut session, &executor).unwrap();
        assert_eq!(session.status, RemoteStatus::Completed);
        assert!(session.is_terminal());
    }

    #[test]
    fn destroy_already_terminal_fails() {
        let executor = mock_executor();
        let mut session = create_remote_session(&test_config(), &executor).unwrap();
        destroy_session(&mut session, &executor).unwrap();
        let err = destroy_session(&mut session, &executor).unwrap_err();
        assert!(matches!(err, SimardError::BridgeTransportError { .. }));
    }

    #[test]
    fn transfer_lifecycle() {
        let executor = mock_executor();
        let mut session = create_remote_session(&test_config(), &executor).unwrap();
        assert_eq!(session.status, RemoteStatus::Running);

        begin_transfer(&mut session).unwrap();
        assert_eq!(session.status, RemoteStatus::Transferring);
        assert!(session.is_active());

        end_transfer(&mut session).unwrap();
        assert_eq!(session.status, RemoteStatus::Running);
    }

    #[test]
    fn begin_transfer_rejects_non_running() {
        let executor = mock_executor();
        let mut session = create_remote_session(&test_config(), &executor).unwrap();
        destroy_session(&mut session, &executor).unwrap();
        let err = begin_transfer(&mut session).unwrap_err();
        assert!(matches!(err, SimardError::BridgeTransportError { .. }));
    }

    #[test]
    fn status_display_covers_all_variants() {
        assert_eq!(RemoteStatus::Creating.to_string(), "creating");
        assert_eq!(RemoteStatus::Running.to_string(), "running");
        assert_eq!(RemoteStatus::Transferring.to_string(), "transferring");
        assert_eq!(RemoteStatus::Completed.to_string(), "completed");
        assert!(
            RemoteStatus::Failed("oops".to_string())
                .to_string()
                .contains("oops")
        );
    }

    #[test]
    fn session_display_is_readable() {
        let executor = mock_executor();
        let session = create_remote_session(&test_config(), &executor).unwrap();
        let s = session.to_string();
        assert!(s.contains("test-vm"));
        assert!(s.contains("remote-engineer-1"));
    }
}
