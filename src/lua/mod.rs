pub mod api;
pub mod file;

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use mlua::prelude::*;

use crate::actions::executor;
use crate::modules::{self, ModuleCtx};
use crate::rules::{Rule, RuleSet};

/// A Lua state with the config loaded and its rules collected.
pub struct Loaded {
    pub lua: Lua,
    pub rules: Vec<Rule>,
}

/// Create the Lua state, install the `slsrt` API and all modules, then run `config`.
pub fn load(config: &Path, dry_run: bool) -> Result<Loaded> {
    let lua = Lua::new();
    let config_dir = config.parent().unwrap_or(Path::new(".")).to_path_buf();

    lua.set_app_data(executor::for_mode(dry_run));
    lua.set_app_data(api::ConfigDir(config_dir.clone()));
    lua.set_app_data(RuleSet::default());

    let setup = || -> LuaResult<()> {
        lua.set_named_registry_value(file::FIELDS_KEY, lua.create_table()?)?;
        lua.set_named_registry_value(file::METHODS_KEY, lua.create_table()?)?;

        let api_table = lua.create_table()?;
        api::install(&lua, &api_table, dry_run)?;
        for module in modules::all() {
            let ctx = ModuleCtx::new(&lua, api_table.clone(), module.name())?;
            module.register(&ctx)?;
        }
        lua.globals().set("slsrt", api_table)?;
        api::extend_package_path(&lua, &config_dir)
    };
    setup().map_err(|e| anyhow!("failed to initialize Lua API: {e}"))?;

    lua.load(config)
        .exec()
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("failed to load config {}", config.display()))?;

    let rules = lua.remove_app_data::<RuleSet>().expect("rule set is installed").into_sorted();
    Ok(Loaded { lua, rules })
}
