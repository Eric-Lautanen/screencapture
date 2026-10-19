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
- Hotkey combos are fixed to `ctrl+alt+1/2` until the settings UI
  exists; live re-registration arrives with it.

## Session 4 - Region UX + icon + verification (done)

- [x] Drawn-rectangle capture selector (`src/selector.rs`): fullscreen
  topmost popup, drag a dashed rubber band, Escape or a <8 px click
  cancels; returns a `capture::Region` in physical pixels. Verified
  end-to-end with a scripted mouse drag.
- [x] Auto window-bounds capture (`src/screen.rs`): the application the
  user had focused just before the tray menu opened is remembered; DWM
  extended frame bounds (no borders/shadows) with a virtual-desktop
  clamp; falls back to the window under the cursor.
- [x] `[capture] mode = full | region | window` in settings; tray menu
  gains "Record region..." and "Record window"; CLI gains
  `--mode full | region | window`.
- [x] Authored tray icon: `tools/gen_icon.rs` renders and packs
  `assets/icon.ico` (5 sizes, std-only); `src/tray/win/ico.rs` embeds it
  with `include_bytes!` and builds the `HICON` through
  `CreateIconFromResourceEx` (system icon stays as fallback).
- [x] `ffprobe` verification after each recording (frame count printed);
  doctor reports the engine as before.
- [x] Shared Win32 declarations moved to `src/winapi.rs` (window
  plumbing used by tray + selector declared and documented once).
- [x] Multimedia timer resolution raised during recordings
  (`timeBeginPeriod(1)`), demo pacing steps in 1 ms — region/window
  captures hit the full profile rate with dead-on durations.
- [x] All gates green: fmt, pedantic clippy `-D warnings`, 25 tests,
  all sources under 500 lines.
- Measurements (release build, single 1920x1200 panel): window/region
  captures sustain the full rate (118 frames/2 s, 1.97 s wall) because
  writes shrink with the region. Full-screen 4K stays GDI-bound at
  ~30 live fps (33 MB blit + write misses the 16 ms deadline), so
  recorded duration still lags wall time there.
- deltas carried into session 5:
  - DXGI Desktop Duplication for true full-screen 4K60 (per-monitor);
    the GDI path stays as fallback.
  - Photos remain full-primary stills; region/window photo selectors
    arrive with the settings UI.
  - Live hotkey re-registration waits for the settings UI.
  - 16-bit still pipeline and the logging ring buffer move to a later
    polish session.

## Session 4b - DXGI Desktop Duplication (done)

- [x] COM plumbing in std Rust (`src/dxgi/{mod,abi,com,ffi,structs}.rs`):
  `dxgi.dll` + `d3d11.dll` loaded by name, manual v-tables for factory /
  adapter / output / output1 / duplication / resource / device / context,
  all slot orders and GUIDs verified against the mingw-w64 WIDL headers
  plus live v-table dumps; every declared slot participates in one
  integrity check before the first call.
- [x] Capture loop: `DuplicateOutput` on the output whose desktop
  coordinates equal the plan region, `AcquireNextFrame(0)` per tick
  (timeout keeps the previous frame, so pacing never depends on the
  compositor), GPU `CopyResource` into a CPU-readable staging texture,
  row-pitch compaction into the same tight BGRA layout the pipe expects
  (`pipe.rs`/`encoder.rs` untouched).
- [x] Policy: duplication carries a plan only when the region is exactly
  one unrotated output, no cursor is requested, and the panel produces
  plain BGRA — HDR, rotations, partial regions, and pointer drawing stay
  GDI behind the same `Source::frame()` contract; pipeline failures fall
  back with a printed reason.
- [x] Doctor reports the duplication probe (`ok capture: desktop
  duplication (WxH)`) without failing otherwise-unhealthy installs.
- [x] All gates green: fmt, pedantic clippy `-D warnings`, 28 tests, all
  sources under 500 lines (`dxgi/ffi.rs` split into abi/com/ffi/structs).
- Measurements (debug build, single 1920x1200 panel, 60 fps profile):
  full-screen 4K output now tracks wall time — 250 frames / 4.17 s wall
  at 60/1 (GDI baseline was ~30 live fps, the exact hole this closes).
  Window/region recording behavior is unchanged and verified (GDI
  fallback with a printed reason, 72 frames/1.2 s in the same test run).
- deltas from 4b (closed in session 4c below, except the last):
  - ~~Session-recovery on `DXGI_ERROR_ACCESS_LOST`~~ done in 4c.
  - ~~Cursor overlay through `GetFramePointerShape`~~ done in 4c.
  - Per-display picking (tray menu/settings) waits for the settings UI;
    CLI uses the primary output for now.

## Session 4c - DXGI hardening: session recovery + cursor compositing (done)

- [x] `DXGI_ERROR_ACCESS_LOST` session recovery: a lost session (mode
  switch, desktop transition, a second duplicator stealing the output) is
  dropped and rebuilt in place — the recording pauses on its last frame
  meanwhile and only stops when the duplication service never comes back
  (60 retries, 20 ms apart). Rebuilds re-run the factory > adapter >
  output chain against the *current* display arrangement, keep the device,
  and re-verify the mode shape in BGRA before swapping the new session
  in. Live smoke: concurrent `--check` probe stealing duplication at
  ~3 s into an 8 s recording — the recording survived and closed
  wall-honest (500 frames / 8.33 s at 60/1).
- [x] Cursor compositing through `src/dxgi/cursor.rs` (pure math, unit
  tested): the duplication pointer stream (`DXGI_OUTDUPL_FRAME_INFO` +
  `GetFramePointerShape`, slot 11) feeds a software compositor following
  the documented semantics cross-checked against Microsoft's duplication
  sample — monochrome AND/XOR masks MSB-first, masked-color XOR rule,
  straight-alpha color blend, top-left anchoring, edge clipping. Pointer
  pose comes from each acquired frame, and on quiet screens (no acquire)
  from the shell's cursor state (`GetCursorInfo`, desktop-to-output
  conversion via the output origin). Verified visually: an extracted
  frame shows the composited arrow at the reported pose.
- [x] Policy change: a requested cursor no longer demotes a plan to GDI —
  duplication now carries full-screen cursor plans by drawing the pointer
  in software, so the fast path applies in both runtime cursor settings
  (still-unit-fallbacks: partial regions, HDR, rotation). Quiet-screen
  shape seeding uses the documented size-probe protocol when no pointer
  update was announced, so the bitmap is always in hand.
- [x] Module split: selection/validation helpers moved to
  `src/dxgi/policy.rs` (`pick_output`, `matches_area`, `shape_of`,
  v-table `surface` preflight, error surfaces) to keep every file under
  the 500-line cap.
- [x] All gates green: fmt, pedantic clippy `-D warnings`, 33 tests,
  longest file 440 lines.
- deltas not carried (closed by this session): session recovery and the
  duplication cursor overlay. Still deferred: per-display picking,
  16-bit stills, logging ring buffer, region/window photos, live hotkey
  re-registration, multi-monitor cursor anchoring check (the pointer
  position is documented output-relative; a secondary-panel run should
  confirm driver conformance), monochrome/XOR cursor shapes only
  exercised by unit test (live cursors here were COLOR type).

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
