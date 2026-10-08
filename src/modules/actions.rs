//! File actions: move, rename, copy, trash, delete.
//!
//! Every action resolves its target, applies the `on_conflict` policy given in the call's
//! options and hands the final operation to the [`Executor`] (real or dry-run).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use mlua::prelude::*;

use super::{Module, ModuleCtx};
use crate::actions::{Executor, OnConflict, conflict};
use crate::lua::file::FileHandle;
use crate::paths;

pub struct Actions;

/// Paths written by actions since the last [`Produced::take`]; lets watch mode ignore
/// events caused by its own moves.
#[derive(Default)]
pub struct Produced(RefCell<Vec<PathBuf>>);

impl Produced {
    pub fn take(&self) -> Vec<PathBuf> {
        std::mem::take(&mut self.0.borrow_mut())
    }
}

/// Parse the optional `{ on_conflict = ... }` table accepted by path-producing actions.
fn parse_opts(opts: Option<LuaTable>) -> LuaResult<OnConflict> {
    let Some(opts) = opts else { return Ok(OnConflict::default()) };
    let mut policy = OnConflict::default();
    for pair in opts.pairs::<String, LuaValue>() {
        let (key, value) = pair?;
        match key.as_str() {
            "on_conflict" => {
                let value = value
                    .as_string()
                    .ok_or_else(|| LuaError::runtime("option `on_conflict` must be a string"))?
                    .to_str()?
                    .to_owned();
                policy = value.parse().map_err(LuaError::runtime)?;
            }
            other => return Err(LuaError::runtime(format!("unknown action option `{other}`"))),
        }
    }
    Ok(policy)
}

/// Resolve a user-supplied path; relative paths are taken relative to the file's directory.
fn target_path(file: &FileHandle, raw: &str) -> LuaResult<PathBuf> {
    let base = file.path().parent().map(Path::to_path_buf).unwrap_or_default();
    paths::resolve(raw, &base).map_err(LuaError::external)
}

fn file_name(file: &FileHandle) -> LuaResult<PathBuf> {
    file.path()
        .file_name()
        .map(PathBuf::from)
        .ok_or_else(|| LuaError::runtime(format!("{} has no file name", file.path().display())))
}

#[derive(Clone, Copy)]
enum Transfer {
    Move,
    Copy,
}

/// Shared implementation of move/rename/copy. Returns the new path, or `nil` if skipped.
fn transfer(lua: &Lua, file: &FileHandle, target: PathBuf, policy: OnConflict, kind: Transfer) -> LuaResult<Option<String>> {
    file.ensure_present()?;
    let from = file.path();
    if target == from {
        return Ok(Some(from.to_string_lossy().into_owned()));
    }
    let Some(to) = conflict::resolve(&target, policy) else {
        log::info!("skipped {}: {} already exists", from.display(), target.display());
        return Ok(None);
    };

    let executor = lua.app_data_ref::<Box<dyn Executor>>().expect("executor is installed");
    match kind {
        Transfer::Move => {
            executor.move_file(&from, &to).map_err(LuaError::external)?;
            file.set_path(to.clone());
        }
        Transfer::Copy => executor.copy_file(&from, &to).map_err(LuaError::external)?,
    }
    if !executor.dry_run()
        && let Some(produced) = lua.app_data_ref::<Produced>()
    {
        produced.0.borrow_mut().push(to.clone());
    }
    Ok(Some(to.to_string_lossy().into_owned()))
}

impl Module for Actions {
    fn name(&self) -> &'static str {
        "actions"
    }

    fn register(&self, ctx: &ModuleCtx) -> LuaResult<()> {
        ctx.lua().set_app_data(Produced::default());

        ctx.file_method("move", |lua, f, (path, opts): (String, Option<LuaTable>)| {
            let target = target_path(f, &path)?;
            transfer(lua, f, target, parse_opts(opts)?, Transfer::Move)
        })?;

        ctx.file_method("move_to", |lua, f, (dir, opts): (String, Option<LuaTable>)| {
            let target = target_path(f, &dir)?.join(file_name(f)?);
            transfer(lua, f, target, parse_opts(opts)?, Transfer::Move)
        })?;

        ctx.file_method("rename", |lua, f, (name, opts): (String, Option<LuaTable>)| {
            if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
                return Err(LuaError::runtime(format!(
                    "invalid file name `{name}` for rename (use move to change directories)"
                )));
            }
            let target = f.path().with_file_name(&name);
            transfer(lua, f, target, parse_opts(opts)?, Transfer::Move)
        })?;

        ctx.file_method("copy_to", |lua, f, (dir, opts): (String, Option<LuaTable>)| {
            let target = target_path(f, &dir)?.join(file_name(f)?);
            transfer(lua, f, target, parse_opts(opts)?, Transfer::Copy)
        })?;

        ctx.file_method("trash", |lua, f, ()| {
            f.ensure_present()?;
            let executor = lua.app_data_ref::<Box<dyn Executor>>().expect("executor is installed");
            executor.trash(&f.path()).map_err(LuaError::external)?;
            f.mark_removed();
            Ok(())
        })?;

        ctx.file_method("delete", |lua, f, ()| {
            f.ensure_present()?;
            let executor = lua.app_data_ref::<Box<dyn Executor>>().expect("executor is installed");
            executor.delete(&f.path()).map_err(LuaError::external)?;
            f.mark_removed();
            Ok(())
        })?;

        Ok(())
    }
}
