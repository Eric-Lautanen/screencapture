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

## Session 2 - Windows tray core

- [ ] `ffi/` module with documented Win32 declarations (edition-2024
  `unsafe extern`), tray via `Shell_NotifyIconW`, hidden message window,
  message loop, resource-file icon (generated from project icon sources).
- [ ] Tray menu: record, photo, profile, settings, quit.
- [ ] Autostart toggle: `HKCU\...\Run` plus settings flag.
- [ ] Single instance guard.

## Session 3 - Recording pipeline

- [ ] Frame capture via GDI/DXGI into a BGRA pipe (`-f rawvideo -pix_fmt
  bgra`); ffmpeg encodes to H.264 (x264) with the profile's quality level.
- [ ] Region capture: drawn rectangle overlay, or auto-detect via visible
  window bounds (per-monitor DPI aware).
- [ ] Still photos: single frame to a maximally high-quality `image format`
      (PNG first; lossless 16-bit pipeline investigation).
- [ ] Framerates/resolutions from settings; on-the-fly reconfiguration from
  the tray menu.

## Session 4 - Hotkeys + polish

- [ ] Global hotkeys (`RegisterHotKey`) wired to settings; conflict
  detection reported through the doctor.
- [ ] Lightweight logging ring buffer for capture sessions.
- [ ] `ffprobe`-based recording verification after each session.

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
