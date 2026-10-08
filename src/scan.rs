use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use log::warn;
use walkdir::WalkDir;

use crate::rules::Rule;

/// A directory to scan or watch, derived from the rules' `paths`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub dir: PathBuf,
    /// True if any enabled rule on this directory is recursive.
    pub recursive: bool,
}

/// Union of all enabled rules' directories. Missing directories are skipped with a warning.
pub fn targets(rules: &[Rule]) -> Vec<Target> {
    let mut dirs: BTreeMap<PathBuf, bool> = BTreeMap::new();
    for rule in rules.iter().filter(|r| r.enabled) {
        for dir in &rule.paths {
            *dirs.entry(dir.clone()).or_default() |= rule.recursive;
        }
    }
    dirs.into_iter()
        .filter(|(dir, _)| {
            let ok = dir.is_dir();
            if !ok {
                warn!("directory {} does not exist or is not a directory; skipping", dir.display());
            }
            ok
        })
        .map(|(dir, recursive)| Target { dir, recursive })
        .collect()
}

/// Files currently under `targets`, deduplicated across overlapping directories.
/// Symlinks to files are included; symlinked directories are not descended into.
pub fn files(targets: &[Target]) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    for target in targets {
        let walker = WalkDir::new(&target.dir)
            .min_depth(1)
            .max_depth(if target.recursive { usize::MAX } else { 1 })
            .follow_links(false);
        for entry in walker {
            match entry {
                Ok(entry) if is_file(entry.path()) => {
                    files.insert(entry.into_path());
                }
                Ok(_) => {}
                Err(e) => warn!("error while scanning {}: {e}", target.dir.display()),
            }
        }
    }
    files
}

/// Regular file, or a symlink pointing at one.
pub fn is_file(path: &Path) -> bool {
    path.is_file()
}
