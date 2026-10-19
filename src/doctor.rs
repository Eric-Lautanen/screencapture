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
    check_capture();
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
    check_display_choice(settings.display);
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

/// Report attached displays and whether the configured Full-mode display
/// exists right now. A stale choice is informational (records still fall
/// back with an honest error when it is requested and missing).
fn check_display_choice(choice: settings::DisplayChoice) {
    let attached = crate::screen::displays();
    match choice.number() {
        Some(number) => {
            let count = u32::try_from(attached.len()).unwrap_or(u32::MAX);
            let exists = (1..=count).contains(&number);
            if exists {
                println!("        display: {number} of {} attached", attached.len());
            } else {
                println!(
                    "        display: {number} is not attached ({} connected); \
                     recording it answers with an error",
                    attached.len()
                );
            }
        }
        None => println!("        display: primary ({} attached)", attached.len()),
    }
}

/// Report every encoder's usability, inside the engine block of
/// `--check` (non-fatal: hardware misses keep x264). The configured
/// codec is marked so the demotion in the settings file is discoverable.
fn report_codecs(binary: &ffmpeg::Binary, loaded: &settings::Loaded) {
    let mut variants = vec![crate::encoder::Video::X264];
    variants.extend(crate::encoder::Video::hardware());
    let profile = &loaded.settings.profile;
    for variant in variants {
        let usable = binary.supports_encoder(variant, profile);
        let verdict = if usable {
            "ready"
        } else {
            "not usable on this machine"
        };
        let marked = match (variant == loaded.settings.codec, usable) {
            (true, true) => " (configured)",
            (true, false) => " (configured; recordings use x264)",
            (false, _) => "",
        };
        let kind = if variant.is_hardware() { "hw" } else { "cpu" };
        println!(
            "        encoder [{kind}]: {} is {verdict}{marked}",
            variant.name()
        );
    }
}

fn check_engine(config: Option<&Path>) -> bool {
    let load = settings::load(config);
    if let Some(binary) = ffmpeg::discover(load.settings.engine.as_deref()) {
        match binary.version() {
            Ok(version) => {
                println!("{OK_TAG}engine: {}", binary.path.display());
                println!("        {version}");
                report_codecs(&binary, &load);
                check_engine_integrity(&binary)
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

/// Engine integrity: the binary's SHA-256 against its neighboring
/// `engine.sha256` record. A missing record self-records (first run after
/// a fresh unpack); a mismatch fails loudly and points at the recovery
/// path, so a silently swapped engine never ships.
fn check_engine_integrity(binary: &ffmpeg::Binary) -> bool {
    let Some(actual) = file_digest(&binary.path) else {
        println!(
            "{FAIL_TAG}engine integrity: cannot hash {}",
            binary.path.display()
        );
        return false;
    };
    let actual_hex = crate::sha256::hex(&actual);
    let record = binary.path.with_file_name("engine.sha256");
    match std::fs::read_to_string(&record) {
        Ok(recorded) => {
            let expected = recorded.trim();
            if expected.len() >= 64 && expected[..64] == actual_hex {
                println!("{OK_TAG}engine integrity: {}..", &actual_hex[..8]);
                true
            } else {
                println!(
                    "{FAIL_TAG}engine integrity: {} does not match its record",
                    binary.path.display()
                );
                println!(
                    "        hint: delete {} after a deliberate engine update to re-record it",
                    record.display()
                );
                false
            }
        }
        Err(_) => match std::fs::write(&record, format!("{actual_hex}\n")) {
            Ok(()) => {
                println!("{OK_TAG}engine integrity: recorded (first run)");
                true
            }
            Err(error) => {
                println!("{FAIL_TAG}engine integrity: cannot write the record ({error})");
                false
            }
        },
    }
}

/// Hash a file in 1 MiB chunks; `None` on any read/IO failure.
fn file_digest(path: &Path) -> Option<[u8; 32]> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = crate::sha256::Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Some(hasher.finish()),
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return None,
        }
    }
}

/// Informational capture-backends line: desktop duplication when the
/// compositor cooperates, which is the fast 4K path. A miss is not an
/// unhealthy install; the GDI fallback records everything, just slower.
#[cfg(windows)]
fn check_capture() {
    match crate::dxgi::probe() {
        Ok((width, height)) => {
            println!("{OK_TAG}capture: desktop duplication ({width}x{height})");
        }
        Err(reason) => {
            println!("        capture: GDI fallback only ({reason})");
        }
    }
}

/// Capture-backends line outside Windows: the native compositor or server
/// backend IS the fast path, so its absence is an unhealthy install.
#[cfg(not(windows))]
fn check_capture() {
    match crate::capture::probe() {
        Ok((label, width, height)) => {
            println!("{OK_TAG}capture: {label} ({width}x{height})");
        }
        Err(reason) => {
            println!("{FAIL_TAG}capture: {reason}");
        }
    }
}
