//! Installation diagnostics: a human-readable health report.

use std::path::Path;
use std::process::ExitCode;

use crate::autostart;
use crate::cli::IDENTITY;
use crate::ffmpeg;
use crate::settings;

const OK_TAG: &str = "ok    ";
const FAIL_TAG: &str = "FAIL  ";

/// Run diagnostics and print a report.
pub fn check(config: Option<&Path>) -> ExitCode {
    println!("{IDENTITY}: check");
    let mut healthy = true;
    println!();
    healthy &= check_settings(config);
    healthy &= check_engine(config);
    println!();
    if healthy {
        println!("result: ok");
        ExitCode::SUCCESS
    } else {
        println!("result: problems found");
        ExitCode::FAILURE
    }
}

/// `--init`: write the default settings document when nothing exists.
pub fn init_config(config: Option<&Path>) -> ExitCode {
    let loaded = settings::load(config);
    let Some(path) = loaded.path.as_deref() else {
        eprintln!("FAIL  settings: cannot resolve user directory");
        return ExitCode::FAILURE;
    };
    if loaded.found {
        println!("{OK_TAG}settings: already present at {}", path.display());
        return ExitCode::SUCCESS;
    }
    if path.exists() {
        eprintln!(
            "FAIL  settings: {} exists but cannot be read; not overwriting",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    for problem in &loaded.problems {
        eprintln!("warn  settings: {problem}");
    }
    match loaded.document.persist(path) {
        Ok(()) => {
            println!("{OK_TAG}settings: wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("FAIL  settings: cannot write {}: {error}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn check_settings(config: Option<&Path>) -> bool {
    let loaded = settings::load(config);
    let Some(path) = loaded.path.as_deref() else {
        println!("{FAIL_TAG}settings: cannot resolve user directory");
        return false;
    };
    println!("settings file: {}", path.display());
    if !loaded.found {
        println!("        not found; defaults in use (run `screencapture --init`)");
    }
    for problem in &loaded.problems {
        println!("        problem: {problem}");
    }
    let settings = &loaded.settings;
    println!("        profile: {}", settings.profile);
    println!(
        "        hotkeys: record {} / photo {}",
        settings.hotkey, settings.photo
    );
    println!(
        "        runtime: autostart={} cursor={}",
        settings.autostart, settings.cursor
    );
    println!("        media:   {}", settings.media_dir.display());
    println!("        codec:   {}", settings.codec.name());
    println!(
        "        engine:  {}",
        settings
            .engine
            .as_ref()
            .map_or_else(|| "bundled".to_owned(), |path| path.display().to_string())
    );
    println!("        autostart os-level: {}", autostart::registered());
    loaded.problems.is_empty()
}

fn check_engine(config: Option<&Path>) -> bool {
    let load = settings::load(config);
    if let Some(binary) = ffmpeg::discover(load.settings.engine.as_deref()) {
        match binary.version() {
            Ok(version) => {
                println!("{OK_TAG}engine: {}", binary.path.display());
                println!("        {version}");
                true
            }
            Err(error) => {
                println!(
                    "{FAIL_TAG}engine: {} failed to run: {error}",
                    binary.path.display()
                );
                false
            }
        }
    } else {
        println!("{FAIL_TAG}engine: ffmpeg not found");
        println!("        hint: place {ENGINE_HINT} beside the executable or set [engine] path");
        false
    }
}

const ENGINE_HINT: &str = "ffmpeg/ffmpeg";
