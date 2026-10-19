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
screencapture              start the tray application
screencapture --record S   record the screen for S seconds, then exit
screencapture --photo      take one still photo, then exit
screencapture --check      diagnose install path, settings, and engine
screencapture --init       write the default settings file if missing
screencapture --config P   use a specific settings file
screencapture --version    print the version
screencapture --help       print help
```

`--record` and `--photo` run the same capture pipelines as the tray, so
they double as headless smoke tests; verify their output with the bundled
`vendor/ffmpeg/ffprobe.exe`.

## Layout

```
Cargo.toml        crate manifest (edition 2024, warnings denied)
src/main.rs       entry point (opts into per-monitor DPI awareness)
src/cli.rs        argument parsing and dispatch
src/app.rs        tray runtime wiring, recording/photo commands
src/capture.rs    GDI frame capture (top-down BGRA, cached DIB target)
src/screen.rs     monitor enumeration in physical pixels
src/clock.rs      UTC timestamp math for output file names
src/pipe.rs       BGRA frame feed into FFmpeg (records and photos)
src/recorder.rs   recording session lifecycle (start/tick/stop)
src/hotkeys.rs    global hotkey bindings (`RegisterHotKey` table)
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
src/tray/win/     Win32 FFI, message loop, tray icon, popup menus,
                  frame timer, and registered hotkeys
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

Sessions 1 to 3 are done: settings persistence and capture profiles, the
bundled FFmpeg engine, diagnostics, the Windows tray (icon, menu, message
loop, autostart sync, single-instance guard), global hotkeys, and the real
recording pipeline — GDI `BitBlt` frames stream into FFmpeg for
H.264/MP4 records and lossless PNG stills, driven by the tray menu,
hotkeys, or `--record`/`--photo`. A dedicated encoder module
(`src/encoder.rs`) isolates x264 so a later codec swap (VP9/AV1/hardware)
needs only edits there. See `ROADMAP.md` for what remains: region-drawing
and window auto-select, DXGI Desktop Duplication for true 4K60, an
authored tray icon, and shipping installers.
