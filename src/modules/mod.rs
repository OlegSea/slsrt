//! Capability modules.
//!
//! A module adds fields/methods to the `file` object and/or functions to the global `slsrt`
//! table. The engine, rules and `FileHandle` know nothing about individual modules, so a new
//! capability (media info, EXIF, ...) is a new file here plus one line in [`all`].

pub mod actions;
pub mod meta;
pub mod video;

use mlua::prelude::*;

use crate::lua::file::{FIELDS_KEY, FileHandle, METHODS_KEY};

pub trait Module {
    fn name(&self) -> &'static str;
    fn register(&self, ctx: &ModuleCtx) -> LuaResult<()>;
}

/// Every module installed into the Lua state, in registration order.
pub fn all() -> Vec<Box<dyn Module>> {
    vec![Box::new(meta::Meta), Box::new(actions::Actions), Box::new(video::Video)]
}

/// Registration API handed to modules.
pub struct ModuleCtx<'a> {
    lua: &'a Lua,
    api: LuaTable,
    fields: LuaTable,
    methods: LuaTable,
    module: &'static str,
}

impl<'a> ModuleCtx<'a> {
    pub(crate) fn new(lua: &'a Lua, api: LuaTable, module: &'static str) -> LuaResult<Self> {
        Ok(ModuleCtx {
            lua,
            api,
            fields: lua.named_registry_value(FIELDS_KEY)?,
            methods: lua.named_registry_value(METHODS_KEY)?,
            module,
        })
    }

    pub fn lua(&self) -> &Lua {
        self.lua
    }

    /// Register a read-only field, accessed as `file.<name>`.
    pub fn file_field<R, F>(&self, name: &str, getter: F) -> LuaResult<()>
    where
        R: IntoLua,
        F: Fn(&Lua, &FileHandle) -> LuaResult<R> + 'static,
    {
        self.ensure_unique(name)?;
        let f = self.lua.create_function(move |lua, this: LuaValue| getter(lua, &FileHandle::from_value(this)?))?;
        self.fields.raw_set(name, f)
    }

    /// Register a method, called as `file:<name>(args...)`.
    pub fn file_method<A, R, F>(&self, name: &str, method: F) -> LuaResult<()>
    where
        A: FromLuaMulti,
        R: IntoLuaMulti,
        F: Fn(&Lua, &FileHandle, A) -> LuaResult<R> + 'static,
    {
        self.ensure_unique(name)?;
        let f = self.lua.create_function(move |lua, mut args: LuaMultiValue| {
            let this = FileHandle::from_value(args.pop_front().unwrap_or(LuaNil))?;
            method(lua, &this, A::from_lua_multi(args, lua)?)
        })?;
        self.methods.raw_set(name, f)
    }

    /// Register a function on the global `slsrt` table.
    pub fn api_fn<A, R, F>(&self, name: &str, func: F) -> LuaResult<()>
    where
        A: FromLuaMulti,
        R: IntoLuaMulti,
        F: Fn(&Lua, A) -> LuaResult<R> + 'static,
    {
        if self.api.contains_key(name)? {
            return Err(self.conflict(name, "slsrt"));
        }
        self.api.raw_set(name, self.lua.create_function(func)?)
    }

    fn ensure_unique(&self, name: &str) -> LuaResult<()> {
        if self.fields.contains_key(name)? || self.methods.contains_key(name)? {
            Err(self.conflict(name, "file"))
        } else {
            Ok(())
        }
    }

    fn conflict(&self, name: &str, target: &str) -> LuaError {
        LuaError::runtime(format!("module `{}`: `{target}.{name}` is already defined", self.module))
    }
}
