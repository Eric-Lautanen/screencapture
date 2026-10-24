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

## Session 11 - Headless rect capture + Linux window picking (done)

- [x] `--rect X,Y,W,H` (CLI, any platform): one desktop rectangle
  drives both `record_demo` (region semantics, no selector) and
  `photo_demo` (replaces the whole-display pick). Parse validates four
  integers, edges inside the sane engine range (1..7680), and rejects
  conflicts (`--display` versus `--rect`; non-region `--mode` with
  `--rect`); tests cover field parsing and rejects. Live-smoked:
  `--record 1 --rect 0,0,640,480` and `--record 1 --rect
  100,100,800,450` both produced correct 4K-target h264 streams per
  ffprobe (one initial run emitted an empty 261-byte mp4 shell that
  two clean reproductions could not reproduce; logged as an
  environment flake, not a pipeline defect).
- [x] Linux window capture (blind-verified): `screen::window_region`
  now answers on Linux with the window-under-the-cursor's client
  rectangle in root coordinates, clamped to the root extent;
  cursor-on-desktop captures the whole root, mirroring the Windows
  desktop-handle answer. `preferred` stays a Windows notion.
- [x] `src/xlib.rs` grew `XQueryPointer`, `XGetGeometry`, and
  `XTranslateCoordinates` surfaces (out-parameter bundles verbatim to
  the ABI), signatures re-checked against the cached upstream
  `Xlib.h`; the X connection moved into an owned `Session` with a
  `Drop` close.
- [x] `--rect` photo semantics share the tray photo pipeline
  (`take_photo` gained an optional rect; tray callers pass `None`).
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus both
  cross targets, 45 tests.
- Deferred: live Linux rect/window runs (no X server here), macOS
  window/region capture (needs the CoreGraphics window-list session
  with `CFArray`/`CFDictionary` surfaces — next parity item).

## Session 12 - macOS window capture (blind, header-verified)

- [x] Fact check pass before any code, same discipline as session 5:
  `CGWindow.h` confirms `CGWindowListCopyWindowInfo(option,
  relativeToWindow) -> CFArrayRef __nullable` (Copy rule — caller
  releases), its on-screen list ordered front-to-back, and the keys as
  `CFStringRef` constants documented for their literal names (bounds =
  a dictionary encoding a CGRect; number and layer = CFNumbers);
  `CGGeometry.h` confirms the modern `CGRectMakeWithDictionary-
  Representation(CFDictionaryRef, CGRect*) -> bool` form (the old
  CGRect-returning form could not be found in any checked SDK, back to
  10.5); `CGWindowLevel.h` (not CGWindow.h) carries the levels
  (normal 0, desktop -2147483623, dock 20, int32_t); `CGEvent.h`
  confirms `CGEventCreate(NULL source)` + `CGEventGetLocation` and the
  global display space (origin at the main display's top-left — the
  *flipped* variant is `CGEventGetUnflippedLocation`); the old new
  window-list options and `kCGNullWindowID = 0` re-confirmed.
- [x] New CoreFoundation C surface `src/corefoundation.rs` (loaded by
  framework path through `dylib`, parked for the process life):
  `CFArrayGetCount`/`GetValueAtIndex`, `CFDictionaryGetValue`,
  `CFNumberGetValue` (with `kCFNumberFloat64Type` coercion),
  `CFStringCreateWithCString` (`kCFStringEncodingUTF8`, `c_char`),
  `CFRelease` — plus null-safe wrappers (`array_len`, `array_value`,
  `dict_value`, `number_f64`, `utf8`, `release`).
- [x] CoreGraphics grew the four picker exports (verified against the SDK
  research): `CGWindowListCopyWindowInfo`,
  `CGRectMakeWithDictionaryRepresentation`, `CGEventCreate`,
  `CGEventGetLocation`; plus `window_list()` (on-screen + all windows,
  raw array) and `bounds_value()` (rect decode).
- [x] `screen::window_region` now answers on macOS: the frontmost
  layer-zero window whose bounds hold the cursor (edge-exclusive
  containment; desktop, dock, menu bar, and overlay levels skip via
  layer numbers; entries without usable parts skip) becomes a
  physical-pixel region through the main display's scale; the cursor on
  the desktop (or an absent answer) takes the whole main display,
  mirroring the Linux backend. `preferred` stays ignored here exactly
  as documented for Linux. Bounds outside the display clamp into its
  pixel extent with a guaranteed nonempty visible sliver.
- [x] `--mode window` and `--rect`-style `photo`/region picking on macOS
  flow through the existing platform-neutral pipelines; the previous
  "macOS window picking lands with the mac capture session" refusal is
  gone, and the tray capability table stands (no mac tray exists yet).
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus both cross
  targets, 45 native tests untouched (the new picker tests —
  containment, pixel clamping, bounds→region math at 2x scale — live in
  `screen/mac.rs` and run on macOS hardware); all sources under 500
  lines.
- Assumptions a live Mac session must verify: the window-dictionary key
  literals match their runtime CF values (Apple's own sample code looks
  the keys up as `CFSTR("kCGWindowBounds")`-style compile-time
  literals, but the heads cannot be checked without the framework);
  cursor containment picks sensibly on layered real-world apps; the
  single main-display scale is right for all screens; the
  multi-window rendering pipeline stays correct with picked regions.
- deltas carried into the next mac session: live smoke on hardware
  (window/region/photo + doctor), multi-display origins + mixed scales
  (deferred with the rest), mac multi-display enumeration.

## Session 13 - 16-bit stills (done, live-verified)

- [x] Settings: a `[photo]` section keyed by `depth` (typed
  `StillDepth::Eight | Sixteen`, default 8; the document stores the key
  either as a bare integer or as text, and both shapes parse — invalid
  values fall back with a problem line). The default template ships
  `depth = 8` so today's PNGs stay byte-for-byte in behavior; 16 is an
  opt-in for future 16-bit capture routes.
- [x] Pipeline (`pipe.rs` `still_args`): the 16 depth adds the PNG's
  documented 48-bit form through `-vf format=rgb48be` (engine
  `-h encoder=png` lists `rgb48be` among `Supported pixel formats`);
  compression arguments are identical for both depths. The constant
  255 alpha plane of desktop frames (GDI opaque, X11 repaired) drops
  along the conversion — a no-information plane, which the settings
  template note mentions.
- [x] Plumb: `take_photo` reads `settings.photo_depth`; `--check`
  prints `photo: 8-bit PNG` (or 16) in the settings block.
- [x] Tests: parse + both document shapes + fallback problem count,
  and the pipe test pins the 16-bit args (`format=rgb48be`) against
  the unchanged 8-bit shape (no filter).
- [x] Live smoke (this box, debug build + bundled ffprobe):
  `depth = 16` lands a full-display PNG probed as `pix_fmt=rgb48be`,
  and the unconfigured baseline still answers the long-standing 8-bit
  `rgba`. First smoke run betrayed a stale `target/debug` exe (the
  edit predates the rebuild), not a pipeline defect — rebuilt, clean.
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus both
  cross targets, 48 tests; all sources under 500 lines
  (settings.rs 463 is the longest).
- deltas carried forward: a 16-bit/HDR capture route (DXGI HDR
  surfaces and the 16-bit stills both sit in the same space) still
  waits for an HDR-capable panel; until then 16-bit depth is the
  future-proof hook only.

## Session 14 - HEVC encoders (done, live-verified)

- [x] Fact pass against this engine's live `ffmpeg -h encoder` pages,
  the same used by the probe: `hevc_amf` carries `-quality` with
  `high_quality` (value 15) exactly like its H.264 twin; `hevc_qsv` has
  no `-look_ahead` (only `look_ahead_depth` gated on extbrc), so its HEVC
  variant stays at vendor defaults; `hevc_nvenc` honors `-preset p4
  -tune hq`; `hevc_mf` defaults (its `-rate_control` table exists but
  defaults win); `libx265` takes `-preset veryfast -crf N` and encodes
  a synthetic clip. Pixel formats re-checked per page: QSV's H.264 **and**
  HEVC pages list only `nv12`/`qsv` inputs.
- [x] `encoder.rs` gains five variants: `x265` (CPU HEVC, aliases
  `libx265`/`hevc`), `amf-hevc`, `qsv-hevc`, `nvenc-hevc`, `mf-hevc`
  (each also parses its bare `hevc_*` name); ids/`-c:v` names/parse
  round-trip unit-tested; probe order extends the hardware table at
  eight. Rate control stays at vendor defaults, matching the session-8
  convention (no HEVC quality-mapping decisions blind-shipped).
- [x] Latent bug fixed on the way out: the pipe's `-pix_fmt` was a hard
  `yuv420p` for every codec, but the QSV pages list only `nv12`/`qsv` —
  on real Intel hardware both QSV variants would always have failed the
  probe and demoted. The pixel format is now per-variant (`nv12` for
  the two QSV units, `yuv420p` elsewhere), still verified against the
  pages, and unit-tested.
- [x] Doctor: the codec table lists `libx264`, `libx265`, and all eight
  hardware variants with the same ready/not-usable readout and the
  configured marker.
- [x] Demotion semantics: `effective_codec` probes only hardware
  choices (software encoders never demote); the fallback notice and
  x264 demotion text are unchanged and honest.
- [x] Live smoke (this engine, debug build):
  `--check` reports `libx264`/`libx265`/`h264_amf`/`h264_mf`/
  `hevc_amf` ready while `h264_qsv`/`h264_nvenc`/`hevc_qsv`/
  `hevc_nvenc`/`hevc_mf` answer not usable (this box has an AMD 780M,
  no discrete NVIDIA, and MFT's HEVC encoder is unavailable locally);
  `codec = amf-hevc` recorded a probe-verified stream
  (ffprobe: `hevc`, 3840x2160 @ 60/1); `codec = nvenc-hevc` printed the
  fallback notice and the file answered `h264`; `codec = x265` landed a
  verified `hevc` stream through the CPU path.
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus both
  cross targets, 50 tests; sources under 500 lines.
- deltas carried forward: quality/rate-control mapping per vendor per
  codec still belongs to an eyeball session with frames to compare;
  x265 encode speed is unmeasured on 4K real-time trajectories (the
  doctor table shows availability, not speed); VP9/unlisted units stay
  out until a reason appears.

## Session 15 - Tray reentrancy fix + click-to-pick window selector (done, live-verified)

- [x] The abort users could hit, root-caused: a running recording's
  frame timer (or any hotkey) dispatched `WM_TIMER`/`WM_HOTKEY` into the
  tray window procedure *while the tray handler was on the stack and a
  modal picker pumped messages* - `emit` re-borrowed the handler
  `RefCell` and the pump panicked (`RefCell already borrowed` inside an
  `extern "system"` callback, unwound to `STATUS_STACK_BUFFER_OVERRUN`).
  Fix: `dispatch` answers with `try_borrow_mut`; a dispatch that cannot
  take the borrow queues onto `PENDING` (a VecDeque) instead, and the
  dispatch that owns the borrow drains the queue when its handler
  returns. Order stays fair; nothing drops silently. Regression test
  drives the exact reentrancy (handler emits inside itself) and asserts
  both events answered with an empty queue.
- [x] Modal pickers no longer tick underneath: `RecordWindow` and
  `RecordRegion` stop the running recording first, so the nested pump
  sees no frame-timer traffic at all (selection cancelled = previous
  recording closed, the new "replace" semantics made explicit).
- [x] "Record window" on the tray now opens a real picker
  (`src/window_picker.rs`): the top-level window under the cursor is
  bordered with a XOR marquee on the screen DC (DWM extended frame
  bounds each hover step), left click records the highlighted window,
  Escape or right click cancels. Input comes from low-level mouse and
  keyboard hooks - an overlay window would swallow every hit test
  aimed below it, so the hooks see what the user sees, pass-through
  only. Previously the pick was "whatever sat under the cursor at menu
  click time", which was the tray popup itself (user-reported 234x114
  region) with no visual.
- [x] The picker inherits the region selector's toolkit: hooks declared
  in its own ffi (`SetWindowsHookExW` family, `MSLLHOOKSTRUCT`,
  `KBDLLHOOKSTRUCT`), screen-space coordinates direct from the hook
  payloads (per-monitor-aware), bounds via the DWM surface that the
  capture itself uses. Hook uninstall is symmetric on every exit path.
- [x] Win32 declaration consolidation (`winapi.rs`): `WindowFromPoint`,
  `GetAncestor`, `GetWindowRect`, `GetDC`/`ReleaseDC`,
  `EnumDisplayMonitors`, and the dwmapi block (`DwmGetWindowAttribute`
  + `DWMWA_EXTENDED_FRAME_BOUNDS`) moved in from `screen/win.rs`,
  which now imports them; `Rect` gains `PartialEq` for hover
  comparisons. `previous_foreground` machinery removed - the tray's
  "what was focused before the menu" stash answered only as a proxy
  for the cursor pick and produced the wrong target; the picker
  replaces it cleanly (Windows; cursor semantics unchanged elsewhere).
- [x] Live smoke (this box, scripted fully from the tray API):
  `PostMessageW` of the tray callback drives the menu, arrow keys pick
  "Record window", the cursor moves over the taskbar and clicks -
  recording starts on the real taskbar strip (`1920x48 from 0,1152`,
  the GDI fallback reason printed for the partial region as designed);
  the tray "Record" item stops it (`saved ... 137 frames`, ffprobe:
  h264 3840x2160 at the profile scale); the tray "Exit" item quits the
  pump cleanly (self-exit verified - zero instances left). The old
  reentrancy pattern (modal flow under an active handler) ran
  throughout with no panic. Note: ctrl+alt+1 sent via `SendInput`
  never registered through `RegisterHotKey` in this harness - the
  scripted stop went through the tray menu instead; human-key hotkeys
  still need your hands to confirm (prior sessions only ever verified
  real-mouse interaction the same way).
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus both
  cross targets, 52 tests; sources under 500 lines (app.rs 449 is the
  longest).
- Deferred (user-requested, next session): the preferences/hotkeys
  window - settings file read/write through the same config format,
  hotkey combos beyond the fixed `ctrl+alt+1/2`, and live hotkey
  re-registration; scoped with the reload path the tray handler needs.
  (Delivered in session 16.)

## Session 16 - Stills through the mode system + Settings window (done)

- [x] One selection path, two outputs: the tray "Photo" (and the photo
  hotkey) honors `[capture] mode` exactly like "Record" - Full screen,
  Drawn region (drag selector), Picked window (the click-to-pick
  session) - and differs only in the output (lossless PNG vs recording).
  The redundant per-target menu items collapse; `select_region` in
  `src/app.rs` is the single shared path, `photo_in` captures one frame
  and feeds the still pipe.
- [x] Separate destination folders: `[media] videos` for recordings and
  `[media] images` for photos (defaults `Videos`,
  `Pictures/Screencapture` on Windows/macOS, XDG dirs + app subfolder on
  Linux); the legacy `[media] directory` key still answers for
  recordings (pre-split configs pointed it at the videos folder), while
  photos keep their Pictures default. Defaults template, reader
  fallbacks, `--check`, and the log/flush dirs follow the split; unit
  tests cover the split and the legacy behavior. Live smoke: `--photo`
  wrote its PNG into the user's `Pictures` default (the first run,
  before this legacy tweak, kept landing in `Videos` - caught and
  fixed), verified with the bundled `ffprobe`.
- [x] Free hotkey combos: `Hotkey` is now a modifier set (ctrl/alt/
  shift/win, any order in parsing, `+`/`-` separators) over a digit,
  letter, or F1-F12 key with at least one modifier required; canonical
  display form via `to_combo`. Defaults moved to the user-chosen
  `ctrl+shift+x` (record) and `ctrl+shift+z` (photo). Parsing refuses
  bare modifiers, two keys, and out-of-range F-keys; round-trip tested.
  Bindings derive `MOD_*` flags straight from the combo
  (`src/hotkeys.rs` rewritten; `MOD_SHIFT`/`MOD_WIN` added).
- [x] Live re-registration: `tray::set_bindings` unregisters the old
  `RegisterHotKey` table and installs the new one on the pump thread
  (Windows); the seam stays stubbed on non-Windows with the rest of the
  tray.
- [x] The Settings window (`src/settings_dialog.rs`, std-only Win32):
  modal class-registered dialog with a combobox row for capture mode,
  display (primary + attached count), video profile, video codec, photo
  depth; checkboxes for cursor and autostart; text rows for the two
  hotkey combos (validated on Save - parse errors and equal combos
  complain and keep the window open); text rows + native folder browse
  (`SHBrowseForFolderW`) for the videos and images folders. Save mutates
  the document, persists, reloads the typed view, resyncs autostart,
  swaps the hotkeys live; Cancel/close leaves all state untouched.
  Tray menu gains "Settings..." (`Event::OpenSettings`).
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus both
  cross targets, 62 tests; sources under 500 lines (the settings dialog
  grew to ~910 lines in one file, so it split into
  `src/settings_dialog/{mod,rows,ffi}.rs` at 411/455/98).
- Deferred: real-mouse confirmation of the settings dialog interactions
  (the picker precedent applies, unit tests + smoke cover the seams);
  the settings UI screenshot pass; non-Windows tray (stays stubbed).

## Session 17 - One interactive session: click the window, drag the region (done)

- [x] Hardening pass (user request): Drop-based RAII guards now own
  every acquire/release pair, so a panic landing between the two calls
  can no longer leak the resource. New `src/guard.rs` (Windows)
  provides `ScreenDc` (`GetDC`/`ReleaseDC`, desktop or per-window),
  `Hook` (`SetWindowsHookExW` uninstalled in `Drop`), and
  `MouseCapture` (`SetCapture`/`ReleaseCapture`) - double release is
  impossible by construction (`Option`-taken handles). `window_picker`
  hooks and every XOR-draw borrow, the selector's marquee repaint and
  drag capture, and the settings browse pidl (`Pidl`, freed by
  `CoTaskMemFree` in `Drop`) all moved onto guards; the manual
  `client_dc`/`release_client_dc` pair is gone. Already-Drop owners
  confirmed and left alone: `dxgi::com::Com<T>` (COM interfaces) and
  `capture::win`'s `Target` (GDI DCs/bitmap). New tests cover handle
  identity/non-null answers, the desktop-DC guard, capture
  take-or-refuse, and module-handle resolution. 73 tests.
- [x] Belt and suspenders (user request): `unsafe_op_in_unsafe_fn =
  "deny"` pinned in `[lints.rust]` so a future unsafe op placed bare in
  an unsafe fn fails every build. Zero fix-ups surfaced on native or
  either cross target - earlier passes already wrapped every pending
  op with local `unsafe { }` blocks carrying `// SAFETY:` comments -
  and the pin was proven live by a deliberately-bare deref canary
  that the compiler rejected (E0133), then removed.
- [x] Layout pins (user request): every `repr(C)` record the FFI passes
  to Windows, X11, or the compositor now carries a compile-time
  `pin_layout!` block pinning its byte size and per-field offsets, so
  an ABI drift breaks the build at the exact offset instead of
  corrupting a distant call. New `src/layout.rs` hosts the crate-wide
  macro; 25 records pinned across `winapi` (`POINT`/`RECT`/`MSG`/
  `WNDCLASSEXW`), GDI capture headers (40-byte `BITMAPINFOHEADER`),
  tray `NOTIFYICONDATAW` (968 bytes - two handle-align pads, verified
  against the compiler's own accounting before the pin was set), the
  settings dialog's `LOGFONTW` (92)/`BROWSEINFOW` (64), the picker's
  two low-level hook payloads, all eleven DXGI records, the COM
  `UnknownVt` prefix, `CURSORINFO`, X11's 136-byte `XImage` (LP64), and
  Quartz's 32-byte `CGRect`. The `interfaces!` macro additionally
  self-checks each generated v-table (IUnknown prefix at slot 0, size
  = 24 + 8 bytes per declared slot). Assertions are plain `const _`,
  evaluated in every build - release included - and on every target.

- [x] User design: the hotkey opens a crosshair session that answers
  both ways - the real window under the cursor highlighted and
  clickable, AND a dragable selection box - for record and photo alike
  (they share `select_region`, so both got it free).
- [x] `window_picker::pick` now answers `Pick::Window(handle) |
  Pick::Region(rect)`: a left press starts as a window pick; if the
  cursor moves beyond an 8 px Chebyshev threshold of the press point,
  the window highlight is released and the press becomes a drag - the
  XOR marquee (`draw_marquee`/`undraw_marquee`, same DrawFocusRect
  trick) follows the cursor and the release answers the normalized
  rectangle (1 px minimum guaranteed). Releases inside the threshold
  keep the old click-to-pick answer; Escape and right click cancel
  either path symmetrically (highlight and marquee both erased).
- [x] `select_region` merges the region/window arms: both `[capture]
  mode` values open the unified session; Full mode keeps the display
  pick. `draw_region` (the standalone overlay) stays only for the CLI
  `--mode region` demo path. `apply_settings`/menu wiring unchanged.
- [x] Track state reworked (`origin`, `dragging`, `marquee`; `picked`
  typed as `Pick`); the file was rewritten cleanly after phased edits
  left duplicated blocks (caught by gates, not shipped).
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus both
  cross targets, 69 tests (new: threshold separation, drag geometry
  normalization + 1 px guarantee, track defaults, variant exhaustive;
  settings-dialog centered-origin, client-area fit).
- [x] Settings-window polish pass (user request): the dialog is now
  centered on the primary display via a tested pure `centered_origin`
  plus `AdjustWindowRectEx` frame math (no fixed guessing), fixed-size
  (`WS_EX_DLGMODALFRAME` modal border, no resize/maximize), the exe's
  baked-in icon in the title bar, a dialog-wide `Segoe UI` 9 pt font
  (`CreateFontIndirectW` + `WM_SETFONT` on every child, explicit GDI
  lifetime), grouped `BS_GROUPBOX` sections (Capture / Recording /
  Still photo / Hotkeys / Folders) over aligned label/control columns,
  sunken `WS_EX_CLIENTEDGE` text fields, and a bottom-right Save
  (default) / Cancel row. Module split kept every file under 500 lines
  (mod 423, rows 448, frame 150, fields 89, ffi 197); the dialog's own
  visual look awaits a real-desktop confirmation like the rest.
  Sources under 500 lines - app.rs had grown past the cap this session,
  so the settings-application block moved to `src/app/settings_apply.rs`
  (app.rs 464, settings_apply.rs 82, window_picker.rs 488).
- Deferred: real-mouse confirmation of click-vs-drag behavior (the
  picker precedent applies - scripted-input verification only);
  double-click-to-drag as an explicit alternative start (the press-
  and-drag threshold covers the request; revisit only if the user
  misses it).

## Session 18 - Full-source audit + ranked fixes (done)

- [x] Phase 1: read every module plus README/ROADMAP and wrote
  `AUDIT.md` (findings D1-D14, robustness matrix, cleanup C1-C7, platform
  asymmetry, measurement discipline), each with file:line evidence and
  severity/effort/blast radius. Baseline gates before edits: fmt, native
  pedantic clippy, 73 tests.
- [x] Engine lifecycle (D1): `pipe::Feed` now owns the child; `Drop`
  closes stdin, then kills and reaps any engine not explicitly finished.
  `Recorder::tick` failures go through `finalize_recording`, which stops
  the timer, waits for ffmpeg, and clears `runtime.engine` instead of
  dropping the process. The headless loop finalizes on a failed tick too.
  `Recorder::start` creates its own output directory, so `--record` no
  longer depends on the tray having made it.
- [x] DXGI (D2): a failed `ID3D11Texture2D` cast now calls `ReleaseFrame`
  before returning, so one recoverable error no longer poisons the next
  `AcquireNextFrame`. Staging scratch allocation uses checked arithmetic.
- [x] Modal correctness (D3/D4): opening Settings stops an active
  recording first, matching the picker rule; the dialog pump calls
  `IsDialogMessageW` and the Save/Cancel buttons moved to standard
  `IDOK`/`IDCANCEL` ids, so the Win32 dialog contract applies to Tab,
  Enter, and Escape; `VK_ESCAPE` has an explicit fallback arm. Live
  keyboard confirmation still needs hands (no GUI automation here).
- [x] CLI/capture edges (D5-D8): `--photo` resolves Full/Region/Window
  like `--record` (and keeps `--rect` as the override), `--record`
  enforces 1-3600, `display_at(0)` returns an error instead of wrapping,
  and the Windows single-instance check clears last-error before
  `CreateMutexW`.
- [x] Single instance + diagnostics (D9-D13): non-Windows capture FAIL now fails `--check`;
  `engine.sha256` comparison is byte-wise (a multibyte record can no
  longer panic); invalid `autostart`/`cursor` values report their
  fallbacks; identical record/photo hotkeys from a hand-edited file are
  reported and reset to the defaults.
- [x] Measurement (D14): `Recorder` counts accepted frames and start time;
  saved-file logs print captured frames, wall seconds, and measured fps
  beside the ffprobe encoded count; `--check` times six blit frames and
  prints frames/seconds/fps/ms-per-frame. Duplication is deliberately not
  timed there: its idle path reuses the last frame without touching the
  GPU, which would report a fictional rate; real recordings now report
  their own measured fps. Timer intervals round to the nearest ms (60 fps
  -> 17 ms), and the demo loop resynchronizes instead of bursting after a
  missed deadline.
- [x] Cleanup (C1-C5): `settings.rs` split into
  `settings/{mod,hotkey,tests}.rs` (388/183/142 lines, all files now under
  500); `Rect` moved onto `winapi` (the dialog re-exports it), `Guid`
  moved onto `winapi` (DXGI and the tray share it), the GDI screen DC now
  lives behind `guard::ScreenDc`, the duplicate `photo_in` was removed,
  the picker's unused `bounds` field went away, and README's layout names
  the current directories.
- [x] Gates: `cargo fmt` 0, native `cargo clippy --all-targets -- -D
  warnings` 0, `aarch64-apple-darwin` clippy 0, `x86_64-unknown-linux-gnu`
  clippy 0, `cargo test` 0 (78 passed). Live smoke: `--check` exit 0 with
  duplication present, engine integrity ok, and a measured GDI timing
  line; `--record 1 --rect` and `--photo --rect` exercised the fixed
  finalized path, and `--photo --mode window` produced a 1920x48 still
  (ffprobe-verified) against the 1920x1200 full display, proving the CLI
  mode override; `--photo --mode bogus` exited 1.
- Deferred: true timestamped/VFR mux (the ffmpeg input is still constant
  fps, so timer drift changes duration), a duplication throughput
  benchmark that forces real updates, mac/Linux live runs, switching a
  failed duplication session to GDI after a resolution change, and the
  remaining doc-only duplications noted in `AUDIT.md`. The Windows race
  where Settings-open queues timer events is now prevented by stopping
  the recording, not by a deeper scheduler redesign.

## Session 19 - Recording folder, menu trim, Stop button (done, live-verified)

- [x] `paths::video_dir` answers `Videos/Screencapture` (macOS
  `Movies/Screencapture`), so recordings get their own folder like
  stills already did. `settings::video_directory` treats the exact old
  default (`Videos`/`Movies`) as unset and adopts the subfolder, while
  any folder chosen by hand is kept; the legacy `[media] directory` key
  goes through the same rule. `--check` now reports
  `C:\Users\<user>\Videos\Screencapture` for the pre-existing config.
- [x] Shared-folder edge case: when `[media] videos` and
  `[media] images` name the same folder, `settings::split_shared_media`
  routes recordings to `<folder>/video` and stills to `<folder>/image`
  (case- and trailing-separator-insensitive comparison). Different
  folders keep their chosen paths untouched. The same helper runs in the
  settings dialog's save path, so a freshly typed shared folder persists
  as the two effective subfolders, and re-saving split paths is
  idempotent. Verified: `--check` on a shared config printed
  `.../sharedmedia/video` and `.../sharedmedia/image`; `--record 1` and
  `--photo --rect` wrote into those subfolders; the dialog save of the
  same path in both fields persisted the split; 95 tests.
- [x] The tray menu no longer carries an Autostart entry; the Settings
  window's checkbox (and the startup sync) remain the single way to
  manage it. `Event::ToggleAutostart` and its handler are gone.
- [x] The recording border owns a **Stop** chip centered on its top
  dashed edge (`src/overlay.rs`, `screencapture.recording.stop`): an
  88x28 non-activating popup, red fill with a white rim and `Stop`
  caption, `WDA_EXCLUDEFROMCAPTURE` so it never lands in the clip. Click
  posts `WM_COMMAND`/`ID_STOP` to the tray window (`tray::stop_requested`)
  which emits `Event::Stop` -> `stop_recording`, so the border is not
  destroyed inside its own window procedure. Geometry clamps the chip
  inside the screen when the top edge is at y=0, and the picker's hover
  border passes no stop action.
- [x] Verified live through the tray: `--check` shows the new videos
  folder; a picker-started recording created
  `Videos\Screencapture\screencapture-*.mp4`; the Stop window's rect sat
  centered on the border's top line with `GetWindowDisplayAffinity`
  `0x11`; clicking it saved the clip (62 frames, 2.31 s) and removed
  both border windows; the extracted frame is clean at the chip's mapped
  pixels. The chip's own paint was photographed with affinity paused
  (`#FF0000` fill, `#FFFFFF` rim/text), since an excluded window cannot
  appear in a screenshot. Gates: fmt, pedantic clippy, 93 tests.

## Session 20 - Selection outline styling (done, live-verified)

- [x] The red dashed frame is now recording-only. `overlay::Border::show`
  takes an `overlay::Style`: `Selection` registers the new
  `screencapture.selection.frame` class and paints a two-tone dotted
  line (white 2 px dot, 1 px gap, black 2 px dot, 1 px gap, per 6 px
  step, with the dot loop covering every edge), while
  `Recording(action)` keeps the old class, red dashes, Stop chip, and
  `WDA_EXCLUDEFROMCAPTURE`. The picker's hover highlight and drag marquee
  both use `Selection`; the recording path is unchanged. Both styles ask
  `WDA_EXCLUDEFROMCAPTURE`, so the outline exists on screen only and
  never lands in a saved still or recording (a picked screenshot had
  baked the dots in while the compositor still held the just-destroyed
  frame).
- [x] Stale XOR claims in the picker docs replaced with the real
  structure (click-through topmost windows + hooks).
- [x] Verified live through the tray: a picker pass created only
  `screencapture.selection.frame` (1440x900) and the crosshair, with no
  recording window; a captured row of the top edge read the dot pattern
  `FFFFFF FFFFFF` / gap / `000000 000000` / gap repeating; pressing
  started `screencapture.recording.frame` plus the Stop chip while the
  selection frame disappeared; Escape removed both styles cleanly; the
  Stop chip still ended and saved the clip. A picker still taken through
  the tray saved a clean frame with no dots, confirming the exclusion
  holds through teardown. Gates: fmt, pedantic clippy, 96 tests.

## Session 21 - Screen-only selection and frame-tight window picks (done, live-verified)

- [x] The picker's selection frame now asks `WDA_EXCLUDEFROMCAPTURE` like
  the recording frame: the dots are screen-only and cannot land in a
  saved still, even when the compositor still shows a just-destroyed
  frame while the pick tears down.
- [x] Window picks trim the Windows 11 visible frame border
  (`DWMWA_VISIBLE_FRAME_BORDER_THICKNESS`, normally 1 px). DWM paints it
  translucently, so keeping it showed what was behind the window along
  its edges (white text at the top, an image slice at the bottom on a
  page-backed desktop). `screen::win::window_bounds` now owns the DWM
  bounds plus that inset; the capture region and the picker's highlight
  share it, so the dotted box frames exactly the saved pixels.
- [x] The picker's duplicate DWM bounds helper is gone (one geometry
  source; `window_picker` calls `screen::win::window_bounds`), and the
  inset math is unit-tested (zero border keeps the frame, 1 px trims
  every side, an oversized border never erases it).
- [x] Verified live on the rebuilt tray: the app window (DWM
  `240,126 1440x900`) captures `1438x898` with zero bright pixels on the
  four edge rows/columns (previously 51 bright top-edge pixels and 283
  bottom-edge pixels from the page behind); the picker outline reads
  `241,127 1438x898`; a maximized `1920x1152` window captures
  `1918x1150`. Gates: fmt, pedantic clippy, 99 tests.

## Session 22 - Recordings keep the capture's aspect ratio (done, live-verified)

- [x] A profile is now a size ceiling, not a new shape:
  `Profile::fit` returns the largest frame with the capture's own aspect
  ratio that fits inside the profile (`1152x720` for a 16:10 window in a
  720p profile), using integer cross-multiplication so no ratio rounding
  can pick the wrong cap; the derived edge is rounded to even pixels for
  H.264 4:2:0 and never below 2.
- [x] `record_args` scales straight to that frame (`scale=1152:720`) with
  no `pad` stage, so nothing is letterboxed or pillarboxed; a capture
  already at the profile's size and shape skips the filter entirely.
- [x] The start log and `size_note` report the real output size
  (`684x891 -> 552x720`) from the same `fit`, so the log cannot claim a
  size the file does not have.
- [x] Tests: `fitting_keeps_the_capture_aspect_ratio` covers 16:10 into
  16:9, matching shapes, wider-than-profile, small captures, odd sizes
  and the zero fallback; `fitting_never_exceeds_the_profile` sweeps every
  profile against portrait, ultrawide and odd captures and checks bounds,
  even edges and a shape error within one edge pixel; `scale_for` and the
  log test pin `1152x720`.
- [x] Verified live on the rebuilt tray (720p60 profile): a `684x891`
  portrait WinForms window recorded for ~3.5 s, logged
  `684x891 -> 552x720`, and ffprobe reports `552x720` at 60/1 with 112
  frames — no black bars. Gates: fmt, pedantic clippy, 102 tests.

## Session 23 - Recordings play at real speed, and starting one is instant (done, live-verified)

- [x] Root cause of the fast playback: `WM_TIMER` at 17 ms really fired
  every ~31 ms (two 15.6 ms ticks), and the encoder was told the stream was
  the profile's 60 fps, so 5 s of screen became ~2.5 s of video. The log
  had been reporting it all along (`31.3 fps measured`).
- [x] Capture and piping moved to a worker thread (`recorder::capture_loop`)
  that owns the frame source, the engine pipe, and the fast multimedia
  timer; the tray only starts and stops it. A 9 MB blit or a full pipe can
  no longer freeze the pump: the tray menu opens in ~14 ms while a
  recording runs.
- [x] Wall-clock pacing: `frames_due` converts elapsed time into the number
  of frames the file should already hold (integer math), and the worker
  repeats the current grab to fill any slots a slow blit missed, capped at
  half a second per step. A capture that cannot reach the profile rate now
  yields a choppy but correct-length clip instead of a fast one.
- [x] The outline is created and painted (`InvalidateRect` +
  `UpdateWindow`) before the engine starts, so the red frame appears in
  ~10 ms after the pick instead of after setup; it is dropped again if the
  engine refuses to start.
- [x] `supports_encoder_cached` remembers each accelerator probe per engine
  path and codec id, and a configured hardware codec is warmed on a
  background thread at startup; the synthetic test encode no longer sits
  between the user and the first recording.
- [x] The frame timer is gone: the pump keeps a 500 ms watchdog
  (`Event::Watchdog`) that finalizes the recording if the worker dies, and
  `Recorder::failure` reports why.
- [x] Tests: `frame_slots_follow_the_wall_clock` pins the 60/30 fps slot
  math and the degenerate fps, `catch_up_stays_inside_half_a_second` pins
  the burst bound; the size and aspect tests are unchanged.
- [x] Verified live (720p60, x264): a 6 s window recording wrote 358
  frames from 179 grabs — `59.9 fps output, 30.0 fps captured` — and
  ffprobe reports `1152x720`, `60/1`, 360 frames, **6.000 s** (previously
  ~3 s). A 3 s full-display run wrote 181 frames in 3.01 s. Picker stills
  contain no dots, no red dashes appear in any captured frame, and the
  `--record`/`--photo` CLI paths still work.

## Session 24 - GitHub-ready README and a green non-Windows build (done)

- [x] README rebuilt as a project page: centered icon/title/tagline, static
  shields badges (platform, edition, zero dependencies, FFmpeg 9, tests,
  CLI on macOS/Linux), a screenshot table (Settings window + tray menu from
  `assets/`), "Why this one" features, install for Windows and macOS/Linux,
  a platform support matrix, a short "How capture works", a development
  section naming the exact gate commands, and a license note. The session
  log moved into a collapsed `<details>` block instead of leading the file.
- [x] Assets added: `assets/settings-window.png` (490x653 — DWM extended
  bounds minus `DWMWA_VISIBLE_FRAME_BORDER_THICKNESS`, so the invisible
  resize margin and the desktop behind it are gone) and
  `assets/tray-menu.png`.
- [x] Cross-target build was red before this session; now clean on
  `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin` and
  `aarch64-apple-darwin`:
  - `Event::Tick` still referenced by the non-Windows event catalog after the
    watchdog rename;
  - `std::path::PathBuf` imported unconditionally in `settings_apply`;
  - dead-code on non-Windows for `Hotkey::from_vk`, `tray::hotkey_available`
    and `tray::stop_requested`, now expressed as
    `cfg_attr(not(windows), expect(dead_code, reason = ...))` (and left
    un-expectated in test builds, where the tests do use `from_vk`).
- [x] Non-Windows first run behaves: `DEFAULT_CAPTURE_MODE` is `full` there
  (the picker is a Windows surface), a settings file that still says
  `region` is demoted to full with a logged problem, the built-in template
  writes the platform default, and a bare `screencapture` prints the command
  list and exits successfully instead of claiming the instance guard,
  installing an autostart entry, and failing with "tray loop ended".
- [x] License: the project is **Apache-2.0** (`LICENSE`, verbatim upstream
  text; `Cargo.toml` carries `license = "Apache-2.0"`; README badge and
  section; the installer payload and uninstall list include `LICENSE`). The
  bundled FFmpeg stays a separate GPLv3+ work — `docs/FFMPEG-LICENSE.md`
  states that split explicitly.
- [x] Gates: fmt, pedantic clippy on all five targets, 103 tests.

## Session 25 - Escape always cancels, stills land on the clipboard (done)

- [x] Escape stopped being a key the selectors could miss. The drawn-rect
  overlay handled `WM_KEYDOWN` only, so it needed the foreground — and
  Windows refuses `SetForegroundWindow` for a process that is not the
  foreground one (starting a capture from a terminal, an IDE, or a menu is
  enough), which is exactly the "sometimes Escape does nothing" report. A
  measured run confirmed it: the overlay was up with the terminal still the
  foreground window, so its own key never arrived.
- [x] New `guard::EscapeWatch`: a `WH_KEYBOARD_LL` hook that turns Escape
  down-events into one callback, swallows both halves of the press, and
  uninstalls on drop (RAII, like the DC and mouse-capture guards). Both
  selectors use it — the tray's `window_picker` lost its private keyboard
  hook, struct, and constants, and the overlay installs the watch around its
  pump — so there is one Escape implementation instead of two. The overlay's
  `WM_KEYDOWN` arm stays as the fallback for a refused hook.
- [x] `src/clipboard.rs`: every still photo is also put on the Windows
  clipboard as `CF_DIB` (bottom-up 32-bit `BITMAPINFOHEADER`, alpha forced
  opaque — both capture routes leave it zero and a transparent DIB pastes as
  a blank image) plus a registered `PNG` format holding the file FFmpeg just
  wrote, so office suites and browsers both paste. The clipboard is taken
  with a short retry window (another app may hold it), emptied, and released
  by a guard; a refused clipboard is a logged note, never a failed capture.
- [x] `take_photo` now answers a `Photo` (path + clipboard outcome), so both
  the tray log and `--photo` say `(... copied to clipboard)` or why not, and
  macOS/Linux photos stay file-only without a misleading claim.
- [x] Live-verified on Windows: a synthetic Escape cancels the overlay with
  the terminal still foreground (before the fix the same press left it
  running); Escape closes the tray picker (its two windows gone, process
  alive); a picked-window photo (1438x898) and a dragged-region photo
  (500x400) both appear on the clipboard and match their PNGs pixel for
  pixel, alpha 255.
- [x] Gates: fmt, pedantic clippy on all five targets, 108 tests (DIB
  layout/row-flip/short-frame, Escape payload, Escape-vs-drag).

## Session 26 - Capture chimes and a tray mute (done, live-verified)

- [x] Every committed still and every stopped recording answers with a short
  synthesized three-note chime (`src/sound.rs`: a std-only PCM oscillator and
  the Win32 `PlaySound` surface), so the capture is audible without watching
  the tray. The waveform is generated in memory, mixed without clipping, and
  silenced before and after the notes.
- [x] `[capture] chimes = false` (Settings: **Play capture chimes**) mutes the
  sound. The tray menu gained a checkable **Mute capture chimes** entry directly
  under **Photo**; clicking it flips and saves the setting immediately, and the
  check mark follows the saved state on every menu open and Settings save.
- [x] Startup seeds the tray's copy from the loaded settings, and the Settings
  save path refreshes it in place, so the menu, the checkbox, and the file never
  drift apart.
- [x] Gates: fmt, pedantic clippy, 129 tests (mute persistence both ways, menu
  check-flag logic, waveform shape/frequency/clipping).

## Session 27 - A configurable chime hotkey (done, live-verified)

- [x] The chime toggle is a third global hotkey beside record and photo,
  defaulting to `ctrl+shift+m`. It is stored as `[hotkeys] chimes`, edited as
  **Chime hotkey** in Settings, and re-registered on Save without a restart.
- [x] Settings validates all three combos as distinct and probes each with a
  live `RegisterHotKey` availability check; the tray dispatches binding id 3
  through the same toggle path as the menu, so the setting persists, playback
  stops immediately, and the menu check mark and Settings checkbox stay in
  step.
- [x] `--check` reports all three combos and `--init` writes the new key into the
  generated template.
- [x] Gates: fmt, pedantic clippy on all five targets, 133 tests (three-way
  hotkey validation, binding-to-toggle dispatch, menu-entry dispatch).

## Session 28 - Cross-platform CI (done)

- [x] `.github/workflows/ci.yml` runs the native gates on `windows-latest`,
  `macos-latest`, and `ubuntu-latest` for every push and pull request: a stable
  toolchain with clippy and rustfmt, `cargo fmt --all --check`, pedantic
  `cargo clippy --all-targets --locked -- -D warnings`, and
  `cargo test --all-targets --locked`.
- [x] The workflow is read-only (`contents: read`), cancels superseded runs per
  ref, caches cargo artifacts, and can be dispatched by hand.
- [x] README development gates now match the workflow (`--locked`, `--all`),
  link to the workflow, and no longer hand-maintain a test count.

## Session 29 - Reproducible engine fetch and the v0.1.0 release

- [x] `scripts/fetch-ffmpeg.ps1`: Windows PowerShell 5.1 plus .NET only (no
  curl, tar, or 7-Zip), pinned to the BtbN GPL archive by URL, byte count, and
  SHA-256. It retries a failed transfer, downloads to a temporary `.part`,
  verifies before extracting, stages only `ffmpeg.exe`, `ffprobe.exe`, and the
  license, and invalidates the old integrity record for the next `--check`.
  `scripts/fetch-ffmpeg.cmd` is the double-click/`cmd.exe` shim.
- [x] CI parses the PowerShell script on the Windows runner, and both the
  `.ps1` and `.cmd` paths were exercised against a clean destination.
- [x] Release metadata: the exe carries version/company/description resources,
  the installer carries matching version keys, and `FFMPEG-LICENSE.md` now
  installs under `docs\` where the README and uninstaller expect it.
- [x] Codec fallback is H.264 first: the requested encoder, then software
  x264, then each hardware H.264 unit, and only then CPU x265. An engine with
  no usable encoder refuses the recording with a clear error before capture;
  `--check` prints the fallback that will be used. Unit-tested and live-smoked
  with an unavailable NVENC choice (warn + h264 MP4).
- [x] Release verification: `--check` clean from the released payload,
  `--record` produced a 1280x720 H.264 MP4 (31 frames), `--photo` wrote and
  clipboard-copied a PNG, fmt/pedantic clippy passed native and four cross
  targets, and all 134 tests passed.
- [x] Tag `v0.1.0` and publish the unsigned `screencapture-setup.exe` plus its
  SHA-256 checksum.

## Session 30 - Icon coverage, safety audit, and v0.1.1

- [x] Icon coverage completed and guarded: the Windows ICO carries
  16/24/32/48/64/128/256, the macOS ICNS carries every `icp4`-`ic14` slot
  (1x and @2x), and Linux hicolor PNGs cover 16-512 including the new 24px
  entry. `tests/icons.rs` parses every shipped asset and fails a release that
  drops a scale.
- [x] The Windows setup and uninstaller now use `assets/icon.ico`; the
  uninstall registry values are quoted so paths with spaces stay intact.
- [x] Safety/security audit fixes:
  - CoreGraphics, CoreFoundation, and libX11 surfaces are cached in
    `OnceLock`, so repeated window-list/capture setup no longer leaks a
    `dlopen` reference per call.
  - DXGI and D3D11 load only from System32 through `LoadLibraryExW` and cache
    the module handle, closing a working-directory/PATH DLL-preload path.
  - Windows FFmpeg discovery no longer falls back to a bare `ffmpeg.exe`
    resolved through `PATH`; the shipped and explicit paths remain.
  - Capture names are reserved atomically with `create_new`, removing the
    same-second overwrite race between processes; failed starts remove the
    placeholder instead of leaving an empty file.
- [x] Gates: fmt, pedantic clippy native and six cross targets, 135 unit tests
  plus 3 icon integration tests. Release build, installer, and CLI smoke.
- [x] Tag `v0.1.1` and publish the rebuilt setup plus its SHA-256 checksum.


## Session 31 - Release pipeline

- [x] `.github/workflows/release.yml` runs on `v*` tags, requires the tag to
  be on `main` and to match `Cargo.toml`, runs the full gates, fetches the
  pinned FFmpeg engine, builds the NSIS installer, and publishes the exe,
  the setup, and `SHA256SUMS.txt`.
- [x] The exe's icon and version resource are compiled on the runner through
  MinGW `windres`, which `build.rs` takes from the `WINDRES` variable.
- [x] `CompanyName` and the product name were added to the NSIS version
  resource so the installer carries the same metadata as the application.
- [x] Bumped the tree to `0.1.2` in `Cargo.toml`, `Cargo.lock`, the exe
  version resource, and the NSIS script.
- [x] `tests/version.rs` fails when the package version and the hand-written
  exe-resource and NSIS versions disagree, so the normal test gate guards
  the bump instead of the tag run.
- [x] CI compiles the installer script and checks the built setup's product
  version on every push, so a broken or stale script fails before a
  release tag.
- [x] `scripts/fetch-ffmpeg.ps1` treats only the engine executables as an
  installed engine, so a fresh checkout can fetch it.
- [x] Code signing was scoped, then dropped: the binaries ship unsigned and
  the release notes point at the public source and the published checksums.

## Session 32 - Recording audio, and a two-column Settings sheet (done)

- [x] `src/audio/` records sound through hand-written WASAPI FFI: the default
  playback device through loopback (`RENDER`) and the default microphone
  through capture (`CAPTURE`), both opened as `IAudioClient` shared-mode
  streams and read with an event handle. No crate dependency was added.
- [x] The two sources are mixed on the recording clock rather than on arrival:
  a worker drains both into queues and writes 10 ms chunks of 48 kHz stereo
  `s16`, filling a gap with silence and trimming a queue that runs fast, so
  audio stays aligned with the paced video.
- [x] The mix reaches the engine through a named pipe it reads as a second
  input (`-f s16le -ar 48000 -ac 2`), mapped as AAC in the same MP4. A source
  that refuses is a logged note, never a failed take: the recording simply
  carries the sources that opened.
- [x] `[recording] mic = false` and `system_audio = true` are the switches,
  parsed independently so a bad value only costs that one source, seeded into
  the template with a comment, and validated in `settings::tests`.
- [x] The Settings window was rebuilt as a two-column sheet — Capture,
  Recording and Still photo on the left; Audio and Hotkeys on the right; full
  width Application and Folders bands beneath — at 728x514 designed pixels.
  The shorter, wider shape fits a 1366x768 laptop at its native scale, where
  the old 490x676 sheet had to shrink. `assets/settings-window.png` was
  recaptured from the new window.
- [x] The Audio group carries both recording switches and a one-line note;
  the microphone and system switches persist through Save like every other
  control (verified by driving the dialog and reading the written file).
- [x] Verified live: a 2 s full-display recording is H.264 3456x2160 plus AAC
  48 kHz stereo, 1.95 s of audio under 2.02 s of video; a desktop notification
  during a take measured `max_volume: -19.7 dB` against `-44.4 dB` mean.
- [x] Cross-target gates stay green: the non-Windows `audio` stubs answer no
  prepared sources and an inert handle, with narrowly scoped `expect`s where
  the stub must keep the Windows signature; clippy is clean on the five
  supported targets.

## Session 33 - A silent failure made visible (done)

- [x] The installed 0.1.9 build stopped saving stills and recordings for a
  stretch: every still failed with `cannot start the encoder: The request is
  not supported. (os error 50)` and every recording failed before its first
  frame, so the red border and its Stop chip appeared and were gone in the
  same breath. The log also showed photos never reached a folder at all, so
  the shell known-folder lookup was not the cause; the engine never started.
- [x] The recording frame is now shown *after* `Recorder::start` answers Ok,
  not before it. A capture that refuses to start no longer flashes a border
  for a recording that does not exist.
- [x] A failed start now says so in the tray as a warning balloon
  (`tray::warn`), with the same text the log gets. The log alone is
  invisible to a tray-only user, which is how the flash read as "broken".
- [x] A refused spawn names the engine (`cannot start <path>: <os error>`);
  the bare OS text sent the last hunt through the installer, the settings
  file, and the capture backend before anyone looked at the path.
- [x] The engine is spawned with all three streams named — stderr included —
  so nothing is inherited from a tray that called `FreeConsole`. An
  inherited standard handle is the documented way `CreateProcess` fails
  with `ERROR_NOT_SUPPORTED` after a console is detached.
- [x] Audio review, with two real fixes: the audio worker joins the
  multithreaded apartment before polling clients activated on the setup
  thread, and a packet the engine flags silent (or reports without lending
  a buffer) keeps its duration instead of being dropped, which would have
  pulled every later sample earlier against the video.
- [x] A/V alignment: the audio worker counts from the same `Instant` the
  frames are paced by, so its connect time becomes silence at the head of
  the track. A 3 s recording went from 2.92 s of audio against 3.02 s of
  video to 3.01 s against 3.02 s; the beeps still measure -6.9 dB peak.
- [x] Tests: `a_refused_spawn_names_the_engine` pins the diagnostic, and
  `append_packet`'s three cases pin packets that carry no samples, carry
  the silent flag, or decode little-endian stereo. The capture loop now
  takes one `Session` value instead of eight arguments.

## Session 34 - Playback that keeps up, and an audio quality choice (done)

- [x] The video profile is a ceiling, not a target: a capture inside the
  profile box keeps its own pixels instead of being upscaled into it. The
  1920x1200 screen had recorded the 4K profile as 3456x2160 (and a full-screen
  video as 3840x2116), which is four times the pixels to encode and decode
  with no detail to show for it; that is why 4K takes stuttered on playback
  while 1080p takes did not. `Profile::fit` now caps and never enlarges.
- [x] Measured on a fresh 6 s take of the same screen while the pointer moved:
  1920x1200 @ 60 fps, 5.4 Mb/s, 60 distinct frames in every moving second
  (against 52 Mb/s and a 3840x2116 frame for the same content before).
- [x] `[recording] audio_quality` chooses the AAC track's bitrate — `speech`
  (96 kbps), `standard` (128 kbps), or `high` (192 kbps, the default, so an
  existing file keeps what it had). Settings exposes the same three choices
  in the Audio group, and Save persists the id.
- [x] Verified live: a 3 s take with `audio_quality = speech` writes a 99,631
  b/s AAC track (48 kHz stereo) under 3.017 s of video from 3.010 s of audio,
  and driving the dialog's combo to Speech writes `audio_quality = speech`.
- [x] `hd-1080p60` is the shipped default profile, and the template writes
  `recording::DEFAULT.id` instead of a hand-copied id, so the default cannot
  drift from the profile table again. `assets/settings-window.png` was
  recaptured from the new sheet with the default settings.
- [x] `--init` writes the commented template itself rather than a rendered
  document, which had been stripping every comment (including the ones that
  explain the audio keys). The write goes through a temporary file and a
  rename, and still refuses to touch an existing file. The start-of-take log
  line now names the audio shape (`system+mic audio at high`) and
  `--check` prints `audio: system=… mic=… quality=…`.

## Session 35 - A control bar worth clicking, and sound that can join (done)

- [x] The recording chrome is one control bar instead of three floating
  chips: a 96x36 pill straddling the frame's top edge with stop, microphone
  and system-sound buttons, a hairline rim, a hover wash, and no red block.
  Icons are line art — a capsule in a cradle, a cone with two waves, one soft
  red square — drawn four times oversized and box-filtered down, so the
  rounded ends and the strokes are antialiased without a drawing dependency.
  The bar is one window, so there are no gaps between buttons for a click to
  fall through.
- [x] Buttons act on mouse-down, and the press captures the pointer: a switch
  changes under the pointer rather than after it lifts, and a hand that
  drifts a pixel still gets its button. The press and the new state are
  painted in the same frame; the log line is written after the paint, off
  the click's path.
- [x] Both audio buttons are always on the bar, and either can bring its
  source into a take that is already running: `Live` grew a `wanted` flag,
  the worker opens whatever is awaiting on each 5 ms pass, and the mixer
  picks it up within one 10 ms chunk. A source that refuses mid-take stops
  being asked for (no retry loop) and its button reads as off again.
- [x] The track now always exists, because a button can only bring sound
  into a pipe the engine already reads. A take whose settings asked for
  nothing writes a silent AAC track; verified live on a take with `mic =
  false` and `system_audio = false` — both buttons pressed mid-take produced
  an AAC 48 kHz stereo stream in the finished file (`microphone unmuted`,
  `system audio unmuted` in the log, no device errors).
- [x] The red dashed frame is gone. In its place: a two-tone hairline (dark
  outside, light inside, so it reads on pale and dark content) and a coral
  viewfinder bracket at each corner. The bracket stands 4 px clear of the
  hairline so it frames the picture instead of thickening the border, is
  sized from the frame's shorter side (`/12`, held between 20 and 64 px, so
  a 4K take and a small window both read), and carries a one-pixel shade
  edge on its inner side so the accent keeps its contrast on white.
- [x] The frame breathes over 3 s between 178 and 216 of 255 on an eased
  (smoothstep) ramp, so it dwells at each end and never fades into a stripe
  or disappears. The breath is a layered-window attribute rather than a
  repaint, so announcing a live take costs the capture no frames at all.
- [x] Verified live on a 1920x1200 full-screen take from a build that let the
  desktop see the frame: the hairline sits at the edge, the coral leg runs 64
  px along the top and sides with its shade edge inside, and the accent's
  measured alpha moved 0.72 -> 0.83 -> 0.79 across one cycle.
- [x] `assets/recording-frame.png` is a real take, not a mock-up: the README
  rendering in an editor window, recorded through the picker, photographed
  mid-take with the microphone switched on. Captured from a build with the
  display affinity off, because the frame and the bar are excluded from
  capture by design and a normal screenshot cannot show them.
- [x] Tests: bar layout (row fits, narrow regions keep stop alone, hit
  testing finds each button and not the gaps), live-switch semantics (a
  button pulls a source in, a refusal stops the retry), frame geometry
  (inside the client, every edge, an inset bracket at every corner, brackets
  scaling and clamped), and the breath's band plus its eased turns.

## Session 36 - Chosen devices, faders, and a quieter mix (done)

- [x] Settings can name the device each source records from. Every
  enumeration lists every endpoint the machine remembers
  (`DEVICE_STATEMASK_ALL`), not just the connected ones, so a Bluetooth
  headset can be picked while it is switched off; the endpoint *id* is what
  is stored (`mic_device`/`system_device` plus the `_name` to show), because
  an id survives a rename and a re-pairing. A device that is not attached any
  more stays in the picker as "(not connected)".
- [x] The worker opens the chosen id, and when that device is away it records
  the Windows default for that flow, says so in the log, and asks again every
  two seconds whether the chosen endpoint has come back — then replaces the
  stream and logs the move. A device unplugged mid-take is reopened the same
  way instead of ending that source for the rest of the recording.
- [x] The default endpoint is now asked for the *multimedia* role first, with
  the console role as a fallback: applications play through multimedia, and
  capturing the console endpoint gave a track of digital silence on machines
  that name a different device for it.
- [x] Two faders, `mic_level` and `system_level`, in dB from -60 (silence) to
  +12. Each source is scaled before the sum, so a quiet microphone and a loud
  game can share one track without touching either device volume; a level
  outside the range is clamped with a note rather than failing the file, and
  unity passes samples through bit for bit. A sum past 95% of full scale now
  bends hyperbolically into the last half decibel instead of clipping into a
  square wave, and a take with one source live is never touched at all.
- [x] Muted sources still give up their samples to the mixer, so unmuting
  resumes at the right moment instead of replaying the muted stretch.
- [x] `standard` is 160 kbps and `high` 320 kbps (from 128 and 192): at 320
  the encoder carries the whole audible band at 48 kHz, so a music mix comes
  back the way it went in.
- [x] The Settings sheet grew a device picker on each source's own row and a
  fader with its dB readout (`WM_HSCROLL` repaints the number), and Save
  persists the ids, names, and levels - clearing a device writes no key at
  all, which is what "follow the Windows default" means in the file.
- [x] Tests: endpoint labels for connected, disabled, and missing devices; a
  chosen id and fader round-trip through the file with clamping and notes; the
  limiter is monotonic, symmetric, and bounded below the rail; a muted channel
  goes quiet without slipping the clock.

## Session 37 - The Settings sheet says less and knows where you are (done)

- [x] The Capture group is gone: capture mode and display are chosen by the
  picker and the tray, so the two comboboxes (and the `Answers` fields and
  `document.set` calls behind them) were removed. Save no longer rewrites
  `capture.mode` or `capture.display`, which stay file-only keys.
- [x] A full capture with `display = primary` now records the display under
  the pointer instead of the one at the desktop origin (`screen::under_cursor`
  over per-monitor rectangles). A hotkey pressed on a second monitor records
  that monitor; the tray's Record display submenu and `--display N` still pin
  a specific one.
- [x] Hotkeys moved under Still photo, so the left column is recording, still,
  and hotkeys while the right column holds the audio sheet alone; the client
  height grew 541 to 572 to pay for the faders. The rows themselves had to
  move with the group - they were still drawing at the right column's x, which
  left the box empty.
- [x] `assets/settings-window.png` recaptured from the running sheet (frame
  bounds only, no desktop edge), and the tray table row now says what Settings
  actually covers.

## Session 38 - Chrome that scales, an audio readout, and checks that run (done)

- [x] The recording frame and its control bar follow the monitor instead of
  being stretched by Windows. The process is per-monitor DPI aware, which
  means the chrome has to scale itself: one `scaled(value, percent)` rounds
  every designed pixel, so the hairline, the bracket inset/leg/thickness, and
  the bar's button grid all grow together. The frame's shade edge is one
  hairline wide, and a narrow region still drops the audio buttons at any
  scale.
- [x] Scale is not the DPI setting alone. Windows' steps are coarse enough
  that the same pill is 24 mm wide on a 15.6-inch 1366x768 panel at 100% and
  15 mm on a 14-inch 1920x1200 panel at 100% — and the bar is aimed at. So
  `overlay::chrome_percent` takes the larger of `GetDpiForWindow` (clamped
  100–400%) and a physical floor: `CreateDCW` on the monitor's own device
  name, then `HORZSIZE`/`HORZRES` give real millimetres, and
  `percent_for(mm, px)` asks for the percent that makes the 96-pixel pill
  24 mm across. Integer arithmetic throughout; a driver whose millimetre
  answer implies under 36 or over 320 DPI (desktop-sized, or made up) is
  ignored and the DPI scale stands alone.
- [x] `assets/recording-frame.png` recaptured around the README in the Editor
  app with the new chrome: a 153x57 pill straddling the frame's top edge, the
  3 px brackets with 100 px legs, and the 2 px two-tone hairline, all at the
  panel's real size. Taken from a build that let the desktop see the frame
  (the capture exclusion off), through the picking session, as the previous
  shot was, and cropped to the same 1440x900 canvas.
- [x] `assets/settings-window.png` recaptured the same way: the sheet run from
  the real settings on a desktop with everything else minimised, cropped to
  the window plus 24 px for its shadow (782x649), so the pair no longer looks
  like shots of two different desktops.
- [x] Verified live on this box, with the probe made per-monitor aware first
  (a DPI-unaware caller is answered virtualized coordinates — at 150% that
  reported a 144x54 bar as 96x36, which is a probe trap, not a bug): the
  14-inch 1920x1200 panel reports 302 mm, so the pill is 153x57 centred at
  x=883 at both 100% and 150% Windows scale — 24.1 by 9.0 mm, the size the bar
  was drawn for — with the frame window still 1920x1200 device pixels and the
  recording 1920x1200 either way. A screenshot from a build with the capture
  exclusion off measured the frame on that panel: 2 px shade + 2 px light
  hairline, a 3 px bracket with 100 px legs, inset 6 px.
- [x] `--check` now reads the audio settings back: the fader positions as
  signed dB, and on Windows the endpoint each source will actually open —
  "System default (8 remembered)", a chosen device, or "(not connected; using
  the default)" for one that is away. A silent track is rarely a mute switch;
  it is usually the wrong endpoint, and this says which one before a take.
- [x] CI records instead of hoping: the Windows installer job now runs `--check`,
  takes a 2 s region recording, probes it for H.264 + AAC and a 1.5–4 s
  duration, and takes a still whose PNG signature is checked, all with the
  bundled engine in the runner's temp folder.
- [x] CI starts the program on macOS and Linux too. Both runners build the
  release profile (LTO and stripped symbols, which clippy never links) and run
  `--check`, requiring a normal exit status and the expected report sections
  rather than a panic. A runner has no desktop and no engine, so the point is
  that the platform code loads and reports, not that it passes.
- [x] Capture failures now name the switch a user has to find. macOS answers
  "Screen Recording is off for this app, or the display is asleep" instead of
  "the compositor refused the capture", `XOpenDisplay` failure names a Wayland
  session without XWayland, and `--check` prints a platform hint under a failed
  `capture:` line (the README carries the same two paragraphs).
- [x] Module split: `overlay/bar.rs` 1434 lines -> 1264 plus `bar/tests.rs`
  (169), `window_picker.rs` 1365 -> 516 plus `window_picker/tests.rs` (848).
  Tests still run on every platform; only their home moved.
- [x] Live round trip of this build: 5 s hotkey recording on the frame's own
  bar (151 frames, 151 encoded, 30.0 fps out of 29.8 captured, H.264 + AAC),
  a 279 KB still on disk with a 1920x1200 clipboard image, the log carrying
  start and save lines, and Settings opening and closing with exit 0. The
  default record combo was claimed by another app on this box, so the fallback
  moved it to `ctrl+shift+9`, logged the swap, and the smoke followed the file
  instead of the default — the conflict path works too.
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus
  `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`, 223 tests green
  (the cross-target run caught one real break: a Windows-only formatter was
  dead code elsewhere, now gated with its test).
- Carried forward: the physical floor is measured through the window's own
  `MonitorFromWindow`, so mixed-scale desktops are handled per window, but a
  second panel at a different scale still wants a live check; and mac/Linux
  have no live capture verification yet — the new CI steps prove they load
  and report, not that a frame arrives.

## Session 39 - Stills at a chosen quality, one-line frame, and a 0.2.0 gate (done)

- [x] Still quality is a setting of its own: `[photo] quality` is
  `lossless` (PNG, the default), `high` (JPEG 95), `standard` (85), or
  `compact` (70), and Settings carries a **Photo quality** row above the PNG
  depth. The MJPEG path asks for `yuvj444p` on purpose: 4:2:0 fringes
  coloured UI text, which is exactly what a screenshot is judged on. The
  file extension follows the choice, the clipboard still receives the
  captured bitmap, and `--check` names the form a take will write
  (`JPEG 95, 8-bit` / `lossless PNG, 8-bit`).
- [x] Measured on this desktop, a 1280x720 take at JPEG 95 wrote 131 KB where
  the PNG wrote 225 KB, and the bundled `ffprobe` read both back
  (`mjpeg`/`yuvj444p` and `png`/`rgba`).
- [x] The recording frame is one continuous line: a light hairline flush with
  the app's own edge, a dark line inset behind it so it reads on pale
  content, and no corner brackets. Both tones come from `frame_rects`, so the
  corners line up by construction; `assets/recording-frame.png` shows it.
- [x] Settings refuses a destination it cannot write: Save creates a missing
  folder and writes (then removes) a probe file in it, so a path that is a
  file, a read-only drive, or an ACL that only grants reading is reported
  while the field is open instead of after a lost take. The sheet grew one
  row (624 design pixels) and still fits a 1366x768 laptop at 100%.
- [x] Tests closed the seams that let a change pass silently: the dialog's
  answers through `store_answers` -> `persist` -> `settings::load` (every
  field, the new quality, and the shared-folder split), the drag's release
  point (the 260x220 drag saved as 65x55), every control id (a duplicate
  makes `GetDlgItem` answer a neighbour), `IDOK`/`IDCANCEL`, engine
  precedence (Windows never falls through to a bare name), the per-take
  audio pipe name, the single-instance mutex, and BGRA stride across widths.
- [x] Gates: fmt, pedantic clippy `-D warnings` on native plus
  `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`, 246 tests green.
- [x] Version 0.2.0: a new setting and a changed frame are a minor release.

## Deferred backlog

- numbered display names via `EnumDisplayDevices`; mac multi-display
  enumeration (per-display picking answers the main display only);
- macOS live smoke for the session-12 window picking (needs hardware);
  mac multi-display origins/enumeration and live hotkey
  re-registration; multi-monitor cursor
  anchoring; monochrome/XOR cursor verification;
- non-Windows tray (`~StatusNotifierItem`); XFixes cursor on Linux;
  Wayland support;
- live mac/linux CI hardening from user reports; the vendor/health
  LGPL-switch option documented in `docs/FFMPEG-LICENSE.md`;
- clipboard handoff on macOS (`NSPasteboard`) and Linux (an X11 selection
  owner), where stills currently land on disk only;
- a real 16-bit (or HDR) capture route for the session-13 depth hook;
  per-vendor HEVC quality mapping (needs an eyeball session);
  installer run/uninstall cycle (needs elevation + GUI, or CI);
- a live level meter in Settings, and non-Windows audio capture (macOS
  `AVCaptureScreenInput` audio, Linux PulseAudio/PipeWire).
