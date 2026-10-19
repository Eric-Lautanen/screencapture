//! Command-line front door: parsing, help text, and dispatch.

use std::io::{Write, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::settings::CaptureMode;
use crate::{app, doctor};

/// Display identity used by version output and diagnostics.
pub const IDENTITY: &str = concat!("screencapture ", env!("CARGO_PKG_VERSION"));

const HELP: &str = "\
screencapture - 4K screen recorder and still-image capture

USAGE:
    screencapture              start the tray application
    screencapture --record S   record for S seconds, then exit
    screencapture --record S --mode full | region | window
    screencapture --display N  capture display N (1-based) in full mode
    screencapture --photo      take one still photo, then exit
    screencapture --check      diagnose install path, settings, and engine
    screencapture --init       write the default settings file if missing
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
}

/// Parse `args` and dispatch; returns the process exit code.
pub fn run(args: &[String]) -> ExitCode {
    match parse(args) {
        Ok(invocation) => dispatch(&invocation),
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("try: screencapture --help");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<Invocation, String> {
    let mut invocation = Invocation {
        command: Command::Run,
        config: None,
        mode: None,
        display: None,
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
            "--check" => invocation.command = Command::Check,
            "--init" => invocation.command = Command::InitConfig,
            "--photo" => invocation.command = Command::Photo,
            "--record" => {
                let raw = items.next().map_or("3", String::as_str);
                match raw.parse::<u32>() {
                    Ok(seconds) if seconds > 0 => invocation.command = Command::Record { seconds },
                    _ => return Err("--record requires a duration in seconds (1-3600)".into()),
                }
            }
            "--version" | "-V" => invocation.command = Command::Version,
            "--help" | "-h" => invocation.command = Command::Help,
            bare => return Err(format!("unknown argument '{bare}'")),
        }
    }
    Ok(invocation)
}

fn dispatch(invocation: &Invocation) -> ExitCode {
    match invocation.command {
        Command::Version => {
            println!("{IDENTITY}");
            ExitCode::SUCCESS
        }
        Command::Help => {
            let _ = stdout().lock().write_all(HELP.as_bytes());
            ExitCode::SUCCESS
        }
        Command::Check => doctor::check(invocation.config.as_deref()),
        Command::InitConfig => doctor::init_config(invocation.config.as_deref()),
        Command::Record { seconds } => app::record_demo(
            invocation.config.as_deref(),
            seconds,
            invocation.mode,
            invocation.display,
        ),
        Command::Photo => app::photo_demo(invocation.config.as_deref(), invocation.display),
        Command::Run => app::run(invocation.config.as_deref()),
    }
}
