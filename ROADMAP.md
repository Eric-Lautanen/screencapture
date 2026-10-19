# Roadmap

Plan of record, in build order. Every session ends with
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test` green, files under 500 lines, no `allow` blocks.

## Session 1 (done)

- [x] Toolchain pinned and verified (rustc 1.99, clippy, rustfmt).
- [x] Latest stable FFmpeg fetched into `vendor/ffmpeg/` (n9.0.2 daily build).
- [x] Std-only settings format, typed settings view, safe defaults.
- [x] Capture profiles up to 4K60/4K120.
- [x] FFmpeg engine discovery (configured > bundled > developer > PATH).
- [x] `--check` / `--init` diagnostics passed end to end.

## Session 2 - Windows tray core (done)

- [x] `src/tray/win/` with documented Win32 declarations (edition-2024
  `unsafe extern`): tray icon via `Shell_NotifyIconW`, hidden message
  window, message loop, popup menus through `user32`/`shell32`. (Icon
  resource generation arrives with the installer session.)
- [x] Tray menu: record, photo, profile submenu, autostart toggle, exit.
- [x] Autostart toggle: `HKCU\...\Run` synced from the settings flag.
- [x] Single-instance guard (`Local\` mutex) verified by smoke test.
- [x] Unix-safe build: non-Windows tray paths return a clear error.

## Session 3 - Recording pipeline (done)

- [x] Frame capture: GDI `BitBlt` into a top-down BGRA DIB (per-monitor
  DPI aware, physical pixels), lent straight to the engine pipe — no
  per-frame allocation or extra copy. `capture::Target` owns the DCs and
  bitmap for the session and is dropped cleanly.
- [x] Engine wiring: raw BGRA frames stream into ffmpeg
  (`-f rawvideo -pix_fmt bgra`), which encodes with the settings codec
  (x264 via `src/encoder.rs`, swappable to VP9/AV1/hardware later without
  touching callers). Frame feed shared by records and photos
  (`src/pipe.rs`).
- [x] Recording lifecycle (`src/recorder.rs`): `start/tick/stop`, output
  stamped UTC `screencapture-YYYYMMDD-HHMMSS.mp4`, profile resolution kept
  by an engine-side `scale` filter when the panel differs; fps comes from
  the profile. The tray loop emits `Event::Tick` from `WM_TIMER`
  (`tray::set_frame_timer`), hotkeys register through
  `RegisterHotKey` (`src/hotkeys.rs`, bound into the tray window).
- [x] Still photos: single frame, lossless PNG
  (`-c:v png -compression_level 9`), same-second clashes get `-2`, `-3`...
  suffixes.
- [x] Headless paths double as smoke tests: `--record S` and `--photo`
  produce files verified by the bundled `ffprobe`.
- [x] All gates green: fmt, pedantic clippy `-D warnings`, 21 tests,
  all sources under 500 lines.
- Measurements (release build, single 1920x1200 panel): every capture tick
  costs ~15-20 ms (blit + pipe write), so the GDI path sustains ~40-45 fps
  at the 4K profile with deadline-paced pacing. Recording duration drifts
  behind wall time when capture fps lags the profile frame rate.
- Known deltas carried into session 4:
  - Region = full primary monitor only; the drawn-rectangle overlay and
    auto window-bounds selectors are next.
  - True 4K60 needs DXGI Desktop Duplication (GDI is the current path and
    will stay as fallback).
  - `stop` waits on the encoder, so the UI thread beats a moment while a
    large finalization completes.
  - Hotkey combos are fixed to `ctrl+alt+1/2` until the settings UI
    exists; live re-registration arrives with it.

## Session 4 - Region UX + DXGI + icon

- [ ] Drawn-rectangle capture overlay (transparent layered window, drag
  select, rubber band), plus auto window-bounds capture.
- [ ] DXGI Desktop Duplication frames (same BGRA pipe) for real 4K60 and
  per-monitor capture; the GDI path stays as fallback.
- [ ] Authored tray icon embedded in the binary (replaces the system
  default loaded today).
- [ ] `ffprobe`-based recording verification after each capture; the
  doctor gains a capture smoke option.
- [ ] Lossless 16-bit still-pipeline investigation (PNG `rgb48` or TIFF)
  behind a settings value.
- [ ] Lightweight logging ring buffer for capture sessions.

## Session 5 - macOS + Linux

- [ ] macOS: ScreenCaptureKit via direct FFI bindings, menu-bar equivalent
  of tray, `Mal`-style launch-agent autostart.
- [ ] Linux: X11/Wayland (portal) capture, `org.kde.StatusNotifierItem`
  tray, desktop autostart entry.
- [ ] Platform parity tests and doctor coverage on all three OSes.

## Session 6 - Ship it

- [ ] FFmpeg licensing decision for the *baked-in* engine: GPL build keeps
  x264 for free use; LGPL build (no x264; VP9/AV1/HW) if the app ships
  proprietary. Vendor layout supports either.
- [ ] Installers: NSIS/WiX on Windows with icon/theming, DMG/pkl on macOS,
      `.deb`/`.rpm` on Linux.
- [ ] Auto-update checks for the engine binary; sha256 match recorded in
      `vendor/ffmpeg/engine.sha256`.
- [ ] Release profile (LTO, strip) and installer smoke tests.
