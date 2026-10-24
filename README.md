<div align="center">

<img src="assets/icon.svg" width="120" alt="screencapture icon" />

<h1>screencapture</h1>

<p><strong>4K screen recording and still capture that lives in your system tray.</strong><br />
Click the window you want, drag a rectangle, or take the whole display —<br />
real-time video in, a finished MP4 or PNG out, no OBS in sight.</p>

<p>
  <a href="https://github.com/Eric-Lautanen/screencapture/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/Eric-Lautanen/screencapture/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%2010%202004%2B%20%7C%2011-0078D6?logo=windows&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024%20edition-000000?logo=rust&logoColor=white">
  <img alt="Dependencies" src="https://img.shields.io/badge/dependencies-0-brightgreen">
  <img alt="Engine" src="https://img.shields.io/badge/engine-FFmpeg%209-007808?logo=ffmpeg&logoColor=white">
  <img alt="Tests" src="https://img.shields.io/badge/tests-133%20passing-success">
  <img alt="CLI" src="https://img.shields.io/badge/CLI-macOS%20%7C%20Linux-lightgrey?logo=linux&logoColor=white">
  <img alt="License" src="https://img.shields.io/badge/license-Apache%202.0-blue?logo=apache&logoColor=white">
</p>

</div>

<table>
  <tr>
    <td align="center"><img src="assets/settings-window.png" width="380" alt="Settings window" /><br />
      <sub><strong>Settings</strong> — every capture choice, validated and live</sub></td>
    <td align="center"><img src="assets/tray-menu.png" width="170" alt="Tray menu" /><br />
      <sub><strong>Tray</strong> — record, photo, profile, displays</sub></td>
  </tr>
</table>

## Why this one

- **Recordings run at real speed.** Frames are captured and piped on a worker
  thread paced against the wall clock, so a 60 fps profile produces a 60 fps
  file — even when a blit misses a deadline, the slot is filled instead of the
  clip running fast.
- **Pick exactly what you mean.** One crosshair session: click the highlighted
  window, or press-and-drag a rectangle. The dotted outline and the red
  recording border are excluded from every capture, so chrome never lands in
  the file.
- **Your aspect ratio survives.** A profile is a size ceiling, not a new shape:
  a 16:10 window through a 720p profile becomes 1152x720, with no black bars.
- **One bundled engine.** FFmpeg 9 ships beside the app (`vendor/ffmpeg/`);
  x264 is the default, and AMF / QSV / NVENC / Media Foundation are probed once,
  cached, and demoted honestly when the machine cannot run them.
- **Paste without the file hunt.** Every still is written as a PNG *and* put
  on the clipboard as a bitmap plus the PNG, so `Ctrl+V` drops it straight into
  a document, a chat, or an editor (`--photo` included; Windows).
- **No dependencies.** Only the Rust standard library — every Win32, DXGI and
  CoreGraphics call is a hand-written FFI declaration.
- **Small and quiet.** A tray app, a single instance, an autostart flag, a
  timestamped log, and `--check` when something looks wrong.

## Principles

- **Std-only, no crates**: platform APIs are reached through direct FFI
  declarations, so the dependency graph is a single node and the binary stays
  small.
- **Gates are the contract**: `cargo fmt --check`, pedantic `clippy` with
  `-D warnings` (no `allow` blocks), and the test suite must pass on Windows,
  Linux, and both macOS targets before a change lands.
- **Maintainable**: every file stays under 500 lines.
- **Honest telemetry**: every recording logs what it actually did — grabs,
  written frames, wall time, measured fps, and the engine's own count — so a
  dropped frame or a fallback codec is visible instead of silent.

## Install

**Windows** — needs Rust 1.85+ (edition 2024) and the bundled engine:

```bat
:: 1. engine: put ffmpeg.exe + ffprobe.exe in vendor\ffmpeg (see Engine provenance)
:: 2. build and run
cargo build --release
.\target\release\screencapture.exe --check
.\target\release\screencapture.exe
```

The tray appears next to the clock; `installer/` holds the NSIS script that
packs the exe, the engine, and the license text into a setup program.

**macOS / Linux** — build the CLI and point it at a system engine (the bundled
build is Windows-only); the tray, pickers, hotkeys, and Settings window are
Windows surfaces (see [Platform support](#platform-support)):

```sh
brew install ffmpeg            # or: sudo apt install ffmpeg
cargo build --release
./target/release/screencapture --check
./target/release/screencapture --record 10 --mode full
```

Any engine path can also be pinned with `[engine] path = /usr/local/bin/ffmpeg`.

## Commands

```
screencapture              start the tray application
screencapture --record S   record for S seconds, then exit
screencapture --record S --mode full | region | window
screencapture --display N  capture display N (1-based) in full mode
screencapture --rect X,Y,W,H  capture one rectangle, headless (region
                           records and photos; any platform)
screencapture --photo [--mode full | region | window]  take one still
                           photo (copied to the clipboard on Windows),
                           then exit
screencapture --check      diagnose install path, settings, and engine
screencapture --init       write the default settings file if missing
screencapture --config P   use a specific settings file
screencapture --version    print the version
screencapture --help       print help
```

`--mode region` opens the fullscreen drag selector (Windows overlay);
`--mode window` records the application window under the cursor
(Windows DWM frame bounds, Linux the X client rectangle). Headless
area picking needs no interaction: `--rect X,Y,W,H` captures that exact
desktop rectangle for both a region recording and a photo, on every
platform. The tray's "Record" and "Photo" honor the settings file's `[capture] mode`
and `[capture] display` (primary or a 1-based number): Full screen captures
the chosen display directly; the region/window modes open one unified
interactive session — a dotted selection outline follows the cursor
under the crosshair, a plain click captures the highlighted (real)
window, and
press-and-drag draws the region box instead (a release inside ~8 px of the
press counts as a click) — Escape or right click cancels either way.
Escape is read through a low-level keyboard watch rather than the window's
own keys, so cancelling works even when Windows refuses the overlay the
foreground (a capture started from a terminal or an IDE is enough). One
selection path, two outputs: whichever target you answer comes back as a
recording or as a still photo depending on which hotkey/menu item started
the session. The red dashed border is reserved for a live recording and carries a
**Stop** chip centered on the top edge: one click ends and saves the
recording, and the chip is excluded from the capture (Escape and the
record hotkey stop it too). Recordings land in `Videos\Screencapture` and stills in
`Pictures\Screencapture` — one folder per medium, never mixed with your
own files — and every still is also placed on the clipboard, as a
bitmap and as the finished PNG, so a paste is one `Ctrl+V` away. A
captured still answers with a soft three-note chime (D, F#, A rising,
Windows) the instant the selection is committed — as the dotted box
disappears, before the PNG is encoded — so the capture is audible without
looking at the tray. Stopping a recording answers with the same chime the
instant the Stop chip, Escape, or the hotkey is used; the file closes in
the background afterwards. The tray's **Mute capture chimes** entry, the
chime hotkey (`ctrl+shift+m` by default), and the Settings window's **Play
capture chimes** box (or `[capture] chimes = false`) mute them; the menu
check mark follows the saved setting. If you point both settings
at the same folder, each medium
gets its own `video`/`image` subfolder there instead of dropping clips
and stills side by side. The tray also has "Record display" entries when more than one
panel is attached. Photo and record land in their own folders (`[media]
images` / `[media] videos`, with the legacy `directory` key still answering
for recordings). Hotkeys are freely settable combos of ctrl/alt/shift/win
plus a digit, letter, or F1–F12 — defaults: `ctrl+shift+x` (record),
`ctrl+shift+z` (photo), and `ctrl+shift+m` (chimes) — and the tray
**Settings...** window edits every choice live (mode, display, profile,
codec, photo depth, cursor, capture chimes, autostart, both folders with
native browse, all three hotkeys; click a field and press the
combo, with a live `RegisterHotKey` probe flagging combos another app owns;
validated and applied on Save), re-registering hotkeys without a restart. The window is a fixed-
size modal-style dialog centered on the primary display, grouped into
labeled sections over aligned columns, with the app icon in the title
bar and a dialog-wide Segoe UI font.

`--record` and `--photo` run the same capture pipelines as the tray, so
they double as headless smoke tests; every completed recording is
verified with the bundled `ffprobe` (frame count is printed to stderr)
and can be inspected with `vendor/ffmpeg/ffprobe.exe`.

## Platform support

| | Windows 10 2004+ / 11 | macOS | Linux (X11) |
|---|---|---|---|
| Tray icon, menus, hotkeys | ✅ | — | — |
| Settings window | ✅ | — (edit the file) | — (edit the file) |
| Click-to-pick window / drag region | ✅ | — | — |
| Whole-display recording (`--record`, Full mode) | ✅ Desktop Duplication | ✅ CoreGraphics | ✅ X11 |
| Window recording (`--mode window`) | ✅ | ✅ compositor window list | ✅ X client rect |
| Exact rectangle (`--rect X,Y,W,H`) | ✅ | ✅ | ✅ |
| Still photos (PNG) | ✅ | ✅ | ✅ |
| Photo copied to the clipboard | ✅ | — | — |
| Autostart, single instance, `--check` | ✅ registry | ✅ LaunchAgent | ✅ XDG entry |

A bare `screencapture` on macOS/Linux prints the command list instead of
starting a tray that cannot exist there. `[capture] mode = region` needs the
Windows overlay, so other platforms default to `full` — and a settings file
that still says `region` is demoted to a whole-display capture with a notice
rather than failing before it starts. The macOS and Linux capture paths are
header-verified and compile for `x86_64`/`aarch64` on both, but have not been
run on that hardware yet; Wayland is explicitly unsupported.

## How capture works

`Recorder` starts a worker thread that owns the frame source and the FFmpeg
pipe. Each iteration grabs a frame, writes as many copies as the wall clock
says are due (one normally, more when a 33 MB `BitBlt` misses a 16 ms slot),
then sleeps until the next frame deadline. The tray pump only starts and stops
the recording and keeps a 500 ms watchdog, so menus and the **Stop** chip stay
responsive mid-capture.

Frames come from Desktop Duplication (`src/dxgi/`) when the region is one whole
unrotated output — with the pointer composited in software when requested — and
from a GDI blit (`src/capture/win.rs`) for windows and partial regions. A lost
duplication session is rebuilt mid-recording; the recording holds its last
frame meanwhile. macOS draws the compositor's window-list image into a BGRA
bitmap context, Linux reads `XGetImage` roots from libX11. Finished files are
verified with the bundled `ffprobe`.

## Layout

```
Cargo.toml        crate manifest (edition 2024, Apache-2.0, warnings denied,
                  fat-LTO release profile)
LICENSE           Apache License 2.0 (the app's own license)
README.md         this file; ROADMAP.md session history + backlog
build.rs          std-only Windows resource glue (bakes assets/icon.ico
                  into the exe via `windres`; skipped elsewhere)
src/main.rs       entry point (opts into per-monitor DPI awareness)
src/cli.rs        argument parsing and dispatch
src/app.rs        tray runtime wiring, recording/photo commands
src/app/demo.rs   headless `--record`/`--photo` pipelines (CLI smoke)
src/app/settings_apply.rs  Settings-window save/reload wiring
src/guard.rs      Drop owners for Win32 DCs, hooks, mouse capture, and the
                  modal selectors' Escape watch
src/clipboard.rs  stills onto the Windows clipboard (DIB + registered PNG)
src/layout.rs     `pin_layout!` compile-time FFI record assertions
src/winapi.rs     shared Win32 declarations (windows, cursors, messages,
                  Rect/Point/Guid records)
src/capture/      frame capture backends behind one Target API (top-down
                  BGRA): Windows GDI blit, macOS CoreGraphics compositor
                  draw, Linux X11 root images
src/dylib.rs      runtime library loading on Unix (mirror of the dxgi loader)
src/coregraphics.rs  CoreGraphics C surface (loads by name, SDK-verified)
src/corefoundation.rs  CoreFoundation C surface (window-list collections
                  for the macOS window picker)
src/xlib.rs       libX11 C surface (loads by soname, header-verified)
src/dxgi/         Desktop Duplication capture: COM ABI shapes, protocol,
                  engine loads, output policy, cursor compositing, and
                  session recovery (factory > adapter > output > duplication)
src/screen/       monitor + window geometry in physical pixels
                  (Windows enumeration; one logical display elsewhere)
src/selector/     fullscreen drawn-rectangle capture selector (drag)
src/window_picker.rs  click-to-pick window selector for the tray's
                  "Record window" (Windows; hover border + hooks)
src/clock.rs      UTC timestamp math for output file names
src/pipe.rs       BGRA frame feed into FFmpeg (records and photos)
src/recorder.rs   recording session: capture worker, wall-clock pacing,
                  failure reporting, start/stop
src/hotkeys.rs    global hotkey bindings (`RegisterHotKey` table)
src/config.rs     std-only settings file format (ordered key = value)
src/settings/     typed settings view with safe defaults (mod.rs, hotkey.rs,
                  tests.rs)
src/recording.rs  capture profiles (480p30, 720p30/60, 1080p30/60,
                  1440p60, 4K30/60/120)
src/encoder.rs    codec selection (x264 default; AMF/QSV/NVENC/Media
                  Foundation hardware choices probe + fall back)
src/ffmpeg.rs     locate and drive the bundled FFmpeg engine
src/autostart.rs  login autostart (registry / LaunchAgent / XDG entry)
src/instance.rs   single-instance guard
src/doctor.rs     `--check` / `--init` diagnostics
src/paths.rs      cross-platform filesystem locations
src/log.rs        diagnostics ring + `screencapture.log` upkeep (trays
                  run console-less; recent notes stay retrievable)
src/sha256.rs     pure-std SHA-256 (engine integrity record upkeep)
src/tray/mod.rs   tray API (events shared by all platforms)
src/tray/win/     Win32 FFI, message loop, tray icon, popup menus,
                  recording watchdog, and registered hotkeys
src/settings_dialog/  modal Settings window (mod, rows, frame, fields, ffi)
assets/icon.svg   icon master art (convert to the .ico stack)
assets/icon.ico   authored tray icon (generated by tools/gen_icon.rs)
assets/icon.rc    Windows resource script (icon into the exe)
assets/settings-window.png, assets/tray-menu.png  README screenshots
installer/        NSIS script + packaging notes (Windows setup)
docs/FFMPEG-LICENSE.md  engine licensing decision + source offer
tools/gen_icon.rs std-only icon generator (render + .ico packer)
vendor/ffmpeg/    the bundled engine (gitignored binaries, licensed GPLv3+)
target/           build output (gitignored)
```

Settings live at `%APPDATA%\Screencapture\settings.cfg` on Windows, with
macOS/Linux equivalents; the tray Settings window reuses the same format,
and hand edits load at the next start.

## Engine provenance

`vendor/ffmpeg/ffmpeg.exe` is BtbN's `ffmpeg-n9.0-latest-win64-gpl-9.0`
static build (`ffmpeg version n9.0.2-28-g669f5ab083-20261018`), unpacked
from the official BtbN/FFmpeg-Builds release archive together with
`ffprobe.exe` and `ffmpeg.LICENSE.txt`. Regenerate it by re-downloading
that asset and copying `bin/ffmpeg.exe` and `bin/ffprobe.exe`; then run
`--check` once so a fresh `engine.sha256` integrity record is written.
Licensing: `docs/FFMPEG-LICENSE.md`.

## Development

The gates are the contract; every one of them must pass before a change lands:

```sh
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings   # pedantic is denied, no allow blocks
cargo test --all-targets --locked                    # 133 tests
cargo clippy --target x86_64-apple-darwin --all-targets -- -D warnings
cargo clippy --target aarch64-apple-darwin --all-targets -- -D warnings
cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings
cargo clippy --target aarch64-unknown-linux-gnu --all-targets -- -D warnings
```

[GitHub Actions](.github/workflows/ci.yml) runs the native gates on
`windows-latest`, `macos-latest`, and `ubuntu-latest` for every push and pull
request; the cross-target commands above still cover the ARM and Intel halves
locally. `ROADMAP.md` holds the session-by-session history and the deferred
backlog; `AUDIT.md` holds the defect audit.

## License

This project is licensed under the **Apache License 2.0** — see
[LICENSE](LICENSE) for the full text. You may use, modify, and distribute it,
including commercially, provided you keep the license and attribution notices.

```
Copyright 2026 the screencapture authors

Licensed under the Apache License, Version 2.0 (the "License");
```

The bundled FFmpeg build is a **separate work** under **GPLv3+**; it ships
unmodified beside the app and is not covered by Apache-2.0. Distributing it
means shipping its license text and pointing at its source
(`docs/FFMPEG-LICENSE.md` records the decision, the shipped text, and the
upstream source URL).

<details>
<summary><strong>Session history</strong> — why things look the way they do</summary>

## Session notes

Session 23 made recordings run in real time. Every clip used to play
back at roughly double speed: the frame timer asked for 17 ms, Windows
quantized it to two 15.6 ms ticks (~31 fps), and the file was still
stamped at the profile's 60 fps, so 20 s of screen became a 10 s video.
Frames are now captured and piped on a worker thread that paces itself
against the wall clock — a slot that a slow blit cannot fill is repeated
so the file always spans the time it recorded — and the tray pump no
longer does any capture work, so menus and the Stop chip stay instant
mid-recording (the tray menu opens in ~14 ms while recording). The red
outline is created and painted before any engine setup, so it appears
in ~10 ms after the pick, and hardware-encoder probes are cached per
process instead of re-run at every start.

Session 22 fixed recording shape: a profile is now a size ceiling, not a
new aspect ratio. `Profile::fit` finds the largest frame that keeps the
capture's own shape inside the profile's resolution (integer math, even
edges), `record_args` scales straight to that frame with no pad stage,
and the start log reports the real output size (`684x891 -> 552x720`).
A 16:10 window recorded through a 720p profile now lands on 1152x720
instead of 1280x720 with 64 px black side bars; anything captured at the
profile's own size and shape is passed through untouched. Live-verified:
a 684x891 portrait window recorded as 552x720 (ffprobe `552x720`).

Session 18 audited the tree (`AUDIT.md`) and fixed the defects it ranked
first: the FFmpeg child is now owned by `Feed`'s `Drop` (killed and reaped
on every early return, with `Recorder` finalizing instead of dropping on a
tick error), a DXGI frame acquired before a failed texture cast is released
before propagating, opening Settings stops a running recording so its modal
pump cannot stall the frame clock, and the Settings dialog now routes
Tab/Enter/Escape through `IsDialogMessageW` with standard `IDOK`/`IDCANCEL`
ids (the Win32 dialog contract; live keyboard confirmation still needs
hands, like the picker). `--photo` honors `[capture] mode` (Full/Region/Window) instead of
always grabbing the display, `--record` enforces its documented 1-3600 s
range, `Recorder::start` creates its output directory, `display_at(0)`
returns an error instead of underflowing, the single-instance mutex clears
last-error before `CreateMutexW`, and `--check` folds non-Windows capture
failures into its result, compares integrity records byte-wise, and prints
a measured blit timing (frames, seconds, fps, ms/frame). Settings loading
now reports invalid booleans and identical hotkeys, and saved-file logs
include captured frames, elapsed wall time, and measured fps beside the
ffprobe count. Cleanup: `settings.rs` split into `settings/{mod,hotkey,tests}.rs`
(all files <= 500 lines), `Rect`/`Guid` unified in `winapi.rs`, the GDI
screen DC moved onto the `ScreenDc` guard, the duplicate photo function and
the picker's unused `bounds` field removed. Gates: fmt, pedantic clippy on
native plus both cross targets, 78 tests; see `AUDIT.md` for what remains
deferred (true timestamped pacing, duplication throughput, mac/Linux live
runs).

Session 20 gave the picker its own look: the selection box (hovered
window and dragged rectangle alike) is now a two-tone dotted outline —
2 px white and black dots in a 6 px step — instead of borrowing the
recording frame's red dashes. That outline is a distinct window class
(`screencapture.selection.frame`), while
`screencapture.recording.frame` stays red dashed and appears only when
frames are actually being written. Both classes ask
`WDA_EXCLUDEFROMCAPTURE`: the outline is screen-only chrome, so a still
or recording saved during a pick never shows it (the compositor can
keep the last pixels of a just-destroyed frame for a moment, which put
the dots into a picked screenshot before). One
`overlay::Style` (`Selection` / `Recording(action)`) picks the class,
the painting, and whether the Stop chip exists. Verified live: a picker
pass showed only the dotted `selection.frame` plus the crosshair and no
recording window; the captured row read `FFFFFF FFFFFF` (white dots),
a transparent gap, `000000 000000` (black dots), a gap, repeating; a
click replaced it with the red recording frame and Stop chip; Escape
tore both down. Gates: fmt, pedantic clippy, 96 tests.

Window picks now hug the opaque frame. Windows 11 draws a 1 px visible
frame border translucently, so a capture that kept it showed whatever
was behind the window along its edges — white text at the top and an
image slice at the bottom when a page was behind it.
`screen::win::window_bounds` trims `DWMWA_VISIBLE_FRAME_BORDER_THICKNESS`
from the DWM extended bounds, and both the capture region and the
picker's outline use those same insetted bounds, so the dotted box
frames exactly the pixels a capture saves. Verified live: this app's
window (DWM `240,126 1440x900`) now captures `1438x898` with zero
bright pixels on all four edge rows/columns, the picker outline reports
the matching `241,127 1438x898`, and a maximized `1920x1152` window
captures `1918x1150`.

Session 19 gave recordings their own folder and an obvious way to stop.
Recordings now default to `Videos\Screencapture`, mirroring
`Pictures\Screencapture`; a config that still names the old plain
`Videos`/`Movies` folder is read as "unset" so existing installs follow
the new default without touching files the user chose themselves. A
folder chosen for both outputs is split into `video`/`image`
subfolders (at load and again on every Settings save, so the persisted
paths show where files really land); two different folders stay exactly
as chosen, still holding the normal `Screencapture` folder. The tray
menu dropped its Autostart toggle (the Settings window owns that flag now),
keeping the menu to record/photo/settings/displays/profile/exit. The
recording border grew a red **Stop** chip centered on its top dashed edge
(`src/overlay.rs`): a small non-activating window, click-through excluded
from the capture via `WDA_EXCLUDEFROMCAPTURE`, that posts a tray command
so the click stops the recording exactly like the hotkey (the border is
not torn down inside its own window procedure). Verified live: the chip
appears at the top center (rect centered on the dashed line), reports
affinity `0x11`, a click saved the clip and removed the border, and the
captured frame is clean where the chip sits; the chip's paint was
photographed with exclusion temporarily off (chip `#FF0000`, white rim and
caption) because an excluded window is invisible to any screenshot. Gates:
fmt, pedantic clippy, 93 tests.

Session 17 unified the interactive session: the region and window capture
modes now share one click-to-pick pass (`src/window_picker.rs` answers a
`Pick` — window or dragged rectangle). A plain press takes the real window
under the crosshair; holding past an 8 px Chebyshev threshold converts the press into a drag and draws the XOR marquee (`src/selector/` equivalents
live in the same hooks), so "Record region" and "Record window" (and their
photo twins) need no separate menu paths — the settings `[capture] mode`
just picks which session the hotkey opens, and both accept either answer.
Verified by the gate battery (fmt, pedantic clippy on native plus both
cross targets, 66 tests including threshold/geometry/track units); the
real-mouse confirmation of click-vs-drag stays with your hands, per the
picker precedent.

Session 16 made stills first-class and gave the app its Settings window.
"Photo" on the tray (and the photo hotkey) now honors the same `[capture]
mode` selection path as "Record" — one drag selector, one window picker,
different outputs — so there is no reduplicated photo menu; the photo lands
in `[media] images` (default `Pictures/Screencapture`) while recordings
stay in `[media] videos`, with the legacy `directory` key still answering
for recordings when `[media] videos` is empty, both editable in the new Settings window with a native
folder browse. The Settings window (`src/settings_dialog/`) edits every
schema choice live — capture mode, display, profile, codec, photo depth,
cursor, autostart — plus the now-free hotkey combos: any ctrl/alt/shift/win
mixture over a digit, letter, or F1–F12 (defaults moved to the user-chosen
`ctrl+shift+x` record and `ctrl+shift+z` photo), validated on Save and
re-registered without restart via `tray::set_bindings`. Verified by gates
(fmt, pedantic clippy on native and both cross targets, 62 tests incl.
new hotkey/folder-split/round-trip tests) and live photo smoke (below);
the tray Settings window itself is gated by unit tests plus the Save
round-trip smoke pending real-mouse confirmation.

Session 15 fixed a hard crash and gave "Record window" its picker:
the abort users could hit was a reentrancy panic in the tray core — a
running recording's frame timer (or a hotkey) dispatched a message into
the tray's window procedure while a modal picker pumped messages, and
`emit` re-borrowed the handler `RefCell`; the fix queues events behind
the running dispatch and replays them in order (regression-tested),
and modal pickers stop the active recording first so nothing ticks
underneath. The tray's "Record window" now opens a click-to-pick
session (`src/window_picker.rs`): a moving border highlights the
real window under the cursor (DWM extended-frame bounds, XOR-drawn
through low-level mouse/keyboard hooks so the desktop keeps behaving),
a click records the highlighted window, and Escape or right click
cancels — no more recording "whatever lay under the menu" with no
visual feedback. Verified live end-to-end through the tray API itself:
menu → "Record window" → hover the taskbar → click → recording on the
real taskbar strip (`1920x48`), stopped from the tray menu (137 frames,
ffprobe-verified), then a clean self-exit; no panic anywhere.
Scoped as the next session (requested): the preferences/hotkeys
window — config-file editing through the same format plus live hotkey
re-registration. (Delivered in session 16 as the Settings window with
the same-format config, live re-registration, and the folder split.)

Session 14 widened the codec belt with HEVC: `x265` (CPU), plus
`amf-hevc | qsv-hevc | nvenc-hevc | mf-hevc` hardware variants parsed
like their H.264 twins (and their bare `hevc_*` names), all probed
with the exact argument set before first use and demoting to x264 with
a notice when unavailable — verified options only, straight from the
engine's `ffmpeg -h encoder` pages (HEVC QSV has no `-look_ahead`, so
its HEVC stays at vendor defaults). `--check` now lists all ten
encoders. Verified live on the AMD 780M box: `libx265`, `h264_amf`,
`h264_mf`, and `hevc_amf` probe ready; the QSV/NVENC families and
`hevc_mf` honestly answer not usable; `amf-hevc` recorded a
probe-verified `hevc` 4K60 stream, the `nvenc-hevc` demotion fell
`nvenc-hevc` fell back to `x264` with the warning, and an `x265`
config landed a verified `hevc` stream over the CPU path. The change
also fixed a latent bug: the piped pixel format is now per-variant —
the QSV pages accept only `nv12`, so the old hard `yuv420p` would
have made both QSV variants permanently unusable on real Intel
hardware.

Session 13 added a `depth` knob to still photos: a new `[photo]`
settings section (default `depth = 8`, exactly today's PNG; `depth =
16` saves the still in the PNG encoder's documented 48-bit form via
`format=rgb48be`, verified against this engine's `png` page and
probe-checked live with the bundled `ffprobe`). The document accepts
both bare integers and quoted text, invalid values fall back with a
warning, `--check` reports the depth, and every capture route is
currently 8-bit — the hook lands ahead of any 16-bit capture source
(see `ROADMAP.md` for the deltas).

Session 12 (blind, header-verified like session 5) brought macOS window
capture up to Linux parity: a new `src/corefoundation.rs` C surface
(`CFArray` count/value, `CFDictionary` lookup, `CFNumber` coercion,
`CFStringCreateWithCString`, `CFRelease`, loaded through the same
dylib path) and four new `src/coregraphics.rs` exports power
`screen::window_region` on macOS — the compositor's front-to-back
on-screen list answers the frontmost layer-zero window whose bounds
hold the cursor (desktop, dock, and overlay levels skip; the desktop
and empty answers take the whole main display, mirroring the X11
backend), decoded through the SDK's rect rule and scaled to physical
pixels, clamped into the display extent with a one-pixel guarantee.
Every signature and constant was re-checked against Apple's SDK
headers first (including the correction that window levels live in
`CGWindowLevel.h`, and that the cursor's global space is
`CGEventGetLocation`, not the flipped variant). The new
containment/pixel-math unit tests run on macOS hardware along with
the rest; everything else stays blind and awaits a live smoke
session. Also picked up in passing: a toolchain update made the new
`items_after_test_module` lint fail the CLI module's long-standing
test layout (moved back to the file end).

Session 11 widened headless capture to any platform and rectangle:
`--rect X,Y,W,H` feeds the exact desktop rectangle to both `--record`
(a region recording, no interactive selector) and `--photo` (replacing
the whole-display pick), with parse validation for edges, ranges, and
conflicts (`--display`/non-region `--mode` reject a `--rect`). Verified
live on Windows: rect recordings and rect photos at off-display-origin
bounds produce correct streams (the earlier full-mode and display
semantics untouched; `-D warnings`, fmt, and 45 tests green across the
native and both cross targets). Linux window capture also landed
blind-verified: `screen::window_region` now answers the X client
rectangle under the cursor (cursor-on-desktop captures the whole root),
built on new `XQueryPointer`/`XGetGeometry`/`XTranslateCoordinates`
surfaces in `src/xlib.rs` with signatures checked against the upstream
`Xlib.h`; the live X run and macOS equivalent stay deferred to sessions
with the corresponding hardware.

Session 9 split `src/app.rs` (it had grown to 478 of the 500-line cap):
the headless `--record`/`--photo` paths moved to `src/app/demo.rs`, and a
diagnostics ring (`src/log.rs`) now captures what console-less tray runs
used to lose: lifecycle and error notes (timestamped) live in a 64-entry
ring that flushes to `screencapture.log` inside the media directory at
every recording stop, on exit, and on tick failures; the file restarts
once it passes 512 KiB. `--check` reports the log file. Live-verified:
a demo record wrote both lifecycle lines to the log and the check line
shows the path and line count.

Session 8 added hardware encoders: settings accept
`codec = amf | qsv | nvenc | mf` (AMD, Intel Quick Sync, NVIDIA NVENC, and
Windows Media Foundation) beside the universal `x264` default. Every
hardware choice is probed with its exact ffmpeg argument set (a short
synthetic encode through the engine) before each recording starts, so a
machine without the accelerator demotes to x264 with a printed notice
instead of dying frame by frame; `--check` prints the availability of all
five encoders and marks the configured one. Verified live on the AMD 780M
reference box: `amf` and `mf` ready (x264 ready by definition), `qsv` and
`nvenc` answered `not usable`, an `amf` config recorded fine, and the
`nvenc` config printed the fallback notice and still recorded.

Session 7 added per-display picking (the top deferred item): full captures
record the display chosen by `[capture] display` (`primary` default, or a
1-based number), `--display N` overrides it on the command line for both
`--record` and `--photo`, and the tray grows a "Record display" submenu when
more than one panel is attached (labels carry size and non-zero origins).
Duplication needs no changes — it already matched regions to outputs, so a
secondary display records through its own output; a stale choice answers
with "display N is not attached" and the doctor lists the display state.
Live-smoke verified on the single-panel reference box (missing-display
error, `--display 1` record, doctor line, flag parse errors).

Session 6 packaged the Windows half: release profile (fat LTO, one codegen
unit, stripped ~0.5 MB exe), the tray icon resource baked into the exe via a
std-only `build.rs` (`windres` compiles `assets/icon.rc` — iconless builds
degrade gracefully when `windres` is absent), a pure-std SHA-256
(`src/sha256.rs`, vector-tested) that keeps an integrity record for the
engine at `vendor/ffmpeg/engine.sha256` (`--check` self-records on first run
and fails loudly when a swapped engine no longer matches), the NSIS installer
script (`installer/`), and the recorded FFmpeg licensing decision
(`docs/FFMPEG-LICENSE.md`: the GPL engine stays, keeping x264/x265 for the
4K profiles; the installer ships the engine's license text and upstream
source URL).

Earlier sessions: settings persistence and capture profiles, the
bundled FFmpeg engine, diagnostics, the Windows tray (running the authored
icon, menu, message loop, autostart sync, and single-instance guard),
global hotkeys, and the recording pipeline — frames stream into FFmpeg
for H.264/MP4 records and lossless PNG stills, driven by the tray menu,
hotkeys, or `--record`/`--photo`. Frame capture picks Desktop Duplication
(COM built by hand in std Rust, `src/dxgi/`) when the plan is one whole
unrotated output — with the cursor composited in software from the
duplication pointer stream when asked for (`src/dxgi/cursor.rs`) — and
falls back to GDI `BitBlt` for regions, windows, and everything the
compositor refuses, printing the reason. A lost duplication session
(display mode switches, competing capture apps) is rebuilt mid-recording;
the recording holds its last frame until the service returns. Region
capture drags a rectangle on a fullscreen selector and
window capture auto-detects the focused application's bounds (DWM
extended frame bounds); both are one menu entry or `--mode` away.
Finished recordings are verified with bundled `ffprobe`. A dedicated
encoder module (`src/encoder.rs`) isolates x264 so a later codec swap
(VP9/AV1/hardware) needs only edits there.

Session 5 brought best-effort macOS and Linux capture: every frame
backend lives behind one `Target` API (`src/capture/`), with macOS
drawing the compositor's window-list image into a premultiplied-BGRA
bitmap context (Retina scales device pixels through the region math) and
Linux reading `XGetImage` roots off libX11 (loaded by soname, alpha
repaired per frame). Login autostart writes the macOS `LaunchAgent`
plist and the freedesktop `.desktop` entry; the single-instance guard
holds an advisory file lock on Unix. Windows-only capabilities answer
with clean messages on Linux (region selector, tray, hotkeys, and
cursor compositing, which X11 leaves for a session with a live
desktop); session 12 lifted the window picker onto macOS through the
compositor's window list, and a later session on that hardware is
expected to cover the live smoke and the remaining gaps per
`ROADMAP.md`. Wayland is explicitly unsupported. None of the mac/Linux
code has run on real hardware yet: it is header-verified (Apple SDK and
xorgproto/Xlib.h) and compiles clean per-target (`cargo clippy --target
aarch64-apple-darwin` / `x86_64-unknown-linux-gnu`), with runtime
hardening expected from CI and user reports.

Measured on the reference panel (1920x1200, 4K profile): full-screen
capture tracks wall time through Desktop Duplication (250 frames /
4.17 s at 60/1 in the reference run, where GDI alone sustained ~30 live
fps because a 33 MB `BitBlt` + pipe write misses the 16 ms frame
deadline), and an interrupted session recovers and still closes honest
(500 frames / 8.33 s after a mid-flight duplication steal). These numbers
are now reproducible rather than historical: `--check` times six blit
frames and prints frames, seconds, fps and ms/frame, and every saved
recording logs captured frames, elapsed wall time and measured fps beside
the ffprobe encoded count. See `ROADMAP.md`'s deferred backlog for what
remains (mac/Linux live runs, timestamped pacing, duplication throughput,
installer run/uninstall).

</details>
