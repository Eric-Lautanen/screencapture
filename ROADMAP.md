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

## Session 5 - macOS + Linux (blind, header-verified)

- [x] Fact check pass before any code: Apple SDK headers verified every
  CoreGraphics symbol in use (`CGWindowListCreateImage` signature and
  top-left point space, `CGBitmapContextCreate` layout params,
  `CG*CTM`/draw/gstate calls, `CGMainDisplayID`/`CGDisplayBounds`/
  `CGDisplayPixelsWide/High`); note `kCGWindowImageShouldBeOpaque`
  is `(1 << 1)` and `kCGWindowImageBestResolution` is `(1 << 3)` —
  both confirmed from `CGWindow.h`, not memory. X11 facts verified
  from libX11 `Xlib.h` + xorgproto `X.h` (which moved out of libX11's
  tree — hence the 404s): `ZPixmap = 2`, `AllPlanes = ~0UL`,
  `XGetImage` signature, the full `XImage` record layout, and
  `f.destroy_image` as function-table entry 1.
- [x] Platform seams: `capture.rs` and `screen.rs` became directories
  (`mod.rs` + `win.rs`/`mac.rs`/`linux.rs`); `Target`/`prepare`/window
  geometry are platform-neutral; `dxgi`, `winapi`, `selector` are
  cfg-gated; `recorder.rs` keeps duplication as the Windows-only fast
  source; non-Windows region records use the native backend.
- [x] New C surfaces loading by name at runtime (mirroring the dxgi
  loader, module parked loaded for the process life): `src/dylib.rs`
  (dlopen/dlsym, `#[link(name = "dl")]` holds on both Unices),
  `src/coregraphics.rs` (18 exports), `src/xlib.rs` (7 exports).
- [x] macOS backend: compositor draws the window-list image (opaque +
  best-resolution options) into a cached premultiplied-BGRA bitmap
  context; CTM flip keeps rows top-down; region pixels map to
  screen-space points through the main display scale. Cursor overlay is
  not composited on this route (documented no-op); multi-display
  origins deferred (needs a Mac-verified session for the rounding
  policy); Retina capture carries device pixels via
  `kCGWindowImageBestResolution`.
- [x] Linux backend: one `XGetImage` per frame off the root drawable in
  `ZPixmap`; layout asserted to 32-bit little-endian true color; the
  server's zeroed alpha byte is forced opaque per frame so PNG stills
  stay visible. Wayland answers a clean unsupported message (no X
  server).
- [x] Doctor parity: `--check` reports the native backend and geometry
  on every platform; a missing backend is a FAIL outside Windows
  (inside Windows a duplication miss is a GDI-fallback notice).
- [x] Unix autostart: macOS `LaunchAgent` plist (`dev.screencapture`)
  and freedesktop `.desktop` entry, file-presence registration,
  XML/Exec escaping, `XDG_CONFIG_HOME` honored; unit-tested with
  explicit-home resolution paths (stay runnable in CI where no user
  agent dir exists... tests use only path math plus temp fixtures).
- [x] Unix single-instance guard via `std::fs` advisory file lock
  (stable File::try_lock in this toolchain): a leftover file is never
  a stale lock because the OS frees the lock at process death; handle
  parked for the process lifetime like the Windows mutex.
- [x] Windows-only capabilities degrade with actionable text on
  mac/Linux: region selector, window picker, tray, hotkeys, and
  `previous_foreground`; `--record`/`--photo` full-screen run
  everywhere.
- [x] Runtime behavior: cursor drawing, tray, and region/window capture
  remain Windows-only this session (see deferred list); mac/Linux code
  has NOT executed — hardening confirmed by cross-target pedantic
  clippy (`aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`) plus user
  reports, with GitHub Actions expected to carry it from here.
- Verified live post-split (this Windows box): `--check` reports
  `desktop duplication (1920x1200)`, a 2 s `--record 2` lands a probe-
  verified MP4, fmt/pedantic clippy/33 tests green, per-target clippy
  green on both non-Windows targets.

## Session 6 - Ship it (Windows half next, testable here)

- [x] FFmpeg licensing decision, recorded in `docs/FFMPEG-LICENSE.md`:
  the engine stays the BtbN **win64-gpl** build (keeps x264/x265 for the
  4K profiles); the app remains a separate work (mere aggregation), so
  distribution obligations cover the engine only — ship its license
  text and the upstream source URL. The `vendor/` layout still supports
  an LGPL switch if a future distribution plan demands it (drop x264,
  re-record, re-verify encoders).
- [x] Release profile: `lto = "fat"`, `codegen-units = 1`, `strip = true`
  (release exe ~0.5 MB on the reference box).
- [x] Engine integrity: pure-std SHA-256 (`src/sha256.rs`, streaming
  hasher, FIPS 180-4 vectors incl. the million-`a` digest verified
  against .NET) under `vendor/ffmpeg/engine.sha256`; `--check` records
  on first run and fails loudly on mismatch with a recovery hint
  (delete the record after a deliberate engine update).
- [x] Icon resource baked into the exe: std-only `build.rs` runs
  `windres` on `assets/icon.rc` (resource id 1 = DEFAULT_ICON) and links
  the COFF object into the binary; missing or failing `windres` prints a
  notice and keeps the build green. Verified live: the exe exports its
  32x32 icon.
- [x] Windows installer script: NSIS/MUI2 `installer/nsis/screencapture.nsi`
  (app + engine + licenses + `engine.sha256`, Start-Menu shortcuts,
  uninstall entries, estimated size, optional launch-on-finish).
  `installer/README.md` explains prerequisites and the build command.
  NSIS is not on this machine — the script is the plan of record;
  smoke tests run wherever NSIS lands (this box later, or CI).
- [ ] DMG (macOS), `.deb`/`.rpm` (Linux) packaging: deferred until the
  mac/Linux runtimes are exercised by CI/user reports (blind packaging
  now would be dead weight to re-verify later).
- Verified this session: fmt + pedantic clippy green on all three
  targets (`-D warnings`), 37 tests green (the four SHA-256 vectors
  included), release build embeds the icon, `--check` records then
  verifies `engine.sha256` (`f9c4d64e..`).

## Session 7 - Per-display picking

- [x] Settings: `[capture] display` (`primary` default, or a 1-based
  number 1-16; anything else falls back with a problem line), typed as
  `DisplayChoice` (`src/settings.rs`, unit-tested).
- [x] CLI: `--display N` overrides the choice for `--record` and
  `--photo` full captures; parse errors are honest (1-16).
- [x] Tray: a "Record display" submenu appears when more than one
  display is attached (labels carry size; non-zero origins marked
  `+x,+y`), ids in a reserved dynamic range `[ID_DISPLAY_BASE,
  ID_EXIT)` that cannot collide with profiles; one-refresh menu built
  per open, like profiles.
- [x] App plumbing: `screen::display_at(n)` (error message names the
  number and its absence), full/record/photo paths resolve choice →
  region; cross-platform intact (mac answers the main display list,
  Linux the X11 root).
- [x] Doctor: `--check` prints `display: primary (N attached)` or the
  numbered choice with an "is not attached" note when stale.
- [x] Live smoke (single-panel reference box): `--display 2` answers
  `error: display 2 is not attached`; `--record 2 --display 1` lands a
  probe-verified file; `--check` prints the display line; `--photo`
  unaffected; flag parse errors guarded.
- Deferred from this space: numbered display names (via
  `EnumDisplayDevices`) — geometry labels suffice until a multi-panel
  machine shows the need; mac multi-display enumeration still waits for
  its Mac-verified session.

## Session 8 - Hardware codecs

- [x] Encoder module upgrade (`src/encoder.rs`): `Video` gains AMF, QSV,
  NVENC, and Windows Media Foundation h264 variants (parse/id/name per
  variant; extension stays `mp4`). Hardware arg sets contain only
  options verified against this engine's `-h encoder` pages
  (AMF `-quality high_quality`; QSV `-look_ahead`; NVENC
  `-preset p4 -tune hq`; MF defaults); rate control stays at vendor
  defaults, vendor probing decides.
- [x] Probe before record (`src/ffmpeg.rs`
  `Binary::supports_encoder`): one short `testsrc2` synthetic encode
  with the encoder's exact argument set (`-f null`); success proves the
  accelerator is present, the driver loads, and every option parses.
- [x] Fallback on start (`src/app.rs` `effective_codec`): hardware
  codec + failed probe → x264 with a `warn` notice, at both recording
  entry points (tray and `--record`/`--photo` pipeline), once per
  recording start.
- [x] Doctor: `--check` now lists all five encoders
  (`encoder [cpu|hw]: <name> is ready|not usable`) and marks the
  configured one; non-fatal (x264 fallback keeps recordings alive).
- [x] Settings template documents the codec choices and the fallback
  contract; round-trip test extended.
- [x] Live smoke (AMD 780M reference box): doctor shows `x264/amf/mf`
  ready and `qsv/nvenc` not usable; `codec = amf` config records a
  verified MP4; `codec = nvenc` config prints the warn and records
  via x264; parse round-trips unit-tested (40 tests).
- Deferred in this space: HEVC hardware variants (driver coverage too
  murky to blind-ship), quality-rate-control parity mapping per vendor
  (needs a quality-comparison session with frames to look at).

## Session 9 - Caps and visibility: app split + log ring

- [x] `src/app.rs` split at 478/500 lines: tray runtime stays in
  `app/mod.rs` (416 lines), the headless `--record`/`--photo`
  pipelines moved to `src/app/demo.rs` (133 lines); helpers stay
  module-private and reachable from the sibling by Rust's default
  visibility, zero public API churn.
- [x] Diagnostics ring (`src/log.rs`, unit-tested): `note(tag, msg)`
  mirrors the console line into a thread-safe 64-entry ring with
  timestamps; `flush_to(media_dir)` appends the ring to
  `screencapture.log` (file restarts past 512 KiB; IO failures silent).
- [x] Tray wiring: settings-load warnings, autostart conflicts, region
  errors, record start/stop (frame counts), photo results, tick
  failures (with immediate flush), encoder demotions, and settings
  saves all flow through the ring; stop + exit + tick-failure flush
  points. Reflections on the channels: the tray handler owns both
  settings and runtime, so the startup clones the log directory for
  the final flush after the loop ends.
- [x] Demo wiring: headless records log start/saved lines and flush, so
  even CLI runs leave a retrievable trail.
- [x] Doctor: `--check` prints `log file: <path> (N lines)` or `none
  yet`; the log check lives in the settings block (media dir known).
- [x] Also cleaned: duplicated `sane_region` doc comment; runtime
  carries `media_dir` for spot flushes.
- [x] Gates: fmt, pedantic clippy `-D warnings` (native + both cross
  targets), 43 tests; live smoke wrote the two-line log correctly and
  `--check` reported it.

## Session 10 - Installer verified (NSIS 3.13 portable)

- [x] NSIS obtained without admin or the SourceForge UI: the portable
  3.13 zip fetched from a direct mirror (`deac-riga.dl.sourceforge.net`,
  direct mirror downloads dodge the JS mirror chooser; `?download=1`
  and a plain UA matter), hash verified against scoop's extraction of
  the upstream manifest (`db14f8af…`).
- [x] Script fixes from the first real compile: `ROOT` became
  overridable (`!ifndef` + `/DROOT=` on the command line — a
  hard-define collides with `/DROOT`), and an invented
  `DefaultLangCode` instruction (not real NSIS) removed; `MUI2` alone
  owns the language table.
- [x] First real build: `installer\screencapture-setup.exe` (~122 MB;
  the engine block compresses ~330 MB to ~128 MB inside), compile log
  clean, 4 install pages, uninstall section included, deterministic
  on rebuild (datablock optimizer's only saving: 1.9 KB).
- [x] Payload proven by the compiler: makensis fails the run on any
  `File:` path it cannot open, so the clean build guarantees every
  payload entry (exe, README, license docs, engine, probe,
  `engine.sha256`) embedded. 7-Zip on this box had no NSIS support for
  post-hoc listing; compiler-log validation suffices.
- [x] `installer/README.md` updated with the working invocation
  (`/DROOT=$PWD`), the verified version + hash, and the explicit note
  that interactive/silent install + uninstall runs belong in GUI/CI
  contexts (the script claims admin; this shell cannot elevate).
- Deferred: the actual run/uninstall cycle (elevation + GUI needs a
  session without automation); ArtifactsPublish-style CI once the repo
  goes public per the earlier Actions plan.
