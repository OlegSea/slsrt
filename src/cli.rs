use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

/// Sort files into directories according to a Lua ruleset.
#[derive(Debug, Parser)]
#[command(name = "slsrt", version, about)]
pub struct Cli {
    /// Path to the entry config file (defaults to `main.lua` in the OS config directory).
    #[arg(short, long, global = true, value_name = "MAIN_LUA")]
    pub config: Option<PathBuf>,

    /// Log what actions would do without touching any files.
    #[arg(short = 'n', long, global = true)]
    pub dry_run: bool,

    /// Increase log verbosity (-v debug, -vv trace).
    #[arg(short, long, global = true, action = ArgAction::Count, conflicts_with = "quiet")]
    pub verbose: u8,

    /// Decrease log verbosity (-q warnings only, -qq errors only).
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub quiet: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Scan every directory referenced by the rules once and apply the rules.
    Run,
    /// Watch every directory referenced by the rules and process new files.
    Watch {
        /// How long a file must stay unchanged before it is processed, in milliseconds.
        #[arg(long, default_value_t = 2000, value_name = "MS")]
        settle_ms: u64,
    },
    /// Load and validate the config, then list the registered rules.
    Check,
}

impl Cli {
    pub fn log_level(&self) -> log::LevelFilter {
        use log::LevelFilter::*;
        match (self.verbose, self.quiet) {
            (0, 0) => Info,
            (1, _) => Debug,
            (v, _) if v >= 2 => Trace,
            (_, 1) => Warn,
            _ => Error,
        }
    }
}
