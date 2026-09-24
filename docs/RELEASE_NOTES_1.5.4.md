# Qlisa 1.5.4

## Improvements

- Windows media conversion and probing now use the pinned BtbN FFmpeg 9.0 GPL runtime. `ffmpeg.exe` and `ffprobe.exe` come from the same archive.
- Windows visual playback now uses the approved shinchiro libmpv build.

## Requirements

- Windows 10 or Windows 11.
- WebView2 Runtime. Supported Windows versions normally include it; install Microsoft's Evergreen WebView2 Runtime if it is missing.
- A separately installed NDI Runtime for NDI input and output. Qlisa does not include or install the NDI Runtime.
