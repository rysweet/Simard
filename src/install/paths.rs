use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{InstallConfig, InstallResult, err};

pub const OODA_UNIT: &str = "simard-ooda.service";
pub const SIGNAL_UNIT: &str = "simard-signal.service";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallLayout {
    pub simard_home: PathBuf,
    pub bin_dir: PathBuf,
    pub binary_path: PathBuf,
    pub prompt_assets_dir: PathBuf,
    pub staging_root: PathBuf,
    pub backup_root: PathBuf,
    pub systemd_user_dir: PathBuf,
    pub ooda_unit_path: PathBuf,
    pub signal_unit_path: PathBuf,
    pub transaction_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagingLayout {
    pub root: PathBuf,
    pub binary: PathBuf,
    pub prompt_assets: PathBuf,
}

pub fn resolve(config: &InstallConfig) -> InstallResult<InstallLayout> {
    let simard_home = resolve_simard_home(config)?;
    validate_install_path("SIMARD_HOME", &simard_home)?;

    let systemd_user_dir = resolve_systemd_user_dir(config)?;
    validate_install_path("systemd user directory", &systemd_user_dir)?;

    let transaction_id = transaction_id()?;
    let bin_dir = simard_home.join("bin");
    let binary_path = bin_dir.join("simard");
    let prompt_assets_dir = simard_home.join("prompt_assets");
    let staging_root = simard_home.join(".install-staging");
    let backup_root = simard_home.join(".install-backups");
    let ooda_unit_path = systemd_user_dir.join(OODA_UNIT);
    let signal_unit_path = systemd_user_dir.join(SIGNAL_UNIT);

    Ok(InstallLayout {
        simard_home,
        bin_dir,
        binary_path,
        prompt_assets_dir,
        staging_root,
        backup_root,
        systemd_user_dir,
        ooda_unit_path,
        signal_unit_path,
        transaction_id,
    })
}

pub fn prepare_staging(layout: &InstallLayout) -> InstallResult<StagingLayout> {
    create_private_dir(&layout.simard_home)?;
    fs::create_dir_all(&layout.bin_dir).map_err(|error| {
        super::InstallError::new(format!(
            "failed to create install bin directory {}: {error}",
            layout.bin_dir.display()
        ))
    })?;
    create_private_dir(&layout.staging_root)?;
    create_private_dir(&layout.backup_root)?;
    fs::create_dir_all(&layout.systemd_user_dir).map_err(|error| {
        super::InstallError::new(format!(
            "failed to create systemd user directory {}: {error}",
            layout.systemd_user_dir.display()
        ))
    })?;

    let root = layout.staging_root.join(&layout.transaction_id);
    if root.exists() {
        return err(format!(
            "installer staging path already exists: {}",
            root.display()
        ));
    }
    create_private_dir(&root)?;

    Ok(StagingLayout {
        binary: root.join("simard"),
        prompt_assets: root.join("prompt_assets"),
        root,
    })
}

pub fn remove_staging(path: &Path) -> InstallResult<()> {
    fs::remove_dir_all(path).map_err(|error| {
        super::InstallError::new(format!(
            "failed to remove installer staging directory {}: {error}",
            path.display()
        ))
    })
}

pub fn backup_path(layout: &InstallLayout, name: &str) -> PathBuf {
    layout
        .backup_root
        .join(format!("{name}.{}.bak", layout.transaction_id))
}

fn resolve_simard_home(config: &InstallConfig) -> InstallResult<PathBuf> {
    if let Some(path) = &config.simard_home {
        return Ok(path.clone());
    }
    if let Some(value) = std::env::var_os("SIMARD_HOME") {
        return Ok(PathBuf::from(value));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| super::InstallError::new("HOME is required to default SIMARD_HOME"))?;
    Ok(home.join(".simard"))
}

fn resolve_systemd_user_dir(config: &InstallConfig) -> InstallResult<PathBuf> {
    if let Some(path) = &config.systemd_user_dir {
        return Ok(path.clone());
    }
    if let Some(value) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(value).join("systemd").join("user"));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        super::InstallError::new("HOME is required to default the systemd user directory")
    })?;
    Ok(home.join(".config").join("systemd").join("user"))
}

pub fn validate_install_path(label: &str, path: &Path) -> InstallResult<()> {
    if path.as_os_str().is_empty() {
        return err(format!("{label} must not be empty"));
    }
    if !path.is_absolute() {
        return err(format!(
            "{label} must be an absolute path for fail-closed installation: {}",
            path.display()
        ));
    }
    let value = os_str_to_unit_path(label, path.as_os_str())?;
    if let Some(ch) = value
        .chars()
        .find(|ch| matches!(ch, '\n' | '\r' | '%') || ch.is_ascii_whitespace())
    {
        return err(format!(
            "{label} contains unsafe character '{ch}' for systemd unit rendering: {}",
            path.display()
        ));
    }
    Ok(())
}

pub fn os_str_to_unit_path<'a>(label: &str, value: &'a OsStr) -> InstallResult<&'a str> {
    value.to_str().ok_or_else(|| {
        super::InstallError::new(format!(
            "{label} must be valid UTF-8 so it can be rendered into a systemd unit"
        ))
    })
}

fn transaction_id() -> InstallResult<String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            super::InstallError::new(format!("system clock is before epoch: {error}"))
        })?
        .as_millis();
    Ok(format!("{}-{millis}", std::process::id()))
}

fn create_private_dir(path: &Path) -> InstallResult<()> {
    fs::create_dir_all(path).map_err(|error| {
        super::InstallError::new(format!(
            "failed to create installer directory {}: {error}",
            path.display()
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            super::InstallError::new(format!(
                "failed to set owner-only permissions on {}: {error}",
                path.display()
            ))
        })?;
    }

    Ok(())
}
