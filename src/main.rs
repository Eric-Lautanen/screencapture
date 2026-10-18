//! `screencapture`: professional 4K screen recording and still-image capture.

mod cli;
mod config;
mod doctor;
mod ffmpeg;
mod paths;
mod recording;
mod settings;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    cli::run(&args)
}
