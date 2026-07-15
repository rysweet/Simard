use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::paths::{InstallLayout, OODA_UNIT, SIGNAL_UNIT};
use super::{InstallError, InstallResult, err};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedUnits {
    pub ooda: String,
    pub signal: String,
}

pub fn render_units(layout: &InstallLayout) -> InstallResult<RenderedUnits> {
    let home = layout.simard_home.to_str().ok_or_else(|| {
        InstallError::new("SIMARD_HOME must be valid UTF-8 for systemd unit rendering")
    })?;
    let binary = layout.binary_path.to_str().ok_or_else(|| {
        InstallError::new("installed binary path must be valid UTF-8 for systemd unit rendering")
    })?;
    let service_path = render_service_path(home)?;

    Ok(RenderedUnits {
        ooda: render_unit(
            "Simard OODA daemon",
            home,
            binary,
            "ooda run",
            &service_path,
        ),
        signal: render_unit(
            "Simard Signal service",
            home,
            binary,
            "signal run",
            &service_path,
        ),
    })
}

pub fn install_units(layout: &InstallLayout, units: &RenderedUnits) -> InstallResult<()> {
    write_unit_atomically(&layout.ooda_unit_path, &units.ooda, &layout.transaction_id)?;
    write_unit_atomically(
        &layout.signal_unit_path,
        &units.signal,
        &layout.transaction_id,
    )?;
    Ok(())
}

pub fn resolve_systemctl(configured: Option<&Path>) -> InstallResult<PathBuf> {
    if let Some(path) = configured {
        if path.components().count() > 1 || path.is_absolute() {
            return validate_executable(path);
        }
        return find_in_path(path);
    }
    find_in_path(Path::new("systemctl"))
}

pub fn activate(systemctl: &Path) -> InstallResult<()> {
    run_systemctl(systemctl, &["--user", "daemon-reload"])?;
    run_systemctl(systemctl, &["--user", "enable", OODA_UNIT])?;
    run_systemctl(systemctl, &["--user", "enable", SIGNAL_UNIT])?;
    run_systemctl(systemctl, &["--user", "restart", OODA_UNIT])?;
    run_systemctl(systemctl, &["--user", "restart", SIGNAL_UNIT])?;
    Ok(())
}

fn render_unit(
    description: &str,
    working_directory: &str,
    binary: &str,
    args: &str,
    service_path: &str,
) -> String {
    format!(
        "[Unit]\nDescription={description}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nWorkingDirectory={working_directory}\nExecStart={binary} {args}\nRestart=always\nRestartSec=10\nEnvironment=SIMARD_HOME={working_directory}\nEnvironment=SIMARD_PROMPT_ASSETS_DIR={working_directory}/prompt_assets/simard\nEnvironment=PATH={service_path}\n\n[Install]\nWantedBy=default.target\n"
    )
}

fn render_service_path(simard_home: &str) -> InstallResult<String> {
    validate_unit_env_path("SIMARD_HOME", simard_home)?;

    let user_home = std::env::var_os("HOME")
        .ok_or_else(|| InstallError::new("HOME is required to render systemd service PATH"))?;
    let user_home = user_home
        .to_str()
        .ok_or_else(|| InstallError::new("HOME must be valid UTF-8 for systemd unit rendering"))?;
    validate_unit_env_path("HOME", user_home)?;

    Ok(format!(
        "{user_home}/.local/bin:{user_home}/.cargo/bin:{simard_home}/bin:/usr/local/bin:/usr/bin:/bin"
    ))
}

fn validate_unit_env_path(label: &str, value: &str) -> InstallResult<()> {
    if value.is_empty() {
        return err(format!(
            "{label} must not be empty for systemd unit rendering"
        ));
    }
    if let Some(ch) = value
        .chars()
        .find(|ch| matches!(ch, '\n' | '\r' | '%' | ':') || ch.is_ascii_whitespace())
    {
        return err(format!(
            "{label} contains unsafe character '{ch}' for systemd unit rendering"
        ));
    }
    Ok(())
}

fn write_unit_atomically(path: &Path, contents: &str, transaction_id: &str) -> InstallResult<()> {
    let parent = path.parent().ok_or_else(|| {
        InstallError::new(format!(
            "systemd unit path has no parent: {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        InstallError::new(format!(
            "failed to create systemd unit directory {}: {error}",
            parent.display()
        ))
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            InstallError::new(format!(
                "systemd unit filename is not valid UTF-8: {}",
                path.display()
            ))
        })?;
    let staged = parent.join(format!(".{file_name}.{transaction_id}.tmp"));
    fs::write(&staged, contents).map_err(|error| {
        InstallError::new(format!(
            "failed to write staged systemd unit {}: {error}",
            staged.display()
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o644)).map_err(|error| {
            InstallError::new(format!(
                "failed to set permissions on staged systemd unit {}: {error}",
                staged.display()
            ))
        })?;
    }

    fs::rename(&staged, path).map_err(|error| {
        InstallError::new(format!(
            "failed atomic systemd unit replacement from {} to {}: {error}",
            staged.display(),
            path.display()
        ))
    })
}

fn run_systemctl(systemctl: &Path, args: &[&str]) -> InstallResult<()> {
    let status = Command::new(systemctl)
        .args(args)
        .status()
        .map_err(|error| {
            InstallError::new(format!(
                "failed to run {} {}: {error}",
                systemctl.display(),
                args.join(" ")
            ))
        })?;
    if !status.success() {
        return err(format!(
            "{} {} failed with status {status}",
            systemctl.display(),
            args.join(" ")
        ));
    }
    Ok(())
}

fn find_in_path(name: &Path) -> InstallResult<PathBuf> {
    let path_value = std::env::var_os("PATH")
        .ok_or_else(|| InstallError::new("PATH is required to resolve systemctl"))?;
    for directory in std::env::split_paths(&path_value) {
        let candidate = directory.join(name);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }
    err(format!(
        "systemctl executable '{}' was not found in PATH",
        name.display()
    ))
}

fn validate_executable(path: &Path) -> InstallResult<PathBuf> {
    if is_executable_file(path) {
        return Ok(path.to_path_buf());
    }
    err(format!(
        "systemctl executable is missing or not executable: {}",
        path.display()
    ))
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}
