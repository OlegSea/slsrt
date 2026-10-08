use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::prelude::*;

/// Registry keys of the tables holding module-provided fields and methods.
pub(crate) const FIELDS_KEY: &str = "slsrt.file.fields";
pub(crate) const METHODS_KEY: &str = "slsrt.file.methods";

/// The `file` object handed to rule filters and actions.
///
/// It carries no capabilities of its own: fields (`file.name`) and methods (`file:size()`)
/// are registered by modules (see `crate::modules`) and resolved through `__index`.
#[derive(Clone)]
pub struct FileHandle(Rc<FileState>);

struct FileState {
    path: RefCell<PathBuf>,
    removed: Cell<bool>,
    /// Per-module lazily computed data, keyed by the type the module stores.
    cache: RefCell<HashMap<TypeId, Rc<dyn Any>>>,
}

impl FileHandle {
    pub fn new(path: PathBuf) -> Self {
        FileHandle(Rc::new(FileState {
            path: RefCell::new(path),
            removed: Cell::new(false),
            cache: RefCell::new(HashMap::new()),
        }))
    }

    pub fn path(&self) -> PathBuf {
        self.0.path.borrow().clone()
    }

    /// Point the handle at a new location (after a move/rename) and drop cached data.
    pub fn set_path(&self, path: PathBuf) {
        *self.0.path.borrow_mut() = path;
        self.0.cache.borrow_mut().clear();
    }

    /// Mark the file as gone (after trash/delete); further fs-based access errors out.
    pub fn mark_removed(&self) {
        self.0.removed.set(true);
        self.0.cache.borrow_mut().clear();
    }

    pub fn is_removed(&self) -> bool {
        self.0.removed.get()
    }

    /// Error if the file was removed by an earlier action.
    pub fn ensure_present(&self) -> LuaResult<()> {
        if self.is_removed() {
            Err(LuaError::runtime(format!(
                "file {} was already removed by a previous action",
                self.0.path.borrow().display()
            )))
        } else {
            Ok(())
        }
    }

    /// Get module data of type `T`, computing it with `init` on first access.
    ///
    /// This is how modules keep expensive parsing lazy: nothing is computed unless a
    /// filter actually calls a method that needs it, and it's computed at most once per path.
    pub fn cached<T: 'static>(&self, init: impl FnOnce(&Path) -> LuaResult<T>) -> LuaResult<Rc<T>> {
        self.ensure_present()?;
        let key = TypeId::of::<T>();
        if let Some(value) = self.0.cache.borrow().get(&key) {
            return Ok(Rc::clone(value).downcast::<T>().expect("cache entry has matching type"));
        }
        let value = Rc::new(init(&self.path())?);
        self.0.cache.borrow_mut().insert(key, value.clone() as Rc<dyn Any>);
        Ok(value)
    }

    /// Recover the handle from a Lua value (the implicit `self` of a method call).
    pub fn from_value(value: LuaValue) -> LuaResult<Self> {
        match value {
            LuaValue::UserData(ud) => Ok(ud.borrow::<FileHandle>()?.clone()),
            other => Err(LuaError::runtime(format!(
                "expected a file object, got {} (did you use `.` instead of `:`?)",
                other.type_name()
            ))),
        }
    }
}

impl LuaUserData for FileHandle {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_function(LuaMetaMethod::Index, |lua, (this, key): (LuaAnyUserData, LuaValue)| {
            let fields: LuaTable = lua.named_registry_value(FIELDS_KEY)?;
            if let Some(getter) = fields.raw_get::<Option<LuaFunction>>(&key)? {
                return getter.call::<LuaValue>(this);
            }
            let methods: LuaTable = lua.named_registry_value(METHODS_KEY)?;
            methods.raw_get::<LuaValue>(key)
        });
        methods.add_meta_method(LuaMetaMethod::ToString, |_, this, ()| {
            Ok(format!("file({})", this.path().display()))
        });
    }
}
