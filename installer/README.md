# Windows installer (NSIS)

Builds `screencapture-setup.exe`: the app (icon baked into the exe), the
integrity-recorded FFmpeg engine beside it, Start-Menu shortcuts, and
uninstall entries.

## Prerequisites

1. NSIS 3.x (MUI2 ships with it): <https://nsis.sourceforge.io/>
2. The bundled engine unpacked at `vendor/ffmpeg/`
   (`ffmpeg.exe`, `ffprobe.exe`, `ffmpeg.LICENSE.txt`, `engine.sha256`).
   The record is written automatically by `target\release\screencapture.exe --check`
   the first time the engine is in place.
3. `docs\FFMPEG-LICENSE.md` records the GPL engine decision and the upstream
   source URL (kept in the payload for the license/source offer).

## Build

```powershell
cargo build --release
makensis -DROOT=. installer/nsis/screencapture.nsi  # from the repo root
```

The installer smoke test (run, install, `--check` from the install directory,
uninstall) is run whenever NSIS is available; the script stays the plan of
record between tests.
