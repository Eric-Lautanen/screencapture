# Windows installer (NSIS)

Builds `screencapture-setup.exe`: the app (icon baked into the exe), the
integrity-recorded FFmpeg engine beside it, Start-Menu shortcuts, and
uninstall entries.

## Prerequisites

1. NSIS 3.x with MUI2 (verified with the **3.13** portable zip,
   SHA-1 `db14f8af2ea346b786c6ae19343f61aaf5e09b8e`): pull
   `nsis-3.13.zip` from the SourceForge `nsis` project (mirror redirect
   gets in the way of some fetchers; a direct
   `https://<mirror>.dl.sourceforge.net/project/nsis/NSIS%203/3.13/nsis-3.13.zip?download=1`
   works). 3.13's `DefaultLangCode` era aside, no extra plugins needed.
2. The bundled engine unpacked at `vendor/ffmpeg/`
   (`ffmpeg.exe`, `ffprobe.exe`, `ffmpeg.LICENSE.txt`, `engine.sha256`).
   Run `scripts\fetch-ffmpeg.cmd` from the repository root to download and
   verify the pinned build with built-in PowerShell and .NET; no curl is
   needed. The integrity record is written by
   `target\release\screencapture.exe --check` after the engine is in place.
3. `docs\FFMPEG-LICENSE.md` records the GPL engine decision and the
   upstream source URL (kept in the payload for the license/source
   offer); `LICENSE` (Apache-2.0, the app's own license) ships next to
   `README.md`.

## Build

The script defines `ROOT` itself (overridable):

```powershell
scripts\fetch-ffmpeg.cmd
cargo build --release
.\target\release\screencapture.exe --check
& <nsis>\bin\makensis.exe "/DROOT=$PWD" installer\nsis\screencapture.nsi
```

Verified on this machine with NSIS 3.13 (portable): compile clean,
output `installer\screencapture-setup.exe` (~122 MB; the engine block
compresses ~330 MB to ~128 MB). Full interactive install/uninstall
runs need a GUI session and an elevation prompt; silent runs claim
admin (`RequestExecutionLevel admin`), so they belong in CI (`Actions`)
once repo packaging lands there.

## Notes

- Uninstall keeps user settings and recordings (intentional).
- Setup and the uninstaller use `assets/icon.ico`; the installed exe carries
  the same multi-size icon as a resource.
- `EstimatedSize` is computed at install time with `${GetSize}`.
