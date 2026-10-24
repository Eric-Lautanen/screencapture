//! `screencapture`: professional 4K screen recording and still-image capture.

mod app;
mod autostart;
mod capture;
mod child;
mod cli;
#[cfg(windows)]
mod clipboard;
mod clock;
mod config;
#[cfg(target_os = "macos")]
mod corefoundation;
#[cfg(target_os = "macos")]
mod coregraphics;
mod doctor;
#[cfg(windows)]
mod dxgi;
#[cfg(unix)]
mod dylib;
mod encoder;
mod ffmpeg;
#[cfg(windows)]
mod guard;
mod hotkeys;
mod instance;
#[cfg(windows)]
mod known_folder;
/// Layout pins: `pin_layout!` is crate-exported by [`layout`], so every
/// FFI module can call `crate::pin_layout!` in place.
mod layout;
mod log;
#[cfg(windows)]
mod overlay;
mod paths;
mod pipe;
mod recorder;
mod recording;
mod screen;
#[cfg(windows)]
mod selector;
mod settings;
#[cfg(windows)]
mod settings_dialog;
mod sha256;
mod sound;
mod tray;
#[cfg(windows)]
mod winapi;
#[cfg(windows)]
mod window_picker;
#[cfg(target_os = "linux")]
mod xlib;

fn main() -> std::process::ExitCode {
    // Opt into physical-pixel geometry before any screen coordinate is read.
    capture::prepare();
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    cli::run(&args)
}
