//! Installation diagnostics: a human-readable health report.

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

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
    let capture = check_capture();
    healthy &= capture;
    if capture {
        check_throughput();
    }
    println!();
    if healthy {
        println!("result: ok");
        ExitCode::SUCCESS
    } else {
        println!("result: problems found");
        ExitCode::FAILURE
    }
}

/// `--init`: write the commented settings template when nothing exists.
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
    match settings::write_template(path) {
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
        "        hotkeys: record {} / photo {} / chimes {}",
        settings.hotkey.to_combo(),
        settings.photo.to_combo(),
        settings.chime_hotkey.to_combo()
    );
    println!(
        "        runtime: autostart={} cursor={} chimes={}",
        settings.autostart, settings.cursor, settings.chimes
    );
    println!("        videos:  {}", settings.video_dir.display());
    println!("        images:  {}", settings.image_dir.display());
    println!("        codec:   {}", settings.codec.name());
    println!(
        "        audio:   system={} mic={} quality={}",
        settings.audio.system,
        settings.audio.mic,
        settings.audio.quality.label()
    );
    println!(
        "        levels:  mic {} / system {}",
        level_text(settings.audio_levels.mic_db),
        level_text(settings.audio_levels.system_db)
    );
    #[cfg(windows)]
    for line in audio_endpoints(settings) {
        println!("        {line}");
    }
    println!(
        "        photo:   {}",
        photo_text(settings.photo_depth, settings.photo_quality)
    );
    check_display_choice(settings.display);
    println!(
        "        engine:  {}",
        settings
            .engine
            .as_ref()
            .map_or_else(|| "bundled".to_owned(), |path| path.display().to_string())
    );
    println!("        autostart os-level: {}", autostart::registered());
    println!("        chime:   {}", chime_status(settings.chimes));
    check_log_file(&loaded.settings.video_dir);
    loaded.problems.is_empty()
}

/// What a still will be written as: the lossless PNG at its depth, or the
/// JPEG step. A lossy choice has no depth to report — every JPEG here is
/// 8-bit — so saying "16-bit" beside it would be a lie about the file.
fn photo_text(
    depth: crate::settings::StillDepth,
    quality: crate::settings::StillQuality,
) -> String {
    if quality == crate::settings::StillQuality::Png {
        format!("{}, {depth}-bit", quality.label())
    } else {
        // A JPEG has no depth of its own: the encoder always writes 8-bit,
        // whatever the PNG setting says.
        format!("{}, 8-bit", quality.label())
    }
}

/// A fader position as the text beside it, signed for a boost.
fn level_text(db: i32) -> String {
    if db > 0 {
        format!("+{db} dB")
    } else {
        format!("{db} dB")
    }
}

/// The endpoint each source will record from, and whether it is there.
///
/// `mic = true` alone never explains a silent track: what matters is which
/// device the take will open, whether it is connected, and whether the
/// machine remembers any at all. A chosen device that is away is stood in
/// for by the Windows default, which is worth seeing before a recording
/// rather than in the log afterwards.
#[cfg(windows)]
fn audio_endpoints(settings: &settings::Settings) -> Vec<String> {
    let mut lines = Vec::new();
    for (label, flow, chosen) in [
        (
            "mic:    ",
            crate::audio::devices::RECORDING,
            settings.audio_devices.mic.as_ref(),
        ),
        (
            "system: ",
            crate::audio::devices::PLAYBACK,
            settings.audio_devices.system.as_ref(),
        ),
    ] {
        let remembered = crate::audio::devices::list(flow)
            .ok()
            .map(|devices| devices.len());
        let connected = chosen.is_some_and(|endpoint| crate::audio::devices::present(&endpoint.id));
        lines.push(endpoint_line(
            label,
            chosen.map(|endpoint| endpoint.name.as_str()),
            connected,
            remembered,
        ));
    }
    lines
}

/// One endpoint line: the Windows default, a connected choice, a choice that
/// is away, or an enumeration that answered nothing.
#[cfg(windows)]
fn endpoint_line(
    label: &str,
    chosen: Option<&str>,
    connected: bool,
    remembered: Option<usize>,
) -> String {
    let remembered = remembered.map_or_else(
        || "no endpoint list".to_owned(),
        |count| format!("{count} remembered"),
    );
    let source = match chosen {
        None => format!("System default ({remembered})"),
        Some(name) if connected => format!("{name} ({remembered})"),
        Some(name) => format!("{name} (not connected; using the default)"),
    };
    format!("{label}{source}")
}

/// Where the confirmation chime goes, and whether this machine can play it
/// at all. A silent capture on a fresh machine is usually zero output
/// devices (a VM, a remote session, or a disabled audio service), which the
/// settings themselves can never show.
fn chime_status(enabled: bool) -> String {
    if !enabled {
        return "muted in settings".to_owned();
    }
    match crate::sound::output_devices() {
        Some(0) => "no audio output device; captures will be silent".to_owned(),
        Some(count) => format!("{count} output device(s)"),
        None => "not available on this platform".to_owned(),
    }
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
/// `--check`. The configured codec is marked, and an unusable one gets
/// the fallback that the recorder will actually choose, so a demotion is
/// discoverable before the first recording.
fn report_codecs(binary: &ffmpeg::Binary, loaded: &settings::Loaded) {
    let requested = loaded.settings.codec;
    let mut variants = vec![crate::encoder::Video::X264, crate::encoder::Video::X265];
    variants.extend(crate::encoder::Video::hardware());
    let profile = &loaded.settings.profile;
    let availability: Vec<_> = variants
        .into_iter()
        .map(|variant| (variant, binary.supports_encoder(variant, profile)))
        .collect();
    for &(variant, usable) in &availability {
        let verdict = if usable {
            "ready"
        } else {
            "not usable on this machine"
        };
        let marked = match (variant == requested, usable) {
            (true, true) => " (configured)",
            (true, false) => " (configured; recordings fall back)",
            (false, _) => "",
        };
        let kind = if variant.is_hardware() { "hw" } else { "cpu" };
        println!(
            "        encoder [{kind}]: {} is {verdict}{marked}",
            variant.name()
        );
    }
    if !availability
        .iter()
        .any(|&(variant, usable)| variant == requested && usable)
    {
        let chosen = requested.choose(|variant| {
            availability
                .iter()
                .any(|&(candidate, usable)| candidate == variant && usable)
        });
        match chosen {
            Some(codec) => println!(
                "        encoder fallback: {} will be used at record start",
                codec.name()
            ),
            None => println!("        encoder fallback: none; recordings will refuse to start"),
        }
    }
}

/// The tray keeps recent diagnostics in a log file in the media
/// directory; report it, hits only (size just counts lines).
fn check_log_file(media_dir: &Path) {
    let file = media_dir.join(crate::log::FILE_NAME);
    let Ok(text) = std::fs::read_to_string(&file) else {
        println!("log file: none yet under {}", media_dir.display());
        return;
    };
    println!(
        "{OK_TAG}log file: {} ({} lines)",
        file.display(),
        text.lines().count()
    );
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
            if digest_matches(&recorded, &actual_hex) {
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

/// Whether the record's first 64 bytes are the digest hex. Byte comparison
/// keeps a corrupted, multibyte record from panicking on a char boundary.
fn digest_matches(recorded: &str, actual_hex: &str) -> bool {
    let recorded = recorded.trim().as_bytes();
    recorded.len() >= 64 && &recorded[..64] == actual_hex.as_bytes()
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
fn check_capture() -> bool {
    match crate::dxgi::probe() {
        Ok((width, height)) => {
            println!("{OK_TAG}capture: desktop duplication ({width}x{height})");
        }
        Err(reason) => {
            println!("        capture: GDI fallback only ({reason})");
        }
    }
    // Duplication is an optimization here; GDI records everything.
    true
}

/// Capture-backends line outside Windows: the native compositor or server
/// backend IS the fast path, so its absence is an unhealthy install.
#[cfg(not(windows))]
fn check_capture() -> bool {
    match crate::capture::probe() {
        Ok((label, width, height)) => {
            println!("{OK_TAG}capture: {label} ({width}x{height})");
            true
        }
        Err(reason) => {
            println!("{FAIL_TAG}capture: {reason}");
            println!("        {}", crate::capture::hint());
            false
        }
    }
}

/// Frames in the throughput sample; enough to average timer noise without
/// making `--check` slow on a 4K desktop.
const TIMED_FRAMES: u32 = 6;

/// Measure one short capture burst through the fastest available backend
/// and print real numbers. A refusal (locked desktop, no compositor) is
/// informational: the install is still healthy, it just cannot be measured.
fn check_throughput() {
    let Ok(display) = crate::screen::primary() else {
        println!("timing: no display available to measure");
        return;
    };
    let region = crate::capture::Region {
        x: display.x,
        y: display.y,
        width: display.width,
        height: display.height,
    };
    // Duplication's idle path reuses its last frame without touching the
    // GPU, so timing it would report a fictional rate. The blit backend
    // copies every pixel on every frame and is the honest baseline here;
    // duplication throughput is reported from real recordings by the
    // recorder's measured-fps log line.
    let measured = crate::capture::Target::new(region, false)
        .ok()
        .and_then(|mut target| measure(TIMED_FRAMES, || target.frame().map(|_| ())));
    let label = if cfg!(windows) {
        "GDI blit"
    } else {
        "native backend"
    };
    report_timing(label, measured);
}

/// Run `grab` up to `frames` times and time the successful ones.
fn measure<F>(frames: u32, mut grab: F) -> Option<(u32, Duration)>
where
    F: FnMut() -> std::io::Result<()>,
{
    let started = Instant::now();
    let mut captured = 0;
    for _ in 0..frames {
        if grab().is_err() {
            break;
        }
        captured += 1;
    }
    (captured > 0).then(|| (captured, started.elapsed()))
}

fn report_timing(label: &str, measured: Option<(u32, Duration)>) {
    match measured {
        Some((frames, elapsed)) => {
            let seconds = elapsed.as_secs_f64();
            let fps = if seconds > 0.0 {
                f64::from(frames) / seconds
            } else {
                0.0
            };
            let per_frame = if frames == 0 {
                0.0
            } else {
                seconds * 1_000.0 / f64::from(frames)
            };
            println!(
                "timing: {label}: {frames} frames in {seconds:.3} s ({fps:.1} fps, {per_frame:.2} ms/frame)"
            );
        }
        None => println!("timing: {label}: no frame captured (locked desktop or refused backend)"),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::endpoint_line;
    use super::{digest_matches, level_text, photo_text};
    use crate::settings::{StillDepth, StillQuality};

    #[test]
    fn a_still_reports_the_form_it_will_be_written_as() {
        assert_eq!(
            photo_text(StillDepth::Eight, StillQuality::Png),
            "Lossless — PNG, 8-bit"
        );
        assert_eq!(
            photo_text(StillDepth::Sixteen, StillQuality::Png),
            "Lossless — PNG, 16-bit"
        );
        // Only the PNG has a depth to report. The other lossless forms and
        // the JPEG steps are 8-bit whatever the setting says, and the report
        // says so rather than repeating a depth the file does not have.
        assert_eq!(
            photo_text(StillDepth::Sixteen, StillQuality::Webp),
            "Lossless — WebP, 8-bit"
        );
        assert_eq!(
            photo_text(StillDepth::Sixteen, StillQuality::Bmp),
            "Lossless — BMP, 8-bit"
        );
        assert_eq!(
            photo_text(StillDepth::Sixteen, StillQuality::Compact),
            "Compact — JPEG 70, 8-bit"
        );
    }

    #[test]
    fn a_fader_reads_signed_in_decibels() {
        assert_eq!(level_text(0), "0 dB");
        assert_eq!(level_text(-60), "-60 dB");
        assert_eq!(level_text(12), "+12 dB");
    }

    #[cfg(windows)]
    #[test]
    fn an_endpoint_line_names_what_the_take_will_open() {
        assert_eq!(
            endpoint_line("mic:    ", None, false, Some(4)),
            "mic:    System default (4 remembered)"
        );
        assert_eq!(
            endpoint_line("system: ", Some("Speakers"), true, Some(8)),
            "system: Speakers (8 remembered)"
        );
        // A chosen device that is away still names itself, and says what the
        // take does instead.
        assert_eq!(
            endpoint_line("mic:    ", Some("Headset"), false, Some(4)),
            "mic:    Headset (not connected; using the default)"
        );
        assert_eq!(
            endpoint_line("mic:    ", None, false, None),
            "mic:    System default (no endpoint list)"
        );
    }

    #[test]
    fn digest_records_compare_bytes() {
        let digest = "a".repeat(64);
        assert!(digest_matches(&format!("{digest}\n"), &digest));
        assert!(!digest_matches(&digest[..63], &digest));
        // A multibyte record must mismatch, not panic on a char boundary.
        let corrupt = format!("{}{}", "é".repeat(32), "x".repeat(10));
        assert!(!digest_matches(&corrupt, &digest));
    }
}
