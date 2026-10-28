//! Command-line front door: parsing, help text, and dispatch.

use std::io::{Write, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::capture::Region;
use crate::settings::CaptureMode;
use crate::{app, console, doctor};

/// Display identity used by version output and diagnostics.
pub const IDENTITY: &str = concat!("screencapture ", env!("CARGO_PKG_VERSION"));

const HELP: &str = "\
screencapture - 4K screen recorder and still-image capture

USAGE:
    screencapture              start the tray application
    screencapture --settings   open the settings window, then exit
    screencapture --record S   record for S seconds, then exit
    screencapture --record S --mode full | region | window
    screencapture --display N  capture display N (1-based) in full mode
    screencapture --rect X,Y,W,H  capture one rectangle (region records, photos)
    screencapture --photo [--mode ...]  take one still photo, then exit
    screencapture --check      diagnose install path, settings, and engine
    screencapture --init       write the commented settings template if missing
    screencapture --config P   use a specific settings file
    screencapture --version    print the version
    screencapture --help       print this help
";

/// Modes the binary can run in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    /// Print the version.
    Version,
    /// Print usage.
    Help,
    /// Run installation diagnostics.
    Check,
    /// Ensure the default settings file exists.
    InitConfig,
    /// Open the settings window once, then exit.
    Settings,
    /// Record for a fixed number of seconds, then exit.
    Record { seconds: u32 },
    /// Take one still photo, then exit.
    Photo,
    /// Start the tray application.
    Run,
}

/// A parsed command with optional settings path and capture overrides.
struct Invocation {
    command: Command,
    config: Option<PathBuf>,
    mode: Option<CaptureMode>,
    /// 1-based display number for Full captures.
    display: Option<u32>,
    /// Headless capture rectangle: region records and photos use it
    /// verbatim, so any platform can run an unattended area capture.
    rect: Option<Region>,
}

/// Parse `args` and dispatch; returns the process exit code.
pub fn run(args: &[String]) -> ExitCode {
    match parse(args) {
        Ok(invocation) => {
            if matches!(invocation.command, Command::Run) {
                // Say why a second launch is about to end, while there is
                // still a terminal to say it in: the tray keeps no console,
                // and a line later the explanation would go nowhere.
                if crate::instance::running() {
                    console::attach();
                    eprintln!(
                        "screencapture is already running; quit it from its notification-area \
                         icon first"
                    );
                    return ExitCode::FAILURE;
                }
                detach_tray_console();
            } else {
                // A one-shot command prints a report, so it writes into the
                // console of the terminal that launched it.
                console::attach();
            }
            dispatch(&invocation)
        }
        Err(message) => {
            console::attach();
            eprintln!("error: {message}");
            eprintln!("try: screencapture --help");
            ExitCode::FAILURE
        }
    }
}

/// The tray is a notification-area icon, not a terminal program: it hands
/// back the console that a launch from a terminal lent it, so closing that
/// window cannot end the tray. Launches from Explorer or the autostart entry
/// never had one to drop, and the tray keeps the GUI subsystem's silence
/// either way.
fn detach_tray_console() {
    #[cfg(windows)]
    crate::winapi::detach_console();
}

fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut invocation = Invocation {
        command: Command::Run,
        config: None,
        mode: None,
        display: None,
        rect: None,
    };
    let mut items = args.iter().peekable();
    while let Some(arg) = items.next().map(String::as_str) {
        match arg {
            "--config" => {
                let Some(value) = items.next().map(String::as_str) else {
                    return Err("--config requires a file path".into());
                };
                invocation.config = Some(PathBuf::from(value));
            }
            "--mode" => {
                let Some(value) = items.next().map(String::as_str) else {
                    return Err("--mode requires full, region, or window".into());
                };
                match CaptureMode::parse(value) {
                    Some(mode) => invocation.mode = Some(mode),
                    None => return Err(format!("unknown mode '{value}' (full, region, window)")),
                }
            }
            "--display" => {
                let Some(value) = items.next().map(String::as_str) else {
                    return Err("--display requires a display number (1-based)".into());
                };
                match value.parse::<u32>() {
                    Ok(number) if (1..=16).contains(&number) => invocation.display = Some(number),
                    _ => return Err(format!("--display expects a number 1-16, not '{value}'")),
                }
            }
            "--rect" => {
                let Some(value) = items.next().map(String::as_str) else {
                    return Err("--rect requires X,Y,W,H".into());
                };
                invocation.rect = Some(parse_rect(value)?);
            }
            "--check" => invocation.command = Command::Check,
            "--init" => invocation.command = Command::InitConfig,
            "--settings" => invocation.command = Command::Settings,
            "--photo" => invocation.command = Command::Photo,
            "--record" => {
                let raw = items.next().map_or("3", String::as_str);
                match raw.parse::<u32>() {
                    Ok(seconds) if (1..=3600).contains(&seconds) => {
                        invocation.command = Command::Record { seconds };
                    }
                    _ => return Err("--record requires a duration in seconds (1-3600)".into()),
                }
            }
            "--version" | "-V" => invocation.command = Command::Version,
            "--help" | "-h" => invocation.command = Command::Help,
            bare => return Err(format!("unknown argument '{bare}'")),
        }
    }
    if invocation.rect.is_some() {
        if invocation.display.is_some() {
            return Err("--display and --rect capture different ways; pick one".into());
        }
        if invocation.mode.is_some() && invocation.mode != Some(CaptureMode::Region) {
            return Err("--rect is a region capture; drop --mode full/--mode window".into());
        }
    }
    Ok(invocation)
}

/// One desktop-coordinate capture rectangle from `X,Y,W,H`, edges inside
/// the sane engine range.
fn parse_rect(value: &str) -> Result<Region, String> {
    const MAX_EDGE: u32 = 7680;
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 4 {
        return Err(format!("--rect expects X,Y,W,H, not '{value}'"));
    }
    let numbers: Vec<i64> = parts
        .iter()
        .map(|part| part.trim().parse::<i64>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("--rect expects four integers, not '{value}'"))?;
    let [x, y, width, height] = numbers.try_into().expect("exactly four integers");
    if width < 1 || width > i64::from(MAX_EDGE) || height < 1 || height > i64::from(MAX_EDGE) {
        return Err(format!("--rect edges must be 1..{MAX_EDGE} pixels"));
    }
    let into = |value: i64, what: &str| {
        i32::try_from(value).map_err(|_| format!("--rect {what} out of range"))
    };
    let width = u32::try_from(width).map_err(|_| "--rect edges out of range".to_string())?;
    let height = u32::try_from(height).map_err(|_| "--rect edges out of range".to_string())?;
    Ok(Region {
        x: into(x, "x")?,
        y: into(y, "y")?,
        width,
        height,
    })
}

/// Print the command list and succeed. The tray is a Windows surface, so a
/// bare invocation on macOS or Linux starts here instead of at an error.
pub(crate) fn help_exit() -> ExitCode {
    let _ = stdout().lock().write_all(HELP.as_bytes());
    ExitCode::SUCCESS
}

fn dispatch(invocation: &Invocation) -> ExitCode {
    match invocation.command {
        Command::Version => {
            println!("{IDENTITY}");
            ExitCode::SUCCESS
        }
        Command::Help => help_exit(),
        Command::Check => doctor::check(invocation.config.as_deref()),
        Command::InitConfig => doctor::init_config(invocation.config.as_deref()),
        Command::Settings => app::open_settings_window(invocation.config.as_deref()),
        Command::Record { seconds } => app::demo::record_demo(
            invocation.config.as_deref(),
            seconds,
            invocation.mode,
            invocation.display,
            invocation.rect,
        ),
        Command::Photo => app::demo::photo_demo(
            invocation.config.as_deref(),
            invocation.mode,
            invocation.display,
            invocation.rect,
        ),
        Command::Run => app::run(invocation.config.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_rect;

    #[test]
    fn rect_parses_fields_and_spacing() {
        let region = parse_rect("100, 200, 1280 , 720 ").expect("rectparses");
        assert_eq!(region.x, 100);
        assert_eq!(region.y, 200);
        assert_eq!(region.width, 1280);
        assert_eq!(region.height, 720);
    }

    #[test]
    fn rect_rejects_bad_edge_counts() {
        assert!(parse_rect("1,2,3").is_err());
        assert!(parse_rect("1,2,3,4,5").is_err());
        assert!(parse_rect("a,2,3,4").is_err());
        assert!(parse_rect("1,2,0,4").is_err());
        assert!(parse_rect("1,2,7681,4").is_err());
    }

    #[test]
    fn record_duration_stays_inside_the_documented_range() {
        let args = |value: &str| vec!["--record".to_owned(), value.to_owned()];
        assert!(super::parse(&args("1")).is_ok());
        assert!(super::parse(&args("3600")).is_ok());
        assert!(super::parse(&args("0")).is_err());
        assert!(super::parse(&args("3601")).is_err());
    }
}
