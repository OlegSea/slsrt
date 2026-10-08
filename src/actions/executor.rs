use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Context, Result};
use log::info;

/// Performs (or pretends to perform) filesystem operations for actions.
/// Conflict resolution happens before these are called; targets are final.
pub trait Executor {
    fn dry_run(&self) -> bool;
    fn move_file(&self, from: &Path, to: &Path) -> Result<()>;
    fn copy_file(&self, from: &Path, to: &Path) -> Result<()>;
    fn trash(&self, path: &Path) -> Result<()>;
    fn delete(&self, path: &Path) -> Result<()>;
}

pub fn for_mode(dry_run: bool) -> Box<dyn Executor> {
    if dry_run { Box::new(DryRunExecutor) } else { Box::new(FsExecutor) }
}

/// Executes actions on the real filesystem.
pub struct FsExecutor;

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("cannot create directory {}", parent.display()))?;
    }
    Ok(())
}

impl Executor for FsExecutor {
    fn dry_run(&self) -> bool {
        false
    }

    fn move_file(&self, from: &Path, to: &Path) -> Result<()> {
        create_parent(to)?;
        match fs::rename(from, to) {
            Ok(()) => {}
            // Different filesystem: fall back to copy + remove.
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                fs::copy(from, to).with_context(|| format!("cannot copy {} to {}", from.display(), to.display()))?;
                fs::remove_file(from).with_context(|| format!("cannot remove {}", from.display()))?;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("cannot move {} to {}", from.display(), to.display()));
            }
        }
        info!("moved {} -> {}", from.display(), to.display());
        Ok(())
    }

    fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
        create_parent(to)?;
        fs::copy(from, to).with_context(|| format!("cannot copy {} to {}", from.display(), to.display()))?;
        info!("copied {} -> {}", from.display(), to.display());
        Ok(())
    }

    fn trash(&self, path: &Path) -> Result<()> {
        trash::delete(path).with_context(|| format!("cannot move {} to trash", path.display()))?;
        info!("trashed {}", path.display());
        Ok(())
    }

    fn delete(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).with_context(|| format!("cannot delete {}", path.display()))?;
        info!("deleted {}", path.display());
        Ok(())
    }
}

/// Logs what would happen without touching the filesystem.
pub struct DryRunExecutor;

impl Executor for DryRunExecutor {
    fn dry_run(&self) -> bool {
        true
    }

    fn move_file(&self, from: &Path, to: &Path) -> Result<()> {
        info!("[dry-run] would move {} -> {}", from.display(), to.display());
        Ok(())
    }

    fn copy_file(&self, from: &Path, to: &Path) -> Result<()> {
        info!("[dry-run] would copy {} -> {}", from.display(), to.display());
        Ok(())
    }

    fn trash(&self, path: &Path) -> Result<()> {
        info!("[dry-run] would trash {}", path.display());
        Ok(())
    }

    fn delete(&self, path: &Path) -> Result<()> {
        info!("[dry-run] would delete {}", path.display());
        Ok(())
    }
}
