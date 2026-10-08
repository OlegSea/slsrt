use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// What to do when an action's target path already exists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OnConflict {
    /// Pick a free name by appending ` (1)`, ` (2)`, ... to the file stem.
    #[default]
    Rename,
    /// Leave the file where it is.
    Skip,
    /// Replace the existing file.
    Overwrite,
}

impl FromStr for OnConflict {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rename" => Ok(Self::Rename),
            "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            other => Err(format!(
                "invalid on_conflict value `{other}` (expected \"rename\", \"skip\" or \"overwrite\")"
            )),
        }
    }
}

fn exists(path: &Path) -> bool {
    // `symlink_metadata` so dangling symlinks also count as taken.
    path.symlink_metadata().is_ok()
}

/// Final destination for `target` under `policy`, or `None` if the action should be skipped.
pub fn resolve(target: &Path, policy: OnConflict) -> Option<PathBuf> {
    if !exists(target) {
        return Some(target.to_path_buf());
    }
    match policy {
        OnConflict::Skip => None,
        OnConflict::Overwrite => Some(target.to_path_buf()),
        OnConflict::Rename => {
            let stem = target.file_stem().unwrap_or_default();
            let ext = target.extension();
            (1u32..)
                .map(|n| {
                    let mut name = OsString::from(stem);
                    name.push(format!(" ({n})"));
                    if let Some(ext) = ext {
                        name.push(".");
                        name.push(ext);
                    }
                    target.with_file_name(name)
                })
                .find(|candidate| !exists(candidate))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.txt");
        assert_eq!(resolve(&target, OnConflict::Skip), Some(target.clone()));

        fs::write(&target, "x").unwrap();
        assert_eq!(resolve(&target, OnConflict::Skip), None);
        assert_eq!(resolve(&target, OnConflict::Overwrite), Some(target.clone()));
        assert_eq!(resolve(&target, OnConflict::Rename), Some(dir.path().join("a (1).txt")));

        fs::write(dir.path().join("a (1).txt"), "x").unwrap();
        assert_eq!(resolve(&target, OnConflict::Rename), Some(dir.path().join("a (2).txt")));
    }

    #[test]
    fn renames_files_without_extension() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".hidden");
        fs::write(&target, "x").unwrap();
        assert_eq!(resolve(&target, OnConflict::Rename), Some(dir.path().join(".hidden (1)")));
    }

    #[test]
    fn parses_policy() {
        assert_eq!("skip".parse::<OnConflict>(), Ok(OnConflict::Skip));
        assert!("nope".parse::<OnConflict>().is_err());
    }
}
