//! `screencapture`: professional 4K screen recording and still-image capture.

mod app;
mod autostart;
mod cli;
mod config;
mod doctor;
mod encoder;
mod ffmpeg;
mod instance;
mod paths;
mod recording;
mod settings;
mod tray;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    cli::run(&args)
}
