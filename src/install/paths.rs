use std::ffi::OsStr;
use std::fs;
use std::fs::OpenOptions;
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
    /// Owned PATH entrypoint: `$ENTRYPOINT_DIR/simard` (default `~/.local/bin/simard`).
    pub entrypoint_path: PathBuf,
    /// Known orphan `simard` paths to reconcile (default `[~/.cargo/bin/simard]`).
    /// The entrypoint path is excluded — it is repaired, never pruned.
    pub orphan_paths: Vec<PathBuf>,
    pub transaction_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagingLayout {
    pub root: PathBuf,
    pub binary: PathBuf,
    pub prompt_assets: PathBuf,
}

#[derive(Debug)]
pub struct InstallLock {
    _file: fs::File,
}

pub fn resolve(config: &InstallConfig) -> InstallResult<InstallLayout> {
    let simard_home = resolve_simard_home(config)?;
    validate_install_path("SIMARD_HOME", &simard_home)?;

    let systemd_user_dir = resolve_systemd_user_dir(config)?;
    validate_install_path("systemd user directory", &systemd_user_dir)?;

    let entrypoint_dir = resolve_entrypoint_dir(config)?;
    validate_install_path("entrypoint directory", &entrypoint_dir)?;
    let entrypoint_path = entrypoint_dir.join("simard");

    let orphan_dirs = resolve_orphan_dirs(config)?;
    let mut orphan_paths = Vec::new();
    for dir in &orphan_dirs {
        validate_install_path("orphan directory", dir)?;
        let path = dir.join("simard");
        // The entrypoint is repaired in place, never pruned as an orphan.
        if path != entrypoint_path && !orphan_paths.contains(&path) {
            orphan_paths.push(path);
        }
    }

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
        entrypoint_path,
        orphan_paths,
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

pub fn acquire_install_lock(layout: &InstallLayout) -> InstallResult<InstallLock> {
    create_private_dir(&layout.simard_home)?;
    let lock_path = layout.simard_home.join(".install.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| {
            super::InstallError::new(format!(
                "failed to open installer lock {}: {error}",
                lock_path.display()
            ))
        })?;

    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            super::InstallError::new(format!(
                "failed to set owner-only permissions on installer lock {}: {error}",
                lock_path.display()
            ))
        })?;

        // SAFETY: flock only uses the open lock-file descriptor; the File stays
        // alive in InstallLock until the installer transaction ends.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            return err(format!(
                "another simard install appears to be running for {}; lock {} could not be acquired: {error}",
                layout.simard_home.display(),
                lock_path.display()
            ));
        }
    }

    Ok(InstallLock { _file: file })
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

fn resolve_entrypoint_dir(config: &InstallConfig) -> InstallResult<PathBuf> {
    if let Some(path) = &config.entrypoint_dir {
        return Ok(path.clone());
    }
    if let Some(value) = std::env::var_os("SIMARD_ENTRYPOINT_DIR") {
        return Ok(PathBuf::from(value));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        super::InstallError::new("HOME is required to default the PATH entrypoint directory")
    })?;
    Ok(home.join(".local").join("bin"))
}

fn resolve_orphan_dirs(config: &InstallConfig) -> InstallResult<Vec<PathBuf>> {
    if let Some(dirs) = &config.orphan_dirs {
        return Ok(dirs.clone());
    }
    if let Some(value) = std::env::var_os("SIMARD_ORPHAN_DIRS") {
        let raw = os_str_to_unit_path("SIMARD_ORPHAN_DIRS", &value)?;
        let dirs: Vec<PathBuf> = raw
            .split(':')
            .filter(|entry| !entry.is_empty())
            .map(PathBuf::from)
            .collect();
        return Ok(dirs);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        super::InstallError::new("HOME is required to default the entrypoint orphan directories")
    })?;
    Ok(vec![home.join(".cargo").join("bin")])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: &Path) -> InstallLayout {
        InstallLayout {
            simard_home: root.to_path_buf(),
            bin_dir: root.join("bin"),
            binary_path: root.join("bin/simard"),
            prompt_assets_dir: root.join("prompt_assets"),
            staging_root: root.join(".install-staging"),
            backup_root: root.join(".install-backups"),
            systemd_user_dir: root.join("systemd"),
            ooda_unit_path: root.join("systemd/simard-ooda.service"),
            signal_unit_path: root.join("systemd/simard-signal.service"),
            entrypoint_path: root.join(".local/bin/simard"),
            orphan_paths: vec![root.join(".cargo/bin/simard")],
            transaction_id: "test-tx".to_string(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn install_lock_is_exclusive_per_simard_home() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let layout = layout(&temp.path().join("simard-home"));

        let first = acquire_install_lock(&layout).expect("first lock");
        let error = acquire_install_lock(&layout)
            .expect_err("second lock for same SIMARD_HOME should fail")
            .to_string();

        assert!(
            error.contains("another simard install appears to be running"),
            "{error}"
        );

        drop(first);
        acquire_install_lock(&layout).expect("lock should be available after guard drop");
    }
}
