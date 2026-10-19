# screencapture

Professional 4K screen recording and still-image capture that lives in your
system tray, built in dependency-free `std` Rust with a bundled `FFmpeg 9`
engine.

## Principles

- **No dependencies**: only the Rust standard library; platform APIs are
  reached through direct FFI declarations.
- **One FFmpeg engine**: the latest FFmpeg build lives in `vendor/ffmpeg/`
  and eventually inside the installer; the app never relies on a
  system-wide FFmpeg.
- **Resource-light**: the tray binary stays small; heavy lifting is handed
  to FFmpeg hardware encoders when available.
- **Clean code**: `cargo fmt --check` and `cargo clippy --all-targets -- -D
  warnings` must always pass. No `allow` blocks, no warnings.
- **Maintainable**: every file stays under 500 lines.

## Commands

```
screencapture              start the tray application (not yet built)
screencapture --check      diagnose install path, settings, and engine
screencapture --init       write the default settings file if missing
screencapture --config P   use a specific settings file
screencapture --version    print the version
screencapture --help       print help
```

## Layout

```
Cargo.toml        crate manifest (edition 2024, warnings denied)
src/main.rs       entry point
src/cli.rs        argument parsing and dispatch
src/app.rs        tray runtime wiring (instance guard, settings sync)
src/config.rs     std-only settings file format (ordered key = value)
src/settings.rs   typed settings view with safe defaults
src/recording.rs  capture profiles (1080p30 ... 4K60, 4K120)
src/encoder.rs    codec selection (x264 today; VP9/AV1/hardware tomorrow)
src/ffmpeg.rs     locate and drive the bundled FFmpeg engine
src/autostart.rs  login autostart (Windows registry; stubs elsewhere)
src/instance.rs   single-instance guard
src/doctor.rs     `--check` / `--init` diagnostics
src/paths.rs      cross-platform filesystem locations
src/tray/mod.rs   tray API (events shared by all platforms)
src/tray/win/     Win32 FFI, message loop, tray icon, popup menus
vendor/ffmpeg/    the bundled engine (gitignored binaries, licensed GPLv3+)
target/           build output (gitignored)
```

Settings live at `%APPDATA%\Screencapture\settings.cfg` on Windows, with
macOS/Linux equivalents; edit by hand today — the tray preferences window
reuses the same format.

## Engine provenance

`vendor/ffmpeg/ffmpeg.exe` is BtbN's `ffmpeg-n9.0-latest-win64-gpl-9.0`
static build (`ffmpeg version n9.0.2-28-g669f5ab083-20261018`), unpacked
from the official BtbN/FFmpeg-Builds release archive together with
`ffprobe.exe` and `ffmpeg.LICENSE.txt`. Regenerate it by re-downloading
that asset and copying `bin/ffmpeg.exe` and `bin/ffprobe.exe`.

## Status

Sessions 1 and 2 are done: settings persistence and capture profiles, the
bundled FFmpeg engine, diagnostics, the Windows tray (icon, menu, message
loop, autostart registry sync, single-instance guard), and a dedicated
encoder module (`src/encoder.rs`) that isolates x264 so a later codec swap
(VP9/AV1/hardware) needs only edits there. See `ROADMAP.md` for what
remains.
