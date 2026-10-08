//! Core functions of the global `slsrt` table that are not tied to a capability module.

use std::path::{Path, PathBuf};

use mlua::prelude::*;

use crate::paths;
use crate::rules::{Rule, RuleSet};

/// Directory of the entry config; base for relative rule paths.
pub(crate) struct ConfigDir(pub PathBuf);

pub(crate) fn install(lua: &Lua, api: &LuaTable, dry_run: bool) -> LuaResult<()> {
    api.raw_set(
        "rule",
        lua.create_function(|lua, table: LuaTable| {
            let base = lua.app_data_ref::<ConfigDir>().expect("config dir is installed").0.clone();
            let rule = Rule::from_table(table, &base)?;
            let mut rules = lua.app_data_mut::<RuleSet>().ok_or_else(|| {
                LuaError::runtime("rules can only be registered while the config is loading")
            })?;
            rules.add(rule)
        })?,
    )?;

    api.raw_set("dry_run", dry_run)?;

    for (name, bytes) in [("KB", 1u64 << 10), ("MB", 1 << 20), ("GB", 1 << 30), ("TB", 1 << 40)] {
        api.raw_set(name, bytes)?;
    }
    for (name, secs) in [("minutes", 60.0), ("hours", 3600.0), ("days", 86400.0)] {
        api.raw_set(name, lua.create_function(move |_, n: f64| Ok(n * secs))?)?;
    }

    api.raw_set(
        "expand",
        lua.create_function(|_, path: String| {
            Ok(paths::expand(&path).map_err(LuaError::external)?.to_string_lossy().into_owned())
        })?,
    )?;

    let path = lua.create_table()?;
    path.raw_set(
        "join",
        lua.create_function(|_, parts: LuaVariadic<String>| {
            let joined: PathBuf = parts.iter().collect();
            Ok(joined.to_string_lossy().into_owned())
        })?,
    )?;
    path.raw_set(
        "exists",
        lua.create_function(|_, p: String| {
            Ok(paths::expand(&p).map_err(LuaError::external)?.symlink_metadata().is_ok())
        })?,
    )?;
    api.raw_set("path", path)?;

    let log = lua.create_table()?;
    for (name, level) in [
        ("debug", log::Level::Debug),
        ("info", log::Level::Info),
        ("warn", log::Level::Warn),
        ("error", log::Level::Error),
    ] {
        log.raw_set(
            name,
            lua.create_function(move |_, msg: LuaValue| {
                log::log!(target: "lua", level, "{}", msg.to_string()?);
                Ok(())
            })?,
        )?;
    }
    api.raw_set("log", log)?;

    Ok(())
}

/// Prepend the config directory to `package.path` so configs can `require` sibling files.
pub(crate) fn extend_package_path(lua: &Lua, dir: &Path) -> LuaResult<()> {
    let package: LuaTable = lua.globals().get("package")?;
    let current: String = package.get("path")?;
    let dir = dir.to_string_lossy();
    let sep = std::path::MAIN_SEPARATOR;
    package.set("path", format!("{dir}{sep}?.lua;{dir}{sep}?{sep}init.lua;{current}"))
}
