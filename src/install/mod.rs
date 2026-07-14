//! Canonical installer transaction for Simard.

pub mod assets;
pub mod binary;
pub mod paths;
pub mod rollback;
pub mod systemd;

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

use paths::InstallLayout;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallConfig {
    pub simard_home: Option<PathBuf>,
    pub dry_run: bool,
    pub systemd_user_dir: Option<PathBuf>,
    pub systemctl: Option<PathBuf>,
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
    message: String,
}

impl InstallError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
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
    paths::remove_staging(&staging.root)?;

    rollback::print_guidance(&layout, prior_binary_backup.as_deref());
    systemd::activate(&systemctl)?;

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
