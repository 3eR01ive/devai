//! Operator probe for the homes registry plane (REQ-HOME-01…09) — runs the
//! exact UI bootstrap against real homes without opening a window:
//! load the registry of the control home, auto-register the launch home,
//! list records, optionally add a home, and print the merged task view.
//!
//!   cargo run -p devai-api --example homes_probe [-- <launch-home> [add-path ...]]
//!
//! The probe never mutates any observed store: it uses only the registry
//! file of the control home (homes.json) and read-only observations.

use std::env::args;
use std::path::{Path, PathBuf};

use devai_api::{HOME_DIR_NAME, Homes};

fn main() -> Result<(), String> {
    let mut argv = args().skip(1);
    let launch = argv
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join(HOME_DIR_NAME));
    let control = dirs_home().join(HOME_DIR_NAME);

    let mut homes = Homes::load(&control).map_err(|e| e.to_string())?;
    let active = homes
        .bootstrap_launch(&launch, std::env::var("DEVAI_HOME").is_ok())
        .map_err(|e| e.to_string())?
        .active;
    println!("control home: {}", control.display());
    match &active {
        Some(path) => println!(
            "stored active: {} (env set: {})",
            path.display(),
            std::env::var("DEVAI_HOME").is_ok()
        ),
        None => println!(
            "stored active: none — every home is switched off (env set: {})",
            std::env::var("DEVAI_HOME").is_ok()
        ),
    }

    for path in argv {
        match homes.add(Path::new(&path)) {
            Ok(r) => println!(
                "added {} → project={} v={:?}",
                r.path, r.project, r.schema_version
            ),
            Err(e) => println!("add {path} REFUSED [{}]: {e}", e.code().0),
        }
    }

    println!("-- home.list --");
    for r in homes.records().map_err(|e| e.to_string())? {
        println!(
            "{} {} {} enabled={} available={} v={:?} {}",
            if r.active { "[A]" } else { "[ ]" },
            r.project,
            r.path,
            r.enabled,
            r.available,
            r.schema_version,
            r.read_error
                .as_ref()
                .map(|i| format!("[{}] {} {}", i.code, i.path, i.message))
                .or_else(|| r.version_note.clone())
                .unwrap_or_default()
        );
    }

    println!("-- home.tasks_all (first 6 per home) --");
    let view = homes.tasks_all(None).map_err(|e| e.to_string())?;
    for block in view {
        println!(
            "### {} · {} · active={} available={} snapshot={}",
            block.home_project,
            block.home_path,
            block.is_active,
            block.available,
            block.snapshot_at
        );
        if let Some(err) = &block.error {
            println!("  error [{}] {}: {}", err.code, err.path, err.message);
        }
        for t in block.records.iter().take(6) {
            println!(
                "  {} [{}] {}",
                short(&t.id.to_string()),
                t.state.as_str(),
                t.title
            );
        }
        println!("  ({} tasks total)", block.records.len());
    }
    Ok(())
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn short(id: &str) -> String {
    id.chars().take(12).collect()
}
