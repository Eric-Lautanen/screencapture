<div align="center">

<img src="assets/icon.svg" width="112" alt="screencapture icon" />

<h1>screencapture</h1>

<p><strong>Screen recording and still capture that stays out of your way.</strong><br />
A Windows tray app with one crosshair for windows and regions,<br />
and the same capture pipeline headless on macOS and Linux.</p>

<p>
  <a href="https://github.com/Eric-Lautanen/screencapture/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/Eric-Lautanen/screencapture/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <img alt="Windows 10/11" src="https://img.shields.io/badge/Windows%2010%202004%2B%20%7C%2011-0078D6?logo=windows&logoColor=white">
  <img alt="CLI on macOS and Linux" src="https://img.shields.io/badge/CLI-macOS%20%7C%20Linux-lightgrey?logo=linux&logoColor=white">
  <img alt="Rust 2024" src="https://img.shields.io/badge/Rust-2024%20edition-000000?logo=rust&logoColor=white">
  <img alt="Zero Rust dependencies" src="https://img.shields.io/badge/Rust%20dependencies-0-brightgreen">
  <img alt="FFmpeg 9" src="https://img.shields.io/badge/engine-FFmpeg%209-007808?logo=ffmpeg&logoColor=white">
  <img alt="Apache 2.0" src="https://img.shields.io/badge/license-Apache%202.0-blue?logo=apache&logoColor=white">
</p>

</div>

## Screenshots

<p align="center">
  <img src="assets/settings-window.png" width="460" alt="The screencapture Settings window" />
  <br />
  <sub><strong>Settings</strong> — capture mode, profile, codec, chimes, hotkeys, and folders in one window.</sub>
</p>

## Why screencapture

- **Record at real speed.** Capture and encoding run on a wall-clock-paced worker,
  so a 60 fps profile produces a 60 fps file even when a slow frame misses its slot.
- **Pick exactly what you mean.** One crosshair session: click a highlighted window
  or drag a rectangle. The selection outline and recording border never appear in
  the saved image.
- **Keep your aspect ratio.** A profile is a size ceiling, not a new shape. A 16:10
  window through a 720p profile becomes 1152×720, with no black bars.
- **Paste without the file hunt.** Every still is saved as a PNG and placed on the
  Windows clipboard as a bitmap plus the finished PNG — one `Ctrl+V` away.
- **Honest hardware acceleration.** AMF, QSV, NVENC, and Media Foundation are
  probed with their real encoder arguments and fall back to x264 with a notice
  when the machine cannot run them.
- **No Rust dependencies.** Every Win32, DXGI, CoreGraphics, and X11 call is a
  hand-written FFI declaration. The binary is just the app and its engine.

## Quick start

### Windows

Requires Rust 1.85+ (edition 2024). Put the engine beside the app first:

```bat
:: vendor\ffmpeg\ffmpeg.exe and ffprobe.exe (see "Engine" below)
cargo build --release
.\target\release\screencapture.exe --check
.\target\release\screencapture.exe
```

The tray appears next to the clock. `--settings` opens the Settings window
directly, and `installer\` contains the NSIS script that packages the exe, the
engine, and the license text into a setup program.

### macOS / Linux

The tray, picker, Settings window, global hotkeys, and clipboard handoff are
Windows surfaces. macOS and Linux use the same capture and encoding pipeline
through the CLI and a system FFmpeg:

```sh
# macOS
brew install ffmpeg

# Debian / Ubuntu
sudo apt install ffmpeg

cargo build --release
./target/release/screencapture --check
./target/release/screencapture --record 10 --mode full
```

Pin a specific engine with `[engine] path = /usr/local/bin/ffmpeg` if needed.
Wayland is not supported.

## Usage

### Global hotkeys

| Action | Default |
|---|---|
| Start / stop recording | `ctrl+shift+x` |
| Take a still | `ctrl+shift+z` |
| Mute / unmute capture chimes | `ctrl+shift+m` |

Combos accept `ctrl`, `alt`, `shift`, and `win` plus a digit, letter, or
`F1`–`F12`. The Settings window validates all three and probes them with a live
`RegisterHotKey` check, then re-registers without a restart.

### Tray menu

| Item | Action |
|---|---|
| **Record** | Start or stop with the configured capture mode |
| **Photo** | Take one still with the configured capture mode |
| **Mute capture chimes** | Toggle the confirmation sound and save the choice |
| **Settings…** | Edit every option live |
| **Profile** | Pick a resolution and frame-rate ceiling |
| **Record display** | Choose a monitor when more than one is attached |
| **Exit** | Stop any active recording and quit |

### Picking a capture target

`[capture] mode` chooses what the Record and Photo actions start:

- **Full screen** records the selected display directly.
- **Drawn region** and **Picked window** share one interactive pass. A plain
  click captures the highlighted window; press and drag to draw a region
  instead. A release within about 8 pixels of the press counts as a click.
- **Escape** or right-click cancels. Escape is read through a low-level keyboard
  watch, so it works even when Windows will not give the overlay focus.

The dotted selection outline and the red recording frame are excluded from the
capture. During a recording, the red frame carries a **Stop** chip on its top
edge; Escape and the record hotkey stop it too.

### Output

Recordings land in `Videos\Screencapture`; stills land in
`Pictures\Screencapture`. If both folders are set to the same path, each medium
gets its own `video` or `image` subfolder instead of mixing files.

A still, or a stopped recording, answers with a soft three-note chime on
Windows. Mute it from the tray menu, with `ctrl+shift+m`, or in Settings.
Every still is also copied to the Windows clipboard.

### Command line

```text
screencapture                        Start the tray (Windows)
screencapture --settings             Open Settings and exit (Windows)
screencapture --record [S]           Record S seconds (1-3600, default 3)
screencapture --record S --mode ...  full | region | window
screencapture --display N            Capture display N (1-based, full mode)
screencapture --rect X,Y,W,H         Capture an exact desktop rectangle
screencapture --photo [--mode ...]   Take one still, then exit
screencapture --check                Diagnose settings, displays, and engine
screencapture --init                 Write the default settings file
screencapture --config P             Use a specific settings file
screencapture --version              Print the version
screencapture --help                 Print help
```

`--rect` is the headless area capture for every platform. Recording and still
commands run the same pipeline as the tray, so they double as smoke tests.
Finished recordings are verified with the bundled `ffprobe` on Windows.

## Configuration

| Platform | Settings file |
|---|---|
| Windows | `%APPDATA%\Screencapture\settings.cfg` |
| macOS | `~/Library/Application Support/Screencapture/settings.cfg` |
| Linux | `$XDG_CONFIG_HOME/Screencapture/settings.cfg` or `~/.config/Screencapture/settings.cfg` |

The file is ordinary `key = value` text, and hand edits load on the next start.
A minimal example:

```ini
[hotkeys]
record = ctrl+shift+x
photo = ctrl+shift+z
chimes = ctrl+shift+m

[capture]
mode = region
display = primary
chimes = true

[photo]
depth = 8

[recording]
profile = uhd-4k60
codec = x264
cursor = false
autostart = true

[media]
videos = "C:\Users\you\Videos\Screencapture"
images = "C:\Users\you\Pictures\Screencapture"
```

Run `screencapture --init` to write the complete template. Unknown keys are
preserved, and invalid values fall back to safe defaults with a logged note.

## Platform support

| | Windows 10 2004+ / 11 | macOS | Linux (X11) |
|---|---|---|---|
| Tray icon, menus, hotkeys | ✅ | — | — |
| Settings window | ✅ | — (edit the file) | — (edit the file) |
| Click-to-pick window / drag region | ✅ | — | — |
| Whole-display recording | ✅ Desktop Duplication | ✅ CoreGraphics | ✅ X11 |
| Window recording | ✅ DWM bounds | ✅ compositor window list | ✅ X client rect |
| Exact rectangle (`--rect`) | ✅ | ✅ | ✅ |
| Still photos (PNG) | ✅ | ✅ | ✅ |
| Clipboard handoff | ✅ | — | — |
| Capture chimes | ✅ | — | — |
| Autostart | ✅ registry | ✅ LaunchAgent | ✅ XDG entry |

Windows is the primary surface. macOS and Linux share the capture pipeline and
CLI, and CI runs their test suites, but live capture on that hardware has not
been verified yet. A bare `screencapture` on those platforms prints the command
list instead of starting a tray that cannot exist there.

## How it works

```text
tray / hotkey → target → paced capture worker → FFmpeg → verified MP4 / PNG
```

The worker owns the frame source and the encoder pipe. Each iteration grabs a
frame and writes as many copies as the wall clock says are due, so a missed
deadline becomes a repeated frame rather than a short clip. The tray pump only
starts, stops, and watches the worker, keeping menus and the Stop chip
responsive mid-recording.

Windows records whole displays through Desktop Duplication, with the mouse
pointer composited in software, and everything else through GDI. macOS draws
the compositor's window list into a BGRA context; Linux reads X11 root images.
A lost duplication session is rebuilt mid-recording while the recording holds
its last frame.

## Development

The gates are the contract:

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

Clippy runs with pedantic lints denied and no `allow` blocks. Cross-check the
ARM and Intel halves with `cargo clippy --target <triple> --all-targets`; the
five targets are kept warning-free.

[GitHub Actions](.github/workflows/ci.yml) runs the native gates on
`windows-latest`, `macos-latest`, and `ubuntu-latest` for every push and pull
request. `main` is protected: all three jobs must pass, the branch must be up to
date, and admins cannot bypass the rules.

- [ROADMAP.md](ROADMAP.md) — session history and the deferred backlog
- [AUDIT.md](AUDIT.md) — defect audit and fixes
- [docs/FFMPEG-LICENSE.md](docs/FFMPEG-LICENSE.md) — engine licensing and source

## Engine

`vendor/ffmpeg/ffmpeg.exe` is BtbN's `ffmpeg-n9.0-latest-win64-gpl-9.0` static
build (`ffmpeg version n9.0.2-28-g669f5ab083-20261018`), shipped with
`ffprobe.exe` and its license text. The binaries are not committed; place them
in `vendor/ffmpeg/` before building. `--check` records their SHA-256 and reports
a mismatch if the engine changes.

On macOS and Linux the app uses the system FFmpeg instead.

## License

The application is [Apache-2.0](LICENSE). The bundled FFmpeg build is a separate
GPLv3+ work: distributing it means shipping its license text and pointing at its
source. See [docs/FFMPEG-LICENSE.md](docs/FFMPEG-LICENSE.md) for the decision and
the upstream link.
