//! `screencapture`: professional 4K screen recording and still-image capture.

// The tray is launched from Explorer, the Start Menu, the installer's finish
// page and the autostart entry, and Windows creates — and shows — a console
// window for a console-subsystem program before any of its code runs. Asking
// for the GUI subsystem is what keeps that terminal from flashing on screen;
// [`console`] hands a printing command the terminal it was started from. The
// unit-test harness keeps the console so its own diagnostics stay visible.
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod app;
mod audio;
mod autostart;
mod capture;
mod child;
mod cli;
#[cfg(windows)]
mod clipboard;
mod clock;
mod config;
mod console;
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
mod trim;
#[cfg(windows)]
mod wgc;
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
