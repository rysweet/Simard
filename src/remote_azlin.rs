//! Wrapper around the `azlin` CLI for VM lifecycle management.
//!
//! `azlin` is an external tool that creates, manages, and destroys Azure VMs.
//! This module shells out to `azlin` commands and parses their output. All
//! functions return `SimardResult` so callers get consistent error handling.
//!
//! In production, `azlin` must be on `$PATH`. Tests inject a mock executor
//! to avoid real VM creation.

use std::fmt::{self, Display, Formatter};
use std::process::Command;

use crate::error::{SimardError, SimardResult};

/// Configuration for VM creation via azlin.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AzlinConfig {
    /// Azure region (e.g. "eastus2"). None uses azlin's default.
    pub region: Option<String>,
    /// VM size (e.g. "Standard_D4s_v3"). None uses azlin's default.
    pub size: Option<String>,
    /// Optional SSH public key path. None uses azlin's default (~/.ssh/id_rsa.pub).
    pub ssh_key_path: Option<String>,
}

/// Represents a VM managed by azlin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AzlinVm {
    /// The name used to identify this VM in azlin commands.
    pub name: String,
    /// The public IP address assigned to the VM.
    pub ip: String,
    /// Current azlin-reported status (e.g. "running", "creating", "deleted").
    pub status: String,
}

impl Display for AzlinVm {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AzlinVm({}, ip={}, status={})",
            self.name, self.ip, self.status
        )
    }
}

/// Trait abstracting azlin CLI execution for testability.
///
/// Production code uses `RealAzlinExecutor` which shells out to the `azlin`
/// binary. Tests inject `MockAzlinExecutor` to simulate VM lifecycle without
/// real infrastructure.
pub trait AzlinExecutor: Send + Sync {
    /// Run an azlin command and return stdout on success.
    fn run(&self, args: &[&str]) -> SimardResult<String>;
}

/// Production executor that invokes the real `azlin` CLI.
pub struct RealAzlinExecutor;

impl AzlinExecutor for RealAzlinExecutor {
    fn run(&self, args: &[&str]) -> SimardResult<String> {
        let output = Command::new("azlin").args(args).output().map_err(|e| {
            SimardError::BridgeSpawnFailed {
                bridge: "azlin".to_string(),
                reason: format!("failed to execute azlin: {e}"),
            }
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::BridgeTransportError {
                bridge: "azlin".to_string(),
                reason: format!(
                    "azlin {} exited with {}: {}",
                    args.first().unwrap_or(&"<unknown>"),
                    output.status,
                    stderr.trim()
                ),
            });
        }

        String::from_utf8(output.stdout).map_err(|e| SimardError::BridgeProtocolError {
            bridge: "azlin".to_string(),
            reason: format!("azlin output was not valid UTF-8: {e}"),
        })
    }
}

/// Create a new VM via `azlin create`.
///
/// Parses the azlin output to extract the VM name and IP address.
/// The VM name is passed directly; azlin assigns the IP.
pub fn azlin_create(
    name: &str,
    config: &AzlinConfig,
    executor: &dyn AzlinExecutor,
) -> SimardResult<AzlinVm> {
    if name.is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: "vm_name".to_string(),
            value: String::new(),
            help: "VM name cannot be empty".to_string(),
        });
    }
    // Reject names with characters that could cause shell injection or azlin issues.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SimardError::InvalidConfigValue {
            key: "vm_name".to_string(),
            value: name.to_string(),
            help: "VM name must contain only alphanumeric characters, hyphens, and underscores"
                .to_string(),
        });
    }

    let mut args = vec!["create", name];
    let region_flag;
    if let Some(ref region) = config.region {
        if !region
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(SimardError::InvalidConfigValue {
                key: "region".to_string(),
                value: region.to_string(),
                help: "region must contain only alphanumeric characters, hyphens, and underscores"
                    .to_string(),
            });
        }
        region_flag = format!("--region={region}");
        args.push(&region_flag);
    }
    let size_flag;
    if let Some(ref size) = config.size {
        if !size
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(SimardError::InvalidConfigValue {
                key: "size".to_string(),
                value: size.to_string(),
                help: "size must contain only alphanumeric characters, hyphens, and underscores"
                    .to_string(),
            });
        }
        size_flag = format!("--size={size}");
        args.push(&size_flag);
    }

    let output = executor.run(&args)?;
    parse_azlin_create_output(name, &output)
}

/// Get an SSH connection string for a VM via `azlin ssh`.
///
/// Returns a connection string like "azureuser@<ip>" that can be used
/// with `ssh` or `scp` commands.
pub fn azlin_ssh(vm: &AzlinVm, executor: &dyn AzlinExecutor) -> SimardResult<String> {
    if vm.status != "running" {
        return Err(SimardError::BridgeTransportError {
            bridge: "azlin".to_string(),
            reason: format!("cannot SSH to VM '{}' in status '{}'", vm.name, vm.status),
        });
    }

    let output = executor.run(&["ssh", &vm.name, "--print-command"])?;
    let trimmed = output.trim().to_string();
    if trimmed.is_empty() {
        return Err(SimardError::BridgeProtocolError {
            bridge: "azlin".to_string(),
            reason: "azlin ssh returned empty output".to_string(),
        });
    }
    Ok(trimmed)
}

/// Destroy a VM via `azlin destroy`.
///
/// This is idempotent — destroying an already-destroyed VM is not an error
/// from azlin's perspective, though the caller should track status.
pub fn azlin_destroy(vm: &AzlinVm, executor: &dyn AzlinExecutor) -> SimardResult<()> {
    executor.run(&["destroy", &vm.name, "--yes"])?;
    Ok(())
}

/// Parse the output of `azlin create` to extract VM details.
///
/// Expected format (one key=value per line):
/// ```text
/// name=simard-remote-1
/// ip=20.10.30.40
/// status=running
/// ```
fn parse_azlin_create_output(expected_name: &str, output: &str) -> SimardResult<AzlinVm> {
    let mut ip = None;
    let mut status = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("ip=") {
            ip = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("status=") {
            status = Some(value.to_string());
        }
    }

    let ip = ip.ok_or_else(|| SimardError::BridgeProtocolError {
        bridge: "azlin".to_string(),
        reason: format!("azlin create output missing 'ip=' field for VM '{expected_name}'"),
    })?;

    Ok(AzlinVm {
        name: expected_name.to_string(),
        ip,
        status: status.unwrap_or_else(|| "running".to_string()),
    })
}

/// Type alias for the mock handler closure to satisfy clippy complexity lint.
type MockHandler = Box<dyn Fn(&[&str]) -> SimardResult<String> + Send + Sync>;

/// Mock executor for testing. Accepts a closure that handles azlin commands.
///
/// Available in all builds so that integration tests (`tests/`) can use it.
/// The struct has zero cost in production since it is only instantiated in tests.
pub struct MockAzlinExecutor {
    handler: MockHandler,
}

impl MockAzlinExecutor {
    pub fn new(handler: impl Fn(&[&str]) -> SimardResult<String> + Send + Sync + 'static) -> Self {
        Self {
            handler: Box::new(handler),
        }
    }
}

impl AzlinExecutor for MockAzlinExecutor {
    fn run(&self, args: &[&str]) -> SimardResult<String> {
        (self.handler)(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success_executor() -> MockAzlinExecutor {
        MockAzlinExecutor::new(|args| match args.first().copied() {
            Some("create") => Ok("name=test-vm\nip=10.0.0.1\nstatus=running\n".to_string()),
            Some("ssh") => Ok("ssh azureuser@10.0.0.1".to_string()),
            Some("destroy") => Ok(String::new()),
            _ => Err(SimardError::BridgeTransportError {
                bridge: "azlin".to_string(),
                reason: format!("unexpected command: {args:?}"),
            }),
        })
    }

    #[test]
    fn create_vm_parses_output() {
        let executor = success_executor();
        let config = AzlinConfig::default();
        let vm = azlin_create("test-vm", &config, &executor).unwrap();
        assert_eq!(vm.name, "test-vm");
        assert_eq!(vm.ip, "10.0.0.1");
        assert_eq!(vm.status, "running");
    }

    #[test]
    fn create_vm_with_region_and_size() {
        let executor = success_executor();
        let config = AzlinConfig {
            region: Some("westus2".to_string()),
            size: Some("Standard_D4s_v3".to_string()),
            ssh_key_path: None,
        };
        let vm = azlin_create("test-vm", &config, &executor).unwrap();
        assert_eq!(vm.status, "running");
    }

    #[test]
    fn create_rejects_empty_name() {
        let executor = success_executor();
        let err = azlin_create("", &AzlinConfig::default(), &executor).unwrap_err();
        assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
    }

    #[test]
    fn create_rejects_invalid_name_chars() {
        let executor = success_executor();
        let err = azlin_create("vm;rm -rf /", &AzlinConfig::default(), &executor).unwrap_err();
        assert!(matches!(err, SimardError::InvalidConfigValue { .. }));
    }

    #[test]
    fn ssh_returns_connection_string() {
        let executor = success_executor();
        let vm = AzlinVm {
            name: "test-vm".to_string(),
            ip: "10.0.0.1".to_string(),
            status: "running".to_string(),
        };
        let conn = azlin_ssh(&vm, &executor).unwrap();
        assert!(conn.contains("azureuser@10.0.0.1"));
    }

    #[test]
    fn ssh_rejects_non_running_vm() {
        let executor = success_executor();
        let vm = AzlinVm {
            name: "test-vm".to_string(),
            ip: "10.0.0.1".to_string(),
            status: "creating".to_string(),
        };
        let err = azlin_ssh(&vm, &executor).unwrap_err();
        assert!(matches!(err, SimardError::BridgeTransportError { .. }));
    }

    #[test]
    fn destroy_succeeds() {
        let executor = success_executor();
        let vm = AzlinVm {
            name: "test-vm".to_string(),
            ip: "10.0.0.1".to_string(),
            status: "running".to_string(),
        };
        azlin_destroy(&vm, &executor).unwrap();
    }

    #[test]
    fn parse_output_missing_ip_fails() {
        let err = parse_azlin_create_output("vm-1", "status=running\n").unwrap_err();
        assert!(matches!(err, SimardError::BridgeProtocolError { .. }));
    }

    #[test]
    fn parse_output_defaults_status_to_running() {
        let vm = parse_azlin_create_output("vm-1", "ip=1.2.3.4\n").unwrap();
        assert_eq!(vm.status, "running");
    }

    #[test]
    fn vm_display_is_readable() {
        let vm = AzlinVm {
            name: "test-vm".to_string(),
            ip: "10.0.0.1".to_string(),
            status: "running".to_string(),
        };
        let s = vm.to_string();
        assert!(s.contains("test-vm"));
        assert!(s.contains("10.0.0.1"));
    }
}
