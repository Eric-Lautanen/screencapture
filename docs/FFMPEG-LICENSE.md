# Bundled FFmpeg engine — license and source

## Decision (Session 6)

The Windows engine stays the **GPL build**

- `ffmpeg-n9.0.2-28-g669f5ab083-win64-gpl-9.0.zip` from
  [`autobuild-2026-10-18-13-01`](https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-18-13-01)
  (upstream: <https://github.com/BtbN/FFmpeg-Builds>), shipped unmodified.
  Archive SHA-256: `a38198cc130ed3fa02097d0baebcea9c1b8a9d9d3d1d8161d691d1f03709158b`.
  [`scripts/fetch-ffmpeg.ps1`](../scripts/fetch-ffmpeg.ps1) downloads and
  verifies that pin with Windows PowerShell and .NET; no curl is required.

- It keeps `libx264` / `libx265`, which the 4K quality profiles rely on.
- Dropping to an LGPL build would lose x264 and force a different encoder mix —
  deferred until a distribution plan demands it (`vendor/` layout supports
  either build out of the box).

## Consequences for distribution

- The engine binaries (`ffmpeg.exe`, `ffprobe.exe`) are covered by the
  **GNU GPL v3** (see `vendor/ffmpeg/ffmpeg.LICENSE.txt`, shipped unmodified).
- Our application code is a **separate work**, licensed under the
  **Apache License 2.0** (see `LICENSE` at the repository root); bundling the
  engine in the installer is mere aggregation and does not put the app itself
  under the GPL.
- Distributing the engine imposes on **us**:
  1. ship the license text with the engine (done: the installer copies
     `FFMPEG-LICENSE.md` under `docs\`, and that file points at the engine's
     own license), and
  2. make the **corresponding source** of that engine available — this
     build is FFmpeg commit
     [`669f5ab083c7732c10f1ce9e4f1915960dd88457`](https://github.com/FFmpeg/FFmpeg/commit/669f5ab083c7732c10f1ce9e4f1915960dd88457),
     built by the [BtbN build scripts](https://github.com/BtbN/FFmpeg-Builds).
     The shipped `docs/FFMPEG-LICENSE.md` records both URLs, which satisfies
     the source offer without rebuilding FFmpeg ourselves.
- The engine integrity record (`engine.sha256`) is shipped alongside so any
  replaced engine fails `--check` loudly instead of shipping silently.

## If the decision changes

`vendor/ffmpeg/engine.sha256` is regenerated automatically on the next
`--check` after the binary swap (record-file absent → re-record; present and
different → hard fail). An LGPL switch also means dropping x264 from the
profile table (`src/settings/mod.rs`) and re-verifying encoder negotiation in
`src/encoder.rs`.
