use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use log::{debug, error, info};
use notify::event::{EventKind, ModifyKind};
use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer};

use crate::engine::Engine;
use crate::scan;

/// Watch every rule directory and process files once they've been quiet for `settle`.
/// Runs until the process is terminated or the watcher fails.
pub fn watch(engine: &Engine, settle: Duration) -> Result<()> {
    let targets = scan::targets(engine.rules());
    if targets.is_empty() {
        bail!("no existing directories to watch (check the rules' `paths`)");
    }

    let (tx, rx) = mpsc::channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(settle, None, tx).context("failed to start file watcher")?;
    for target in &targets {
        let mode = if target.recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        debouncer
            .watch(&target.dir, mode)
            .with_context(|| format!("failed to watch {}", target.dir.display()))?;
        info!("watching {}{}", target.dir.display(), if target.recursive { " (recursive)" } else { "" });
    }

    // Files our own actions produced; their create events must not re-trigger the rules.
    let ignore_for = settle * 3 + Duration::from_secs(5);
    let mut produced: HashMap<PathBuf, Instant> = HashMap::new();

    for result in rx {
        let events = match result {
            Ok(events) => events,
            Err(errors) => {
                errors.iter().for_each(|e| error!("watch error: {e}"));
                continue;
            }
        };

        let now = Instant::now();
        produced.retain(|_, at| now.duration_since(*at) < ignore_for);

        let paths: BTreeSet<PathBuf> = events
            .into_iter()
            .filter(|e| matches!(e.kind, EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))))
            .flat_map(|e| e.event.paths)
            .collect();

        for path in paths {
            if !scan::is_file(&path) {
                continue;
            }
            if produced.remove(&path).is_some() {
                debug!("ignoring {}: produced by a previous action", path.display());
                continue;
            }
            engine.process(&path);
        }

        let now = Instant::now();
        produced.extend(engine.take_produced().into_iter().map(|p| (p, now)));
    }
    Ok(())
}
