//! Command-line front door: parsing, help text, and dispatch.

use std::io::{Write, stdout};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::doctor;

/// Display identity used by version output and diagnostics.
pub const IDENTITY: &str = concat!("screencapture ", env!("CARGO_PKG_VERSION"));

const HELP: &str = "\
screencapture - 4K screen recorder and still-image capture

USAGE:
    screencapture              start the tray application
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
    /// Start the tray application.
    Run,
}

/// A parsed command with an optional explicit settings path.
struct Invocation {
    command: Command,
    config: Option<PathBuf>,
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
            "--check" => invocation.command = Command::Check,
            "--init" => invocation.command = Command::InitConfig,
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
        Command::Run => {
            eprintln!("note: the interactive tray runtime is not built yet; running diagnostics.");
            doctor::check(invocation.config.as_deref())
        }
    }
}
