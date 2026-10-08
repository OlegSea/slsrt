use std::path::{Path, PathBuf};

use mlua::prelude::*;

use crate::paths;

const KNOWN_KEYS: &[&str] = &[
    "name",
    "description",
    "paths",
    "recursive",
    "priority",
    "enabled",
    "continue",
    "filter",
    "action",
];

/// A validated rule registered from Lua via `slsrt.rule { ... }`.
#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub description: Option<String>,
    /// Absolute (canonical if they existed at load time) directories the rule applies to.
    pub paths: Vec<PathBuf>,
    pub recursive: bool,
    pub priority: i64,
    pub enabled: bool,
    /// Keep evaluating lower-priority rules after this one fires.
    pub cont: bool,
    pub filter: Option<LuaFunction>,
    pub action: LuaFunction,
}

impl Rule {
    /// Parse and validate a rule table. Relative `paths` are resolved against `base_dir`.
    pub fn from_table(table: LuaTable, base_dir: &Path) -> LuaResult<Self> {
        let name: String = match table.raw_get::<LuaValue>("name")? {
            LuaValue::String(s) if !s.to_str()?.trim().is_empty() => s.to_str()?.to_owned(),
            LuaValue::Nil => return Err(LuaError::runtime("rule is missing required field `name`")),
            _ => return Err(LuaError::runtime("rule field `name` must be a non-empty string")),
        };
        let err = |msg: String| LuaError::runtime(format!("rule `{name}`: {msg}"));

        for pair in table.pairs::<LuaValue, LuaValue>() {
            let (key, _) = pair?;
            match &key {
                LuaValue::String(s) if KNOWN_KEYS.contains(&&*s.to_str()?) => {}
                LuaValue::String(s) => {
                    return Err(err(format!(
                        "unknown field `{}` (expected one of: {})",
                        s.to_str()?,
                        KNOWN_KEYS.join(", ")
                    )));
                }
                other => return Err(err(format!("unexpected non-string key {other:?}"))),
            }
        }

        let field = |key: &str, expected: &str| -> LuaResult<LuaValue> {
            let value: LuaValue = table.raw_get(key)?;
            let ok = match expected {
                "string" => value.is_string(),
                "boolean" => value.is_boolean(),
                "integer" => value.is_integer() || value.as_number().is_some_and(|n| n.fract() == 0.0),
                "function" => value.is_function(),
                "table" => value.is_table(),
                _ => unreachable!(),
            };
            if value.is_nil() || ok {
                Ok(value)
            } else {
                Err(err(format!("field `{key}` must be of type {expected}, got {}", value.type_name())))
            }
        };

        let description = field("description", "string")?.as_string().map(|s| s.to_string_lossy());
        let recursive = field("recursive", "boolean")?.as_boolean().unwrap_or(false);
        let enabled = field("enabled", "boolean")?.as_boolean().unwrap_or(true);
        let cont = field("continue", "boolean")?.as_boolean().unwrap_or(false);
        let priority = match field("priority", "integer")? {
            LuaValue::Nil => 0,
            LuaValue::Integer(i) => i,
            LuaValue::Number(n) => n as i64,
            _ => unreachable!(),
        };
        let filter = field("filter", "function")?.as_function().cloned();
        let action = field("action", "function")?
            .as_function()
            .cloned()
            .ok_or_else(|| err("missing required field `action`".into()))?;

        let paths_value = field("paths", "table")?;
        let paths_table = paths_value
            .as_table()
            .ok_or_else(|| err("missing required field `paths` (a list of directories)".into()))?;
        let mut paths = Vec::new();
        for entry in paths_table.sequence_values::<LuaValue>() {
            let raw = match entry? {
                LuaValue::String(s) => s.to_str()?.to_owned(),
                other => return Err(err(format!("`paths` entries must be strings, got {}", other.type_name()))),
            };
            let resolved = paths::resolve(&raw, base_dir).map_err(|e| err(e.to_string()))?;
            paths.push(paths::canonical_or_self(&resolved));
        }
        if paths.is_empty() {
            return Err(err("`paths` must contain at least one directory".into()));
        }

        Ok(Rule { name, description, paths, recursive, priority, enabled, cont, filter, action })
    }

    /// Whether this rule should be considered for the file at `path`.
    pub fn applies_to(&self, path: &Path) -> bool {
        self.enabled
            && self.paths.iter().any(|dir| {
                if self.recursive {
                    path != dir && path.starts_with(dir)
                } else {
                    path.parent() == Some(dir.as_path())
                }
            })
    }
}

/// Rules collected while `main.lua` runs. Stored as Lua app data during loading.
#[derive(Debug, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    pub fn add(&mut self, rule: Rule) -> LuaResult<()> {
        if self.rules.iter().any(|r| r.name == rule.name) {
            return Err(LuaError::runtime(format!("rule `{}` is already registered", rule.name)));
        }
        self.rules.push(rule);
        Ok(())
    }

    /// Final rule order: priority descending, registration order for ties.
    pub fn into_sorted(mut self) -> Vec<Rule> {
        // `sort_by_key` is stable, so equal priorities keep registration order.
        self.rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
        self.rules
    }
}
