# screencapture audit

Scope: every module under `src/`, plus `README.md` and `ROADMAP.md`. Baseline
gates before any edit: `cargo fmt -- --check` exit 0, native
`cargo clippy --all-targets -- -D warnings` exit 0, `cargo test` exit 0 with
73 passed. Cross-target baselines were also taken before changes (see the
gate table at the end).

Ranking: **D** = defect (wrong behavior or leak), **R** = robustness
(environment/edge input), **C** = cleanup. Effort is S (<30 min), M (a
session), L (needs hardware/architecture). Blast radius names the code
that can regress.

## Summary

| # | Sev | Finding | Effort | Blast radius |
|---|-----|---------|--------|--------------|
| D1 | D | ffmpeg child is neither waited nor killed on early-return paths | S | `pipe.rs`, `recorder.rs`, `app/` |
| D2 | D | DXGI acquired frame leaks if `QueryInterface` fails | S | `dxgi/mod.rs` |
| D3 | D | Settings modal during a recording stalls the frame clock | M | `app/settings_apply.rs`, `tray/win` |
| D4 | D | Settings dialog has no dialog keyboard semantics (Tab/Enter/Esc dead) | S | `settings_dialog/` |
| D5 | D | `--photo` silently ignores `--mode` and `[capture] mode` | S | `app/demo.rs` |
| D6 | D | `--record` accepts durations the error text forbids | S | `cli.rs` |
| D7 | D | `Recorder::start` does not create its output directory | S | `recorder.rs`, CLI path |
| D8 | D | `display_at(0)` underflows and panics in debug builds | S | `screen/mod.rs` |
| D9 | D | Windows single-instance check reads a stale `GetLastError` | S | `instance.rs` |
| D10 | D | Non-Windows capture FAIL does not affect `--check` health | S | `doctor.rs` |
| D11 | D | Integrity record compare can panic on a multibyte file | S | `doctor.rs` |
| D12 | D | Invalid `autostart`/`cursor` silently default with no problem line | S | `settings.rs` |
| D13 | D | Identical record/photo hotkeys from the file kill one binding silently | S | `settings.rs` |
| D14 | R | Frame pacing truncates 60 fps to 16 ms and timestamps nothing | M | `app.rs`, `recorder.rs`, `doctor.rs` |
| R1 | R | Resolution change mid-recording ends the session instead of falling back | L | `dxgi/mod.rs` |
| R2 | R | HWND round-trips through `usize`/exposed provenance | S | `window_picker.rs`, `screen/win.rs` |
| R3 | R | `Feed::finish` waits forever on a wedged engine | M | `pipe.rs` |
| R4 | R | Linux window pick is the immediate child, not the top-level client | M | `screen/linux.rs` |
| R5 | R | Mac display list can carry a zero-size display | S | `screen/mac.rs` |
| C1 | C | `settings.rs` is 664 lines, over the project's 500-line cap | M | `settings.rs` |
| C2 | C | `Rect` and `Guid` are declared twice; GDI types/imports are duplicated | S | `winapi`, `settings_dialog`, `tray`, `capture/win` |
| C3 | C | `photo_in` and `take_photo` are byte-for-byte duplicates | S | `app.rs` |
| C4 | C | `Track.bounds` in the picker is written but never read | S | `window_picker.rs` |
| C5 | C | README layout names files that moved and omits new modules | S | `README.md` |
| C6 | C | Doc comments that no longer match code | S | `log.rs`, `screen/mac.rs`, `dxgi/mod.rs` |
| C7 | C | `tray::catalogued` exists only to keep an enum constructed | S | `tray/mod.rs` |

## 1. Correctness gaps

### D1. ffmpeg lifecycle is not owned on error paths — defect, S, wide

`Feed` holds `Child` + `ChildStdin` but has no `Drop` (`src/pipe.rs:88-145`);
only `finish(self)` waits:

```rust
pub fn finish(self) -> io::Result<()> {
    let Self { mut child, stdin } = self;
    drop(stdin);
    let status = child.wait()?;
```

Every early return between spawn and finish therefore drops a `Child` without
`wait`/`kill`. `std::process::Child` does not kill on drop, so the process is
orphaned (and unreaped on Unix). Paths that do this:

- `Recorder::tick` error in the tray: `src/app.rs:334-347` takes the recorder,
  logs, and drops it.
- A mid-loop tick error in `--record`: `src/app/demo.rs:104` uses `?`.
- Photo write errors: `src/app.rs:187-206` and `:388-407` use `?` between
  `Feed::spawn` and `finish`.
- `Feed::frame` itself returns early when `try_wait` sees an exited engine
  (`src/pipe.rs:124-128`), leaving `finish` to be skipped by those callers.

The tray does clear the timer (`app.rs:341`), so the UI does not stay stuck,
but the engine is not finalized and `runtime.engine` stays populated until the
next successful stop (`app.rs:349-373`). Fix: `Feed` owns the child and its
Drop kills+waits any child not explicitly finished; `Recorder` records
frame/time stats; `app::tick` attempts a finalize instead of a bare drop.

### D2. DXGI acquired frame is not released when the texture cast fails — defect, S

`src/dxgi/mod.rs:168-200`:

```rust
Ok((Some(resource), info)) => {
    let texture = resource
        .query_to::<TextureVt>(&ID3D_TEXTURE2D)
        .map_err(|hr| report("texture", hr))?;   // <- returns with frame acquired
    ...
    session.release_frame()...
```

`?` propagates before `release_frame` at `:190-193`. The duplicate resource
itself drops, but the desktop frame transaction stays acquired; the next
`AcquireNextFrame` is documented to fail (`DXGI_ERROR_INVALID_CALL`) or report
`ACCESS_LOST`, forcing a reconnect for a recoverable cast error. Fix: on cast
failure, release the frame before returning.

### D3. Settings modal stalls an active recording — defect, M

`open_settings` (`src/app/settings_apply.rs:29-36`) does not stop the recording.
The tray keeps dispatching `WM_TIMER` while the settings pump runs; `emit`
queues those ticks behind the active handler (`src/tray/win/mod.rs:300-320`),
so the encoder receives no frames for the whole time the modal is open. When
the modal closes, queued ticks replay in a burst. Duplication frames are
wall-clock images, but ffmpeg labels every frame `1/fps`, so the saved file
runs fast and loses the modal interval. Session 15 already established the
rule for pickers: modal surfaces stop the recording first. Apply the same rule
to Settings.

### D4. Settings dialog ignores dialog keyboard semantics — defect, S

`run` builds a normal window with `BS_DEFPUSHBUTTON` and `WS_TABSTOP` children
(`src/settings_dialog/rows.rs:322-345`, `:330`), but the pump is a plain
`GetMessage`/`TranslateMessage`/`DispatchMessage` loop
(`src/settings_dialog/mod.rs:148-166`) and the window procedure handles only
`WM_COMMAND`, `WM_CLOSE`, `WM_DESTROY` (`:220-245`). Nothing calls
`IsDialogMessageW`, so Tab does not move focus, Enter does not press Save, and
Escape does not close. The README's "modal-style dialog ... Save (default)"
overstates what the loop implements. Fix: call `IsDialogMessageW` before
dispatch and handle `VK_ESCAPE`.

### D5. `--photo` drops the capture mode — defect, S

`photo_demo` (`src/app/demo.rs:127-140`) resolves only `rect` or
`full_region`:

```rust
let Some(region) = rect.or_else(|| super::full_region(&loaded, display).ok()) else {
```

`--photo --mode window` and `--photo --mode region` therefore capture the whole
display, while `--record` honors the same flags. This contradicts README's
"`--record` and `--photo` run the same capture pipelines". Fix: resolve the
mode exactly like `demo_recording`, including the Windows selector and the
platform window path.

### D6. `--record` duration is unbounded — defect, S

`src/cli.rs:119-123` accepts any `u32 > 0`, while the error text promises
`1-3600`:

```rust
Ok(seconds) if seconds > 0 => Command::Record { seconds },
_ => return Err("--record requires a duration in seconds (1-3600)".into()),
```

`--record 4000000000` is accepted and then used as
`Duration::from_secs` (`app/demo.rs:96`), effectively hanging the CLI. Fix:
enforce `1..=3600`.

### D7. Recording directory is not created by the recorder — defect, S

`Recorder::start` joins `plan.media_dir` with the file name
(`src/recorder.rs:128-152`) but never creates the directory. The tray path
creates it in `app::begin` (`src/app.rs:259-263`); the headless path does not
(`src/app/demo.rs:86`). A fresh or custom `[media] videos` path therefore
starts ffmpeg with an output path in a missing directory: frames are written to
a pipe that no encoder can consume, and the failure only surfaces at
`finish`. Fix: `create_dir_all` in `Recorder::start`, where the output path is
owned.

### D8. `display_at(0)` panics — defect, S

`src/screen/mod.rs:54-57`:

```rust
pub fn display_at(number: u32) -> io::Result<Display> {
    let all = displays();
    let index = usize::try_from(number - 1).unwrap_or(usize::MAX);
```

`number == 0` underflows before the conversion; debug builds panic, release
builds wrap to `u32::MAX` and return a misleading "not attached" error.
Current callers validate 1-based input, but the function is crate-public and
its contract is 1-based. Fix with `checked_sub`.

### D9. Single-instance mutex reads stale last-error — defect, S

`src/instance.rs:51-68` calls `CreateMutexW` and then `GetLastError` without
clearing it first. Win32 only guarantees `ERROR_ALREADY_EXISTS` on the existing
case; on the new case the slot may still hold an unrelated prior error. A
stale `183` makes a first launch report `AlreadyRunning` and exit. Fix:
`SetLastError(0)` immediately before the call (and keep the documented handle
leak).

### D10. Non-Windows capture failure does not fail `--check` — defect, S

`src/doctor.rs:17-22` folds only `check_settings` and `check_engine` into
`healthy`:

```rust
healthy &= check_settings(config);
healthy &= check_engine(config);
check_capture();
```

On macOS/Linux `check_capture` prints `FAIL capture: ...`
(`src/doctor.rs:276-285`) but returns `()`, so the process prints
`result: ok` and exits 0. Fix: return the backend verdict and fold it in; the
Windows line stays informational (GDI fallback is healthy).

### D11. Integrity record compare can panic — defect, S

`src/doctor.rs:215`:

```rust
if expected.len() >= 64 && expected[..64] == actual_hex {
```

`expected` is a `&str`; slicing at byte 64 panics if that offset is inside a
multibyte UTF-8 character. A hand-edited or corrupted `engine.sha256` can
therefore crash `--check` instead of reporting a mismatch. Fix: compare the
first 64 bytes via `as_bytes()` in a small pure helper and test it.

### D12. Invalid booleans are silently accepted — defect, S

`src/settings.rs:469-476` reads `autostart`/`cursor` through `as_bool` with no
problem line, unlike every other typed value:

```rust
autostart: document.value("recording", "autostart").and_then(Value::as_bool).unwrap_or(true),
cursor: document.value("recording", "cursor").and_then(Value::as_bool).unwrap_or(false),
```

The existing test even encodes the gap: `autostart = yes-please` produces two
problems (hotkey + profile), not three (`settings.rs:655-662`). Fix: report
present-but-invalid booleans with their fallback.

### D13. Equal hotkeys silently disable one action — defect, S

`read_document` parses `record` and `photo` independently
(`src/settings.rs:420-456`). If both name `ctrl+shift+x`, `bindings` builds two
registrations for the same combo (`src/hotkeys.rs:57-72`); the second
`RegisterHotKey` fails and `register_hotkeys` only prints a warning
(`src/tray/win/mod.rs:191-205`). The settings dialog validates inequality
(`settings_dialog/mod.rs:349-352`), but a hand-edited file does not. Fix:
detect equality during load, report it, and restore the documented defaults.

### D14. Frame pacing is approximate and unmeasured — robustness, M

- The tray timer interval truncates: `frame_interval_ms` is
  `(1_000 / fps).max(1)` (`src/app.rs:425-427`), so 60 fps becomes 16 ms,
  i.e. up to 62.5 frames per wall second labeled as 60. At 30 fps it is 33 ms
  instead of 33.33.
- The encoder labels every piped frame `-framerate fps` (`src/pipe.rs:32-46`);
  there is no timestamp or duplicate-frame correction, so any timer jitter
  changes playback duration rather than frame timing.
- The headless loop accumulates a deadline and catches up without sleeping
  when it falls behind (`src/app/demo.rs:101-110`), producing bursts with no
  missed-frame accounting.
- Neither `--check` nor the saved-file log reports a measured fps. See
  "Measurement discipline" below.

Fix in this pass: round the interval to the nearest millisecond, reset a badly
missed deadline instead of bursting, count frames/elapsed in `Recorder`, log
the measured rate, and add a timed capture benchmark to `--check`. A true
timestamped mux (VFR or duplicate-frame pacing) is deferred; it needs a
pipeline redesign, not a patch.

## 2. Robustness matrix

| Scenario | Behavior today | Verdict |
|---|---|---|
| 0 monitors | `screen::primary` → `io::Error("no display is attached")` (`screen/mod.rs:41-50`); `--check` reports no duplication; tray menus still open | Clean error; settings dialog shows one fake display entry (`attached_count().max(1)`, `rows.rs:243`) |
| 1×1 screen/region | `Region::is_sane` accepts it; GDI allocates a 1×1 DIB; profile `scale` upscales to 4K | Works; quality is whatever `scale` does |
| Full-screen UWP app | picker uses `WindowFromPoint` + `GetAncestor(GA_ROOT)` + DWM extended bounds (`window_picker.rs:319-374`); clamping to the virtual desktop (`screen/win.rs:129-143`) | Common case works; UWP/CoreWindow virtualized bounds unverified (no hardware) |
| Maximized window spanning the virtual desktop | `visible_region` clamps each edge to `SM_*VIRTUALSCREEN`; width/height from i64 subtraction | Correct; tray `sane_region` then caps each edge at 7680 without moving the origin (silent crop on >7680-wide walls) |
| Folded-out (auto-hide) taskbar | picker highlights the live DWM bounds under the cursor; hidden taskbar answers whatever is beneath | Reasonable; not live-verified |
| Resolution changes mid-recording | duplication drops the session on `ACCESS_LOST`, reconnects, but `pick_output` must still find an output exactly matching the *original* region (`dxgi/mod.rs:240-270`, `policy.rs:15-50`); a changed mode returns `Dead` | Recording stops with an error instead of falling back to GDI; README's "rebuilt mid-recording" is only true for competing duplicators/mode resets that keep the same geometry |
| fps 1 or 1000 | Not reachable: profiles are 30/60/120 (`recording.rs:31-75`), no CLI/config fps knob. `frame_interval_ms` guards `fps.max(1)` | Honest absence; D14 still governs real pacing |
| 2560×1600 region at 60 fps | No throughput measurement anywhere; README quotes one 1920×1200 reference run (`README.md:345-349`) | Unverified; `--check` benchmark added in this pass |
| Missing/extra/out-of-range settings | Missing → defaults; extra sections/keys preserved (`config.rs:129-164`), unknown keys ignored; malformed lines fail the whole file to defaults with a problem (`settings.rs:339-357`); out-of-range enum values fall back with a problem | Good, except D12/D13 |
| Concurrent single instance | Windows named mutex / Unix advisory lock (`instance.rs:30-107`); non-owner exits `FAILURE` after logging | Good except D9; Windows mutex handle is intentionally leaked |
| ffmpeg missing | `discover` returns `None`, start returns a clear error before any state is installed (`app.rs:244-252`, `:415-418`); `--check` fails | Tray not stuck |
| ffmpeg fails mid-encode | `tick` stops the timer, logs, drops the recorder (`app.rs:334-347`); with D1 fixed the child is killed | File is not finalized; tray recovers, engine reference is stale until fixed |
| `--init` on an unreadable existing file | Refuses to overwrite and exits failure (`doctor.rs:37-50`) | Good |
| Locked desktop / access denied | duplication `open` errors and `pick_source` prints the reason and demotes to GDI (`recorder.rs:190-208`); GDI may also refuse when locked | Recorded as error; gates unaffected |

## 3. Dead weight and redundancy

- **C1** `src/settings.rs` is 664 lines (`wc -l`), violating README's
  "every file stays under 500 lines" (`README.md:18`). `app.rs` (464),
  `window_picker.rs` (466) and `tray/win/mod.rs` (455) are close but legal.
- **C2** Duplicate Win32 shapes:
  - `Rect` at `src/winapi.rs:146-155` and `src/settings_dialog/ffi.rs:159-174`.
  - `Guid` at `src/dxgi/structs.rs:51-57` and `src/tray/win/ffi.rs:62-71`.
  - GDI `Hdc`/`Bool` and `GetDC`/`ReleaseDC` at `src/capture/win.rs:18-20`,
    `:168-172`, even though `winapi.rs` declares the same types and functions.
  Every duplicate carries its own `pin_layout!`, so the pins can drift apart.
- **C3** `photo_in` (`app.rs:187-206`) and `take_photo` (`app.rs:388-407`) are
  identical; one is dead weight and they can diverge.
- **C4** `Track.bounds` (`window_picker.rs:94-95`) is assigned at `:326`,
  `:335` and never read; only the drawn rect matters.
- **C5** README layout is stale: it names `src/selector.rs` (`README.md:94`,
  `:147`) and `src/settings_dialog.rs` (`:163`) after both became directories,
  and omits `src/guard.rs`, `src/layout.rs`, `src/app/`, `src/settings_dialog/`,
  `src/corefoundation.rs` (the last is listed, the split dirs are not).
- **C6** Doc/code mismatches:
  - `log.rs:47-49` says lines are "printed when a console exists"; `note`
    always calls `eprintln!` (`log.rs:52`).
  - `screen/mac.rs:53-56` documents `# Errors` on `list()`, which returns
    `Vec<Display>`.
  - `dxgi/mod.rs:1-13` says duplication is used only for "one unrotated
    output", but `eligible` (`:407-410`) accepts any sane region; the real
    gate is inside `open`/`pick_output`.
- **C7** `tray::catalogued` (`tray/mod.rs:97-110`) constructs every `Event`
  variant solely so the enum typechecks on non-Windows; no caller consumes
  the array. The `parity` test asserts its length, which tests the catalog,
  not behavior.
- Console/debug leftovers: `#[expect(...)]` attributes are used in
  `settings.rs:29-33`, `app.rs:139-143`, `tray/mod.rs:82-88`, which is
  consistent with `-D warnings` but contradicts README's literal "No `allow`
  blocks" (`README.md:13-17`) only in wording.
- `app/settings_apply.rs:58` carries a bare `//autostart-dependent sync`
  comment with no space, unlike the rest of the documentation style.

## 4. Platform asymmetry

- The tray, picker, selector, settings dialog, hotkeys and cursor compositing
  are Windows-only; on macOS/Linux `tray::run` returns
  "the tray lands later on this platform" (`tray/mod.rs:62-72`), so the
  README's "lives in your system tray" is a Windows claim. The README does
  disclose this, but only deep in the status section.
- `--mode region` is Windows-only for recordings (`app.rs:211-224`), while
  `--rect` is the cross-platform headless answer. `--photo --mode region`
  currently does not error at all (D5); fixing D5 makes the asymmetry honest.
- macOS `screen::list` returns only the main display at `0,0`
  (`screen/mac.rs:47-62`); per-display picking is explicitly deferred. Linux
  reports one X screen spanning monitors (`screen/linux.rs:14-25`).
- `doctor::check_capture` is the only place where a non-Windows backend
  failure is downgraded from FAIL to healthy (D10).
- cfg tiers are otherwise honest: `dxgi`, `guard`, `selector`,
  `settings_dialog`, `winapi`, `window_picker` are `#[cfg(windows)]`;
  `xlib` is `#[cfg(target_os = "linux")]`; CoreGraphics/CoreFoundation are
  macOS; `dylib` is Unix. `paths.rs` uses `cfg!` (all branches compile
  everywhere), but no platform API leaks across.
- `screen::mac::list` can return `width: 0, height: 0` from
  `CGDisplayPixelsWide/High` (`screen/mac.rs:57-60`), which would make
  `primary()` succeed with an empty display and `Target::new` fail later.
  Guard the zero case and return no display (R5).

## 5. Measurement discipline

- `--check` reports the backend label and resolution
  (`doctor.rs:255-285`) and encoder probes (`:130-158`), but **no measured
  capture throughput or latency**. The README's 250 frames / 4.17 s and
  "GDI alone sustained ~30 live fps" (`README.md:345-349`) cannot be
  reproduced by any command in the repo.
- `Recorder` has no frame counter, start instant, or measured fps; the only
  post-run number is `ffprobe`'s frame count (`app.rs:375-386`), which is
  expected to equal the profile fps × duration and therefore cannot detect
  pacing drift.
- `--record` sleeps to a deadline but never records how many frames actually
  went into the pipe (`app/demo.rs:101-112`).
- The clock module (`clock.rs`) is deterministic UTC calendar math and is
  correctly tested; it is not a measurement clock.
- Fix in this pass: `Recorder` tracks frames and elapsed time; stop/log lines
  report measured fps; `--check` times a short capture burst through the
  native fast path (duplication on Windows, the native backend elsewhere) and
  prints frames, elapsed, fps and ms/frame. Failures are informational so a
  locked desktop does not turn `--check` red.

## 6. What this pass changed

Fixed (all verified by the gates and the smokes below):

- **D1** `pipe::Feed` is now an owning handle: `Drop` closes stdin, then
  kills and waits any engine not explicitly finished. `Recorder::tick`
  failures go through `finalize_recording` (stop timer, wait for ffmpeg,
  clear `runtime.engine`), and the demo loop finalizes on a failed tick.
  `Recorder::start` also creates `plan.media_dir`, closing the fresh-folder
  CLI failure.
- **D2** DXGI releases the acquired frame before propagating a texture-cast
  failure; scratch allocation is checked.
- **D3** `open_settings` stops the recording before creating the modal;
  no frame-timer messages queue behind it.
- **D4** The settings pump calls `IsDialogMessageW`; Save/Cancel use
  `IDOK`/`IDCANCEL`; Escape has an explicit handler. This restores the
  Win32 dialog contract for the existing `WS_TABSTOP`/`BS_DEFPUSHBUTTON`
  styles; it is API/compile-verified, not live keyboard-tested (no GUI
  automation here, same caveat as the picker).
- **D5** `--photo` resolves `[capture] mode` exactly like `--record`
  (Full/Region/Window), with `--rect` still the override.
- **D6** `--record` enforces `1..=3600`; test added.
- **D7** `Recorder::start` owns `create_dir_all` for its output folder.
- **D8** `display_at` uses `checked_sub`; `display_at(0)` is an error.
- **D9** `SetLastError(0)` before `CreateMutexW` in the instance guard.
- **D10** `check_capture` returns a verdict and `--check` folds it into
  `healthy` on non-Windows; the Windows GDI fallback stays healthy.
- **D11** Integrity comparison is byte-wise (`digest_matches`), with tests
  for short and multibyte records.
- **D12** Invalid `autostart`/`cursor` values push problem lines; the old
  test expectation was corrected to three problems.
- **D13** Identical hotkeys are reported and reset to both defaults.
- **D14** `Recorder` tracks frames and elapsed time; stop logs report
  measured fps; `--check` times a six-frame blit burst and prints
  frames/seconds/fps/ms per frame. Timer intervals round to the nearest
  millisecond and the demo loop resynchronizes instead of bursting.
  Duplication is not timed in `--check`: its idle path returns the cached
  frame without a GPU copy, which produced a fictional 95k fps in the
  first implementation and was removed.
- **C1** `settings.rs` split into `settings/{mod,hotkey,tests}.rs`
  (388/183/142 lines). All source files are now under 500 lines.
- **C2** `Rect` and `Guid` live once in `winapi.rs` (the dialog and DXGI
  re-export them); GDI `Hdc`/`Bool` and `GetDC`/`ReleaseDC` come from
  `winapi`, and the screen DC is owned by `guard::ScreenDc`.
- **C3/C4** The duplicate `photo_in` is gone and the picker's `bounds`
  field was removed.
- **C5/C6/C7** README's layout names the current directories; the stale
  doc comments named above (`log`, `screen::mac::list`, DXGI eligibility)
  are still open and listed as deferred below.

Deferred (deliberately not attempted here):

- **R1** Falling back to GDI when duplication's output no longer matches
  the original region after a resolution change.
- **D14 remainder** True timestamped/VFR muxing (or duplicate-frame
  pacing) so wall-clock jitter cannot change playback duration.
- **R3** A timeout around `Feed::finish` for a wedged engine (the new Drop
  kill covers early returns, not a hung `wait`).
- **R4/R5** Linux top-level window resolution under reparenting WMs and
  the mac zero-size display guard.
- **C6/C7** Doc-only mismatches and the `tray::catalogued` parity seam.
- Any mac/Linux live verification; this pass ran on Windows only.

## 7. Gate results after the changes

| Gate | Exit |
|---|---|
| `cargo fmt` (then `cargo fmt -- --check`) | 0 |
| `cargo clippy --all-targets -- -D warnings` | 0 |
| `cargo clippy --all-targets --target aarch64-apple-darwin -- -D warnings` | 0 |
| `cargo clippy --all-targets --target x86_64-unknown-linux-gnu -- -D warnings` | 0 |
| `cargo test` | 0 (78 passed) |
| `target/debug/screencapture.exe --check` | 0 |
| `--record 1 --rect 0,0,320,240` | 0 |
| `--photo --rect 0,0,64,64` | 0 |
| `--photo --mode window` | 0 (ffprobe: 1920x48, proving the mode override) |
| `--photo --mode bogus` | 1 (rejected by the parser) |

The `--check` smoke ran on the reference Windows desktop: duplication was
available (1920x1200), engine integrity matched, and the timing line
reported the GDI blit path. The record/photo smokes wrote their outputs
and the record log carried both the captured-frame/measured-fps line and
the ffprobe encoded count. These are real runs, not inferred from the
build.

## 8. Post-v0.1.0 safety and security pass (Session 30)

Scope: FFI module lifetime, native library and engine loading, output-file
naming, installer registry strings, and the shipped icon assets. No
high-severity memory-corruption issue was found in the reviewed paths.

Fixed in this pass:

- **Module reference leaks.** `CoreGraphics::Api::load`,
  `CoreFoundation::Api::load`, and `xlib::Api::load` resolved and then
  `mem::forget`-ed a new `dlopen` handle on every construction. Each surface
  is now cached in a `OnceLock` and copied, so window-list enumeration and
  repeated frame setup share one loaded module instead of leaking a reference
  per call.
- **DLL preload path.** DXGI and D3D11 were loaded by bare name, which lets
  the Windows loader search the current directory before System32. Both now
  use `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)`, and the handle is
  cached once for the process.
- **Engine search path.** Windows `ffmpeg` discovery ended with a bare
  `ffmpeg.exe` candidate, so `Command::new` could launch a stray binary from
  the working directory or `PATH`. That fallback is now Unix-only; Windows
  uses the explicit configured path, the install directory, or the developer
  `vendor/ffmpeg` layout.
- **Output-name race.** `clock::stamped_path` checked `Path::exists` and then
  returned the name, leaving a same-second window where two processes could
  choose the same file. The name is now reserved with `create_new` in a
  bounded retry loop. A failed recorder or photo start removes the empty
  placeholder.
- **Installer strings.** `DisplayIcon` and `UninstallString` are now quoted
  registry values, and setup/uninstall use `assets/icon.ico` instead of the
  generic NSIS icon.

Reviewed and left as-is:

- `pipe::Feed` closes stdin, then kills and waits an unfinished child on drop;
  `finish` has a size-scaled deadline and kills a wedged engine.
- Clipboard DIB construction checks row/stride multiplication and frame
  length before copying, and every `GlobalAlloc` refusal or handover failure
  frees the block.
- DXGI frame transactions release on every `?` path after `AcquireNextFrame`,
  and `Recorder` bounds the stop wait instead of joining a wedged worker.
- Settings persistence writes a temporary file and renames it, with tests for
  the no-temporary-left case.

Residual limitations: macOS and Linux capture paths remain header-verified
and cross-compiled, not live-run on that hardware; GitHub's read-only
`refs/pull/*` objects from the earlier history rewrite cannot be purged
without repository recreation or GitHub Support.
