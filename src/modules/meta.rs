//! Core filesystem metadata: names, size, timestamps, flags, extension and glob checks.
//!
//! This is the reference module: other capability modules follow the same shape —
//! a `Module` impl registering fields/methods, and lazily cached per-file data via
//! [`FileHandle::cached`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use globset::{GlobBuilder, GlobMatcher};
use mlua::prelude::*;

use super::{Module, ModuleCtx};
use crate::lua::file::FileHandle;

pub struct Meta;

/// Cached `fs::metadata` for a file (follows symlinks).
struct Stat(fs::Metadata);

fn stat(file: &FileHandle) -> LuaResult<std::rc::Rc<Stat>> {
    file.cached(|path| {
        fs::metadata(path)
            .map(Stat)
            .map_err(|e| LuaError::runtime(format!("cannot read metadata of {}: {e}", path.display())))
    })
}

fn unix_seconds(time: std::io::Result<SystemTime>) -> Option<i64> {
    let time = time.ok()?;
    Some(match time.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    })
}

fn lossy(s: Option<&std::ffi::OsStr>) -> Option<String> {
    s.map(|s| s.to_string_lossy().into_owned())
}

/// Compiled glob cache shared by all `file:matches` calls.
#[derive(Default)]
struct GlobCache(RefCell<HashMap<String, GlobMatcher>>);

impl Module for Meta {
    fn name(&self) -> &'static str {
        "meta"
    }

    fn register(&self, ctx: &ModuleCtx) -> LuaResult<()> {
        ctx.lua().set_app_data(GlobCache::default());

        ctx.file_field("path", |_, f| Ok(f.path().to_string_lossy().into_owned()))?;
        ctx.file_field("name", |_, f| Ok(lossy(f.path().file_name())))?;
        ctx.file_field("stem", |_, f| Ok(lossy(f.path().file_stem())))?;
        ctx.file_field("ext", |_, f| {
            Ok(lossy(f.path().extension()).map(|e| e.to_lowercase()).unwrap_or_default())
        })?;
        ctx.file_field("dir", |_, f| Ok(lossy(f.path().parent().map(|p| p.as_os_str()))))?;

        ctx.file_method("size", |_, f, ()| Ok(stat(f)?.0.len()))?;
        ctx.file_method("modified", |_, f, ()| Ok(unix_seconds(stat(f)?.0.modified())))?;
        ctx.file_method("created", |_, f, ()| Ok(unix_seconds(stat(f)?.0.created())))?;
        ctx.file_method("accessed", |_, f, ()| Ok(unix_seconds(stat(f)?.0.accessed())))?;
        ctx.file_method("age", |_, f, ()| {
            let modified = stat(f)?.0.modified().ok();
            Ok(modified.map(|m| SystemTime::now().duration_since(m).map(|d| d.as_secs()).unwrap_or(0)))
        })?;

        ctx.file_method("is_hidden", |_, f, ()| {
            let dotfile = f.path().file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
                Ok(dotfile || stat(f)?.0.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
            }
            #[cfg(not(windows))]
            Ok(dotfile)
        })?;
        ctx.file_method("is_symlink", |_, f, ()| {
            f.ensure_present()?;
            Ok(fs::symlink_metadata(f.path()).is_ok_and(|m| m.file_type().is_symlink()))
        })?;
        ctx.file_method("is_readonly", |_, f, ()| Ok(stat(f)?.0.permissions().readonly()))?;

        ctx.file_method("has_ext", |_, f, exts: LuaVariadic<String>| {
            let Some(ext) = f.path().extension().map(|e| e.to_string_lossy().into_owned()) else {
                return Ok(false);
            };
            Ok(exts.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext)))
        })?;

        ctx.file_method("matches", |lua, f, pattern: String| {
            let cache = lua.app_data_ref::<GlobCache>().expect("glob cache is installed");
            let mut cache = cache.0.borrow_mut();
            if !cache.contains_key(&pattern) {
                let glob = GlobBuilder::new(&pattern)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| LuaError::runtime(format!("invalid glob `{pattern}`: {e}")))?;
                cache.insert(pattern.clone(), glob.compile_matcher());
            }
            let matcher = &cache[&pattern];
            // Patterns with a separator are matched against the full path, others against the name.
            let path = f.path();
            Ok(if pattern.contains('/') {
                matcher.is_match(&path)
            } else {
                path.file_name().is_some_and(|n| matcher.is_match(n))
            })
        })?;

        Ok(())
    }
}
