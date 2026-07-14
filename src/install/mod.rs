//! Canonical installer transaction for Simard.

pub mod assets;
pub mod binary;
mod health;
pub mod paths;
pub mod rollback;
pub mod systemd;

#[cfg(test)]
mod health_regressions;

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::time::Duration;

use paths::InstallLayout;

pub(crate) const REQUIRED_TYPED_OODA_ASSETS: [&str; 2] = [
    "simard/recipes/goal-session-actor.yaml",
    "simard/policies/goal-session-capabilities.toml",
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallConfig {
    pub simard_home: Option<PathBuf>,
    pub dry_run: bool,
    pub systemd_user_dir: Option<PathBuf>,
    pub systemctl: Option<PathBuf>,
    pub health_check: Option<PathBuf>,
    pub rollback_manifest: Option<PathBuf>,
    help_only: bool,
}

impl InstallConfig {
    pub fn with_help_only(mut self) -> Self {
        self.help_only = true;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallOutcome {
    pub simard_home: PathBuf,
    pub binary_path: PathBuf,
    pub prompt_assets_path: PathBuf,
    pub ooda_unit_path: PathBuf,
    pub signal_unit_path: PathBuf,
    pub prior_binary_backup: Option<PathBuf>,
    pub activated: bool,
}

#[derive(Debug)]
pub struct InstallError {
    kind: InstallErrorKind,
    message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallErrorKind {
    General,
    SnapshotFailure,
    HealthCheckFailure,
    RollbackFailure,
}

impl InstallError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::with_kind(InstallErrorKind::General, message)
    }

    pub fn with_kind(kind: InstallErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> InstallErrorKind {
        self.kind
    }
}

impl Display for InstallError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for InstallError {}

pub type InstallResult<T> = Result<T, InstallError>;

pub(crate) fn err<T>(message: impl Into<String>) -> InstallResult<T> {
    Err(InstallError::new(message))
}

pub fn run(config: InstallConfig) -> InstallResult<InstallOutcome> {
    if config.help_only {
        return Ok(empty_help_outcome());
    }

    let layout = paths::resolve(&config)?;
    if let Some(manifest) = &config.rollback_manifest {
        if config.dry_run {
            return err("--dry-run cannot be combined with --rollback");
        }
        let systemctl = systemd::resolve_systemctl(config.systemctl.as_deref())?;
        let _install_lock = paths::acquire_install_lock(&layout)?;
        rollback::restore_verified_backup(&layout, manifest, &systemctl).map_err(|error| {
            InstallError::with_kind(InstallErrorKind::RollbackFailure, error.to_string())
        })?;
        println!(
            "Rolled back Simard from verified manifest {}",
            manifest.display()
        );
        return Ok(outcome(&layout, None, true));
    }
    let current_binary = binary::current_binary()?;
    let prompt_source = assets::discover_prompt_asset_root()?;
    assets::validate_prompt_source(&prompt_source)?;
    assets::validate_live_prompt_target(&layout.prompt_assets_dir)?;
    let rendered_units = systemd::render_units(&layout)?;

    if config.dry_run {
        print_dry_run_plan(&layout, &current_binary, &prompt_source, &config);
        return Ok(outcome(&layout, None, false));
    }

    let systemctl = systemd::resolve_systemctl(config.systemctl.as_deref())?;
    let _install_lock = paths::acquire_install_lock(&layout)?;
    let staging = paths::prepare_staging(&layout)?;
    binary::stage_binary(&current_binary, &staging.binary)?;
    assets::stage_prompt_assets(&prompt_source, &staging.prompt_assets)?;
    let verified_backup =
        rollback::create_verified_backup(&layout, &systemctl).map_err(|error| {
            InstallError::with_kind(InstallErrorKind::SnapshotFailure, error.to_string())
        })?;
    let configured_health_check = config
        .health_check
        .clone()
        .or_else(|| std::env::var_os("SIMARD_INSTALL_HEALTH_CHECK").map(PathBuf::from));

    let install_result: InstallResult<Option<PathBuf>> = (|| {
        let prior_binary_backup = if binary::live_binary_matches_source(&current_binary, &layout)? {
            println!(
                "Installed binary already matches {}; keeping it in place",
                layout.binary_path.display()
            );
            None
        } else {
            let backup = binary::preserve_prior_binary(&layout)?;
            binary::replace_live_binary(&staging.binary, &layout.binary_path)?;
            backup
        };
        assets::replace_live_prompt_assets(&staging.prompt_assets, &layout)?;
        systemd::install_units(&layout, &rendered_units)?;
        systemd::activate(&systemctl)?;
        let (health_program, health_args): (&std::path::Path, &[&str]) =
            if let Some(health_check) = configured_health_check.as_deref() {
                (health_check, &[])
            } else {
                (&layout.binary_path, &["self-health", "--json"])
            };
        health::run_with_args(health_program, health_args, Duration::from_secs(120)).map_err(
            |error| {
                InstallError::with_kind(
                    InstallErrorKind::HealthCheckFailure,
                    format!("health check failed: {error}"),
                )
            },
        )?;
        Ok(prior_binary_backup)
    })();

    let prior_binary_backup = match install_result {
        Ok(backup) => backup,
        Err(install_error) => {
            let rollback_result = rollback::restore_verified_backup(
                &layout,
                &verified_backup.manifest_path,
                &systemctl,
            );
            let _ = paths::remove_staging(&staging.root);
            return match rollback_result {
                Ok(()) => err(format!(
                    "installation failed and was rolled back: {install_error}"
                )),
                Err(rollback_error) => Err(InstallError::with_kind(
                    InstallErrorKind::RollbackFailure,
                    format!(
                        "installation failed: {install_error}; rollback failed: {rollback_error}"
                    ),
                )),
            };
        }
    };
    paths::remove_staging(&staging.root)?;

    rollback::print_guidance(
        &layout,
        prior_binary_backup.as_deref(),
        &verified_backup.manifest_path,
    );
    println!("Installed Simard to {}", layout.simard_home.display());
    println!(
        "Installed prompt assets to {}",
        layout.prompt_assets_dir.display()
    );
    println!(
        "Installed user units: {}, {}",
        layout.ooda_unit_path.display(),
        layout.signal_unit_path.display()
    );

    Ok(outcome(&layout, prior_binary_backup, true))
}

fn empty_help_outcome() -> InstallOutcome {
    InstallOutcome {
        simard_home: PathBuf::new(),
        binary_path: PathBuf::new(),
        prompt_assets_path: PathBuf::new(),
        ooda_unit_path: PathBuf::new(),
        signal_unit_path: PathBuf::new(),
        prior_binary_backup: None,
        activated: false,
    }
}

fn outcome(
    layout: &InstallLayout,
    prior_binary_backup: Option<PathBuf>,
    activated: bool,
) -> InstallOutcome {
    InstallOutcome {
        simard_home: layout.simard_home.clone(),
        binary_path: layout.binary_path.clone(),
        prompt_assets_path: layout.prompt_assets_dir.clone(),
        ooda_unit_path: layout.ooda_unit_path.clone(),
        signal_unit_path: layout.signal_unit_path.clone(),
        prior_binary_backup,
        activated,
    }
}

fn print_dry_run_plan(
    layout: &InstallLayout,
    current_binary: &std::path::Path,
    prompt_source: &std::path::Path,
    config: &InstallConfig,
) {
    let systemctl = config
        .systemctl
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "systemctl".to_string());

    println!("[dry-run] Would install current binary:");
    println!(
        "  {} -> {}",
        current_binary.display(),
        layout.binary_path.display()
    );
    println!("[dry-run] Would install prompt_assets:");
    println!(
        "  {} -> {}",
        prompt_source.display(),
        layout.prompt_assets_dir.display()
    );
    println!("[dry-run] Would write user systemd units:");
    println!("  {}", layout.ooda_unit_path.display());
    println!("  {}", layout.signal_unit_path.display());
    println!("[dry-run] Activation plan:");
    println!("  {systemctl} --user daemon-reload");
    println!("  {systemctl} --user enable simard-ooda.service");
    println!("  {systemctl} --user enable simard-signal.service");
    println!("  {systemctl} --user restart simard-ooda.service");
    println!("  {systemctl} --user restart simard-signal.service");
}
