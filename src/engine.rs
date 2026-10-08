use std::path::{Path, PathBuf};

use anyhow::Result;
use log::{debug, error, info};
use mlua::prelude::*;

use crate::lua::{self, file::FileHandle};
use crate::modules::actions::Produced;
use crate::rules::Rule;

/// Outcome of running the rules against one file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Names of rules whose action ran successfully.
    pub fired: Vec<String>,
    /// Number of filter/action errors.
    pub errors: usize,
}

pub struct Engine {
    lua: Lua,
    rules: Vec<Rule>,
}

impl Engine {
    pub fn load(config: &Path, dry_run: bool) -> Result<Self> {
        let lua::Loaded { lua, rules } = lua::load(config, dry_run)?;
        Ok(Engine { lua, rules })
    }

    /// Rules in evaluation order.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Paths written by actions since the last call.
    pub fn take_produced(&self) -> Vec<PathBuf> {
        self.lua.app_data_ref::<Produced>().map(|p| p.take()).unwrap_or_default()
    }

    /// Run the applicable rules, in priority order, against the file at `path`.
    ///
    /// Errors never propagate: a failing filter skips that rule, a failing action stops
    /// processing of this file (its state is unknown), and both are logged.
    pub fn process(&self, path: &Path) -> Outcome {
        let mut outcome = Outcome::default();
        let file = FileHandle::new(path.to_path_buf());
        let userdata = match self.lua.create_userdata(file.clone()) {
            Ok(ud) => ud,
            Err(e) => {
                error!("{}: cannot create file object: {e}", path.display());
                outcome.errors += 1;
                return outcome;
            }
        };

        for rule in &self.rules {
            if file.is_removed() {
                break;
            }
            // Re-check with the current path: an earlier `continue` rule may have moved the file.
            if !rule.applies_to(&file.path()) {
                continue;
            }

            if let Some(filter) = &rule.filter {
                match filter.call::<LuaValue>(&userdata) {
                    Ok(v) if !matches!(v, LuaNil | LuaValue::Boolean(false)) => {}
                    Ok(_) => {
                        debug!("rule `{}`: filter rejected {}", rule.name, file.path().display());
                        continue;
                    }
                    Err(e) => {
                        error!("rule `{}`: filter failed for {}: {e}", rule.name, file.path().display());
                        outcome.errors += 1;
                        continue;
                    }
                }
            }

            info!("rule `{}` matched {}", rule.name, file.path().display());
            if let Err(e) = rule.action.call::<()>(&userdata) {
                error!("rule `{}`: action failed for {}: {e}", rule.name, file.path().display());
                outcome.errors += 1;
                break;
            }
            outcome.fired.push(rule.name.clone());
            if !rule.cont {
                break;
            }
        }
        outcome
    }
}
