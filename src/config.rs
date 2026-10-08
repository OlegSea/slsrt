use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;

pub const CONFIG_ENV: &str = "SLSRT_CONFIG";
pub const ENTRY_FILE: &str = "main.lua";

/// Resolve the entry config file: `--config` → `$SLSRT_CONFIG` → `<config dir>/main.lua`.
pub fn locate(cli_path: Option<PathBuf>) -> Result<PathBuf> {
    let (path, source) = if let Some(path) = cli_path {
        (path, "--config")
    } else if let Some(path) = std::env::var_os(CONFIG_ENV).filter(|p| !p.is_empty()) {
        (PathBuf::from(path), CONFIG_ENV)
    } else {
        let dirs = ProjectDirs::from("", "", "slsrt")
            .context("could not determine the OS config directory; pass --config")?;
        (dirs.config_dir().join(ENTRY_FILE), "default location")
    };

    if !path.is_file() {
        bail!("config file not found: {} (from {source})", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("failed to resolve config path {}", path.display()))
}
