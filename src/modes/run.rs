use anyhow::Result;
use log::info;

use crate::engine::Engine;
use crate::scan;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub scanned: usize,
    pub matched: usize,
    pub errors: usize,
}

/// Scan every rule directory once and process each file found.
pub fn run(engine: &Engine) -> Result<Summary> {
    let targets = scan::targets(engine.rules());
    // Snapshot first, so files moved into another rule's directory aren't processed twice.
    let files = scan::files(&targets);

    let mut summary = Summary { scanned: files.len(), ..Default::default() };
    for path in &files {
        if !scan::is_file(path) {
            continue; // removed or moved by something else meanwhile
        }
        let outcome = engine.process(path);
        summary.matched += usize::from(!outcome.fired.is_empty());
        summary.errors += outcome.errors;
    }

    info!(
        "processed {} file(s) in {} director{}: {} matched, {} error(s)",
        summary.scanned,
        targets.len(),
        if targets.len() == 1 { "y" } else { "ies" },
        summary.matched,
        summary.errors
    );
    Ok(summary)
}
