use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

use slsrt::cli::{Cli, Command};
use slsrt::engine::Engine;
use slsrt::{config, modes};

fn main() -> ExitCode {
    let cli = Cli::parse();
    env_logger::Builder::new()
        .filter_level(cli.log_level())
        .parse_default_env()
        .init();

    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            log::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let config = config::locate(cli.config)?;
    log::debug!("using config {}", config.display());
    let engine = Engine::load(&config, cli.dry_run)?;

    match cli.command {
        Command::Check => {
            print_rules(&engine);
            Ok(ExitCode::SUCCESS)
        }
        Command::Run => {
            let summary = modes::run::run(&engine)?;
            Ok(if summary.errors > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS })
        }
        Command::Watch { settle_ms } => {
            modes::watch::watch(&engine, Duration::from_millis(settle_ms))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn print_rules(engine: &Engine) {
    let rules = engine.rules();
    println!("{} rule(s), in evaluation order:", rules.len());
    for rule in rules {
        let mut flags = Vec::new();
        if !rule.enabled {
            flags.push("disabled");
        }
        if rule.recursive {
            flags.push("recursive");
        }
        if rule.cont {
            flags.push("continue");
        }
        let flags = if flags.is_empty() { String::new() } else { format!(" [{}]", flags.join(", ")) };
        println!("  {:>4}  {}{flags}", rule.priority, rule.name);
        if let Some(description) = &rule.description {
            println!("        {description}");
        }
        for dir in &rule.paths {
            let missing = if dir.is_dir() { "" } else { "  (missing)" };
            println!("        - {}{missing}", dir.display());
        }
    }
}
