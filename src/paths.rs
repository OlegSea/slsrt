use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use directories::BaseDirs;

/// Expand a leading `~` and `$VAR` / `${VAR}` references. Unknown variables are an error.
pub fn expand(input: &str) -> Result<PathBuf> {
    let mut out = String::with_capacity(input.len());

    let rest = if input == "~" || input.starts_with("~/") || input.starts_with("~\\") {
        let home = BaseDirs::new()
            .ok_or_else(|| anyhow!("cannot expand `~`: home directory is unknown"))?
            .home_dir()
            .to_string_lossy()
            .into_owned();
        out.push_str(&home);
        &input[1..]
    } else {
        input
    };

    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let name: String = if chars.peek() == Some(&'{') {
            chars.next();
            let name: String = chars.by_ref().take_while(|&c| c != '}').collect();
            if name.is_empty() {
                bail!("empty variable reference `${{}}` in path `{input}`");
            }
            name
        } else {
            let mut name = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    name.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if name.is_empty() {
                out.push('$');
                continue;
            }
            name
        };
        let value = std::env::var(&name)
            .map_err(|_| anyhow!("environment variable `{name}` used in path `{input}` is not set"))?;
        out.push_str(&value);
    }

    Ok(PathBuf::from(out))
}

/// Expand `input` and resolve it against `base` if it is relative.
pub fn resolve(input: &str, base: &Path) -> Result<PathBuf> {
    let path = expand(input)?;
    Ok(normalize(&if path.is_absolute() { path } else { base.join(path) }))
}

/// Lexically remove `.` and `..` components without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Canonicalize if the path exists, otherwise return it unchanged.
pub fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_env_vars() {
        // SAFETY: tests in this module don't read this variable concurrently.
        unsafe { std::env::set_var("SLSRT_TEST_VAR", "abc") };
        assert_eq!(expand("/x/$SLSRT_TEST_VAR/y").unwrap(), PathBuf::from("/x/abc/y"));
        assert_eq!(expand("/x/${SLSRT_TEST_VAR}y").unwrap(), PathBuf::from("/x/abcy"));
        assert_eq!(expand("/x/$/y").unwrap(), PathBuf::from("/x/$/y"));
        assert!(expand("/x/$SLSRT_SURELY_UNSET_VAR").is_err());
    }

    #[test]
    fn expands_tilde() {
        let home = BaseDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(expand("~/a").unwrap(), home.join("a"));
        assert_eq!(expand("a/~").unwrap(), PathBuf::from("a/~"));
    }

    #[test]
    fn resolves_relative() {
        assert_eq!(resolve("../b/./c", Path::new("/base/a")).unwrap(), PathBuf::from("/base/b/c"));
        assert_eq!(resolve("/abs", Path::new("/base")).unwrap(), PathBuf::from("/abs"));
    }
}
