//! `screencapture`: professional 4K screen recording and still-image capture.

mod app;
mod autostart;
mod capture;
mod cli;
mod clock;
mod config;
mod doctor;
mod encoder;
mod ffmpeg;
mod hotkeys;
mod instance;
mod paths;
mod pipe;
mod recorder;
mod recording;
mod screen;
mod settings;
mod tray;

fn main() -> std::process::ExitCode {
    // Opt into physical-pixel geometry before any screen coordinate is read.
    capture::prepare();
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    cli::run(&args)
}
