# Bundled FFmpeg engine — license and source

## Decision (Session 6)

The Windows engine stays the **GPL build**

- `vendor/ffmpeg/ffmpeg-n9.0-latest-win64-gpl-9.0.zip`
  (upstream: <https://github.com/BtbN/FFmpeg-Builds>, `n9.0-latest-win64-gpl`
  release bundle, shipped unmodified).

- It keeps `libx264` / `libx265`, which the 4K quality profiles rely on.
- Dropping to an LGPL build would lose x264 and force a different encoder mix —
  deferred until a distribution plan demands it (`vendor/` layout supports
  either build out of the box).

## Consequences for distribution

- The engine binaries (`ffmpeg.exe`, `ffprobe.exe`) are covered by the
  **GNU GPL v3** (see `vendor/ffmpeg/ffmpeg.LICENSE.txt`, shipped unmodified).
- Our application code is a **separate work**; bundling them in the installer
  is mere aggregation and does not put the app itself under the GPL.
- Distributing the engine imposes on **us**:
  1. ship the license text with the engine (done: the installer copies
     `FFMPEG-LICENSE.md` next to `README.md`, and this file points at the
     engine's own license), and
  2. make the **corresponding source** of that engine available — the
     the upstream build's own page publishes the exact source revision; the
     installer header records the upstream URL. That satisfies the source
     offer without rebuilding FFmpeg ourselves.
- The engine integrity record (`engine.sha256`) is shipped alongside so any
  replaced engine fails `--check` loudly instead of shipping silently.

## If the decision changes

`vendor/ffmpeg/engine.sha256` is regenerated automatically on the next
`--check` after the binary swap (record-file absent → re-record; present and
different → hard fail). An LGPL switch also means dropping x264 from the
profile table (`src/settings.rs`) and re-verifying encoder negotiation in
`src/encoder.rs`.
